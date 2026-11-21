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

/// Routes `/me`, to be nested under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new().route("/me", get(me))
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
