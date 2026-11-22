//! The bot's channel posts (T08): claims, rate-limit counts, which match
//! results were announced, and what the posts show.
//!
//! A post is claimed (inserted with `message_id` NULL) before it is sent,
//! and the message ID is stored once Discord answered. The unique key
//! `(guild_id, kind, post_key)` makes every post happen once, also across
//! restarts; see `docs/adr/0013-bot-announcements-launch.md`.

use chrono::{DateTime, Utc};
use domain::MatchId;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteExecutor};

use crate::{DbResult, Ts};

/// What a post is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PostKind {
    /// The daily digest; its key is the local date.
    Digest,
    /// A bundle of match results; its key is `m<first match ID>`.
    Results,
}

impl PostKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PostKind::Digest => "digest",
            PostKind::Results => "results",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "digest" => Some(PostKind::Digest),
            "results" => Some(PostKind::Results),
            _ => None,
        }
    }
}

/// A claimed post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotPost {
    pub id: i64,
    pub guild_id: String,
    pub kind: PostKind,
    pub post_key: String,
    pub channel_id: String,
    /// `None` while the send is in flight or was interrupted.
    pub message_id: Option<String>,
    pub content_hash: Option<String>,
    pub posted_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
}

/// A post to claim.
#[derive(Debug, Clone)]
pub struct NewPost<'a> {
    pub guild_id: &'a str,
    pub kind: PostKind,
    pub post_key: &'a str,
    pub channel_id: &'a str,
    pub posted_at: DateTime<Utc>,
}

macro_rules! post_columns {
    () => {
        "id, guild_id, kind, post_key, channel_id, message_id, content_hash, posted_at, edited_at"
    };
}

fn post_from_row(row: SqliteRow) -> DbResult<BotPost> {
    let kind: String = row.try_get("kind")?;
    Ok(BotPost {
        id: row.try_get("id")?,
        guild_id: row.try_get("guild_id")?,
        kind: PostKind::parse(&kind).ok_or_else(|| sqlx::Error::ColumnDecode {
            index: "kind".to_owned(),
            source: format!("unknown post kind `{kind}`").into(),
        })?,
        post_key: row.try_get("post_key")?,
        channel_id: row.try_get("channel_id")?,
        message_id: row.try_get("message_id")?,
        content_hash: row.try_get("content_hash")?,
        posted_at: row.try_get("posted_at")?,
        edited_at: row.try_get("edited_at")?,
    })
}

/// Claims a post. Returns its ID, or `None` if it was claimed before.
pub async fn claim_post(ex: impl SqliteExecutor<'_>, p: &NewPost<'_>) -> DbResult<Option<i64>> {
    sqlx::query_scalar(
        "INSERT INTO bot_posts (guild_id, kind, post_key, channel_id, posted_at) \
         VALUES (?, ?, ?, ?, ?) ON CONFLICT (guild_id, kind, post_key) DO NOTHING RETURNING id",
    )
    .bind(p.guild_id)
    .bind(p.kind.as_str())
    .bind(p.post_key)
    .bind(p.channel_id)
    .bind(Ts(p.posted_at))
    .fetch_optional(ex)
    .await
}

pub async fn get_post(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    kind: PostKind,
    post_key: &str,
) -> DbResult<Option<BotPost>> {
    sqlx::query(concat!(
        "SELECT ",
        post_columns!(),
        " FROM bot_posts WHERE guild_id = ? AND kind = ? AND post_key = ?"
    ))
    .bind(guild_id)
    .bind(kind.as_str())
    .bind(post_key)
    .try_map(post_from_row)
    .fetch_optional(ex)
    .await
}

/// Stores the Discord message of a sent post and the hash of its content.
pub async fn set_message(
    ex: impl SqliteExecutor<'_>,
    post_id: i64,
    message_id: &str,
    content_hash: &str,
) -> DbResult<()> {
    sqlx::query("UPDATE bot_posts SET message_id = ?, content_hash = ? WHERE id = ?")
        .bind(message_id)
        .bind(content_hash)
        .bind(post_id)
        .execute(ex)
        .await?;
    Ok(())
}

