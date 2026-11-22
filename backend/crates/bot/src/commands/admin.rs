//! `/admin …`: settings, sync status, the unlinked queue (T03) and manual
//! match actions (T05). Every subcommand runs behind [`super::admin_check`],
//! which is set on the `/admin` root. Replies are ephemeral.

use chrono::{DateTime, Duration, Utc};
use domain::{EntityType, MapStatus, MatchId, MatchStatus};
use poise::serenity_prelude as serenity;
use store::matches::{ListQuery, MatchListItem, Segment};
use sync::{ManualError, Outcome, Override, OverrideField};

use super::{Context, Error, admin_check, guild_of, say, user};
use crate::render::{team_label, ts};
use crate::schedule;

/// Server admin tools for the pick'em.
#[poise::command(
    slash_command,
    guild_only,
    check = "admin_check",
    subcommands("channel", "role", "digest", "status", "backup", "link", "match_"),
    subcommand_required
)]
pub async fn admin(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

// ---- settings -----------------------------------------------------------------

/// The channel for the daily digest and results.
#[poise::command(slash_command, subcommands("channel_set"), subcommand_required)]
async fn channel(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Post the daily digest and results in this channel.
#[poise::command(slash_command, rename = "set")]
async fn channel_set(
    ctx: Context<'_>,
    #[description = "Where the bot posts"]
    #[channel_types("Text", "News")]
    channel: serenity::ChannelId,
) -> Result<(), Error> {
    let guild = guild_of(ctx)?;
    let data = ctx.data();
    store::guilds::set_announce_channel(
        &data.pool,
        &guild,
        &channel.get().to_string(),
        data.clock.now(),
    )
    .await?;
    audit(
        ctx,
        "set_announce_channel",
        "guild",
        &guild,
        channel.get().to_string(),
    )
    .await?;
    say(
        ctx,
        format!(
            "The digest and results now go to <#{channel}>. Results of matches that \
             finished before now are not posted. The bot needs Send Messages and Embed \
             Links there.",
            channel = channel.get()
        ),
    )
    .await
}

/// The role whose members are pick'em admins.
#[poise::command(slash_command, subcommands("role_set"), subcommand_required)]
async fn role(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Make a role pick'em admins (besides Manage Server); leave empty to clear.
#[poise::command(slash_command, rename = "set")]
async fn role_set(
    ctx: Context<'_>,
    #[description = "The admin role; empty clears it"] role: Option<serenity::RoleId>,
) -> Result<(), Error> {
    let guild = guild_of(ctx)?;
    let data = ctx.data();
    let role_id = role.map(|r| r.get().to_string());
    store::guilds::set_admin_role(&data.pool, &guild, role_id.as_deref(), data.clock.now()).await?;
    audit(
        ctx,
        "set_admin_role",
        "guild",
        &guild,
        role_id.clone().unwrap_or_default(),
    )
    .await?;
    let text = match role {
        Some(r) => format!(
            "Members with <@&{}> are now pick'em admins, besides everyone with Manage Server.",
            r.get()
        ),
        None => "The admin role is cleared: only members with Manage Server are admins.".to_owned(),
    };
    say(ctx, text).await
}

/// When the daily digest is posted.
#[poise::command(slash_command, subcommands("digest_set"), subcommand_required)]
async fn digest(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Set the local time and time zone of the daily digest.
#[poise::command(slash_command, rename = "set")]
async fn digest_set(
    ctx: Context<'_>,
    #[description = "Local time, 24 h, e.g. 10:00"] time: String,
    #[description = "IANA time zone, e.g. Europe/Berlin"] timezone: String,
) -> Result<(), Error> {
    let guild = guild_of(ctx)?;
    let t = schedule::parse_hhmm(&time)
        .ok_or_else(|| user(format!("`{time}` is not a time like 10:00 (24 h).")))?;
    let tz = schedule::parse_tz(&timezone).ok_or_else(|| {
        user(format!(
            "`{timezone}` is not an IANA time zone, such as Europe/Berlin or America/New_York."
        ))
    })?;
    let data = ctx.data();
    let hhmm = schedule::format_hhmm(t);
    store::guilds::set_digest_schedule(&data.pool, &guild, &hhmm, tz.name(), data.clock.now())
        .await?;
    audit(
        ctx,
        "set_digest_schedule",
        "guild",
        &guild,
        format!("{hhmm} {}", tz.name()),
    )
    .await?;
    say(
        ctx,
        format!(
            "The daily digest goes out at {hhmm} ({}). Today's digest is not posted again \
             if it already went out.",
            tz.name()
        ),
    )
    .await
}

// ---- sync status (T06) ----------------------------------------------------------

/// Sync health, API quota per data source, and backups.
#[poise::command(slash_command)]
async fn status(ctx: Context<'_>) -> Result<(), Error> {
    let sync = match &ctx.data().sync {
        Some(sync) => Some(sync.status().await?),
        None => None,
    };
    let backup = match &ctx.data().backup {
        Some(b) => Some(b.status().await),
        None => None,
    };
    say(ctx, status_text(sync.as_ref(), backup.as_ref())).await
}

fn ago(t: Option<DateTime<Utc>>) -> String {
    t.map(|t| ts(t, 'R')).unwrap_or_else(|| "never".to_owned())
}

/// The text of `/admin status`.
pub fn status_text(
    sync: Option<&sync::SyncStatus>,
    backup: Option<&backup::BackupStatus>,
) -> String {
    let mut out = match sync {
        Some(s) => sync_lines(s),
        None => vec!["The sync worker is not running.".to_owned()],
    };
    out.push(String::new());
    out.extend(backup_lines(backup));
    truncate(&out.join("\n"), 1900)
}

/// Human-readable file size, e.g. `12.3 MB`.
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

fn backup_lines(b: Option<&backup::BackupStatus>) -> Vec<String> {
    let mut out = vec!["**Backups**".to_owned()];
    let Some(b) = b else {
        out.push("Backups are not running.".to_owned());
        return out;
    };
    match &b.last {
        Some(last) => out.push(format!(
            "Last backup {} · {} · `{}`",
            ts(last.created_at, 'R'),
            size_text(last.size),
            last.file_name
        )),
        None => out.push("No backup yet.".to_owned()),
    }
    if let Some(e) = &b.last_error {
        out.push(format!(
            "⚠️ failed {}: {}",
            ts(e.at, 'R'),
            truncate(&e.message, 200)
        ));
    }
    if b.running {
        out.push("A backup is running now.".to_owned());
    }
    out.push(match b.next_run {
        Some(n) => format!("Next daily backup {}", ts(n, 'R')),
        None => "Daily backups are off.".to_owned(),
    });
    out
}

// ---- backups (P4) ---------------------------------------------------------------

/// Database backups.
#[poise::command(slash_command, subcommands("backup_now"), subcommand_required)]
async fn backup(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Back up the database now.
#[poise::command(slash_command, rename = "now")]
async fn backup_now(ctx: Context<'_>) -> Result<(), Error> {
    let Some(service) = ctx.data().backup.clone() else {
        return say(ctx, "Backups are not running.").await;
    };
    // A backup of a large database takes longer than Discord's 3 s.
    ctx.defer_ephemeral().await?;
    match service.run(backup::BackupKind::Manual).await {
        Ok(o) => {
            let mut text = format!(
                "Backup written: `{}` ({}, {:.1} s).",
                o.backup.file_name,
                size_text(o.backup.size),
                o.duration.as_secs_f64()
            );
            if let Some(e) = &o.extra_error {
                text.push_str(&format!("\n⚠️ {}", truncate(e, 300)));
            }
            say(ctx, text).await
        }
        Err(backup::BackupError::AlreadyRunning) => {
            say(ctx, "A backup is already running. Try again in a moment.").await
        }
        Err(e) => {
            say(
                ctx,
                format!("⚠️ The backup failed: {}", truncate(&e.to_string(), 300)),
            )
            .await
        }
    }
}

fn sync_lines(s: &sync::SyncStatus) -> Vec<String> {
    let mut out = vec!["**Sources**".to_owned()];
    if s.sources.is_empty() {
        out.push("No source makes external calls.".to_owned());
    }
    for src in &s.sources {
        let mut parts = vec![format!("{} requests this hour", src.requests_this_hour)];
        if let (Some(left), Some(per_hour)) = (src.budget_left, src.budget_per_hour) {
            parts.push(format!("budget {left}/{per_hour} left"));
        }
        if let Some(q) = src.remaining_quota {
            parts.push(format!("source quota {q} left"));
        }
        parts.push(format!("last request {}", ago(src.last_request_at)));
        out.push(format!("`{}` · {}", src.source, parts.join(" · ")));
    }
    out.push(String::new());
    out.push("**Jobs**".to_owned());
    if s.jobs.is_empty() {
        out.push("No job has run yet.".to_owned());
    }
    for j in &s.jobs {
        let health = match (&j.last_error, j.last_success_at, j.last_run_at) {
            (Some(e), _, run) => format!("⚠️ failed {}: {}", ago(run), truncate(e, 120)),
            (None, Some(ok), _) => format!("ok {}", ts(ok, 'R')),
            (None, None, _) => "not run yet".to_owned(),
        };
        let next = j
            .next_run_at
            .map(|n| format!(" · next {}", ts(n, 'R')))
            .unwrap_or_default();
        out.push(format!("`{}` ({}) · {health}{next}", j.job, j.source));
    }
    // Leave room for the backup section within Discord's 2000 characters.
    vec![truncate(&out.join("\n"), 1500)]
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut t: String = s.chars().take(max - 1).collect();
    t.push('…');
    t
}

// ---- unlinked queue (T03) ---------------------------------------------------------

/// Source entities that could not be matched automatically.
#[poise::command(
    slash_command,
    subcommands("link_list", "link_merge", "link_create"),
    subcommand_required
)]
async fn link(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// An unlinked entry: `source/type/external_id`.
fn parse_entry(entry: &str) -> Result<(String, EntityType, String), Error> {
    let mut parts = entry.trim().splitn(3, '/');
    let (Some(source), Some(kind), Some(ext)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(user(format!(
            "`{entry}` is not an entry; pick one from the list (source/type/id)."
        )));
    };
    let kind: EntityType = kind
        .parse()
        .map_err(|_| user(format!("`{kind}` is not a team, event, stage or match.")))?;
    Ok((source.to_owned(), kind, ext.to_owned()))
}

fn entry_label(u: &store::unlinked::UnlinkedRef) -> String {
    let detail = u
        .detail
        .as_deref()
        .map(|d| format!(" · {d}"))
        .unwrap_or_default();
    format!(
        "{}/{}/{} · {}{detail}",
        u.source, u.entity_type, u.external_id, u.reason
    )
}

async fn autocomplete_entry(
    ctx: Context<'_>,
    partial: &str,
) -> serenity::CreateAutocompleteResponse {
    let partial = partial.to_lowercase();
    let entries = store::unlinked::list(&ctx.data().pool, None)
        .await
        .unwrap_or_default();
    let mut r = serenity::CreateAutocompleteResponse::new();
    for u in entries
        .iter()
        .filter(|u| entry_label(u).to_lowercase().contains(&partial))
        .take(25)
    {
        r = r.add_string_choice(
            truncate(&entry_label(u), 100),
            format!("{}/{}/{}", u.source, u.entity_type, u.external_id),
        );
    }
    r
}

/// List the unlinked queue.
#[poise::command(slash_command, rename = "list")]
async fn link_list(ctx: Context<'_>) -> Result<(), Error> {
    let entries = store::unlinked::list(&ctx.data().pool, None).await?;
    if entries.is_empty() {
        return say(ctx, "The unlinked queue is empty.").await;
    }
    let mut lines: Vec<String> = entries
        .iter()
        .take(20)
        .map(|u| {
            format!(
                "`{}` · first seen {}",
                entry_label(u),
                ts(u.first_seen_at, 'R')
            )
        })
        .collect();
    if entries.len() > 20 {
        lines.push(format!("… and {} more", entries.len() - 20));
    }
    lines.push(String::new());
    lines.push(
        "Use `/admin link merge` to link an entry to an existing team, event, stage or \
         match (by its ID), or `/admin link create` to add it as new."
            .to_owned(),
    );
    say(ctx, truncate(&lines.join("\n"), 1900)).await
}

/// Link an unlinked entry to an existing entity.
#[poise::command(slash_command, rename = "merge")]
async fn link_merge(
    ctx: Context<'_>,
    #[description = "The unlinked entry"]
    #[autocomplete = "autocomplete_entry"]
    entry: String,
    #[description = "ID of the existing team, event, stage or match"] id: i64,
) -> Result<(), Error> {
    let (source, kind, ext) = parse_entry(&entry)?;
    let data = ctx.data();
    let actor = ctx.author().id.to_string();
    let report = data
        .reconciler
        .link(&source, kind, &ext, id, &actor, data.clock.now())
        .await
        .map_err(reconcile_error)?;
    say(
        ctx,
        format!(
            "Linked `{entry}` to {kind} {id}. {} updated, {} still queued.",
            report.updated, report.queued
        ),
    )
    .await
}

/// Add an unlinked entry as a new entity.
#[poise::command(slash_command, rename = "create")]
async fn link_create(
    ctx: Context<'_>,
    #[description = "The unlinked entry"]
    #[autocomplete = "autocomplete_entry"]
    entry: String,
) -> Result<(), Error> {
    let (source, kind, ext) = parse_entry(&entry)?;
    let data = ctx.data();
    let actor = ctx.author().id.to_string();
    let (id, _) = data
        .reconciler
        .create_from_unlinked(&source, kind, &ext, &actor, data.clock.now())
        .await
        .map_err(reconcile_error)?;
    say(ctx, format!("Created {kind} {id} from `{entry}`.")).await
}

fn reconcile_error(e: sync::ReconcileError) -> Error {
    match e {
        sync::ReconcileError::Invalid(m) => user(m),
        other => other.into(),
    }
}

// ---- matches (T05) ----------------------------------------------------------------

/// Set results and fix matches by hand.
#[poise::command(
    slash_command,
    rename = "match",
    subcommands(
        "match_result",
        "match_clear",
        "match_void",
        "match_unvoid",
        "match_reopen"
    ),
    subcommand_required
)]
async fn match_(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

fn match_label(i: &MatchListItem) -> String {
    let when =
        i.m.fields
            .scheduled_at
            .map(|t| t.format(" · %d %b %H:%M UTC").to_string())
            .unwrap_or_default();
    format!(
        "#{} {} vs {}{when} · {}",
        i.m.id.0,
        team_label(i.team_a.as_ref()),
        team_label(i.team_b.as_ref()),
        i.m.fields.status
    )
}

/// Matches of followed events around now (upcoming, live, finished in the
/// last week) whose label contains `partial`.
async fn autocomplete_match(
    ctx: Context<'_>,
    partial: &str,
) -> serenity::CreateAutocompleteResponse {
    let data = ctx.data();
    let mut r = serenity::CreateAutocompleteResponse::new();
    let Some(guild) = ctx.guild_id().map(|g| g.to_string()) else {
        return r;
    };
    let now = data.clock.now();
    let partial = partial.trim().trim_start_matches('#').to_lowercase();
    let mut items = Vec::new();
    for (segment, bound) in [
        (Segment::Live, None),
        (Segment::Upcoming, None),
        (Segment::Finished, Some(now - Duration::days(7))),
    ] {
        let q = ListQuery {
            guild_id: &guild,
            segment,
            bound,
            event_id: None,
            after: None,
            limit: 25,
        };
        items.extend(
            store::matches::list_view(&data.pool, &q)
                .await
                .unwrap_or_default(),
        );
    }
    for i in items
        .iter()
        .filter(|i| match_label(i).to_lowercase().contains(&partial))
        .take(25)
    {
        r = r.add_int_choice(truncate(&match_label(i), 100), i.m.id.0);
    }
    r
}

fn manual_error(e: ManualError) -> Error {
    match e {
        ManualError::NotFound(_) | ManualError::Invalid(_) | ManualError::Conflict(_) => {
            user(e.to_string())
        }
        ManualError::Reconcile(e) => e.into(),
    }
}

fn describe(o: &Outcome) -> String {
    let f = &o.current.fields;
    let mut s = format!(
        "Match #{}: {} · {}–{}",
        o.current.id.0, f.status, f.maps_won_a, f.maps_won_b
    );
    if f.voided {
        s.push_str(" · void");
    }
    s
}

/// Checks that map scores agree with the series score.
pub fn check_result(series: (u8, u8), maps: &[(u16, u16)]) -> Result<(), String> {
    if maps.is_empty() {
        return Ok(());
    }
    if maps.iter().any(|(a, b)| a == b) {
        return Err("A map can't end in a draw.".to_owned());
    }
    let won_a = maps.iter().filter(|(a, b)| a > b).count();
    let won_b = maps.len() - won_a;
    if (won_a, won_b) != (usize::from(series.0), usize::from(series.1)) {
        return Err(format!(
            "The maps give {won_a}–{won_b}, but the series score is {}–{}.",
            series.0, series.1
        ));
    }
    Ok(())
}

/// Enter a final result: series score and optionally map scores.
#[poise::command(slash_command, rename = "result")]
async fn match_result(
    ctx: Context<'_>,
    #[description = "The match"]
    #[autocomplete = "autocomplete_match"]
    r#match: i64,
    #[description = "Maps won, team A first, e.g. 2-1"] series: String,
    #[description = "Map scores in order, e.g. 13-11, 9-13, 13-7"] maps: Option<String>,
) -> Result<(), Error> {
    let score = schedule::parse_score::<u8>(&series)
        .ok_or_else(|| user(format!("`{series}` is not a series score like 2-1.")))?;
    let map_scores = match maps.as_deref() {
        None => Vec::new(),
        Some(m) => schedule::parse_map_scores(m).ok_or_else(|| {
            user(format!(
                "`{m}` is not a list of map scores like 13-11, 9-13."
            ))
        })?,
    };
    check_result(score, &map_scores).map_err(user)?;
    let data = ctx.data();
    let actor = ctx.author().id.to_string();
    let id = MatchId(r#match);
    for (n, rounds) in map_scores.iter().enumerate() {
        let number = u8::try_from(n + 1).map_err(|_| user("Too many maps."))?;
        let ov = Override::Map {
            number,
            status: MapStatus::Finished,
            rounds: Some(*rounds),
        };
        data.manual
            .set(id, ov, &actor, data.clock.now())
            .await
            .map_err(manual_error)?;
    }
    data.manual
        .set(id, Override::Series { score }, &actor, data.clock.now())
        .await
        .map_err(manual_error)?;
    let ov = Override::Status {
        status: MatchStatus::Finished,
        started_at: None,
        ended_at: None,
    };
    let o = data
        .manual
        .set(id, ov, &actor, data.clock.now())
        .await
        .map_err(manual_error)?;
    say(
        ctx,
        format!(
            "Result saved. {}. Points are recalculated now; the result is posted with the \
             next bundle.",
            describe(&o)
        ),
    )
    .await
}

/// Which override to clear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum OverrideChoice {
    #[name = "status"]
    Status,
    #[name = "schedule"]
    Schedule,
    #[name = "series score"]
    Series,
    #[name = "map 1"]
    Map1,
    #[name = "map 2"]
    Map2,
    #[name = "map 3"]
    Map3,
    #[name = "map 4"]
    Map4,
    #[name = "map 5"]
    Map5,
}

impl From<OverrideChoice> for OverrideField {
    fn from(c: OverrideChoice) -> Self {
        match c {
            OverrideChoice::Status => OverrideField::Status,
            OverrideChoice::Schedule => OverrideField::Schedule,
            OverrideChoice::Series => OverrideField::Series,
            OverrideChoice::Map1 => OverrideField::Map(1),
            OverrideChoice::Map2 => OverrideField::Map(2),
            OverrideChoice::Map3 => OverrideField::Map(3),
            OverrideChoice::Map4 => OverrideField::Map(4),
            OverrideChoice::Map5 => OverrideField::Map(5),
        }
    }
}

/// Remove a manual value; the source's value applies again.
#[poise::command(slash_command, rename = "clear-override")]
async fn match_clear(
    ctx: Context<'_>,
    #[description = "The match"]
    #[autocomplete = "autocomplete_match"]
    r#match: i64,
    #[description = "Which value"] field: OverrideChoice,
) -> Result<(), Error> {
    let data = ctx.data();
    let o = data
        .manual
        .clear(
            MatchId(r#match),
            field.into(),
            &ctx.author().id.to_string(),
            data.clock.now(),
        )
        .await
        .map_err(manual_error)?;
    say(ctx, format!("Override cleared. {}.", describe(&o))).await
}

/// Void a match: nobody gets points for it; picks are kept.
#[poise::command(slash_command, rename = "void")]
async fn match_void(
    ctx: Context<'_>,
    #[description = "The match"]
    #[autocomplete = "autocomplete_match"]
    r#match: i64,
) -> Result<(), Error> {
    let data = ctx.data();
    let o = data
        .manual
        .void(
            MatchId(r#match),
            &ctx.author().id.to_string(),
            data.clock.now(),
        )
        .await
        .map_err(manual_error)?;
    say(ctx, format!("Voided. {}.", describe(&o))).await
}

/// Take a void back; the match scores normally again.
#[poise::command(slash_command, rename = "unvoid")]
async fn match_unvoid(
    ctx: Context<'_>,
    #[description = "The match"]
    #[autocomplete = "autocomplete_match"]
    r#match: i64,
) -> Result<(), Error> {
    let data = ctx.data();
    let o = data
        .manual
        .unvoid(
            MatchId(r#match),
            &ctx.author().id.to_string(),
            data.clock.now(),
        )
        .await
        .map_err(manual_error)?;
    say(ctx, format!("Void taken back. {}.", describe(&o))).await
}

/// Reopen picks of a match that hasn't started, e.g. after a postponement.
#[poise::command(slash_command, rename = "reopen")]
async fn match_reopen(
    ctx: Context<'_>,
    #[description = "The match"]
    #[autocomplete = "autocomplete_match"]
    r#match: i64,
) -> Result<(), Error> {
    let data = ctx.data();
    let o = data
        .manual
        .reopen(
            MatchId(r#match),
            &ctx.author().id.to_string(),
            data.clock.now(),
        )
        .await
        .map_err(manual_error)?;
    say(ctx, format!("Picks are open again. {}.", describe(&o))).await
}

// ---- audit --------------------------------------------------------------------------

/// Audits a settings change with the admin's Discord user ID.
async fn audit(
    ctx: Context<'_>,
    action: &str,
    entity_type: &str,
    entity_id: &str,
    after: String,
) -> Result<(), Error> {
    let data = ctx.data();
    store::audit::append(
        &data.pool,
        &store::audit::AuditRecord {
            actor: ctx.author().id.to_string(),
            action: action.to_owned(),
            entity_type: entity_type.to_owned(),
            entity_id: Some(entity_id.to_owned()),
            before: None,
            after: Some(serde_json::json!({ "value": after })),
            at: data.clock.now(),
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_must_agree_with_the_maps() {
        assert!(check_result((2, 1), &[]).is_ok());
        assert!(check_result((2, 1), &[(13, 11), (9, 13), (16, 14)]).is_ok());
        assert!(check_result((2, 0), &[(13, 11), (9, 13)]).is_err());
        assert!(check_result((1, 0), &[(15, 15)]).is_err());
        assert!(check_result((2, 1), &[(13, 11), (13, 9)]).is_err());
    }

    #[test]
    fn entries_parse() {
        let (s, k, e) = parse_entry("pandascore/team/12/34").unwrap();
        assert_eq!(
            (s.as_str(), k, e.as_str()),
            ("pandascore", EntityType::Team, "12/34")
        );
        assert!(parse_entry("pandascore/team").is_err());
        assert!(parse_entry("pandascore/player/1").is_err());
    }

    #[test]
    fn status_text_names_sources_jobs_and_errors() {
        use chrono::TimeZone;
        let now = Utc.with_ymd_and_hms(2026, 11, 22, 12, 0, 0).unwrap();
        let s = sync::SyncStatus {
            now,
            sources: vec![sync::SourceStatus {
                source: "pandascore",
                budget_per_hour: Some(500),
                budget_per_minute: Some(30),
                budget_left: Some(380),
                requests_this_hour: 120,
                hour_started_at: now,
                last_request_at: Some(now),
                remaining_quota: Some(880),
            }],
            jobs: vec![
                sync::JobStatus {
                    job: "live".into(),
                    source: "pandascore".into(),
                    last_run_at: Some(now),
                    last_success_at: None,
                    last_error: Some("timeout".into()),
                    next_run_at: None,
                },
                sync::JobStatus {
                    job: "structure".into(),
                    source: "pandascore".into(),
                    last_run_at: Some(now),
                    last_success_at: Some(now),
                    last_error: None,
                    next_run_at: Some(now),
                },
            ],
        };
        let text = status_text(Some(&s), None);
        assert!(text.contains("`pandascore` · 120 requests this hour · budget 380/500 left · source quota 880 left"), "{text}");
        assert!(text.contains("`live` (pandascore) · ⚠️ failed"), "{text}");
        assert!(text.contains(": timeout"), "{text}");
        assert!(text.contains("`structure` (pandascore) · ok <t:"), "{text}");
        assert!(
            text.contains("**Backups**\nBackups are not running."),
            "{text}"
        );
    }

    #[test]
    fn status_text_shows_the_last_backup_and_problems() {
        use chrono::TimeZone;
        let now = Utc.with_ymd_and_hms(2026, 11, 22, 3, 0, 0).unwrap();
        let mut b = backup::BackupStatus {
            last: Some(backup::BackupInfo {
                path: "/data/backups/pickem-20261122T030000Z-daily.db.zst".into(),
                file_name: "pickem-20261122T030000Z-daily.db.zst".into(),
                kind: backup::BackupKind::Daily,
                created_at: now,
                size: 12_345_678,
            }),
            last_error: None,
            next_run: Some(now + Duration::days(1)),
            running: false,
        };
        let text = status_text(None, Some(&b));
        assert!(
            text.starts_with("The sync worker is not running."),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "Last backup <t:{}:R> · 12.3 MB · `pickem-20261122T030000Z-daily.db.zst`",
                now.timestamp()
            )),
            "{text}"
        );
        assert!(text.contains("Next daily backup <t:"), "{text}");
        assert!(!text.contains("⚠️"), "{text}");

        b.last = None;
        b.next_run = None;
        b.running = true;
        b.last_error = Some(backup::FailedRun {
            at: now,
            message: "creating /data/backups: Permission denied".into(),
        });
        let text = status_text(None, Some(&b));
        assert!(text.contains("No backup yet."), "{text}");
        assert!(text.contains("⚠️ failed <t:"), "{text}");
        assert!(text.contains("Permission denied"), "{text}");
        assert!(text.contains("A backup is running now."), "{text}");
        assert!(text.contains("Daily backups are off."), "{text}");
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(999), "999 B");
        assert_eq!(size_text(1_500), "1.5 KB");
        assert_eq!(size_text(28_834_071), "28.8 MB");
        assert_eq!(size_text(3_000_000_000), "3.0 GB");
    }
}
