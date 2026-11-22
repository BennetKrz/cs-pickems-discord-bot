//! Channel posts (T08): the daily digest and match results.
//!
//! The announcer reads everything from the database on each tick, so a
//! restart or a lagged event bus loses nothing. Per guild with an announce
//! channel, in this order:
//!
//! 1. Posts claimed earlier but never confirmed (interrupted by a restart
//!    or a Discord error) are sent again with the same nonce; Discord
//!    returns the message it already created instead of posting twice.
//! 2. The next result bundle, once due (see [`schedule::due_bundle`]).
//! 3. The day's digest once the local digest time has passed, if any match
//!    is still pickable; afterwards it is edited when its content changes
//!    (pick counts, locks), at most once a minute.
//!
//! New posts respect [`schedule::POSTS_PER_HOUR`], counted from the stored
//! posts; edits don't count. A post that is over the limit waits for a free
//! slot.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use domain::{Clock, DomainEvent, MatchId};
use store::SqlitePool;
use store::bot::{BotPost, NewPost, PostKind};
use store::guilds::GuildSettings;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio_util::sync::CancellationToken;

use crate::render::{self, Post, ResultEntry};
use crate::schedule;

/// How often the announcer looks for due posts and changed digests.
pub const TICK_EVERY: std::time::Duration = std::time::Duration::from_secs(30);
/// A sent digest is edited at most this often.
pub const EDIT_EVERY: Duration = Duration::minutes(1);
/// The most matches a digest lists.
const DIGEST_LIMIT: i64 = 25;
/// Best pickers shown per match (ties with the last are included).
const TOP_PICKERS: usize = 3;
/// After a failed send, the guild is skipped this long.
const BACKOFF_FAILED: Duration = Duration::minutes(1);
/// After Discord refused the channel (deleted, no access), this long.
const BACKOFF_GONE: Duration = Duration::minutes(5);

/// Why Discord did not take a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboxError {
    /// The channel or message is gone, or the bot may not post there
    /// (403/404). Retrying soon won't help.
    Gone(String),
    /// Anything else (network, rate limit, 5xx); worth retrying.
    Failed(String),
}

impl std::fmt::Display for OutboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutboxError::Gone(m) => write!(f, "refused by Discord: {m}"),
            OutboxError::Failed(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for OutboxError {}

/// Where posts go: Discord in production (`discord::DiscordOutbox`), a
/// recorder in tests.
#[async_trait]
pub trait Outbox: Send + Sync {
    /// Sends a message and returns its ID. `nonce` makes Discord return the
    /// existing message if the same nonce was sent shortly before.
    async fn send(&self, channel_id: &str, post: &Post, nonce: &str)
    -> Result<String, OutboxError>;
    /// Replaces a sent message's content.
    async fn edit(
        &self,
        channel_id: &str,
        message_id: &str,
        post: &Post,
    ) -> Result<(), OutboxError>;
}

/// What one tick did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TickReport {
    /// New posts sent.
    pub sent: u32,
    /// Interrupted posts sent again.
    pub resent: u32,
    /// Digests edited.
    pub edited: u32,
    /// Due posts held back by the rate limit.
    pub deferred: u32,
    /// Guilds skipped after an error.
    pub failed: u32,
}

#[derive(Debug)]
enum TickError {
    Db(sqlx::Error),
    Outbox(OutboxError),
}

impl From<sqlx::Error> for TickError {
    fn from(e: sqlx::Error) -> Self {
        TickError::Db(e)
    }
}

impl From<OutboxError> for TickError {
    fn from(e: OutboxError) -> Self {
        TickError::Outbox(e)
    }
}

/// The channel-post worker.
pub struct Announcer {
    pool: SqlitePool,
    clock: Arc<dyn Clock>,
    outbox: Arc<dyn Outbox>,
    /// When set, only this guild gets posts (`DISCORD_GUILD_ID`).
    only_guild: Option<String>,
    /// Guilds to skip until the given time, after an error.
    backoff: HashMap<String, DateTime<Utc>>,
}

/// A guild's digest schedule, with invalid stored values replaced by the
/// defaults (the command validates, so this only guards against hand edits).
fn schedule_of(g: &GuildSettings) -> (Tz, NaiveTime) {
    let tz = schedule::parse_tz(&g.timezone).unwrap_or_else(|| {
        tracing::warn!(guild = %g.guild_id, timezone = %g.timezone, "invalid time zone, using the default");
        chrono_tz::Europe::Berlin
    });
    let time = schedule::parse_hhmm(&g.digest_time)
        .or_else(|| schedule::parse_hhmm(store::guilds::DEFAULT_DIGEST_TIME))
        .unwrap_or(NaiveTime::MIN);
    (tz, time)
}

impl Announcer {
    pub fn new(
        pool: SqlitePool,
        clock: Arc<dyn Clock>,
        outbox: Arc<dyn Outbox>,
        only_guild: Option<String>,
    ) -> Self {
        Self {
            pool,
            clock,
            outbox,
            only_guild,
            backoff: HashMap::new(),
        }
    }

