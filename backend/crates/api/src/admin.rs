//! Admin API for match data (T05): manual overrides, void and reopen.
//!
//! Every route needs an [`AdminUser`], resolved by the [`AdminAuth`] in the
//! app state. Until T07 brings Discord sessions, production uses
//! [`DenyAll`]; dev builds accept an `X-Dev-User` header ([`DevHeaderAuth`]).
//! The handlers only translate HTTP to `sync::ManualService`, which audits
//! every change with the admin's Discord user ID.

use async_trait::async_trait;
use axum::extract::{FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{post, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use domain::{MapStatus, Match, MatchFormat, MatchId, MatchMap, MatchStatus};
use serde::{Deserialize, Serialize};
use sync::{ManualError, Outcome, Override, OverrideField};

use crate::AppState;

// ---- authentication ---------------------------------------------------------

/// Why a request is not an admin's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// No valid identity (401).
    Unauthenticated(String),
    /// A valid user who is not an admin (403).
    Forbidden(String),
}

/// Decides whether a request comes from an admin and who it is.
#[async_trait]
pub trait AdminAuth: Send + Sync {
    /// The admin's Discord user ID.
    async fn authenticate(&self, headers: &HeaderMap) -> Result<String, AuthError>;
}

/// Rejects every request. The production default until T07 adds sessions.
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyAll;

#[async_trait]
impl AdminAuth for DenyAll {
    async fn authenticate(&self, _headers: &HeaderMap) -> Result<String, AuthError> {
        Err(AuthError::Unauthenticated(
            "admin sign-in is not available yet".to_owned(),
        ))
    }
}

/// Dev builds only: trusts an `X-Dev-User: <Discord user ID>` header. Never
/// compiled into production builds.
#[cfg(any(test, feature = "dev"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct DevHeaderAuth;

#[cfg(any(test, feature = "dev"))]
#[async_trait]
impl AdminAuth for DevHeaderAuth {
    async fn authenticate(&self, headers: &HeaderMap) -> Result<String, AuthError> {
        let user = headers
            .get("x-dev-user")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .unwrap_or_default();
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
        match state.admin_auth.authenticate(&parts.headers).await {
            Ok(id) => Ok(AdminUser(id)),
            Err(AuthError::Unauthenticated(m)) => Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
                m,
            )),
            Err(AuthError::Forbidden(m)) => {
                Err(ApiError::new(StatusCode::FORBIDDEN, "forbidden", m))
            }
        }
    }
}

// ---- errors -----------------------------------------------------------------

/// A JSON error response: `{"error": code, "message": text}`.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.code, "message": self.message });
        (self.status, Json(body)).into_response()
    }
}

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::http::StatusCode;
    use chrono::TimeZone;
    use serde_json::json;
    use sources::fake::sourced;
    use sources::{SrcEvent, SrcMatch, SrcTeam};

    use super::*;
    use crate::test_util::{reconciler, send, temp_db};

    const ADMIN: &str = "300000000000000001";

    /// A router with one Bo3 from source `alpha` (score 0:0, scheduled), and
    /// the admin auth `auth`.
    async fn app(auth: Arc<dyn AdminAuth>) -> (Router, i64, store::SqlitePool, tempfile::TempDir) {
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
        let state = AppState::new(rec, clock).with_admin_auth(auth);
        let app = crate::router(state, std::path::Path::new("/nonexistent"));
        (app, id, pool, dir)
    }

    #[tokio::test]
    async fn production_default_denies_every_admin_request() {
        let (app, id, _pool, _dir) = app(Arc::new(DenyAll)).await;
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
        let (app, id, _pool, _dir) = app(Arc::new(DevHeaderAuth)).await;
        let uri = format!("/api/admin/matches/{id}/void");
        for headers in [&[][..], &[("x-dev-user", "alice")][..]] {
            let (status, _) = send(&app, "POST", &uri, headers, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn admin_overrides_void_and_errors() {
        let (app, id, pool, _dir) = app(Arc::new(DevHeaderAuth)).await;
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
}
