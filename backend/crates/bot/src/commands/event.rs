//! `/event …` (T09): admins choose which events are synced and pickable.
//! Every subcommand runs behind [`super::admin_check`], set on the `/event`
//! root. Replies are ephemeral.
//!
//! - `search` asks every source that can search (budgeted, cached 24 h by
//!   the sync worker) and follows the event chosen in a select menu.
//! - `follow` follows an event that is already in the database (synced,
//!   linked or entered before) without a search.
//! - `unfollow` asks for confirmation when the server has picks on it.
//! - `list` shows the followed events.
//!
//! Select menus and buttons are answered by a collector in the command
//! itself. Their custom IDs start with [`CUSTOM_PREFIX`] and carry the
//! command's interaction ID, so they never reach the launch handler and
//! never mix between invocations.

use std::time::Duration;

use domain::{EntityType, EventId};
use poise::serenity_prelude as serenity;
use store::events::FollowFilter;
use sync::{FollowError, SearchHit};

use super::{Context, Error, admin_check, guild_of, say, user};
use crate::discord;
use crate::render::events::{self as render, HitRow, MAX_OPTIONS};
use crate::render::truncate;

/// Prefix of the select menu and button IDs of `/event` commands.
pub const CUSTOM_PREFIX: &str = "event:";
/// How long a search result can be chosen.
const SELECT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long the unfollow question waits for an answer.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// What a component of an `/event` reply does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventComponent {
    Select,
    Confirm,
    Cancel,
}

impl EventComponent {
    fn as_str(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Confirm => "confirm",
            Self::Cancel => "cancel",
        }
    }

    /// The custom ID of this component on the reply to `interaction`.
    pub fn custom_id(self, interaction: u64) -> String {
        format!("{CUSTOM_PREFIX}{interaction}:{}", self.as_str())
    }

    /// Parses a custom ID made by [`Self::custom_id`].
    pub fn parse(custom_id: &str) -> Option<(u64, Self)> {
        let (id, kind) = custom_id.strip_prefix(CUSTOM_PREFIX)?.split_once(':')?;
        let kind = match kind {
            "select" => Self::Select,
            "confirm" => Self::Confirm,
            "cancel" => Self::Cancel,
            _ => return None,
        };
        Some((id.parse().ok()?, kind))
    }
}