    /// Runs until `cancel`: a tick every `every`, and an early one when a
    /// match finishes or its result changes.
    pub async fn run(
        mut self,
        mut events: broadcast::Receiver<DomainEvent>,
        every: std::time::Duration,
        cancel: CancellationToken,
    ) {
        let mut interval = tokio::time::interval(every);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut bus_open = true;
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                _ = interval.tick() => {}
                ev = events.recv(), if bus_open => match ev {
                    Ok(DomainEvent::MatchFinished { .. } | DomainEvent::ResultCorrected { .. }) => {}
                    Ok(_) => continue,
                    Err(RecvError::Lagged(n)) => {
                        tracing::debug!(skipped = n, "announcer lagged behind the event bus");
                    }
                    Err(RecvError::Closed) => {
                        bus_open = false;
                        continue;
                    }
                },
            }
            match self.tick().await {
                Ok(r) if r != TickReport::default() => {
                    tracing::info!(
                        sent = r.sent,
                        resent = r.resent,
                        edited = r.edited,
                        deferred = r.deferred,
                        failed = r.failed,
                        "announcer tick"
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("announcer tick failed: {e}"),
            }
        }
        tracing::debug!("announcer stopped");
    }

    /// One pass over every guild with an announce channel.
    pub async fn tick(&mut self) -> Result<TickReport, sqlx::Error> {
        let now = self.clock.now();
        let mut report = TickReport::default();
        let guilds = store::guilds::with_announce_channel(&self.pool).await?;
        for g in guilds {
            if self
                .only_guild
                .as_deref()
                .is_some_and(|only| only != g.guild_id)
            {
                continue;
            }
            if self
                .backoff
                .get(&g.guild_id)
                .is_some_and(|until| now < *until)
            {
                continue;
            }
            match self.guild_tick(&g, now, &mut report).await {
                Ok(()) => {
                    self.backoff.remove(&g.guild_id);
                }
                Err(TickError::Db(e)) => return Err(e),
                Err(TickError::Outbox(e)) => {
                    report.failed += 1;
                    let wait = match e {
                        OutboxError::Gone(_) => BACKOFF_GONE,
                        OutboxError::Failed(_) => BACKOFF_FAILED,
                    };
                    tracing::warn!(
                        guild = %g.guild_id,
                        channel = g.announce_channel_id.as_deref().unwrap_or_default(),
                        "cannot post to the announce channel ({e}); retrying in {} min. \
                         The bot needs Send Messages and Embed Links there.",
                        wait.num_minutes()
                    );
                    self.backoff.insert(g.guild_id.clone(), now + wait);
                }
            }
        }
        Ok(report)
    }

