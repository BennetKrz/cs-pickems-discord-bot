//! Dev-only endpoints: the fixture driver (T05) and dev sign-in (T07).
//! Compiled only with the `dev` feature (and in this crate's tests), never
//! into production builds.
//!
//! The [`FixtureDriver`] owns the fake clock and the fixture source. It keeps
//! the scenario's anchor and the clock in `sync_state` (job `fixture`), so a
//! restart neither moves the scenario nor turns the clock back. Each sync
//! ingests the event, then its matches, through the normal reconciler, so
//! advancing the clock emits the normal domain events.
//!
//! - `GET /api/dev/clock` → `{now, anchor}`
//! - `POST /api/dev/clock/advance` `{"minutes": n}` → `{now, anchor, report}`
//! - `POST /api/dev/session` `{user_id, guild_id?, username?}` → a session
//!   like `/api/auth/token` gives, without Discord ([`session_routes`]).

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, DurationRound, Utc};
use domain::{Clock, FakeClock};
use serde::{Deserialize, Serialize};
use sources::SourceAdapter;
pub use sources::fixture::Scenario;
use sources::fixture::{FIXTURE, FixtureAdapter};
use store::SqlitePool;
use store::sync_state::{self, SyncState};
use sync::{Reconciler, Report};
use tokio::sync::Mutex;

use crate::AppState;
use crate::auth::{DiscordUser, is_snowflake, session_response};
use crate::error::ApiError;
use crate::me::me_body;

/// The `sync_state` job that stores the anchor and the clock.
pub const JOB: &str = "fixture";

/// The longest single jump of the dev clock: one week.
pub const MAX_ADVANCE_MINUTES: i64 = 7 * 24 * 60;

