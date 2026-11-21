//! Rate limits for `/api/auth/*` and `/api/admin/*` (T16).
//!
//! The app always runs behind a reverse proxy (the bundled Caddy or an
//! existing Traefik) and publishes no port of its own. Both proxies set
//! `X-Forwarded-For` to the address they saw, discarding or appending to
//! whatever the client sent. The **rightmost** entry is therefore the one
//! our proxy wrote and cannot be forged by the client; it is the key. Without
//! the header (tests, local `cargo run`) the peer address is used.
//!
//! Discord's Activity proxy hides users' IP addresses, so all Activity
//! traffic arrives from Discord's proxy and shares few keys. The bursts are
//! sized for a whole server opening the Activity at once; the limits mainly
//! stop someone hammering the public hostname directly.
//!
//! See `docs/adr/0008-container-image-and-proxy.md`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderValue, Request, Response, StatusCode, header};
use axum::response::IntoResponse;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::KeyExtractor;
use tower_governor::{GovernorError, GovernorLayer};

use crate::AppState;
use crate::error::ApiError;

/// One token bucket per client key: `burst` requests at once, refilled by
/// one request every `refill`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    pub burst: u32,
    pub refill: Duration,
}

/// `/api/auth/*`: sign-in at every Activity launch.
pub const AUTH: Limit = Limit {
    burst: 30,
    refill: Duration::from_secs(2),
};

/// `/api/admin/*`: admin screens and actions.
pub const ADMIN: Limit = Limit {
    burst: 60,
    refill: Duration::from_secs(1),
};

/// How often idle keys are dropped from the limiter's memory.
const CLEANUP_EVERY: Duration = Duration::from_secs(60);

/// The client address as seen by our reverse proxy: the rightmost
/// `X-Forwarded-For` entry, else the TCP peer, else `0.0.0.0` (one shared
/// bucket), so extraction never fails.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProxiedIp;

impl KeyExtractor for ProxiedIp {
    type Key = IpAddr;

    fn extract<T>(&self, req: &Request<T>) -> Result<IpAddr, GovernorError> {
        Ok(forwarded_for(req)
            .or_else(|| {
                req.extensions()
                    .get::<ConnectInfo<SocketAddr>>()
                    .map(|c| c.0.ip())
            })
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)))
    }
}

/// The last entry of the last `X-Forwarded-For` header, if it is an IP
/// address (optionally with a port).
fn forwarded_for<T>(req: &Request<T>) -> Option<IpAddr> {
    let value = req
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .next_back()?;
    let last = value.to_str().ok()?.rsplit(',').next()?.trim();
    last.parse::<IpAddr>()
        .ok()
        .or_else(|| last.parse::<SocketAddr>().ok().map(|s| s.ip()))
}

/// Wraps `routes` in a rate limit of `limit` per client key.
pub fn limit(routes: Router<AppState>, limit: Limit) -> Router<AppState> {
    let config = Arc::new(
        GovernorConfigBuilder::default()
            .key_extractor(ProxiedIp)
            .period(limit.refill)
            .burst_size(limit.burst)
            .finish()
            .expect("rate limit with non-zero burst and period"),
    );
    let weak = Arc::downgrade(&config);
    spawn_cleanup(move || {
        let Some(config) = weak.upgrade() else {
            return false;
        };
        config.limiter().retain_recent();
        true
    });
    routes.layer(GovernorLayer::new(config).error_handler(too_many_requests))
}

/// Runs `cleanup` every minute to drop idle keys, so the limiter's memory
/// stays bounded, until it returns `false` (the router, and with it the
/// limiter, was dropped). Without a tokio runtime (a router built outside
/// one) there is no cleanup.
fn spawn_cleanup(cleanup: impl Fn() -> bool + Send + 'static) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    rt.spawn(async move {
        let mut tick = tokio::time::interval(CLEANUP_EVERY);
        tick.tick().await;
        loop {
            tick.tick().await;
            if !cleanup() {
                return;
            }
        }
    });
}