/// Choose which events are synced and can be picked.
#[poise::command(
    slash_command,
    guild_only,
    check = "admin_check",
    subcommands("search", "follow", "unfollow", "list"),
    subcommand_required
)]
pub async fn event(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

fn follow_error(e: FollowError) -> Error {
    match e {
        FollowError::NeedsLink { source_id, name } => user(format!(
            "**{name}** comes from `{source_id}`, which can't add new events by itself. \
             Link it to a known event with `/admin link`, then follow that with `/event follow`."
        )),
        FollowError::UnknownEvent(_) => user("That event doesn't exist (any more)."),
        e @ (FollowError::Db(_) | FollowError::Reconcile(_)) => e.into(),
    }
}

/// The interaction ID of a slash command, for unique component IDs.
fn interaction_id(ctx: Context<'_>) -> u64 {
    match ctx {
        poise::Context::Application(a) => a.interaction.id.get(),
        poise::Context::Prefix(p) => p.msg.id.get(),
    }
}

/// Search the data sources for an event and follow one of the results.
#[poise::command(slash_command)]
async fn search(
    ctx: Context<'_>,
    #[description = "Part of the event's name, e.g. IEM Cologne"]
    #[min_length = 2]
    #[max_length = 100]
    name: String,
) -> Result<(), Error> {
    let guild = guild_of(ctx)?;
    let data = ctx.data();
    let Some(sync) = data.sync.clone() else {
        return say(
            ctx,
            "The sync worker is not running, so searching is not possible.",
        )
        .await;
    };
    // A source may take a few seconds to answer.
    ctx.defer_ephemeral().await?;
    let out = sync.search(&name).await;

    let hits: Vec<SearchHit> = out.hits;
    let mut followed = Vec::with_capacity(hits.len());
    for h in &hits {
        let known =
            store::refs::resolve(&data.pool, h.source, EntityType::Event, &h.event.dto.ext_id)
                .await?;
        let f = match known {
            Some(id) => store::guilds::is_followed(&data.pool, &guild, EventId(id)).await?,
            None => false,
        };
        followed.push(f);
    }
    let rows: Vec<HitRow<'_>> = hits
        .iter()
        .zip(&followed)
        .map(|(h, &followed)| HitRow {
            name: &h.event.dto.name,
            tier: h.event.dto.tier,
            begin_at: h.event.dto.begin_at,
            end_at: h.event.dto.end_at,
            source: h.source,
            followed,
        })
        .collect();
    let skipped: Vec<(&str, String)> = out.skipped.iter().map(|(s, w)| (*s, w.clone())).collect();
    let post = render::search_results(&name, &rows, &skipped);
    if rows.is_empty() {
        ctx.send(discord::reply(&post, true)).await?;
        return Ok(());
    }

    let select_id = EventComponent::Select.custom_id(interaction_id(ctx));
    let options = rows
        .iter()
        .take(MAX_OPTIONS)
        .enumerate()
        .map(|(i, r)| {
            let (label, description) = render::hit_option(r);
            serenity::CreateSelectMenuOption::new(label, i.to_string()).description(description)
        })
        .collect();
    let menu = serenity::CreateSelectMenu::new(
        &select_id,
        serenity::CreateSelectMenuKind::String { options },
    )
    .placeholder("Follow an event…");
    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(discord::embed(&post))
                .components(vec![serenity::CreateActionRow::SelectMenu(menu)])
                .ephemeral(true),
        )
        .await?;

    let picked = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
        .author_id(ctx.author().id)
        .custom_ids(vec![select_id])
        .timeout(SELECT_TIMEOUT)
        .await;
    let Some(c) = picked else {
        // Time is up: drop the menu, keep the list.
        reply
            .edit(
                ctx,
                poise::CreateReply::default()
                    .embed(discord::embed(&post))
                    .components(Vec::new()),
            )
            .await?;
        return Ok(());
    };
    let index = match &c.data.kind {
        serenity::ComponentInteractionDataKind::StringSelect { values } => {
            values.first().and_then(|v| v.parse::<usize>().ok())
        }
        _ => None,
    };
    let Some(hit) = index.and_then(|i| hits.get(i)) else {
        return Err(user("That choice is not in the list. Search again."));
    };

    let admin = ctx.author().id.to_string();
    let result = sync::follow::follow_hit(
        &data.reconciler,
        Some(&sync),
        &guild,
        &admin,
        hit,
        data.clock.now(),
    )
    .await;
    let message = match result {
        Ok(o) => {
            let p = render::followed(&o.event, o.newly_followed);
            serenity::CreateInteractionResponseMessage::new()
                .embed(discord::embed(&p))
                .components(discord::components(&p))
        }
        Err(e) => {
            let text = match follow_error(e).downcast::<super::UserError>() {
                Ok(u) => u.0,
                Err(e) => {
                    tracing::error!("cannot follow the event: {e:#}");
                    super::INTERNAL.to_owned()
                }
            };
            serenity::CreateInteractionResponseMessage::new()
                .content(text)
                .embeds(Vec::new())
                .components(Vec::new())
        }
    };
    c.create_response(
        ctx.serenity_context(),
        serenity::CreateInteractionResponse::UpdateMessage(message),
    )
    .await?;
    Ok(())
}

/// Known events this guild does (`followed`) or doesn't follow whose name
/// contains `partial`.
async fn autocomplete_events(
    ctx: Context<'_>,
    partial: &str,
    filter: FollowFilter,
) -> serenity::CreateAutocompleteResponse {
    let mut r = serenity::CreateAutocompleteResponse::new();
    let Some(guild) = ctx.guild_id().map(|g| g.to_string()) else {
        return r;
    };
    let events = store::events::search_known(
        &ctx.data().pool,
        &guild,
        partial,
        filter,
        MAX_OPTIONS as i64,
    )
    .await
    .unwrap_or_default();
    for e in events {
        let when = e
            .begin_at
            .map(|t| t.format(" · %-d %b %Y").to_string())
            .unwrap_or_default();
        let label = format!("{} · {}{when}", e.name, render::tier_label(Some(e.tier)));
        r = r.add_int_choice(truncate(&label, 100), e.id.0);
    }
    r
}

async fn autocomplete_unfollowed(
    ctx: Context<'_>,
    partial: &str,
) -> serenity::CreateAutocompleteResponse {
    autocomplete_events(ctx, partial, FollowFilter::NotFollowed).await
}

async fn autocomplete_followed(
    ctx: Context<'_>,
    partial: &str,
) -> serenity::CreateAutocompleteResponse {
    autocomplete_events(ctx, partial, FollowFilter::Followed).await
}

