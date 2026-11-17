//! Admin API: manual overrides, void and reopen of matches (T05), and the
//! sync status and a manual resync of an event (T06).
//!
//! Every route needs an [`AdminUser`], resolved by the [`AdminAuth`] in the
//! app state: in production the session plus the admin rule
//! ([`crate::auth::AuthService`], T07); dev builds also accept an
//! `X-Dev-User` header ([`DevHeaderAuth`]).
//! The handlers only translate HTTP to `sync::ManualService`, which audits
//! every change with the admin's Discord user ID.

#[cfg(any(test, feature = "dev"))]
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use domain::{EventId, MapStatus, Match, MatchFormat, MatchId, MatchMap, MatchStatus};
use serde::{Deserialize, Serialize};
use store::audit::{self, AuditRecord};
use sync::{ManualError, Outcome, Override, OverrideField, SyncHandle, SyncStatus};

use crate::AppState;
pub use crate::error::ApiError;

// ---- authentication ---------------------------------------------------------

/// Why a request is not an admin's (or not signed in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// No valid session (401).
    Unauthenticated(String),
    /// A signed-in member who is not an admin (403).
    Forbidden(String),
    /// Discord could not be asked, so admin status is unknown (503).
    Unavailable(String),
    /// A database error (500); details are logged where it happened.
    Internal,
}

impl From<AuthError> for ApiError {
    fn from(e: AuthError) -> Self {
        match e {
            AuthError::Unauthenticated(m) => {
                ApiError::new(StatusCode::UNAUTHORIZED, "unauthenticated", m)
            }
            AuthError::Forbidden(m) => ApiError::new(StatusCode::FORBIDDEN, "forbidden", m),
            AuthError::Unavailable(m) => {
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "discord_unavailable", m)
            }
            AuthError::Internal => ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "database error",
            ),
        }
    }
}

/// Decides whether a request comes from an admin and who it is.
#[async_trait]
pub trait AdminAuth: Send + Sync {
    /// The admin's Discord user ID.
    async fn authenticate(&self, headers: &HeaderMap) -> Result<String, AuthError>;
}

/// Dev builds only: trusts an `X-Dev-User: <Discord user ID>` header, and
/// falls back to `inner` (the session) when the header is absent. Never
/// compiled into production builds.
#[cfg(any(test, feature = "dev"))]
pub struct DevHeaderAuth {
    inner: Arc<dyn AdminAuth>,
}

#[cfg(any(test, feature = "dev"))]
impl DevHeaderAuth {
    pub fn new(inner: Arc<dyn AdminAuth>) -> Self {
        Self { inner }
    }
}

#[cfg(any(test, feature = "dev"))]
#[async_trait]
impl AdminAuth for DevHeaderAuth {
    async fn authenticate(&self, headers: &HeaderMap) -> Result<String, AuthError> {
        let Some(value) = headers.get("x-dev-user") else {
            return self.inner.authenticate(headers).await;
        };
        let user = value.to_str().map(str::trim).unwrap_or_default();
        if user.is_empty() || !user.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AuthError::Unauthenticated(
                "dev build: send X-Dev-User with a Discord user ID".to_owned(),
            ));
        }
        Ok(user.to_owned())
    }
}

/// An authenticated admin; extracting it rejects everyone else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminUser(pub String);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        Ok(AdminUser(
            state.admin_auth.authenticate(&parts.headers).await?,
        ))
    }
}

// ---- errors -----------------------------------------------------------------

impl From<ManualError> for ApiError {
    fn from(e: ManualError) -> Self {
        match e {
            ManualError::NotFound(_) => {
                Self::new(StatusCode::NOT_FOUND, "not_found", e.to_string())
            }
            ManualError::Invalid(m) => Self::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid", m),
            ManualError::Conflict(m) => Self::new(StatusCode::CONFLICT, "conflict", m),
            ManualError::Reconcile(e) => {
                tracing::error!("admin action failed: {e}");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "the change could not be saved",
                )
            }
        }
    }
}

// ---- responses --------------------------------------------------------------

/// A match as admins see it after a change. Internal IDs only.
#[derive(Debug, Serialize)]
pub struct MatchBody {
    pub id: i64,
    pub event_id: i64,
    pub stage_id: Option<i64>,
    pub team_a_id: Option<i64>,
    pub team_b_id: Option<i64>,
    pub format: MatchFormat,
    pub status: MatchStatus,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub lock_at: Option<DateTime<Utc>>,
    pub locked: bool,
    pub voided: bool,
    pub score: (u8, u8),
    pub winner_team_id: Option<i64>,
    pub maps: Vec<MapBody>,
}

