//! Slash commands (T08). Member commands reply ephemerally with a launch
//! button; everything else happens in the Activity. Admin commands live
//! under `/admin`, whose check (the admin rule of T07, applied to the
//! interaction) runs before every subcommand: poise runs the checks of
//! every parent command.
//!
//! Commands are registered as guild commands, so changes show up at once.
//! `/event …` (T09) sits next to `/admin`, behind the same check.

pub mod admin;
pub mod event;
pub mod member;

use std::sync::Arc;

use poise::serenity_prelude as serenity;

use crate::BotDeps;

/// Shared state of the commands.
pub type Data = Arc<BotDeps>;
pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, Data, Error>;

/// The reply to a non-admin who runs an admin command.
pub const NOT_ADMIN: &str = "Only server admins can do this.";
/// The reply when a command fails for a reason the user can't fix.
pub const INTERNAL: &str = "Something went wrong. Try again in a moment.";

/// An error whose message is meant for the user (bad input, a match that
/// doesn't allow the action). Shown as is, ephemerally.
#[derive(Debug)]
pub struct UserError(pub String);

impl std::fmt::Display for UserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UserError {}

/// Shorthand: `Err(user("…"))`.
pub fn user(msg: impl Into<String>) -> Error {
    UserError(msg.into()).into()
}

/// Every command the bot registers.
pub fn commands() -> Vec<poise::Command<Data, Error>> {
    vec![
        member::leaderboard(),
        member::mypicks(),
        admin::admin(),
        event::event(),
    ]
}

/// Whether the member behind an interaction is an admin: the rule of T07
/// (`domain::admin`) on the interaction's computed permissions and roles,
/// with the configured admin role read fresh from the database.
pub fn admin_allowed(
    permissions: Option<serenity::Permissions>,
    roles: &[serenity::RoleId],
    admin_role_id: Option<&str>,
) -> bool {
    let roles: Vec<String> = roles.iter().map(|r| r.get().to_string()).collect();
    domain::admin::is_admin_from_interaction(permissions.map(|p| p.bits()), &roles, admin_role_id)
}

/// The check on `/admin`.
pub async fn admin_check(ctx: Context<'_>) -> Result<bool, Error> {
    let (Some(guild_id), poise::Context::Application(app)) = (ctx.guild_id(), ctx) else {
        return Ok(false);
    };
    let Some(member) = app.interaction.member.as_ref() else {
        return Ok(false);
    };
    let admin_role = store::guilds::get_settings(&ctx.data().pool, &guild_id.to_string())
        .await?
        .and_then(|s| s.admin_role_id);
    Ok(admin_allowed(
        member.permissions,
        &member.roles,
        admin_role.as_deref(),
    ))
}

/// The global check: with `DISCORD_GUILD_ID` set, commands work only there.
pub async fn guild_check(ctx: Context<'_>) -> Result<bool, Error> {
    let allowed = match (&ctx.data().guild_id, ctx.guild_id()) {
        (None, _) => true,
        (Some(only), Some(g)) => *only == g.to_string(),
        (Some(_), None) => false,
    };
    if allowed {
        Ok(true)
    } else {
        Err(user("Use this in the server the pick'em runs in."))
    }
}

/// What the user sees for a failed command, if anything.
pub fn error_text(error: &poise::FrameworkError<'_, Data, Error>) -> Option<String> {
    match error {
        poise::FrameworkError::CommandCheckFailed { error: None, .. } => Some(NOT_ADMIN.to_owned()),
        poise::FrameworkError::CommandCheckFailed { error: Some(e), .. }
        | poise::FrameworkError::Command { error: e, .. } => {
            Some(match e.downcast_ref::<UserError>() {
                Some(u) => u.0.clone(),
                None => INTERNAL.to_owned(),
            })
        }
        poise::FrameworkError::ArgumentParse { error, .. } => {
            Some(format!("That input doesn't work: {error}"))
        }
        poise::FrameworkError::GuildOnly { .. } => {
            Some("Use this in the server the pick'em runs in.".to_owned())
        }
        _ => None,
    }
}