/// Records an edit of a sent post.
pub async fn set_edited(
    ex: impl SqliteExecutor<'_>,
    post_id: i64,
    content_hash: &str,
    at: DateTime<Utc>,
) -> DbResult<()> {
    sqlx::query("UPDATE bot_posts SET content_hash = ?, edited_at = ? WHERE id = ?")
        .bind(content_hash)
        .bind(Ts(at))
        .bind(post_id)
        .execute(ex)
        .await?;
    Ok(())
}

/// Claimed posts of a guild that have no message yet (a send was
/// interrupted), oldest first.
pub async fn unsent(ex: impl SqliteExecutor<'_>, guild_id: &str) -> DbResult<Vec<BotPost>> {
    sqlx::query(concat!(
        "SELECT ",
        post_columns!(),
        " FROM bot_posts WHERE guild_id = ? AND message_id IS NULL ORDER BY posted_at, id"
    ))
    .bind(guild_id)
    .try_map(post_from_row)
    .fetch_all(ex)
    .await
}

/// How many posts a guild got after `since` (the post rate limit: a post
/// exactly an hour old no longer counts).
pub async fn count_since(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    since: DateTime<Utc>,
) -> DbResult<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM bot_posts WHERE guild_id = ? AND posted_at > ?")
        .bind(guild_id)
        .bind(Ts(since))
        .fetch_one(ex)
        .await
}

/// Marks matches as announced by a result post. Matches announced before
/// are left alone.
pub async fn claim_results(
    conn: &mut sqlx::SqliteConnection,
    guild_id: &str,
    post_id: i64,
    match_ids: &[MatchId],
) -> DbResult<()> {
    for m in match_ids {
        sqlx::query(
            "INSERT INTO announced_results (guild_id, match_id, post_id) VALUES (?, ?, ?) \
             ON CONFLICT (guild_id, match_id) DO NOTHING",
        )
        .bind(guild_id)
        .bind(m.0)
        .bind(post_id)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// The matches a result post announces, by ID.
pub async fn post_matches(ex: impl SqliteExecutor<'_>, post_id: i64) -> DbResult<Vec<MatchId>> {
    sqlx::query_scalar("SELECT match_id FROM announced_results WHERE post_id = ? ORDER BY match_id")
        .bind(post_id)
        .fetch_all(ex)
        .await
        .map(|ids: Vec<i64>| ids.into_iter().map(MatchId).collect())
}

/// A finished match whose result is not announced yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unannounced {
    pub match_id: MatchId,
    /// `ended_at`, else when settlement saw it finished, else its start.
    pub finished_at: DateTime<Utc>,
}

/// Finished, not voided matches of events the guild follows whose result
/// is not announced in it yet, that finished at or after `since`; earliest
/// finish first.
pub async fn unannounced_results(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    since: Option<DateTime<Utc>>,
) -> DbResult<Vec<Unannounced>> {
    sqlx::query(
        "SELECT id, finished_at FROM ( \
           SELECT m.id, coalesce(m.ended_at, ms.finished_seen_at, m.started_at, m.scheduled_at) \
             AS finished_at \
           FROM matches m \
           JOIN followed_events fe ON fe.event_id = m.event_id AND fe.guild_id = ?1 \
           LEFT JOIN match_scoring ms ON ms.guild_id = ?1 AND ms.match_id = m.id \
           WHERE m.status = 'finished' AND m.voided = 0 \
             AND NOT EXISTS (SELECT 1 FROM announced_results ar \
                             WHERE ar.guild_id = ?1 AND ar.match_id = m.id)) \
         WHERE finished_at IS NOT NULL AND (?2 IS NULL OR finished_at >= ?2) \
         ORDER BY finished_at, id",
    )
    .bind(guild_id)
    .bind(Ts::opt(since))
    .try_map(|row: SqliteRow| {
        Ok(Unannounced {
            match_id: MatchId(row.try_get("id")?),
            finished_at: row.try_get("finished_at")?,
        })
    })
    .fetch_all(ex)
    .await
}

/// A member who scored on a match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopPicker {
    pub user_id: String,
    /// Server nickname, else global display name, else username.
    pub display_name: String,
    pub points: i64,
}

