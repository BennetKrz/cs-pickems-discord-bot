//! HTTP API (axum).
//!
//! Routes:
//! - `GET /healthz` – liveness and DB check, no auth.
//! - `/api/auth/*` – sign-in through the Activity (T07), see [`auth`].
//! - `GET /api/me` – the signed-in member (T07), see [`me`].
//! - `/api/img/*` – avatars and team logos through the image proxy (T07),
//!   see [`img`]. No session needed.
//! - `/api/admin/*` – admin actions on match data (T05) and the sync
//!   status and resync (T06), see [`admin`].
//! - `/api/dev/*` – dev builds only (feature `dev`): dev sign-in (T07) and
//!   the fixture clock (T05).
//! - `/api/*` – unknown paths answer a JSON 404. Every JSON `GET` gets an
//!   ETag ([`etag`]).
//! - everything else – the built frontend (`frontend/dist`). The Activity
//!   uses an in-memory router and always loads at `/`, so there is no SPA
//!   fallback: unknown files are a plain 404.
//!
//! Every route except `/healthz`, `/api/auth/*`, `/api/img/*` and the
//! frontend needs `Authorization: Bearer <session>`.

pub mod admin;
pub mod auth;
#[cfg(any(test, feature = "dev"))]
pub mod dev;
pub mod error;
pub mod etag;
pub mod img;
pub mod me;
pub mod ratelimit;

use std::fmt::Write as _;
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

use crate::admin::AdminAuth;
use crate::auth::AuthService;
use crate::img::ImageProxy;

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
    /// Sign-in, sessions and the admin check (T07).
    pub auth: Arc<AuthService>,
    /// Who may use `/api/admin/*`: normally [`Self::auth`]; dev builds wrap
    /// it in [`admin::DevHeaderAuth`].
    pub admin_auth: Arc<dyn AdminAuth>,
    pub images: Arc<ImageProxy>,
    /// The running sync scheduler (T06); `None` when it is not running,
    /// e.g. in tests.
    pub sync: Option<SyncHandle>,
}

impl AppState {
    /// State on top of the shared reconciler, without Discord and without
    /// image fetching: sign-in answers 503 and admin requests 401 until
    /// [`Self::with_auth`] and [`Self::with_images`] set the real ones.
    pub fn new(reconciler: Arc<Reconciler>, clock: Arc<dyn Clock>) -> Self {
        let pool = reconciler.pool().clone();
        let auth = Arc::new(AuthService::unconfigured(pool.clone()));
        Self {
            pool,
            clock,
            manual: ManualService::new(reconciler),
            admin_auth: auth.clone(),
            auth,
            images: Arc::new(ImageProxy::disabled()),
            sync: None,
        }
    }

    pub fn with_sync(mut self, handle: SyncHandle) -> Self {
        self.sync = Some(handle);
        self
    }

    /// Sets the auth service; it also guards the admin routes.
    pub fn with_auth(mut self, auth: Arc<AuthService>) -> Self {
        self.admin_auth = auth.clone();
        self.auth = auth;
        self
    }

    /// Replaces only the admin guard, e.g. with [`admin::DevHeaderAuth`].
    pub fn with_admin_auth(mut self, auth: Arc<dyn AdminAuth>) -> Self {
        self.admin_auth = auth;
        self
    }

    pub fn with_images(mut self, images: Arc<ImageProxy>) -> Self {
        self.images = images;
        self
    }
}

/// Lowercase hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
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
        .nest("/auth", ratelimit::limit(auth::routes(), ratelimit::AUTH))
        .merge(me::routes())
        .nest("/img", img::routes())
        .nest(
            "/admin",
            ratelimit::limit(admin::routes(), ratelimit::ADMIN),
        )
        .merge(extra)
        .fallback(api_not_found)
        .layer(axum::middleware::from_fn(etag::layer));

    Router::new()
        .route("/healthz", get(healthz))
        .nest("/api", api)
        .fallback_service(ServeDir::new(static_dir))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Serves `app` on `listener` until `shutdown` fires, then drains in-flight