/// Replies ephemerally to failed commands and logs unexpected errors.
pub async fn on_error(error: poise::FrameworkError<'_, Data, Error>) {
    if let poise::FrameworkError::Command { error: e, ctx, .. } = &error
        && e.downcast_ref::<UserError>().is_none()
    {
        tracing::error!(command = %ctx.command().qualified_name, "command failed: {e:#}");
    }
    match (error_text(&error), error.ctx()) {
        (Some(text), Some(ctx)) => {
            let reply = poise::CreateReply::default().content(text).ephemeral(true);
            if let Err(e) = ctx.send(reply).await {
                tracing::warn!("cannot send the error reply: {e}");
            }
        }
        _ => {
            if let Err(e) = poise::builtins::on_error(error).await {
                tracing::warn!("error while handling a framework error: {e}");
            }
        }
    }
}

/// The guild of a guild-only command as a string.
pub fn guild_of(ctx: Context<'_>) -> Result<String, Error> {
    ctx.guild_id()
        .map(|g| g.to_string())
        .ok_or_else(|| user("Use this in the server the pick'em runs in."))
}

/// Sends an ephemeral text reply.
pub async fn say(ctx: Context<'_>, text: impl Into<String>) -> Result<(), Error> {
    ctx.send(
        poise::CreateReply::default()
            .content(text.into())
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every leaf command with its full path and whether a check guards it
    /// (its own or a parent's).
    fn leaves(
        cmds: &[poise::Command<Data, Error>],
        path: &str,
        guarded: bool,
        out: &mut Vec<(String, bool)>,
    ) {
        for c in cmds {
            let p = format!("{path}/{}", c.name);
            let g = guarded || !c.checks.is_empty();
            if c.subcommands.is_empty() {
                out.push((p, g));
            } else {
                assert!(c.subcommand_required, "{p}: groups need a subcommand");
                leaves(&c.subcommands, &p, g, out);
            }
        }
    }

    #[test]
    fn every_admin_command_is_behind_the_admin_check() {
        let cmds = commands();
        for root in ["admin", "event"] {
            let c = cmds.iter().find(|c| c.name == root).unwrap();
            assert_eq!(c.checks.len(), 1, "/{root} has exactly the admin check");
            assert!(c.guild_only, "/{root}");
        }

        let mut all = Vec::new();
        leaves(&cmds, "", false, &mut all);
        let (admin_cmds, member_cmds): (Vec<_>, Vec<_>) = all
            .iter()
            .partition(|(p, _)| p.starts_with("/admin/") || p.starts_with("/event/"));
        for (p, guarded) in &admin_cmds {
            assert!(guarded, "{p} is not guarded");
        }
        // Everything outside /admin is a member command, unguarded.
        let members: Vec<&str> = member_cmds.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(members, ["/leaderboard", "/mypicks"]);
        let paths: Vec<&str> = admin_cmds.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            [
                "/admin/channel/set",
                "/admin/role/set",
                "/admin/digest/set",
                "/admin/status",
                "/admin/backup/now",
                "/admin/link/list",
                "/admin/link/merge",
                "/admin/link/create",
                "/admin/match/result",
                "/admin/match/clear-override",
                "/admin/match/void",
                "/admin/match/unvoid",
                "/admin/match/reopen",
                "/event/search",
                "/event/follow",
                "/event/unfollow",
                "/event/list",
            ]
        );
    }

    #[test]
    fn commands_have_slash_actions_and_short_descriptions() {
        fn walk(cmds: &[poise::Command<Data, Error>]) {
            for c in cmds {
                assert!(
                    c.slash_action.is_some() || !c.subcommands.is_empty(),
                    "{}",
                    c.name
                );
                let d = c.description.as_deref().unwrap_or_default();
                assert!(!d.is_empty() && d.len() <= 100, "{}: {d:?}", c.name);
                assert!(!d.to_lowercase().contains("bet"), "{}: {d}", c.name);
                walk(&c.subcommands);
            }
        }
        walk(&commands());
    }

    #[test]
    fn admin_rule_on_interaction_data() {
        use serenity::{Permissions, RoleId};
        let roles = [RoleId::new(10), RoleId::new(13)];
        let send = Some(Permissions::SEND_MESSAGES);
        assert!(admin_allowed(Some(Permissions::MANAGE_GUILD), &[], None));
        assert!(admin_allowed(Some(Permissions::ADMINISTRATOR), &[], None));
        assert!(admin_allowed(send, &roles, Some("13")));
        assert!(!admin_allowed(send, &roles, Some("14")));
        assert!(!admin_allowed(send, &roles, None));
        assert!(!admin_allowed(None, &[RoleId::new(10)], None));
    }
}