/// The best pickers of a match in a guild: the `n` highest point totals
/// above zero, plus everyone tied with the last of them.
pub async fn top_pickers(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    match_id: MatchId,
    n: usize,
) -> DbResult<Vec<TopPicker>> {
    let all: Vec<TopPicker> = sqlx::query(
        "SELECT p.user_id, r.points, \
           coalesce(nullif(trim(gm.nick), ''), nullif(trim(u.global_name), ''), u.username) \
             AS display_name \
         FROM pick_results r \
         JOIN picks p ON p.id = r.pick_id \
         JOIN users u ON u.discord_user_id = p.user_id \
         LEFT JOIN guild_members gm ON gm.guild_id = p.guild_id AND gm.user_id = p.user_id \
         WHERE p.guild_id = ? AND p.match_id = ? AND r.points > 0 \
         ORDER BY r.points DESC, display_name, p.user_id",
    )
    .bind(guild_id)
    .bind(match_id.0)
    .try_map(|row: SqliteRow| {
        Ok(TopPicker {
            user_id: row.try_get("user_id")?,
            points: row.try_get("points")?,
            display_name: row.try_get("display_name")?,
        })
    })
    .fetch_all(ex)
    .await?;
    Ok(cut_with_ties(all, n))
}

/// The first `n` entries (sorted by points, descending) plus those tied
/// with the `n`-th.
fn cut_with_ties(mut all: Vec<TopPicker>, n: usize) -> Vec<TopPicker> {
    if n == 0 {
        return Vec::new();
    }
    if let Some(last) = all.get(n - 1).map(|p| p.points) {
        let keep = all
            .iter()
            .position(|p| p.points < last)
            .unwrap_or(all.len());
        all.truncate(keep);
    }
    all
}

/// Display names of members of a guild, by user ID. Users without a
/// stored profile are missing.
pub async fn display_names(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    user_ids: &[String],
) -> DbResult<Vec<(String, String)>> {
    sqlx::query_as(
        "SELECT u.discord_user_id, \
           coalesce(nullif(trim(gm.nick), ''), nullif(trim(u.global_name), ''), u.username) \
         FROM users u \
         LEFT JOIN guild_members gm ON gm.guild_id = ?1 AND gm.user_id = u.discord_user_id \
         WHERE u.discord_user_id IN (SELECT value FROM json_each(?2)) \
         ORDER BY u.discord_user_id",
    )
    .bind(guild_id)
    .bind(sqlx::types::Json(user_ids))
    .fetch_all(ex)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(points: i64) -> TopPicker {
        TopPicker {
            user_id: points.to_string(),
            display_name: String::new(),
            points,
        }
    }

    fn points(v: &[TopPicker]) -> Vec<i64> {
        v.iter().map(|p| p.points).collect()
    }

    #[test]
    fn top_n_keeps_ties_with_the_last() {
        let all = vec![p(9), p(7), p(5), p(5), p(3)];
        assert_eq!(points(&cut_with_ties(all.clone(), 3)), [9, 7, 5, 5]);
        assert_eq!(points(&cut_with_ties(all.clone(), 2)), [9, 7]);
        assert_eq!(points(&cut_with_ties(all.clone(), 10)), [9, 7, 5, 5, 3]);
        assert!(cut_with_ties(all, 0).is_empty());
        let tied = vec![p(4), p(4), p(4), p(4)];
        assert_eq!(cut_with_ties(tied, 3).len(), 4);
    }
}
