//! HTTP API (axum).
//!
//! Routes:
//! - `GET /healthz` – liveness and DB check, no auth.
//! - `/api/admin/*` – admin actions on match data (T05) and the sync
//!   status and resync (T06), see [`admin`].
//! - `/api/dev/*` – dev builds only (feature `dev`): the fixture clock (T05).
//! - `/api/*` – the JSON API (T07 onwards); unknown paths answer a JSON 404.
//! - everything else – the built frontend (`frontend/dist`). The Activity
//!   uses an in-memory router and always loads at `/`, so there is no SPA
//!   fallback: unknown files are a plain 404.

pub mod admin;
#[cfg(any(test, feature = "dev"))]
pub mod dev;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use domain::Clock;
use serde::Serialize;
use store::SqlitePool;
use sync::{ManualService, Reconciler, SyncHandle};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::admin::{AdminAuth, DenyAll};

/// Crate version reported by `/healthz`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Git commit reported by `/healthz`, embedded at build time.
pub const GIT_SHA: &str = env!("GIT_SHA");

const DB_PING_TIMEOUT: Duration = Duration::from_secs(2);

/// Shared state for all handlers.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    /// The source of "now": the system clock, or the fake clock in fixture
    /// mode (T05).
    pub clock: Arc<dyn Clock>,
    pub manual: ManualService,
    pub admin_auth: Arc<dyn AdminAuth>,
    /// The running sync scheduler (T06); `None` when it is not running,
    /// e.g. in tests.
    pub sync: Option<SyncHandle>,
}

impl AppState {
    /// State on top of the shared reconciler. Admin requests are denied
    /// until an [`AdminAuth`] is set with [`Self::with_admin_auth`].
    pub fn new(reconciler: Arc<Reconciler>, clock: Arc<dyn Clock>) -> Self {
        Self {
            pool: reconciler.pool().clone(),
            clock,
            manual: ManualService::new(reconciler),
            admin_auth: Arc::new(DenyAll),
            sync: None,
        }
    }

    pub fn with_sync(mut self, handle: SyncHandle) -> Self {
        self.sync = Some(handle);
        self
    }

    pub fn with_admin_auth(mut self, auth: Arc<dyn AdminAuth>) -> Self {
        self.admin_auth = auth;
        self
    }
}

/// Builds the full HTTP router.
pub fn router(state: AppState, static_dir: &Path) -> Router {
    router_with(state, static_dir, Router::new())
}

/// Like [`router`], with `extra` routes nested under `/api`, such as the dev
/// routes in dev builds.
pub fn router_with(state: AppState, static_dir: &Path, extra: Router<AppState>) -> Router {
    if !static_dir.join("index.html").is_file() {
        tracing::warn!(
            static_dir = %static_dir.display(),
            "frontend not found; run `pnpm build` in frontend/ or set STATIC_DIR"
        );
    }

    let api = Router::new()
        .nest("/admin", admin::routes())
        .merge(extra)
        .fallback(api_not_found);

    Router::new()
        .route("/healthz", get(healthz))
        .nest("/api", api)
        .fallback_service(ServeDir::new(static_dir))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Serves `app` on `listener` until `shutdown` fires, then drains in-flight
/// requests.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
}

#[derive(Debug, Serialize)]
struct Health {
    version: &'static str,
    git_sha: &'static str,
    db: &'static str,
}

async fn healthz(State(state): State<AppState>) -> (StatusCode, Json<Health>) {
    let db_ok = matches!(
        tokio::time::timeout(DB_PING_TIMEOUT, store::ping(&state.pool)).await,
        Ok(Ok(()))
    );
    if !db_ok {
        tracing::warn!("health check: database ping failed");
    }
    let status = if db_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(Health {
            version: VERSION,
            git_sha: GIT_SHA,
            db: if db_ok { "ok" } else { "error" },
        }),
    )
}

async fn api_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "not_found" })),
    )
        .into_response()
}

/// Helpers shared by this crate's tests.
#[cfg(test)]
pub(crate) mod test_util {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use sync::{EventBus, Precedence};
    use tower::ServiceExt;

    /// A reconciler on `pool` with `sources` enabled, the first as primary.
    pub fn reconciler(pool: SqlitePool, sources: &[&str]) -> Arc<Reconciler> {
        let enabled: Vec<String> = sources.iter().map(|s| (*s).to_owned()).collect();
        Arc::new(Reconciler::new(
            pool,
            Precedence::from_enabled(&enabled),
            enabled.first().cloned(),
            EventBus::new(),
        ))
    }

    /// Default state (system clock, admin denied) on `pool`.
    pub fn state(pool: SqlitePool) -> AppState {
        AppState::new(reconciler(pool, &[]), Arc::new(domain::SystemClock))
    }

    /// A migrated database in a temp directory.
    pub async fn temp_db() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", dir.path().join("test.db").display());
        (store::open(&url).await.unwrap(), dir)
    }

    /// Sends one request; returns status and the body parsed as JSON
    /// (`Null` if it is not JSON).
    pub async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut req = Request::builder().method(method).uri(uri);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::state;
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn pool() -> SqlitePool {
        store::connect("sqlite::memory:").await.unwrap()
    }

    async fn get(app: Router, uri: &str) -> (StatusCode, Option<String>, Vec<u8>) {
        let res = app
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let ctype = res
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_owned());
        let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
        (status, ctype, body)
    }

    fn dist() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "<!doctype html><p>hi</p>").unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/app.js"), "console.log(1)").unwrap();
        dir
    }

    #[tokio::test]
    async fn healthz_ok() {
        let dir = dist();
        let app = router(state(pool().await), dir.path());
        let (status, ctype, body) = get(app, "/healthz").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ctype.as_deref(), Some("application/json"));
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["version"], VERSION);
        assert_eq!(json["git_sha"], GIT_SHA);
        assert!(!GIT_SHA.is_empty());
        assert_eq!(json["db"], "ok");
    }

    #[tokio::test]
    async fn healthz_reports_db_error() {
        let dir = dist();
        let pool = pool().await;
        pool.close().await;
        let app = router(state(pool), dir.path());
        let (status, _, body) = get(app, "/healthz").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["db"], "error");
    }

    #[tokio::test]
    async fn serves_frontend_at_root() {
        let dir = dist();
        let app = router(state(pool().await), dir.path());
        let (status, ctype, body) = get(app.clone(), "/").await;
        assert_eq!(status, StatusCode::OK);
        assert!(ctype.unwrap().starts_with("text/html"));
        assert_eq!(body, b"<!doctype html><p>hi</p>");

        let (status, _, body) = get(app.clone(), "/assets/app.js").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"console.log(1)");

        let (status, _, _) = get(app, "/assets/missing.js").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn unknown_api_path_is_json_404() {
        let dir = dist();
        let app = router(state(pool().await), dir.path());
        for uri in ["/api", "/api/nope", "/api/deeply/nested"] {
            let (status, ctype, body) = get(app.clone(), uri).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
            assert_eq!(ctype.as_deref(), Some("application/json"), "{uri}");
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["error"], "not_found");
        }
    }

    #[tokio::test]
    async fn missing_static_dir_is_404_not_panic() {
        let app = router(state(pool().await), Path::new("/definitely/not/here"));
        let (status, _, _) = get(app, "/").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
