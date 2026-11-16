//! Discord users and their guild memberships. IDs are Discord snowflakes as
//! decimal strings.

use chrono::{DateTime, Utc};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteExecutor};

use crate::{DbResult, Ts};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub discord_user_id: String,
    pub username: String,
    pub global_name: Option<String>,
    pub avatar_hash: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildMember {
    pub guild_id: String,
    pub user_id: String,
    pub nick: Option<String>,
    pub last_seen_at: DateTime<Utc>,
}

fn user_from_row(row: SqliteRow) -> DbResult<User> {
    Ok(User {
        discord_user_id: row.try_get("discord_user_id")?,
        username: row.try_get("username")?,
        global_name: row.try_get("global_name")?,
        avatar_hash: row.try_get("avatar_hash")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn member_from_row(row: SqliteRow) -> DbResult<GuildMember> {
    Ok(GuildMember {
        guild_id: row.try_get("guild_id")?,
        user_id: row.try_get("user_id")?,
        nick: row.try_get("nick")?,
        last_seen_at: row.try_get("last_seen_at")?,
    })
}

/// Inserts or refreshes a user's profile.
pub async fn upsert_user(ex: impl SqliteExecutor<'_>, u: &User) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO users (discord_user_id, username, global_name, avatar_hash, updated_at) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT (discord_user_id) DO UPDATE SET \
           username = excluded.username, global_name = excluded.global_name, \
           avatar_hash = excluded.avatar_hash, updated_at = excluded.updated_at",
    )
    .bind(&u.discord_user_id)
    .bind(&u.username)
    .bind(&u.global_name)
    .bind(&u.avatar_hash)
    .bind(Ts(u.updated_at))
    .execute(ex)
    .await?;
    Ok(())
}

pub async fn get_user(
    ex: impl SqliteExecutor<'_>,
    discord_user_id: &str,
) -> DbResult<Option<User>> {
    sqlx::query(
        "SELECT discord_user_id, username, global_name, avatar_hash, updated_at \
         FROM users WHERE discord_user_id = ?",
    )
    .bind(discord_user_id)
    .try_map(user_from_row)
    .fetch_optional(ex)
    .await
}

/// Inserts or refreshes a guild membership. The user must exist.
pub async fn upsert_member(ex: impl SqliteExecutor<'_>, m: &GuildMember) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO guild_members (guild_id, user_id, nick, last_seen_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT (guild_id, user_id) DO UPDATE SET \
           nick = excluded.nick, last_seen_at = excluded.last_seen_at",
    )
    .bind(&m.guild_id)
    .bind(&m.user_id)
    .bind(&m.nick)
    .bind(Ts(m.last_seen_at))
    .execute(ex)
    .await?;
    Ok(())
}

pub async fn get_member(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    user_id: &str,
) -> DbResult<Option<GuildMember>> {
    sqlx::query(
        "SELECT guild_id, user_id, nick, last_seen_at FROM guild_members \
         WHERE guild_id = ? AND user_id = ?",
    )
    .bind(guild_id)
    .bind(user_id)
    .try_map(member_from_row)
    .fetch_optional(ex)
    .await
}