#[derive(Debug, Serialize)]
pub struct MapBody {
    pub number: u8,
    pub map_name: Option<String>,
    pub status: MapStatus,
    pub rounds: Option<(u16, u16)>,
    pub winner_team_id: Option<i64>,
}

impl MatchBody {
    fn new(m: &Match, maps: &[MatchMap]) -> Self {
        let f = &m.fields;
        Self {
            id: m.id.0,
            event_id: f.event_id.0,
            stage_id: f.stage_id.map(|s| s.0),
            team_a_id: f.team_a_id.map(|t| t.0),
            team_b_id: f.team_b_id.map(|t| t.0),
            format: f.format,
            status: f.status,
            scheduled_at: f.scheduled_at,
            started_at: f.started_at,
            ended_at: f.ended_at,
            lock_at: f.lock_at,
            locked: f.locked,
            voided: f.voided,
            score: (f.maps_won_a, f.maps_won_b),
            winner_team_id: f.winner_team_id.map(|t| t.0),
            maps: maps
                .iter()
                .map(|m| MapBody {
                    number: m.number,
                    map_name: m.map_name.clone(),
                    status: m.status,
                    rounds: m.rounds_a.zip(m.rounds_b),
                    winner_team_id: m.winner_team_id.map(|t| t.0),
                })
                .collect(),
        }
    }
}

type ApiResult = Result<Json<MatchBody>, ApiError>;

fn body(o: Outcome) -> ApiResult {
    Ok(Json(MatchBody::new(&o.current, &o.maps)))
}

// ---- requests -----------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusReq {
    pub status: MatchStatus,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleReq {
    pub scheduled_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeriesReq {
    pub score: (u8, u8),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapReq {
    /// Default: finished.
    #[serde(default)]
    pub status: Option<MapStatus>,
    #[serde(default)]
    pub rounds: Option<(u16, u16)>,
}

// ---- routes -------------------------------------------------------------------

/// Routes under `/api/admin`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/matches/{id}/overrides/status",
            put(set_status).delete(clear_status),
        )
        .route(
            "/matches/{id}/overrides/schedule",
            put(set_schedule).delete(clear_schedule),
        )
        .route(
            "/matches/{id}/overrides/series",
            put(set_series).delete(clear_series),
        )
        .route(
            "/matches/{id}/overrides/maps/{n}",
            put(set_map).delete(clear_map),
        )
        .route("/matches/{id}/void", post(void))
        .route("/matches/{id}/unvoid", post(unvoid))
        .route("/matches/{id}/reopen", post(reopen))
        .route("/status", get(sync_status))
        .route("/events/{id}/resync", post(resync))
}

async fn set(st: &AppState, id: i64, ov: Override, admin: &AdminUser) -> ApiResult {
    body(
        st.manual
            .set(MatchId(id), ov, &admin.0, st.clock.now())
            .await?,
    )
}

async fn clear(st: &AppState, id: i64, field: OverrideField, admin: &AdminUser) -> ApiResult {
    body(
        st.manual
            .clear(MatchId(id), field, &admin.0, st.clock.now())
            .await?,
    )
}

async fn set_status(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
    Json(r): Json<StatusReq>,
) -> ApiResult {
    let ov = Override::Status {
        status: r.status,
        started_at: r.started_at,
        ended_at: r.ended_at,
    };
    set(&st, id, ov, &admin).await
}

async fn clear_status(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
) -> ApiResult {
    clear(&st, id, OverrideField::Status, &admin).await
}

async fn set_schedule(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
    Json(r): Json<ScheduleReq>,
) -> ApiResult {
    let ov = Override::Schedule {
        scheduled_at: r.scheduled_at,
    };
    set(&st, id, ov, &admin).await
}

async fn clear_schedule(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
) -> ApiResult {
    clear(&st, id, OverrideField::Schedule, &admin).await
}

async fn set_series(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
    Json(r): Json<SeriesReq>,
) -> ApiResult {
    set(&st, id, Override::Series { score: r.score }, &admin).await
}

async fn clear_series(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
) -> ApiResult {
    clear(&st, id, OverrideField::Series, &admin).await
}

