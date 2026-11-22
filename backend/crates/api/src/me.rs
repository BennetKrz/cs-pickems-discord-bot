//! `GET /api/me`: the signed-in member as the Activity shows them (T07).

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::AppState;
use crate::auth::Member;
use crate::error::ApiError;
use crate::img;

/// The signed-in member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MeBody {
    pub user_id: String,
    pub guild_id: String,
    pub username: String,
    /// Server nickname, else global display name, else username.
    pub display_name: String,
    /// Relative URL of the avatar through the image proxy.
    pub avatar_url: String,
    pub is_admin: bool,
}

/// Routes `/me` and `/me/pending-view`, to be nested under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me", get(me))
        .route("/me/pending-view", get(pending_view))
}

/// Which view the Activity opens on start (T08).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PendingViewBody {
    /// The match whose *Pick now* button the member clicked in the last
    /// 10 minutes, if any.
    pub match_id: Option<i64>,
}

/// `GET /api/me/pending-view`: takes the member's pending view, so it
/// opens once. Discord documents no launch parameters, so bot buttons
/// store the view server-side instead (see `bot::launch`).
async fn pending_view(
    State(st): State<AppState>,
    member: Member,
) -> Result<Json<PendingViewBody>, ApiError> {
    let match_id =
        store::pending_views::take(&st.pool, &member.guild_id, &member.user_id, st.clock.now())
            .await
            .map_err(|e| ApiError::db("pending view", e))?;
    Ok(Json(PendingViewBody {
        match_id: match_id.map(|m| m.0),
    }))
}

async fn me(State(st): State<AppState>, member: Member) -> Result<Json<MeBody>, ApiError> {
    Ok(Json(me_body(&st, &member).await?))
}

/// The name the Activity shows for a member: server nickname, else global
/// display name, else username. Blank names are skipped.
pub fn display_name(nick: Option<&str>, global_name: Option<&str>, username: &str) -> String {
    [nick, global_name]
        .into_iter()
        .flatten()
        .find(|n| !n.trim().is_empty())
        .unwrap_or(username)
        .to_owned()
}

/// Builds the member's profile from the database. Admin status comes from
/// the admin check; if Discord can't be asked, it is `false` for now.
pub async fn me_body(st: &AppState, m: &Member) -> Result<MeBody, ApiError> {
    let user = store::users::get_user(&st.pool, &m.user_id)
        .await
        .map_err(|e| ApiError::db("me", e))?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
                "unknown user; sign in again",
            )
        })?;
    let nick = store::users::get_member(&st.pool, &m.guild_id, &m.user_id)
        .await
        .map_err(|e| ApiError::db("me", e))?
        .and_then(|gm| gm.nick);
    let is_admin = match st.auth.is_admin(&m.guild_id, &m.user_id).await {
        Ok(admin) => admin,
        Err(e) => {
            // The cause is logged where Discord failed.
            tracing::debug!(user = %m.user_id, "admin check failed, showing as member: {e:?}");
            false
        }
    };
    let display_name = display_name(nick.as_deref(), user.global_name.as_deref(), &user.username);
    Ok(MeBody {
        avatar_url: img::avatar_url(&user.discord_user_id, user.avatar_hash.as_deref()),
        user_id: user.discord_user_id,
        guild_id: m.guild_id.clone(),
        username: user.username,
        display_name,
        is_admin,
    })
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use chrono::Duration;
    use domain::{
        EventStatus, EventTier, FakeClock, MatchFields, MatchFormat, MatchId, NewEvent, NewTeam,
    };
    use serde_json::json;

    use crate::test_util::{ALICE, AuthApp, GUILD, OTHER_GUILD, PICKER, auth_app, send, t0};

    async fn a_match(app: &AuthApp) -> MatchId {
        let ev = store::events::insert(
            &app.pool,
            &NewEvent {
                name: "IEM".into(),
                tier: EventTier::S,
                begin_at: None,
                end_at: None,
                status: EventStatus::Running,
            },
        )
        .await
        .unwrap();
        let team = |name: &'static str| {
            let pool = app.pool.clone();
            async move {
                store::teams::insert(
                    &pool,
                    &NewTeam {
                        name: name.into(),
                        acronym: None,
                        logo_url: None,
                        country: None,
                    },
                )
                .await
                .unwrap()
            }
        };
        let (a, b) = (team("A").await, team("B").await);
        store::matches::insert(
            &app.pool,
            &MatchFields::scheduled(ev, Some(a), Some(b), MatchFormat::Bo3, Some(t0())),
        )
        .await
        .unwrap()
    }

    fn advance(clock: &FakeClock, by: Duration) {
        clock.set(domain::Clock::now(clock) + by);
    }

    #[tokio::test]
    async fn pending_view_opens_once_for_its_member_and_guild() {
        let app = auth_app(None).await;
        let m = a_match(&app).await;
        let alice = app.sign_in(ALICE, GUILD).await;
        let alice_other = app.sign_in(ALICE, OTHER_GUILD).await;
        let picker = app.sign_in(PICKER, GUILD).await;

        // Nothing pending.
        let (s, body) = app.get("/api/me/pending-view", &alice).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body, json!({ "match_id": null }));

        // Alice clicked *Pick now* in GUILD.
        store::pending_views::put(&app.pool, GUILD, ALICE, m, t0() + Duration::minutes(10))
            .await
            .unwrap();
        // Not for another member, nor for Alice in another guild.
        let (_, body) = app.get("/api/me/pending-view", &picker).await;
        assert_eq!(body, json!({ "match_id": null }));
        let (_, body) = app.get("/api/me/pending-view", &alice_other).await;
        assert_eq!(body, json!({ "match_id": null }));
        // Alice gets it once.
        let (_, body) = app.get("/api/me/pending-view", &alice).await;
        assert_eq!(body, json!({ "match_id": m.0 }));
        let (_, body) = app.get("/api/me/pending-view", &alice).await;
        assert_eq!(body, json!({ "match_id": null }));
    }

    #[tokio::test]
    async fn pending_view_expires_after_ten_minutes() {
        let app = auth_app(None).await;
        let m = a_match(&app).await;
        let alice = app.sign_in(ALICE, GUILD).await;
        store::pending_views::put(&app.pool, GUILD, ALICE, m, t0() + Duration::minutes(10))
            .await
            .unwrap();
        advance(&app.clock, Duration::minutes(10));
        let (_, body) = app.get("/api/me/pending-view", &alice).await;
        assert_eq!(body, json!({ "match_id": null }));

        // A newer click replaces an older one.
        let later = t0() + Duration::minutes(25);
        store::pending_views::put(&app.pool, GUILD, ALICE, m, later)
            .await
            .unwrap();
        advance(&app.clock, Duration::minutes(5));
        let (_, body) = app.get("/api/me/pending-view", &alice).await;
        assert_eq!(body, json!({ "match_id": m.0 }));
    }

    #[tokio::test]
    async fn pending_view_needs_a_session() {
        let app = auth_app(None).await;
        let (s, _) = send(&app.app, "GET", "/api/me/pending-view", &[], None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
}