/// requests. Handlers see the peer address as `ConnectInfo<SocketAddr>`
/// (the rate limit's fallback key, [`ratelimit`]).
pub async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
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

    // ---- a fake Discord and a signed-in app (T07) ------------------------------

    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;
    use chrono::{DateTime, TimeZone, Utc};
    use domain::FakeClock;

    use crate::auth::{
        AuthConfig, AuthService, DiscordApi, DiscordError, DiscordGuild, DiscordMember, DiscordUser,
    };

    pub const GUILD: &str = "100000000000000001";
    /// A second guild the bot is in; only [`ALICE`] is a member there.
    pub const OTHER_GUILD: &str = "100000000000000002";
    /// A guild the bot is not in.
    pub const FOREIGN_GUILD: &str = "100000000000000003";
    /// Role with *Manage Server*.
    pub const MOD_ROLE: &str = "500000000000000001";
    /// Role configured as the pick'em admin role in some tests.
    pub const PICKEM_ROLE: &str = "500000000000000002";
    /// Role without admin rights.
    pub const PLAIN_ROLE: &str = "500000000000000003";
    pub const OWNER: &str = "200000000000000001";
    pub const MOD: &str = "200000000000000002";
    pub const PICKER: &str = "200000000000000003";
    /// A normal member with nick, global name and avatar.
    pub const ALICE: &str = "200000000000000004";
    /// A Discord user who is not a member of [`GUILD`].
    pub const STRANGER: &str = "200000000000000009";

    /// The sign-in code that belongs to `user_id` in [`FakeDiscord`].
    pub fn code(user_id: &str) -> String {
        format!("code-{user_id}")
    }

    pub fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 20, 9, 0, 0).unwrap()
    }

    /// An in-memory Discord. Codes are `code-<user id>`, access tokens
    /// `at-<user id>`. A guild that is not added is one the bot is not in.
    #[derive(Default)]
    pub struct FakeDiscord {
        users: Mutex<HashMap<String, DiscordUser>>,
        guilds: Mutex<HashMap<String, DiscordGuild>>,
        members: Mutex<HashMap<(String, String), DiscordMember>>,
        calls: Mutex<Vec<&'static str>>,
        down: AtomicBool,
    }

    impl FakeDiscord {
        pub fn add_user(
            &self,
            id: &str,
            username: &str,
            global: Option<&str>,
            avatar: Option<&str>,
        ) {
            self.users.lock().unwrap().insert(
                id.to_owned(),
                DiscordUser {
                    id: id.to_owned(),
                    username: username.to_owned(),
                    global_name: global.map(str::to_owned),
                    avatar: avatar.map(str::to_owned),
                },
            );
        }

        pub fn add_guild(&self, id: &str, owner: &str, roles: &[(&str, u64)]) {
            self.guilds.lock().unwrap().insert(
                id.to_owned(),
                DiscordGuild {
                    owner_id: owner.to_owned(),
                    role_permissions: roles.iter().map(|(r, p)| ((*r).to_owned(), *p)).collect(),
                },
            );
        }

        pub fn add_member(&self, guild: &str, user: &str, nick: Option<&str>, roles: &[&str]) {
            self.members.lock().unwrap().insert(
                (guild.to_owned(), user.to_owned()),
                DiscordMember {
                    nick: nick.map(str::to_owned),
                    roles: roles.iter().map(|r| (*r).to_owned()).collect(),
                },
            );
        }

        pub fn set_roles(&self, guild: &str, user: &str, roles: &[&str]) {
            let mut members = self.members.lock().unwrap();
            let m = members
                .get_mut(&(guild.to_owned(), user.to_owned()))
                .unwrap();
            m.roles = roles.iter().map(|r| (*r).to_owned()).collect();
        }

        pub fn set_down(&self, down: bool) {
            self.down.store(down, Ordering::SeqCst);
        }

        /// The calls so far, by kind: `exchange`, `me`, `member`, `guild`.
        pub fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }

        pub fn clear_calls(&self) {
            self.calls.lock().unwrap().clear();
        }

        fn call(&self, kind: &'static str) -> Result<(), DiscordError> {
            self.calls.lock().unwrap().push(kind);
            if self.down.load(Ordering::SeqCst) {
                return Err(DiscordError::Unavailable);
            }
            Ok(())
        }
    }

    #[async_trait]
    impl DiscordApi for FakeDiscord {
        async fn exchange_code(&self, code: &str) -> Result<String, DiscordError> {
            self.call("exchange")?;
            let user = code
                .strip_prefix("code-")
                .ok_or(DiscordError::InvalidGrant)?;
            if !self.users.lock().unwrap().contains_key(user) {
                return Err(DiscordError::InvalidGrant);
            }
            Ok(format!("at-{user}"))
        }

        async fn current_user(&self, access_token: &str) -> Result<DiscordUser, DiscordError> {
            self.call("me")?;
            let id = access_token
                .strip_prefix("at-")
                .ok_or(DiscordError::InvalidToken)?;
            self.users
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .ok_or(DiscordError::InvalidToken)
        }

        async fn guild_member(
            &self,
            guild_id: &str,
            user_id: &str,
        ) -> Result<Option<DiscordMember>, DiscordError> {
            self.call("member")?;
            if !self.guilds.lock().unwrap().contains_key(guild_id) {
                return Err(DiscordError::NoAccess);
            }
            Ok(self
                .members
                .lock()
                .unwrap()
                .get(&(guild_id.to_owned(), user_id.to_owned()))
                .cloned())
        }

        async fn guild(&self, guild_id: &str) -> Result<Option<DiscordGuild>, DiscordError> {
            self.call("guild")?;
            self.guilds
                .lock()
                .unwrap()
                .get(guild_id)
                .cloned()
                .map(Some)
                .ok_or(DiscordError::NoAccess)
        }
    }

    /// [`GUILD`] with an owner, a moderator (Manage Server), a member with
    /// [`PICKEM_ROLE`], and Alice; [`OTHER_GUILD`] with Alice only; plus a
    /// stranger who is in no guild.
    pub fn fake_discord() -> Arc<FakeDiscord> {
        let d = FakeDiscord::default();
        d.add_guild(
            GUILD,
            OWNER,
            &[
                (GUILD, 0x400), // @everyone: view channels
                (MOD_ROLE, domain::admin::MANAGE_GUILD),
                (PICKEM_ROLE, 0),
                (PLAIN_ROLE, 0x800),
            ],
        );
        d.add_guild(OTHER_GUILD, "999", &[(OTHER_GUILD, 0)]);
        d.add_user(OWNER, "owner", None, None);
        d.add_user(MOD, "mod", Some("Moderator"), None);
        d.add_user(PICKER, "picker", None, None);
        d.add_user(ALICE, "alice", Some("Alice"), Some("abc123"));
        d.add_user(STRANGER, "stranger", None, None);
        d.add_member(GUILD, OWNER, None, &[]);
        d.add_member(GUILD, MOD, None, &[MOD_ROLE]);
        d.add_member(GUILD, PICKER, None, &[PICKEM_ROLE]);
        d.add_member(GUILD, ALICE, Some("ali"), &[PLAIN_ROLE]);
        d.add_member(OTHER_GUILD, ALICE, None, &[]);
        Arc::new(d)
    }

    pub struct AuthApp {
        pub app: Router,
        pub pool: SqlitePool,
        pub discord: Arc<FakeDiscord>,
        pub clock: Arc<FakeClock>,
        pub auth: Arc<AuthService>,
        pub dir: tempfile::TempDir,
    }

    impl AuthApp {
        /// Signs `user_id` in to `guild`; returns the session token.
        pub async fn sign_in(&self, user_id: &str, guild: &str) -> String {
            let (status, body) = send(
                &self.app,
                "POST",
                "/api/auth/token",
                &[],
                Some(serde_json::json!({ "code": code(user_id), "guild_id": guild })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            body["session_token"].as_str().unwrap().to_owned()
        }

        /// A request with the session token.
        pub async fn get(&self, uri: &str, token: &str) -> (StatusCode, serde_json::Value) {
            let auth = format!("Bearer {token}");
            send(
                &self.app,
                "GET",
                uri,
                &[("authorization", auth.as_str())],
                None,
            )
            .await
        }
    }

    /// A router whose auth runs on [`fake_discord`] and a fake clock at
    /// [`t0`]; `configured_guild` is `DISCORD_GUILD_ID`.
    pub async fn auth_app(configured_guild: Option<&str>) -> AuthApp {
        let (pool, dir) = temp_db().await;
        auth_app_on(pool, dir, fake_discord(), configured_guild)
    }

    /// Like [`auth_app`] on an existing database and Discord.
    pub fn auth_app_on(
        pool: SqlitePool,
        dir: tempfile::TempDir,
        discord: Arc<FakeDiscord>,
        configured_guild: Option<&str>,
    ) -> AuthApp {
        let clock = Arc::new(FakeClock::new(t0()));
        let auth = Arc::new(AuthService::new(
            pool.clone(),
            discord.clone(),
            AuthConfig::new("424242424242424242", configured_guild.map(str::to_owned)),
            clock.clone(),
        ));
        let state =
            AppState::new(reconciler(pool.clone(), &[]), clock.clone()).with_auth(auth.clone());
        let app = crate::router(state, Path::new("/nonexistent"));
        AuthApp {
            app,
            pool,
            discord,
            clock,
            auth,
            dir,
        }
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
