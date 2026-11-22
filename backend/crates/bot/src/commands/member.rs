//! Member commands: `/leaderboard` and `/mypicks`. Both reply ephemerally
//! with a short summary and a button that opens the Activity.

use std::collections::HashMap;

use domain::MatchId;
use store::matches::{ListQuery, Segment};

use super::{Context, Error, guild_of};
use crate::discord;
use crate::render;

/// Rows on `/leaderboard`.
const TOP: usize = 10;
/// Open matches `/mypicks` looks at.
const OPEN_LIMIT: i64 = 100;

/// The server's top 10 pickers.
#[poise::command(slash_command, guild_only)]
pub async fn leaderboard(ctx: Context<'_>) -> Result<(), Error> {
    let guild = guild_of(ctx)?;
    let pool = &ctx.data().pool;
    // T14 replaces this with the leaderboard query and its tie-break.
    let totals: Vec<_> = store::scoring::totals_for_guild(pool, &guild)
        .await?
        .into_iter()
        .take(TOP)
        .collect();
    let ids: Vec<String> = totals.iter().map(|t| t.user_id.clone()).collect();
    let names: HashMap<String, String> = store::bot::display_names(pool, &guild, &ids)
        .await?
        .into_iter()
        .collect();
    let rows: Vec<(String, i64)> = totals
        .into_iter()
        .map(|t| {
            let name = names.get(&t.user_id).cloned().unwrap_or(t.user_id);
            (name, t.points)
        })
        .collect();
    ctx.send(discord::reply(&render::leaderboard(&rows), true))
        .await?;
    Ok(())
}

/// Your picks on matches that are still open.
#[poise::command(slash_command, guild_only)]
pub async fn mypicks(ctx: Context<'_>) -> Result<(), Error> {
    let guild = guild_of(ctx)?;
    let user = ctx.author().id.to_string();
    let data = ctx.data();
    let now = data.clock.now();
    let upcoming = store::matches::list_view(
        &data.pool,
        &ListQuery {
            guild_id: &guild,
            segment: Segment::Upcoming,
            bound: None,
            event_id: None,
            after: None,
            limit: OPEN_LIMIT,
        },
    )
    .await?;
    let open: Vec<_> = upcoming
        .into_iter()
        .filter(|i| render::is_pickable(i, now))
        .collect();
    let ids: Vec<MatchId> = open.iter().map(|i| i.m.id).collect();
    let mut conn = data.pool.acquire().await?;
    let picks: HashMap<MatchId, domain::PickParts> =
        store::picks::for_user_matches(&mut conn, &guild, &user, &ids)
            .await?
            .into_iter()
            .map(|(p, _)| (p.match_id, p.parts))
            .collect();
    drop(conn);
    let mine: Vec<_> = open
        .iter()
        .filter_map(|i| picks.get(&i.m.id).map(|p| (i, p)))
        .collect();
    let missing = open.len() - mine.len();
    ctx.send(discord::reply(&render::my_picks(&mine, missing), true))
        .await?;
    Ok(())
}
