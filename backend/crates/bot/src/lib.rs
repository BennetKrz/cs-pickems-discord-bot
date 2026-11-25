//! Discord bot (T08): slash commands with poise, launch buttons that open
//! the Activity, and channel posts (daily digest, match results).
//!
//! - [`commands`]: `/leaderboard`, `/mypicks` and `/admin …`.
//! - [`launch`]: button clicks → pending view + `LAUNCH_ACTIVITY`; the
//!   Entry Point command → `LAUNCH_ACTIVITY` without a channel message.
//! - [`announce`]: the digest and result posts, at most once each.
//! - [`render`]: what posts say; [`discord`]: posts as Discord messages.
//! - [`schedule`]: local digest time, bundling, nonces, parsing.
//!
//! The gateway connection needs no privileged intent (only `GUILDS`, to
//! know the guilds to register commands in). Posts go over REST and don't
//! wait for the gateway. See `docs/adr/0013-bot-announcements-launch.md`
//! and `docs/adr/0019-silent-entry-point.md`.

pub mod announce;
pub mod commands;
pub mod discord;
pub mod launch;
pub mod render;
pub mod schedule;

use std::sync::Arc;
use std::time::Duration;

use domain::{Clock, DomainEvent};
use poise::serenity_prelude as serenity;
use store::SqlitePool;
use sync::{ManualService, Reconciler, SyncHandle};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::announce::Announcer;
use crate::commands::{Data, Error};

/// What the bot reads and writes.
pub struct BotDeps {
    pub pool: SqlitePool,
    /// The app clock: the fake clock in fixture mode (T05), like the lock
    /// ticker, so posts follow the simulated time.
    pub clock: Arc<dyn Clock>,
    pub manual: ManualService,
    pub reconciler: Arc<Reconciler>,
    /// The sync worker, for `/admin status`.
    pub sync: Option<SyncHandle>,
    /// Backups (P4), for `/admin backup now` and `/admin status`.
    pub backup: Option<backup::BackupService>,
    /// `DISCORD_GUILD_ID`: when set, the bot serves only this guild.
    pub guild_id: Option<String>,
}

/// First wait after the gateway failed; doubles up to [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Runs the bot until `cancel`: the gateway with the commands, and the
/// announcer. `events` is a subscription to the domain-event bus, taken
/// before anything publishes. A gateway failure (e.g. a bad token) is
/// logged and retried with backoff; it never stops the rest of the app.
pub async fn run(
    deps: BotDeps,
    token: String,
    events: broadcast::Receiver<DomainEvent>,
    cancel: CancellationToken,
) {
    let deps = Arc::new(deps);
    let http = Arc::new(serenity::Http::new(&token));
    let announcer = Announcer::new(
        deps.pool.clone(),
        deps.clock.clone(),
        Arc::new(discord::DiscordOutbox::new(http)),
        deps.guild_id.clone(),
    );
    let announcer = tokio::spawn(announcer.run(events, announce::TICK_EVERY, cancel.clone()));

    let mut backoff = FIRST_BACKOFF;
    loop {
        match gateway(deps.clone(), &token, &cancel).await {
            Ok(()) => break, // cancelled
            Err(e) => {
                tracing::error!(
                    "Discord gateway failed: {e}; retrying in {} s. Slash commands and \
                     buttons don't work until it connects (check DISCORD_BOT_TOKEN).",
                    backoff.as_secs()
                );
            }
        }
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }

    if let Err(e) = announcer.await {
        tracing::warn!("announcer task ended abnormally: {e}");
    }
    tracing::info!("bot stopped");
}

/// One gateway session. `Ok` when cancelled, `Err` when it failed.
async fn gateway(
    deps: Arc<BotDeps>,
    token: &str,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let only_guild = deps.guild_id.clone();
    let framework = poise::Framework::<Data, Error>::builder()
        .options(poise::FrameworkOptions {
            commands: commands::commands(),
            on_error: |e| Box::pin(commands::on_error(e)),
            command_check: Some(|ctx| Box::pin(commands::guild_check(ctx))),
            event_handler: |framework, event| Box::pin(on_event(framework, event)),
            ..Default::default()
        })
        .setup(move |ctx, ready, framework| {
            Box::pin(async move {
                let guilds: Vec<serenity::GuildId> = match &only_guild {
                    Some(g) => vec![serenity::GuildId::new(g.parse()?)],
                    None => ready.guilds.iter().map(|g| g.id).collect(),
                };
                for g in guilds {
                    register(ctx, &framework.options().commands, g).await;
                }
                if let Err(e) = launch::claim_entry_point(&ctx.http).await {
                    tracing::warn!(
                        "cannot switch the Entry Point command to the bot, so Discord keeps \
                         posting a message on each launch: {e}"
                    );
                }
                tracing::info!(user = %ready.user.name, "bot connected to the Discord gateway");
                Ok(deps)
            })
        })
        .build();

    let mut client = serenity::ClientBuilder::new(token, serenity::GatewayIntents::GUILDS)
        .framework(framework)
        .await?;
    let shards = client.shard_manager.clone();
    tokio::select! {
        res = client.start() => match res {
            Ok(()) => Err(anyhow::anyhow!("the gateway connection ended")),
            Err(e) => Err(e.into()),
        },
        () = cancel.cancelled() => {
            shards.shutdown_all().await;
            Ok(())
        }
    }
}

/// Registers the commands as guild commands, which update at once. The
/// app's Entry Point command is global and created by Discord; guild
/// commands don't touch it (only its handler is switched, by
/// [`launch::claim_entry_point`]).
async fn register(
    ctx: &serenity::Context,
    commands: &[poise::Command<Data, Error>],
    guild: serenity::GuildId,
) {
    match poise::builtins::register_in_guild(ctx, commands, guild).await {
        Ok(()) => tracing::info!(%guild, "slash commands registered"),
        Err(e) => tracing::error!(%guild, "cannot register slash commands: {e}"),
    }
}

async fn on_event(
    framework: poise::FrameworkContext<'_, Data, Error>,
    event: &serenity::FullEvent,
) -> Result<(), Error> {
    let ctx = framework.serenity_context;
    let data = framework.user_data;
    match event {
        serenity::FullEvent::InteractionCreate {
            interaction: serenity::Interaction::Component(c),
        } => {
            if let Err(e) = launch::handle_component(ctx, c, data).await {
                tracing::warn!(custom_id = %c.data.custom_id, "cannot launch the Activity: {e}");
            }
        }
        serenity::FullEvent::InteractionCreate {
            interaction: serenity::Interaction::Command(c),
        } if launch::is_entry_point(c) => {
            if let Err(e) = launch::handle_entry_point(ctx, c).await {
                tracing::warn!("cannot launch the Activity from the Entry Point command: {e}");
            }
        }
        // A guild added after start-up gets the commands too.
        serenity::FullEvent::GuildCreate {
            guild,
            is_new: Some(true),
        } if data
            .guild_id
            .as_deref()
            .is_none_or(|g| g == guild.id.to_string()) =>
        {
            register(ctx, &framework.options.commands, guild.id).await;
        }
        _ => {}
    }
    Ok(())
}