/// A 429 in the API's JSON error shape, with `Retry-After`.
fn too_many_requests(e: GovernorError) -> Response<Body> {
    let wait = match e {
        GovernorError::TooManyRequests { wait_time, .. } => wait_time,
        // ProxiedIp never fails; keep a sane answer anyway.
        GovernorError::UnableToExtractKey | GovernorError::Other { .. } => 1,
    }
    .max(1);
    tracing::debug!(retry_after = wait, "rate limit exceeded");
    let mut res = ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        format!("too many requests; retry in {wait} s"),
    )
    .into_response();
    res.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(wait));
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{state, temp_db};
    use axum::routing::get;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn req(uri: &str, xff: Option<&str>) -> Request<Body> {
        let mut r = Request::get(uri);
        if let Some(v) = xff {
            r = r.header("x-forwarded-for", v);
        }
        r.body(Body::empty()).unwrap()
    }

    #[test]
    fn rightmost_forwarded_for_wins() {
        let key = |xff: Option<&str>| ProxiedIp.extract(&req("/", xff)).unwrap();
        assert_eq!(
            key(Some("6.6.6.6, 10.0.0.1, 203.0.113.7")),
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            key(Some("2001:db8::1")),
            "2001:db8::1".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            key(Some("203.0.113.7:4711")),
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );
        // Garbage or absent: the peer, here absent too, so the shared key.
        assert_eq!(
            key(Some("1.2.3.4, nonsense")),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
        assert_eq!(key(None), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }

    #[test]
    fn peer_address_is_the_fallback() {
        let mut r = req("/", None);
        r.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 9], 5000))));
        assert_eq!(
            ProxiedIp.extract(&r).unwrap(),
            "192.0.2.9".parse::<IpAddr>().unwrap()
        );
    }

    fn tiny(burst: u32) -> Limit {
        Limit {
            burst,
            refill: Duration::from_secs(3600),
        }
    }

    async fn status(app: &Router, uri: &str, xff: &str) -> Response<Body> {
        app.clone().oneshot(req(uri, Some(xff))).await.unwrap()
    }

    #[tokio::test]
    async fn excess_requests_get_a_json_429_per_client() {
        let (pool, _dir) = temp_db().await;
        let routes = Router::new().route("/x", get(|| async { "ok" }));
        let app = limit(routes, tiny(2)).with_state(state(pool));

        for _ in 0..2 {
            assert_eq!(
                status(&app, "/x", "203.0.113.1").await.status(),
                StatusCode::OK
            );
        }
        let res = status(&app, "/x", "203.0.113.1").await;
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry: u64 = res.headers()[header::RETRY_AFTER]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(retry >= 1);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "rate_limited");

        // Another client has its own bucket.
        assert_eq!(
            status(&app, "/x", "203.0.113.2").await.status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn auth_and_admin_are_limited_but_health_and_frontend_are_not() {
        let (pool, _dir) = temp_db().await;
        let dist = tempfile::tempdir().unwrap();
        std::fs::write(dist.path().join("index.html"), "hi").unwrap();
        let app = crate::router(state(pool), dist.path());

        for (uri, burst) in [
            ("/api/auth/config", AUTH.burst),
            ("/api/admin/status", ADMIN.burst),
        ] {
            for _ in 0..burst {
                assert_ne!(
                    status(&app, uri, "198.51.100.1").await.status(),
                    StatusCode::TOO_MANY_REQUESTS,
                    "{uri}"
                );
            }
            assert_eq!(
                status(&app, uri, "198.51.100.1").await.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "{uri}"
            );
        }
        // Auth and admin buckets were independent (admin got its full burst
        // after auth was exhausted); health, frontend and other API routes
        // have no limit.
        for uri in ["/healthz", "/", "/api/nope"] {
            for _ in 0..AUTH.burst + ADMIN.burst {
                assert_ne!(
                    status(&app, uri, "198.51.100.1").await.status(),
                    StatusCode::TOO_MANY_REQUESTS,
                    "{uri}"
                );
            }
        }
    }
}
