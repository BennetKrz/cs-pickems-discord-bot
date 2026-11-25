//! Launch buttons (T08): every bot post and reply carries buttons that
//! open the Activity. Discord launches it when the interaction is answered
//! with `LAUNCH_ACTIVITY` (callback type 12).
//!
//! Discord documents no launch parameters, so a *Pick now* button for a
//! match stores a pending view (member → match, 10 min); the Activity takes
//! it on start through `GET /api/me/pending-view` and opens that match.
//!
//! The Entry Point command (App Launcher) is switched to `APP_HANDLER`, so
//! the bot launches the Activity itself and Discord doesn't post a "… used
//! Launch" message in the channel on every launch (ADR 0019).

use chrono::{DateTime, Utc};
use poise::serenity_prelude as serenity;
use store::SqlitePool;

use crate::BotDeps;
use crate::render::ButtonAction;
use crate::schedule::PENDING_VIEW_TTL;

/// Stores the pending view of an *open match* button. Returns whether one
/// was stored: not for the plain launch button, and not for a match that
/// doesn't exist (any more).
pub async fn remember_view(
    pool: &SqlitePool,
    guild_id: &str,
    user_id: &str,
    action: ButtonAction,
    now: DateTime<Utc>,
) -> Result<bool, sqlx::Error> {
    let ButtonAction::Open(match_id) = action else {
        return Ok(false);
    };
    if store::matches::get(pool, match_id).await?.is_none() {
        return Ok(false);
    }
    store::pending_views::put(pool, guild_id, user_id, match_id, now + PENDING_VIEW_TTL).await?;
    Ok(true)
}

/// Answers a button click: remembers the view, then launches the Activity.
/// Clicks on buttons that aren't ours are ignored.
pub async fn handle_component(
    ctx: &serenity::Context,
    c: &serenity::ComponentInteraction,
    deps: &BotDeps,
) -> anyhow::Result<()> {
    let Some(action) = ButtonAction::parse(&c.data.custom_id) else {
        return Ok(());
    };
    if let Some(guild) = c.guild_id.map(|g| g.to_string())
        && deps.guild_id.as_deref().is_none_or(|only| only == guild)
    {
        let user = c.user.id.to_string();
        // Opening the Activity matters more than the deep link: on a
        // database error, launch anyway on the default view.
        if let Err(e) = remember_view(&deps.pool, &guild, &user, action, deps.clock.now()).await {
            tracing::warn!(%guild, "cannot store the pending view: {e}");
        }
    }
    c.create_response(
        &ctx.http,
        serenity::CreateInteractionResponse::LaunchActivity,
    )
    .await?;
    Ok(())
}

/// Whether this command interaction is the app's Entry Point command (the
/// App Launcher's "Launch").
pub fn is_entry_point(c: &serenity::CommandInteraction) -> bool {
    c.data.kind == serenity::CommandType::PrimaryEntryPoint
}

/// The Entry Point command that Discord still handles itself (and so
/// announces every launch in the channel), if any.
pub fn entry_point_to_claim(commands: &[serenity::Command]) -> Option<serenity::CommandId> {
    commands
        .iter()
        .find(|c| c.kind == serenity::CommandType::PrimaryEntryPoint)
        .filter(|c| c.handler != Some(serenity::EntryPointHandlerType::AppHandler))
        .map(|c| c.id)
}

/// Switches the global Entry Point command to `APP_HANDLER`. Only `handler`
/// is sent, so its name and description stay as they are. Does nothing when
/// it is already switched.
pub async fn claim_entry_point(http: &serenity::Http) -> anyhow::Result<()> {
    let commands = http.get_global_commands().await?;
    let Some(id) = entry_point_to_claim(&commands) else {
        return Ok(());
    };
    let body = serde_json::json!({ "handler": serenity::EntryPointHandlerType::AppHandler });
    http.edit_global_command(id, &body).await?;
    tracing::info!("the Entry Point command now launches the Activity without a channel message");
    Ok(())
}

/// Answers the Entry Point command by launching the Activity. Unlike
/// Discord's own handler, this posts nothing in the channel.
pub async fn handle_entry_point(
    ctx: &serenity::Context,
    c: &serenity::CommandInteraction,
) -> anyhow::Result<()> {
    c.create_response(
        &ctx.http,
        serenity::CreateInteractionResponse::LaunchActivity,
    )
    .await?;
    Ok(())
}