/// Follow an event that is already known, without a search.
#[poise::command(slash_command)]
async fn follow(
    ctx: Context<'_>,
    #[description = "The event"]
    #[autocomplete = "autocomplete_unfollowed"]
    event: i64,
) -> Result<(), Error> {
    let guild = guild_of(ctx)?;
    let data = ctx.data();
    let o = sync::follow::follow(
        &data.pool,
        data.sync.as_ref(),
        &guild,
        &ctx.author().id.to_string(),
        EventId(event),
        data.clock.now(),
    )
    .await
    .map_err(follow_error)?;
    ctx.send(discord::reply(
        &render::followed(&o.event, o.newly_followed),
        true,
    ))
    .await?;
    Ok(())
}

/// Stop following an event; picks, points and history stay.
#[poise::command(slash_command)]
async fn unfollow(
    ctx: Context<'_>,
    #[description = "The event"]
    #[autocomplete = "autocomplete_followed"]
    event: i64,
) -> Result<(), Error> {
    let guild = guild_of(ctx)?;
    let data = ctx.data();
    let id = EventId(event);
    let Some(ev) = store::events::get(&data.pool, id).await? else {
        return Err(user("That event doesn't exist (any more)."));
    };
    if !store::guilds::is_followed(&data.pool, &guild, id).await? {
        return say(ctx, render::unfollowed(&ev.name, false)).await;
    }
    let admin = ctx.author().id.to_string();
    let picks = store::picks::count_in_event(&data.pool, &guild, id).await?;
    if picks == 0 {
        let o = sync::follow::unfollow(&data.pool, &guild, &admin, id, data.clock.now())
            .await
            .map_err(follow_error)?;
        return say(ctx, render::unfollowed(&o.event.name, o.was_followed)).await;
    }

    let base = interaction_id(ctx);
    let (confirm_id, cancel_id) = (
        EventComponent::Confirm.custom_id(base),
        EventComponent::Cancel.custom_id(base),
    );
    let buttons = serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(&confirm_id)
            .label("Unfollow")
            .style(serenity::ButtonStyle::Danger),
        serenity::CreateButton::new(&cancel_id)
            .label("Keep following")
            .style(serenity::ButtonStyle::Secondary),
    ]);
    let reply = ctx
        .send(
            poise::CreateReply::default()
                .content(render::unfollow_prompt(&ev.name, picks))
                .components(vec![buttons])
                .ephemeral(true),
        )
        .await?;
    let answer = serenity::ComponentInteractionCollector::new(ctx.serenity_context())
        .author_id(ctx.author().id)
        .custom_ids(vec![confirm_id, cancel_id])
        .timeout(CONFIRM_TIMEOUT)
        .await;
    let Some(c) = answer else {
        reply
            .edit(
                ctx,
                poise::CreateReply::default()
                    .content(format!("No answer: **{}** is still followed.", ev.name))
                    .components(Vec::new()),
            )
            .await?;
        return Ok(());
    };
    let text = match EventComponent::parse(&c.data.custom_id) {
        Some((_, EventComponent::Confirm)) => {
            let o = sync::follow::unfollow(&data.pool, &guild, &admin, id, data.clock.now())
                .await
                .map_err(follow_error)?;
            render::unfollowed(&o.event.name, o.was_followed)
        }
        _ => format!("Nothing changed: **{}** is still followed.", ev.name),
    };
    c.create_response(
        ctx.serenity_context(),
        serenity::CreateInteractionResponse::UpdateMessage(
            serenity::CreateInteractionResponseMessage::new()
                .content(text)
                .components(Vec::new()),
        ),
    )
    .await?;
    Ok(())
}

/// The events this server follows.
#[poise::command(slash_command)]
async fn list(ctx: Context<'_>) -> Result<(), Error> {
    let guild = guild_of(ctx)?;
    let rows = store::guilds::followed_overview(&ctx.data().pool, &guild).await?;
    ctx.send(discord::reply(&render::followed_list(&rows), true))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::ButtonAction;

    #[test]
    fn component_ids_round_trip_and_stay_away_from_launch_buttons() {
        for kind in [
            EventComponent::Select,
            EventComponent::Confirm,
            EventComponent::Cancel,
        ] {
            let id = kind.custom_id(1_234_567_890_123);
            assert!(id.len() <= 100, "Discord's custom ID limit");
            assert_eq!(EventComponent::parse(&id), Some((1_234_567_890_123, kind)));
            assert_eq!(ButtonAction::parse(&id), None, "{id}");
        }
        assert_eq!(EventComponent::parse("event:12:nope"), None);
        assert_eq!(EventComponent::parse("event:x:select"), None);
        assert_eq!(EventComponent::parse("open:12"), None);
    }
}