async fn set_map(
    State(st): State<AppState>,
    admin: AdminUser,
    Path((id, n)): Path<(i64, u8)>,
    Json(r): Json<MapReq>,
) -> ApiResult {
    let ov = Override::Map {
        number: n,
        status: r.status.unwrap_or(MapStatus::Finished),
        rounds: r.rounds,
    };
    set(&st, id, ov, &admin).await
}

async fn clear_map(
    State(st): State<AppState>,
    admin: AdminUser,
    Path((id, n)): Path<(i64, u8)>,
) -> ApiResult {
    clear(&st, id, OverrideField::Map(n), &admin).await
}

async fn void(State(st): State<AppState>, admin: AdminUser, Path(id): Path<i64>) -> ApiResult {
    body(
        st.manual
            .void(MatchId(id), &admin.0, st.clock.now())
            .await?,
    )
}

async fn unvoid(State(st): State<AppState>, admin: AdminUser, Path(id): Path<i64>) -> ApiResult {
    body(
        st.manual
            .unvoid(MatchId(id), &admin.0, st.clock.now())
            .await?,
    )
}

async fn reopen(State(st): State<AppState>, admin: AdminUser, Path(id): Path<i64>) -> ApiResult {
    body(
        st.manual
            .reopen(MatchId(id), &admin.0, st.clock.now())
            .await?,
    )
}

// ---- sync (T06) ---------------------------------------------------------------

fn sync_handle(st: &AppState) -> Result<&SyncHandle, ApiError> {
    st.sync.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "sync_unavailable",
            "the sync worker is not running",
        )
    })
}

fn internal(e: sqlx::Error) -> ApiError {
    tracing::error!("admin request failed: {e}");
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "database error",
    )
}

/// `GET /api/admin/status`: request counters and budget per source, and the
/// state of every sync job.
async fn sync_status(
    State(st): State<AppState>,
    _admin: AdminUser,
) -> Result<Json<SyncStatus>, ApiError> {
    let handle = sync_handle(&st)?;
    Ok(Json(handle.status().await.map_err(internal)?))
}

#[derive(Debug, Serialize)]
struct ResyncBody {
    event_id: i64,
    queued: bool,
}