#[derive(Debug, thiserror::Error)]
pub enum DevError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Reconcile(#[from] sync::ReconcileError),
    #[error(transparent)]
    Source(#[from] sources::SourceError),
    #[error("stored fixture state is invalid: {0}")]
    State(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct Saved {
    anchor: DateTime<Utc>,
    clock: DateTime<Utc>,
}

/// Drives the fixture source on a fake clock.
pub struct FixtureDriver {
    pool: SqlitePool,
    reconciler: Arc<Reconciler>,
    adapter: Arc<FixtureAdapter>,
    clock: Arc<FakeClock>,
    /// Serializes syncs and clock moves.
    busy: Mutex<()>,
}

impl FixtureDriver {
    /// Restores anchor and clock from the database, or starts both at
    /// `start` (rounded down to the minute) on first use.
    pub async fn open(
        pool: SqlitePool,
        reconciler: Arc<Reconciler>,
        scenario: Scenario,
        start: DateTime<Utc>,
    ) -> Result<Self, DevError> {
        let saved = match sync_state::get(&pool, JOB, FIXTURE).await? {
            Some(SyncState {
                cursor: Some(c), ..
            }) => serde_json::from_str::<Saved>(&c).map_err(|e| DevError::State(e.to_string()))?,
            _ => {
                let t = start
                    .duration_trunc(Duration::minutes(1))
                    .map_err(|e| DevError::State(e.to_string()))?;
                Saved {
                    anchor: t,
                    clock: t,
                }
            }
        };
        let clock = Arc::new(FakeClock::new(saved.clock));
        let adapter = Arc::new(FixtureAdapter::new(scenario, saved.anchor, clock.clone()));
        let driver = Self {
            pool,
            reconciler,
            adapter,
            clock,
            busy: Mutex::new(()),
        };
        driver.save().await?;
        Ok(driver)
    }

    /// The fake clock; the app uses it as its clock in fixture mode.
    pub fn clock(&self) -> Arc<FakeClock> {
        self.clock.clone()
    }

    pub fn anchor(&self) -> DateTime<Utc> {
        self.adapter.anchor()
    }

    /// The fixture source, for searches (T09): the app passes it to the sync
    /// scheduler as a search-only source, so `/event search` finds the
    /// fixture event. Its sync stays with this driver.
    pub fn adapter(&self) -> Arc<FixtureAdapter> {
        self.adapter.clone()
    }

    /// Syncs the scenario at the current clock.
    pub async fn sync(&self) -> Result<Report, DevError> {
        let _busy = self.busy.lock().await;
        self.sync_locked().await
    }

    /// Moves the clock forward, stores it, and syncs.
    pub async fn advance(&self, by: Duration) -> Result<(DateTime<Utc>, Report), DevError> {
        let _busy = self.busy.lock().await;
        let now = self.clock.advance(by);
        self.save().await?;
        let report = self.sync_locked().await?;
        Ok((now, report))
    }

    async fn sync_locked(&self) -> Result<Report, DevError> {
        let now = self.clock.now();
        let ev = self.adapter.fetch_event(self.adapter.event_id()).await?;
        let mut report = self.reconciler.ingest_event(FIXTURE, &ev, now).await?;
        let ms = self
            .adapter
            .fetch_event_matches(self.adapter.event_id())
            .await?;
        report += self.reconciler.ingest_matches(FIXTURE, &ms, now).await?;
        let mut st = sync_state::get(&self.pool, JOB, FIXTURE)
            .await?
            .unwrap_or_else(|| SyncState::new(JOB, FIXTURE, now));
        st.last_run_at = Some(now);
        if report.failed == 0 {
            st.last_success_at = Some(now);
            st.last_error = None;
        } else {
            st.last_error = Some(format!("{} record(s) failed", report.failed));
        }
        st.updated_at = now;
        sync_state::put(&self.pool, &st).await?;
        Ok(report)
    }

    async fn save(&self) -> Result<(), DevError> {
        let now = self.clock.now();
        let saved = Saved {
            anchor: self.anchor(),
            clock: now,
        };
        let mut st = sync_state::get(&self.pool, JOB, FIXTURE)
            .await?
            .unwrap_or_else(|| SyncState::new(JOB, FIXTURE, now));
        st.cursor = Some(serde_json::to_string(&saved).expect("serializable"));
        st.updated_at = now;
        sync_state::put(&self.pool, &st).await?;
        Ok(())
    }
}

// ---- HTTP ---------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ClockBody {
    now: DateTime<Utc>,
    anchor: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<ReportBody>,
}

#[derive(Debug, Serialize)]
struct ReportBody {
    created: u32,
    linked: u32,
    updated: u32,
    queued: u32,
    failed: u32,
    events: u32,
}

impl From<Report> for ReportBody {
    fn from(r: Report) -> Self {
        Self {
            created: r.created,
            linked: r.linked,
            updated: r.updated,
            queued: r.queued,
            failed: r.failed,
            events: r.events,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdvanceReq {
    minutes: i64,
}

/// Routes `/dev/clock` and `/dev/clock/advance`, to be nested under `/api`.
pub fn routes<S>(driver: Arc<FixtureDriver>) -> Router<S> {
    Router::new()
        .route("/dev/clock", get(clock))
        .route("/dev/clock/advance", post(advance))
        .with_state(driver)
}

async fn clock(State(d): State<Arc<FixtureDriver>>) -> Json<ClockBody> {
    Json(ClockBody {
        now: d.clock.now(),
        anchor: d.anchor(),
        report: None,
    })
}

async fn advance(
    State(d): State<Arc<FixtureDriver>>,
    Json(r): Json<AdvanceReq>,
) -> Result<Json<ClockBody>, ApiError> {
    if !(1..=MAX_ADVANCE_MINUTES).contains(&r.minutes) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            format!("minutes must be between 1 and {MAX_ADVANCE_MINUTES}"),
        ));
    }
    let (now, report) = d.advance(Duration::minutes(r.minutes)).await.map_err(|e| {
        tracing::error!("fixture sync failed: {e}");
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
    })?;
    Ok(Json(ClockBody {
        now,
        anchor: d.anchor(),
        report: Some(report.into()),
    }))
}

// ---- dev sign-in (T07) ----------------------------------------------------------

/// The guild of dev sessions when neither the request nor
/// `DISCORD_GUILD_ID` names one.
pub const DEV_GUILD_ID: &str = "100000000000000000";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevSessionReq {
    user_id: String,
    #[serde(default)]
    guild_id: Option<String>,
    #[serde(default)]
    username: Option<String>,
}

/// Route `/dev/session`, to be nested under `/api`: signs a user in without
/// Discord, so the Activity can run in a normal browser against fixture
/// data. Upserts the user (keeping a stored profile) and the membership.
/// Admin routes still take `X-Dev-User`; admin status in `me` needs Discord.
pub fn session_routes() -> Router<AppState> {
    Router::new().route("/dev/session", post(dev_session))
}

async fn dev_session(
    State(st): State<AppState>,
    Json(r): Json<DevSessionReq>,
) -> Result<Response, ApiError> {
    if !is_snowflake(&r.user_id) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            "user_id must be a Discord ID",
        ));
    }
    let guild = r
        .guild_id
        .or_else(|| st.auth.config().guild_id.clone())
        .unwrap_or_else(|| DEV_GUILD_ID.to_owned());
    let guild = st.auth.check_guild_claim(Some(&guild))?;
    let db = |e| ApiError::db("dev session", e);
    let stored = store::users::get_user(&st.pool, &r.user_id)
        .await
        .map_err(db)?;
    let nick = store::users::get_member(&st.pool, &guild, &r.user_id)
        .await
        .map_err(db)?
        .and_then(|m| m.nick);
    let suffix = &r.user_id[r.user_id.len().saturating_sub(4)..];
    let user = DiscordUser {
        id: r.user_id.clone(),
        username: r
            .username
            .or_else(|| stored.as_ref().map(|u| u.username.clone()))
            .unwrap_or_else(|| format!("dev-{suffix}")),
        global_name: stored.as_ref().and_then(|u| u.global_name.clone()),
        avatar: stored.as_ref().and_then(|u| u.avatar_hash.clone()),
    };
    let session = st.auth.start_session(&user, &guild, nick).await?;
    let me = me_body(&st, &session.member).await?;
    Ok(session_response(session, None, me))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;
    use sources::fixture::FIXTURE;

    use super::*;
    use crate::test_util::{ALICE, GUILD, OTHER_GUILD, auth_app, reconciler, send, temp_db};

    #[tokio::test]
    async fn dev_sessions_skip_discord_but_not_the_guild_rule() {
        let a = auth_app(Some(GUILD)).await;
        let app = crate::router_with(
            crate::AppState::new(reconciler(a.pool.clone(), &[]), a.clock.clone())
                .with_auth(a.auth.clone()),
            std::path::Path::new("/nonexistent"),
            session_routes(),
        );
        let post =
            |body: serde_json::Value| send(&app, "POST", "/api/dev/session", &[], Some(body));

        let (status, body) = post(json!({ "user_id": ALICE })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.get("access_token"), None);
        assert_eq!(body["me"]["guild_id"], GUILD, "DISCORD_GUILD_ID by default");
        assert_eq!(body["me"]["username"], "dev-0004");
        let token = body["session_token"].as_str().unwrap();
        let (status, me) = a.get("/api/me", token).await;
        assert_eq!(
            (status, me["user_id"].as_str()),
            (StatusCode::OK, Some(ALICE))
        );
        assert!(
            !a.discord
                .calls()
                .iter()
                .any(|c| *c == "exchange" || *c == "me"),
            "no OAuth"
        );

        // A stored profile is kept.
        let (_, body) = post(json!({ "user_id": ALICE })).await;
        assert_eq!(body["me"]["username"], "dev-0004");

        let (status, _) = post(json!({ "user_id": "alice" })).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let (status, body) = post(json!({ "user_id": ALICE, "guild_id": OTHER_GUILD })).await;
        assert_eq!(
            (status, body["error"].as_str()),
            (StatusCode::FORBIDDEN, Some("wrong_guild"))
        );
    }

    fn start() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 42).unwrap()
    }

    async fn driver(pool: &SqlitePool, start: DateTime<Utc>) -> Arc<FixtureDriver> {
        let rec = reconciler(pool.clone(), &[FIXTURE]);
        Arc::new(
            FixtureDriver::open(pool.clone(), rec, Scenario::default_scenario(), start)
                .await
                .unwrap(),
        )
    }

    #[tokio::test]
    async fn advancing_the_clock_syncs_and_survives_a_restart() {
        let (pool, _dir) = temp_db().await;
        let d = driver(&pool, start()).await;
        let anchor = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
        assert_eq!(d.anchor(), anchor, "rounded down to the minute");
        let rep = d.sync().await.unwrap();
        assert_eq!((rep.created, rep.failed), (12 + 8 + 1 + 2, 0));

        let rec = reconciler(pool.clone(), &[FIXTURE]);
        let state = crate::AppState::new(rec, d.clock());
        let app = crate::router_with(
            state,
            std::path::Path::new("/nonexistent"),
            routes(d.clone()),
        );

        let (status, body) = send(&app, "GET", "/api/dev/clock", &[], None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["now"], json!(anchor));

        let (status, body) = send(
            &app,
            "POST",
            "/api/dev/clock/advance",
            &[],
            Some(json!({ "minutes": 30 })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let later = anchor + Duration::minutes(30);
        assert_eq!(body["now"], json!(later));
        assert!(body["report"]["events"].as_u64().unwrap() >= 2, "{body}");

        for minutes in [0, MAX_ADVANCE_MINUTES + 1] {
            let (status, _) = send(
                &app,
                "POST",
                "/api/dev/clock/advance",
                &[],
                Some(json!({ "minutes": minutes })),
            )
            .await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        }

        // A restart later in real time keeps the anchor and the clock.
        let d2 = driver(&pool, start() + Duration::days(3)).await;
        assert_eq!((d2.anchor(), d2.clock().now()), (anchor, later));
        let rep = d2.sync().await.unwrap();
        assert_eq!((rep.created, rep.events), (0, 0), "nothing new");
    }
}