    async fn guild_tick(
        &self,
        g: &GuildSettings,
        now: DateTime<Utc>,
        report: &mut TickReport,
    ) -> Result<(), TickError> {
        let Some(channel) = g.announce_channel_id.as_deref() else {
            return Ok(());
        };
        let (tz, digest_time) = schedule_of(g);

        // 1. Interrupted posts.
        for post in store::bot::unsent(&self.pool, &g.guild_id).await? {
            self.deliver(g, tz, &post, now).await?;
            report.resent += 1;
        }

        // 2. Results.
        let unannounced =
            store::bot::unannounced_results(&self.pool, &g.guild_id, g.announce_since).await?;
        if let Some(ids) = schedule::due_bundle(&unannounced, now) {
            if self.has_slot(&g.guild_id, now).await? {
                let key = schedule::results_key(ids[0]);
                if let Some(post) = self
                    .claim(g, channel, PostKind::Results, &key, &ids, now)
                    .await?
                {
                    self.deliver(g, tz, &post, now).await?;
                    report.sent += 1;
                }
            } else {
                report.deferred += 1;
            }
        }

        // 3. The digest.
        if let Some(date) = schedule::digest_due(tz, digest_time, now) {
            let key = date.to_string();
            match store::bot::get_post(&self.pool, &g.guild_id, PostKind::Digest, &key).await? {
                None => {
                    let items = self.day_items(&g.guild_id, tz, date).await?;
                    if items.iter().any(|i| render::is_pickable(i, now)) {
                        if self.has_slot(&g.guild_id, now).await? {
                            if let Some(post) = self
                                .claim(g, channel, PostKind::Digest, &key, &[], now)
                                .await?
                            {
                                self.deliver(g, tz, &post, now).await?;
                                report.sent += 1;
                            }
                        } else {
                            report.deferred += 1;
                        }
                    }
                }
                Some(post) => {
                    if self.refresh_digest(g, tz, &post, now).await? {
                        report.edited += 1;
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether the guild may get another post now.
    async fn has_slot(&self, guild_id: &str, now: DateTime<Utc>) -> Result<bool, sqlx::Error> {
        let recent =
            store::bot::count_since(&self.pool, guild_id, now - Duration::hours(1)).await?;
        Ok(recent < schedule::POSTS_PER_HOUR)
    }

    /// Claims a post (and for results, its matches) in one transaction.
    /// `None` if it was claimed before.
    async fn claim(
        &self,
        g: &GuildSettings,
        channel: &str,
        kind: PostKind,
        key: &str,
        matches: &[MatchId],
        now: DateTime<Utc>,
    ) -> Result<Option<BotPost>, sqlx::Error> {
        let mut tx = store::begin_write(&self.pool).await?;
        let new = NewPost {
            guild_id: &g.guild_id,
            kind,
            post_key: key,
            channel_id: channel,
            posted_at: now,
        };
        let Some(id) = store::bot::claim_post(&mut *tx, &new).await? else {
            return Ok(None);
        };
        store::bot::claim_results(&mut tx, &g.guild_id, id, matches).await?;
        let post = store::bot::get_post(&mut *tx, &g.guild_id, kind, key).await?;
        tx.commit().await?;
        debug_assert!(post.as_ref().is_some_and(|p| p.id == id));
        Ok(post)
    }

    /// Renders and sends a claimed post, then stores its message ID.
    async fn deliver(
        &self,
        g: &GuildSettings,
        tz: Tz,
        post: &BotPost,
        now: DateTime<Utc>,
    ) -> Result<(), TickError> {
        let content = self.render(g, tz, post, now).await?;
        let nonce = schedule::nonce(&g.guild_id, post.kind, &post.post_key);
        let message_id = self.outbox.send(&post.channel_id, &content, &nonce).await?;
        let hash = schedule::content_hash(&content.fingerprint());
        store::bot::set_message(&self.pool, post.id, &message_id, &hash).await?;
        tracing::info!(
            guild = %g.guild_id,
            kind = post.kind.as_str(),
            key = %post.post_key,
            message = %message_id,
            "posted"
        );
        Ok(())
    }

    /// Edits a sent digest if its content changed and the last edit is old
    /// enough. Returns whether it edited.
    async fn refresh_digest(
        &self,
        g: &GuildSettings,
        tz: Tz,
        post: &BotPost,
        now: DateTime<Utc>,
    ) -> Result<bool, TickError> {
        let Some(message_id) = post.message_id.as_deref() else {
            return Ok(false); // still unsent; step 1 handles it
        };
        let last = post.edited_at.unwrap_or(post.posted_at);
        if now - last < EDIT_EVERY {
            return Ok(false);
        }
        let content = self.render(g, tz, post, now).await?;
        let hash = schedule::content_hash(&content.fingerprint());
        if post.content_hash.as_deref() == Some(hash.as_str()) {
            return Ok(false);
        }
        match self
            .outbox
            .edit(&post.channel_id, message_id, &content)
            .await
        {
            Ok(()) => {}
            // Someone deleted the digest: remember the content anyway, so
            // it isn't retried every minute.
            Err(OutboxError::Gone(e)) => {
                tracing::info!(guild = %g.guild_id, "digest message gone, not editing it: {e}");
            }
            Err(e) => return Err(e.into()),
        }
        store::bot::set_edited(&self.pool, post.id, &hash, now).await?;
        Ok(true)
    }

    async fn render(
        &self,
        g: &GuildSettings,
        tz: Tz,
        post: &BotPost,
        now: DateTime<Utc>,
    ) -> Result<Post, sqlx::Error> {
        match post.kind {
            PostKind::Digest => {
                let date: NaiveDate = post
                    .post_key
                    .parse()
                    .unwrap_or_else(|_| schedule::local_date(tz, post.posted_at));
                let items = self.day_items(&g.guild_id, tz, date).await?;
                Ok(render::digest(date, &items, now))
            }
            PostKind::Results => {
                let ids = store::bot::post_matches(&self.pool, post.id).await?;
                let entries = result_entries(&self.pool, &g.guild_id, &ids).await?;
                Ok(render::results(&entries))
            }
        }
    }

    async fn day_items(
        &self,
        guild_id: &str,
        tz: Tz,
        date: NaiveDate,
    ) -> Result<Vec<store::matches::MatchListItem>, sqlx::Error> {
        let (from, to) = schedule::day_window(tz, date);
        store::matches::list_lock_window(&self.pool, guild_id, from, to, DIGEST_LIMIT).await
    }
}

/// The matches of a result post with their maps and best pickers.
pub async fn result_entries(
    pool: &SqlitePool,
    guild_id: &str,
    ids: &[MatchId],
) -> Result<Vec<ResultEntry>, sqlx::Error> {
    let items = store::matches::list_by_ids(pool, guild_id, ids).await?;
    let mut maps = store::matches::maps_for(pool, ids).await?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let id = item.m.id;
        let top = store::bot::top_pickers(pool, guild_id, id, TOP_PICKERS).await?;
        let (mine, rest): (Vec<_>, Vec<_>) = maps.into_iter().partition(|m| m.match_id == id);
        maps = rest;
        out.push(ResultEntry {
            item,
            maps: mine,
            top,
        });
    }
    Ok(out)
}