/// `POST /api/admin/events/{id}/resync`: runs the event's structure sync at
/// once, also for an event nobody follows or that has finished.
async fn resync(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<i64>,
) -> Result<(StatusCode, Json<ResyncBody>), ApiError> {
    let handle = sync_handle(&st)?;
    let exists = store::events::get(&st.pool, EventId(id))
        .await
        .map_err(internal)?
        .is_some();
    if !exists {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("event {id} not found"),
        ));
    }
    audit::append(
        &st.pool,
        &AuditRecord {
            actor: admin.0,
            action: "resync".to_owned(),
            entity_type: "event".to_owned(),
            entity_id: Some(id.to_string()),
            before: None,
            after: None,
            at: st.clock.now(),
        },
    )
    .await
    .map_err(internal)?;
    handle.sync_event_now(EventId(id));
    Ok((
        StatusCode::ACCEPTED,
        Json(ResyncBody {
            event_id: id,
            queued: true,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use chrono::TimeZone;
    use serde_json::json;
    use sources::fake::sourced;
    use sources::{SrcEvent, SrcMatch, SrcTeam};

    use super::*;
    use crate::test_util::{reconciler, send, temp_db};

    const ADMIN: &str = "300000000000000001";

    /// Wraps the state's admin guard in [`DevHeaderAuth`], as dev builds do.
    fn with_dev_header(state: AppState) -> AppState {
        let inner = state.admin_auth.clone();
        state.with_admin_auth(Arc::new(DevHeaderAuth::new(inner)))
    }

    /// A router with one Bo3 from source `alpha` (score 0:0, scheduled); with
    /// `dev_header`, admin requests may use `X-Dev-User`.
    async fn app(dev_header: bool) -> (Router, i64, store::SqlitePool, tempfile::TempDir) {
        let (pool, dir) = temp_db().await;
        let rec = reconciler(pool.clone(), &["alpha"]);
        let now = Utc.with_ymd_and_hms(2026, 11, 20, 9, 0, 0).unwrap();
        rec.ingest_event("alpha", &sourced(&SrcEvent::new("e1", "IEM Test")), now)
            .await
            .unwrap();
        let m = SrcMatch {
            team_a: Some(SrcTeam::named("t1", "Vitality")),
            team_b: Some(SrcTeam::named("t2", "NAVI")),
            format: Some(MatchFormat::Bo3),
            scheduled_at: Some(Utc.with_ymd_and_hms(2026, 11, 20, 18, 0, 0).unwrap()),
            score: Some((0, 0)),
            ..SrcMatch::new("m1", "e1")
        };
        rec.ingest_matches("alpha", &[sourced(&m)], now)
            .await
            .unwrap();
        let id = store::refs::resolve(&pool, "alpha", domain::EntityType::Match, "m1")
            .await
            .unwrap()
            .unwrap();
        let clock = Arc::new(domain::FakeClock::new(now));
        let mut state = AppState::new(rec, clock);
        if dev_header {
            state = with_dev_header(state);
        }
        let app = crate::router(state, std::path::Path::new("/nonexistent"));
        (app, id, pool, dir)
    }

    #[tokio::test]
    async fn without_a_session_every_admin_request_is_401() {
        // Production ignores X-Dev-User.
        let (app, id, _pool, _dir) = app(false).await;
        let dev = [("x-dev-user", ADMIN)];
        for (method, path, body) in [
            ("PUT", "overrides/series", Some(json!({ "score": [2, 0] }))),
            ("DELETE", "overrides/series", None),
            ("POST", "void", None),
            ("POST", "reopen", None),
        ] {
            let uri = format!("/api/admin/matches/{id}/{path}");
            let (status, body) = send(&app, method, &uri, &dev, body).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
            assert_eq!(body["error"], "unauthenticated");
        }
    }

    #[tokio::test]
    async fn dev_header_auth_needs_a_user_id() {
        let (app, id, _pool, _dir) = app(true).await;
        let uri = format!("/api/admin/matches/{id}/void");
        for headers in [&[][..], &[("x-dev-user", "alice")][..]] {
            let (status, _) = send(&app, "POST", &uri, headers, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn dev_header_auth_falls_back_to_the_session() {
        use crate::test_util::{ALICE, GUILD, OWNER, auth_app};

        let a = auth_app(Some(GUILD)).await;
        let state = AppState::new(reconciler(a.pool.clone(), &[]), a.clock.clone())
            .with_auth(a.auth.clone());
        let app = crate::router(with_dev_header(state), std::path::Path::new("/nonexistent"));
        for (user, expected) in [
            (OWNER, (StatusCode::SERVICE_UNAVAILABLE, "sync_unavailable")),
            (ALICE, (StatusCode::FORBIDDEN, "forbidden")),
        ] {
            let auth = format!("Bearer {}", a.sign_in(user, GUILD).await);
            let (status, body) = send(
                &app,
                "GET",
                "/api/admin/status",
                &[("authorization", &auth)],
                None,
            )
            .await;
            assert_eq!(
                (status, body["error"].as_str().unwrap()),
                expected,
                "{user}"
            );
        }
    }

    #[tokio::test]
    async fn admin_overrides_void_and_errors() {
        let (app, id, pool, _dir) = app(true).await;
        let h = [("x-dev-user", ADMIN)];
        let base = format!("/api/admin/matches/{id}");

        let (status, body) = send(
            &app,
            "PUT",
            &format!("{base}/overrides/series"),
            &h,
            Some(json!({ "score": [2, 1] })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["score"], json!([2, 1]));
        assert_eq!(body["winner_team_id"], body["team_a_id"]);

        let (status, body) = send(
            &app,
            "PUT",
            &format!("{base}/overrides/maps/1"),
            &h,
            Some(json!({ "rounds": [13, 5] })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["maps"][0]["rounds"], json!([13, 5]));
        assert_eq!(body["maps"][0]["status"], "finished");

        let (status, body) = send(
            &app,
            "DELETE",
            &format!("{base}/overrides/series"),
            &h,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["score"], json!([0, 0]), "the source's value");

        // Errors are JSON with a code.
        let (status, body) = send(
            &app,
            "PUT",
            &format!("{base}/overrides/series"),
            &h,
            Some(json!({ "score": [2, 2] })),
        )
        .await;
        assert_eq!(
            (status, body["error"].as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, Some("invalid"))
        );
        let (status, body) = send(
            &app,
            "PUT",
            "/api/admin/matches/999/overrides/series",
            &h,
            Some(json!({ "score": [1, 0] })),
        )
        .await;
        assert_eq!(
            (status, body["error"].as_str()),
            (StatusCode::NOT_FOUND, Some("not_found"))
        );

        // Void, then void again is a conflict; reopen of an unlocked match too.
        let (status, body) = send(&app, "POST", &format!("{base}/void"), &h, None).await;
        assert_eq!(
            (status, body["voided"].as_bool()),
            (StatusCode::OK, Some(true))
        );
        let (status, body) = send(&app, "POST", &format!("{base}/void"), &h, None).await;
        assert_eq!(
            (status, body["error"].as_str()),
            (StatusCode::CONFLICT, Some("conflict"))
        );
        let (status, _) = send(&app, "POST", &format!("{base}/unvoid"), &h, None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(&app, "POST", &format!("{base}/reopen"), &h, None).await;
        assert_eq!(status, StatusCode::CONFLICT);

        // Every change is audited with the admin from the request.
        let audit = store::audit::list_for_entity(&pool, "match", &id.to_string())
            .await
            .unwrap();
        assert_eq!(audit.len(), 5);
        assert!(audit.iter().all(|a| a.record.actor == ADMIN));
    }

    #[tokio::test]
    async fn sync_status_and_resync() {
        use sources::fake::FakeAdapter;
        use sync::{BudgetLimits, Scheduler, SchedulerParts, SourceEntry, SyncConfig};

        let (pool, _dir) = temp_db().await;
        let rec = reconciler(pool.clone(), &["alpha"]);
        let now = Utc.with_ymd_and_hms(2026, 11, 20, 9, 0, 0).unwrap();
        let clock = Arc::new(domain::FakeClock::new(now));
        let src = Arc::new(FakeAdapter::new("alpha"));
        src.put_event(SrcEvent::new("e1", "IEM Test"));
        rec.ingest_event("alpha", &sourced(&SrcEvent::new("e1", "IEM Test")), now)
            .await
            .unwrap();
        let (mut scheduler, handle) = Scheduler::open(SchedulerParts {
            reconciler: rec.clone(),
            clock: clock.clone(),
            config: SyncConfig::default(),
            sources: vec![SourceEntry {
                adapter: src.clone(),
                budget: Some(BudgetLimits {
                    per_hour: 500,
                    per_minute: 30,
                }),
            }],
            seed: Some(1),
        })
        .await
        .unwrap();

        let base = crate::AppState::new(rec, clock);
        let dev = [("x-dev-user", ADMIN)];
        let no_sync = crate::router(
            with_dev_header(base.clone()),
            std::path::Path::new("/nonexistent"),
        );
        let app = crate::router(
            with_dev_header(base.clone()).with_sync(handle),
            std::path::Path::new("/nonexistent"),
        );
        let denied = crate::router(base, std::path::Path::new("/nonexistent"));

        let (status, _) = send(&denied, "GET", "/api/admin/status", &dev, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, body) = send(&no_sync, "GET", "/api/admin/status", &dev, None).await;
        assert_eq!(
            (status, body["error"].as_str()),
            (StatusCode::SERVICE_UNAVAILABLE, Some("sync_unavailable"))
        );

        let (status, body) = send(&app, "GET", "/api/admin/status", &dev, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["sources"][0]["source"], "alpha");
        assert_eq!(body["sources"][0]["budget_per_hour"], 500);
        assert_eq!(body["sources"][0]["requests_this_hour"], 0);
        assert_eq!(body["jobs"], json!([]));

        let (status, _) = send(&app, "POST", "/api/admin/events/999/resync", &dev, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body) = send(&app, "POST", "/api/admin/events/1/resync", &dev, None).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        let audit = store::audit::list_for_entity(&pool, "event", "1")
            .await
            .unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].record.actor, ADMIN);

        // The unfollowed event is synced on the next tick.
        scheduler.tick().await.unwrap();
        assert_eq!(src.calls(), 2, "fetch_event and fetch_event_matches");
        let (_, body) = send(&app, "GET", "/api/admin/status", &dev, None).await;
        assert_eq!(body["sources"][0]["requests_this_hour"], 2);
        assert_eq!(body["jobs"][0]["job"], "structure:1");
        assert!(body["jobs"][0]["last_success_at"].is_string(), "{body}");
    }
}
