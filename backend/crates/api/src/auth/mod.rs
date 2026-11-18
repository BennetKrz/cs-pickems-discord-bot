//! Sign-in, sessions and the admin check (T07).
//!
//! The Activity gets an OAuth2 code from the Embedded App SDK
//! (`commands.authorize`, scope `identify`) and posts it with the guild ID
//! the SDK reports. The guild ID is a client claim, so [`AuthService::login`]
//! verifies everything itself:
//!
//! 1. the guild is present and equals `DISCORD_GUILD_ID` when that is set;
//! 2. the code is exchanged with the client secret, and `/users/@me` names
//!    the user;
//! 3. the bot token confirms that the user is a member of the guild.
//!
//! It then upserts `users` and `guild_members` (this is how someone becomes
//! a player) and creates an opaque session of 7 days. The session travels as
//! `Authorization: Bearer`: the Activity runs in a third-party iframe, where
//! cookies are unreliable. Only the token's SHA-256 is stored.
//!
//! Routes, nested under `/api/auth`:
//! - `GET /config` → `{client_id}`, so one build serves any Discord app;
//! - `POST /token` `{code, guild_id}` → `{session_token, expires_at,
//!   access_token, me}`. The Discord access token goes back to the
//!   Activity for `commands.authenticate`; the server keeps no copy.
//!
//! See `docs/adr/0007-discord-auth-sessions.md`.

pub mod discord;

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use domain::Clock;
use domain::admin::AdminFacts;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use store::SqlitePool;
use store::sessions::Session;
use store::users::{GuildMember, User};

pub use self::discord::{
    DiscordApi, DiscordClient, DiscordClientConfig, DiscordError, DiscordGuild, DiscordMember,
    DiscordUser, Unconfigured,
};
use crate::AppState;
use crate::admin::{AdminAuth, AuthError};
use crate::error::ApiError;
use crate::hex;
use crate::me::{MeBody, me_body};

/// How long a session lasts. Fixed: it does not slide with use.
pub const SESSION_TTL: Duration = Duration::days(7);
/// How long Discord's answer to "is this member an admin" is reused.
pub const ADMIN_CACHE_TTL: Duration = Duration::minutes(5);
/// After Discord failed an admin check, it is not asked again for this long:
/// polls and admin requests during an outage then fail fast.
pub const DISCORD_BACKOFF: Duration = Duration::seconds(30);
/// Above this many entries, a cache insert first drops expired entries.
const CACHE_PRUNE_AT: usize = 1024;

/// Settings of [`AuthService`].
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// The Discord application's client ID, which the Activity's SDK needs.
    pub client_id: String,
    /// `DISCORD_GUILD_ID`: when set, only this guild is accepted.
    pub guild_id: Option<String>,
    pub session_ttl: Duration,
    pub admin_cache_ttl: Duration,
}

impl AuthConfig {
    pub fn new(client_id: impl Into<String>, guild_id: Option<String>) -> Self {
        Self {
            client_id: client_id.into(),
            guild_id,
            session_ttl: SESSION_TTL,
            admin_cache_ttl: ADMIN_CACHE_TTL,
        }
    }
}

/// A signed-in member: the session's user and guild. As an extractor it
/// rejects requests without a valid session with 401.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub user_id: String,
    pub guild_id: String,
}

impl FromRequestParts<AppState> for Member {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        Ok(state.auth.member(&parts.headers).await?)
    }
}

/// A new session. `token` is the bearer token; it exists only here and in
/// the response to the client.
pub struct NewSession {
    pub member: Member,
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

/// A successful sign-in.
pub struct Login {
    pub session: NewSession,
    /// The user's Discord access token, for `commands.authenticate`.
    pub access_token: String,
}

/// Sign-in, session lookup and the admin check.
pub struct AuthService {
    pool: SqlitePool,
    discord: Arc<dyn DiscordApi>,
    config: AuthConfig,
    /// Always the real clock in the app: the fixture dev clock (T05) must
    /// not expire sessions. Tests inject a fake one.
    clock: Arc<dyn Clock>,
    cache: AdminCache,
}

impl AuthService {
    pub fn new(
        pool: SqlitePool,
        discord: Arc<dyn DiscordApi>,
        config: AuthConfig,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            pool,
            discord,
            config,
            clock,
            cache: AdminCache::default(),
        }
    }

    /// Without Discord: existing sessions work, sign-in and admin checks
    /// answer 503. The default of [`AppState::new`].
    pub fn unconfigured(pool: SqlitePool) -> Self {
        Self::new(
            pool,
            Arc::new(Unconfigured),
            AuthConfig::new("", None),
            Arc::new(domain::SystemClock),
        )
    }

    pub fn config(&self) -> &AuthConfig {
        &self.config
    }

    /// Signs a member in with an authorization code from the Activity.
    pub async fn login(&self, code: &str, guild_claim: Option<&str>) -> Result<Login, ApiError> {
        let guild_id = self.check_guild_claim(guild_claim)?;
        if code.trim().is_empty() {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "code is required",
            ));
        }
        let access_token = self
            .discord
            .exchange_code(code)
            .await
            .map_err(login_error)?;
        let user = self
            .discord
            .current_user(&access_token)
            .await
            .map_err(login_error)?;
        let member = match self.discord.guild_member(&guild_id, &user.id).await {
            Ok(Some(m)) => m,
            Ok(None) | Err(DiscordError::NoAccess) => {
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    "not_a_member",
                    "you are not a member of this server",
                ));
            }
            Err(e) => return Err(login_error(e)),
        };
        let now = self.clock.now();
        cache_put(
            &self.cache.members,
            (guild_id.clone(), user.id.clone()),
            Some(member.roles.clone()),
            now,
            self.config.admin_cache_ttl,
        );
        let session = self.start_session(&user, &guild_id, member.nick).await?;
        Ok(Login {
            session,
            access_token,
        })
    }

    /// Checks the guild ID the client claims. Returns it if acceptable.
    pub(crate) fn check_guild_claim(&self, claim: Option<&str>) -> Result<String, ApiError> {
        let Some(claim) = claim.map(str::trim).filter(|g| !g.is_empty()) else {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "guild_required",
                "open the Activity from a server, not from a direct message",
            ));
        };
        if !is_snowflake(claim) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "guild_id must be a Discord ID",
            ));
        }
        if self.config.guild_id.as_deref().is_some_and(|g| g != claim) {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "wrong_guild",
                "this app serves another server",
            ));
        }
        Ok(claim.to_owned())
    }

    /// Upserts the user and their membership, then creates a session.
    /// Expired sessions of everyone are purged on the way.
    pub(crate) async fn start_session(
        &self,
        user: &DiscordUser,
        guild_id: &str,
        nick: Option<String>,
    ) -> Result<NewSession, ApiError> {
        let now = self.clock.now();
        let token = new_token()?;
        let expires_at = now + self.config.session_ttl;
        let db = |e| ApiError::db("sign-in", e);

        let mut tx = store::begin_write(&self.pool).await.map_err(db)?;
        store::sessions::delete_expired(&mut *tx, now)
            .await
            .map_err(db)?;
        store::users::upsert_user(
            &mut *tx,
            &User {
                discord_user_id: user.id.clone(),
                username: user.username.clone(),
                global_name: user.global_name.clone().filter(|n| !n.is_empty()),
                avatar_hash: user.avatar.clone(),
                updated_at: now,
            },
        )
        .await
        .map_err(db)?;
        store::users::upsert_member(
            &mut *tx,
            &GuildMember {
                guild_id: guild_id.to_owned(),
                user_id: user.id.clone(),
                nick: nick.filter(|n| !n.is_empty()),
                last_seen_at: now,
            },
        )
        .await
        .map_err(db)?;
        store::sessions::create(
            &mut *tx,
            &Session {
                token_hash: hash_token(&token),
                user_id: user.id.clone(),
                guild_id: guild_id.to_owned(),
                created_at: now,
                expires_at,
            },
        )
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;

        Ok(NewSession {
            member: Member {
                user_id: user.id.clone(),
                guild_id: guild_id.to_owned(),
            },
            token,
            expires_at,
        })
    }

    /// The member behind the request's `Authorization: Bearer` token.
    pub async fn member(&self, headers: &HeaderMap) -> Result<Member, AuthError> {
        let token = bearer(headers).ok_or_else(|| {
            AuthError::Unauthenticated("sign in first: no session token".to_owned())
        })?;
        if !is_token(token) {
            return Err(AuthError::Unauthenticated(
                "invalid session token".to_owned(),
            ));
        }
        let session = store::sessions::get_valid(&self.pool, &hash_token(token), self.clock.now())
            .await
            .map_err(|e| {
                tracing::error!("session lookup: database error: {e}");
                AuthError::Internal
            })?
            .ok_or_else(|| {
                AuthError::Unauthenticated("the session has expired; sign in again".to_owned())
            })?;
        if self
            .config
            .guild_id
            .as_deref()
            .is_some_and(|g| g != session.guild_id)
        {
            return Err(AuthError::Unauthenticated(
                "the session belongs to another server".to_owned(),
            ));
        }
        Ok(Member {
            user_id: session.user_id,
            guild_id: session.guild_id,
        })
    }

    /// Whether the member is an admin: the owner, *Manage Server* (or
    /// *Administrator*), or the role set with `/admin role`. Discord's data
    /// is cached for `admin_cache_ttl`; the configured role is read fresh,
    /// so changing it takes effect at once.
    pub async fn is_admin(&self, guild_id: &str, user_id: &str) -> Result<bool, AuthError> {
        let now = self.clock.now();
        let ttl = self.config.admin_cache_ttl;
        let unavailable = |e: DiscordError| {
            self.cache.backoff_until(now + DISCORD_BACKOFF);
            AuthError::Unavailable(format!("cannot check admin rights: {e}"))
        };
        let backing_off = || {
            self.cache.in_backoff(now).then(|| {
                AuthError::Unavailable("Discord is unavailable; retrying shortly".to_owned())
            })
        };

        let guild = match cache_get(&self.cache.guilds, &guild_id.to_owned(), now, ttl) {
            Some(g) => g,
            None => {
                if let Some(e) = backing_off() {
                    return Err(e);
                }
                let g = match self.discord.guild(guild_id).await {
                    Ok(g) => g,
                    Err(DiscordError::NoAccess) => None,
                    Err(e) => return Err(unavailable(e)),
                };
                cache_put(&self.cache.guilds, guild_id.to_owned(), g.clone(), now, ttl);
                g
            }
        };
        let Some(guild) = guild else {
            return Ok(false);
        };

        let key = (guild_id.to_owned(), user_id.to_owned());
        let roles = match cache_get(&self.cache.members, &key, now, ttl) {
            Some(r) => r,
            None => {
                if let Some(e) = backing_off() {
                    return Err(e);
                }
                let r = match self.discord.guild_member(guild_id, user_id).await {
                    Ok(m) => m.map(|m| m.roles),
                    Err(DiscordError::NoAccess) => None,
                    Err(e) => return Err(unavailable(e)),
                };
                cache_put(&self.cache.members, key, r.clone(), now, ttl);
                r
            }
        };
        let Some(roles) = roles else {
            return Ok(false);
        };

        let admin_role_id = store::guilds::get_settings(&self.pool, guild_id)
            .await
            .map_err(|e| {
                tracing::error!("admin check: database error: {e}");
                AuthError::Internal
            })?
            .and_then(|s| s.admin_role_id);
        Ok(domain::admin::is_admin(&AdminFacts {
            guild_id,
            owner_id: &guild.owner_id,
            role_permissions: &guild.role_permissions,
            user_id,
            member_roles: &roles,
            admin_role_id: admin_role_id.as_deref(),
        }))
    }
}

/// Admin routes in production: a valid session of an admin.
#[async_trait]
impl AdminAuth for AuthService {
    async fn authenticate(&self, headers: &HeaderMap) -> Result<String, AuthError> {
        let m = self.member(headers).await?;
        if self.is_admin(&m.guild_id, &m.user_id).await? {
            Ok(m.user_id)
        } else {
            Err(AuthError::Forbidden(
                "only server admins can do this".to_owned(),
            ))
        }
    }
}

fn login_error(e: DiscordError) -> ApiError {
    match e {
        DiscordError::InvalidGrant | DiscordError::InvalidToken => ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_code",
            "the sign-in code is invalid or expired; try again",
        ),
        DiscordError::NoAccess => ApiError::new(
            StatusCode::FORBIDDEN,
            "not_a_member",
            "you are not a member of this server",
        ),
        DiscordError::Unavailable => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "discord_unavailable",
            "Discord could not be reached; try again shortly",
        ),
    }
}

// ---- tokens -------------------------------------------------------------------

/// 32 random bytes from the OS, as 64 lowercase hex digits.
fn new_token() -> Result<String, ApiError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| {
        tracing::error!("no OS randomness for a session token: {e}");
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "cannot create a session",
        )
    })?;
    Ok(hex(&bytes))
}

/// What the database stores instead of the token.
pub(crate) fn hash_token(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

fn is_token(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The token of an `Authorization: Bearer <token>` header. The scheme is
/// case-insensitive.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.trim().split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|t| !t.is_empty())
}

/// A Discord snowflake: 1 to 20 decimal digits.
pub fn is_snowflake(s: &str) -> bool {
    (1..=20).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

// ---- admin cache ----------------------------------------------------------------

struct Cached<T> {
    value: T,
    at: DateTime<Utc>,
}

type CacheMap<K, V> = Mutex<HashMap<K, Cached<V>>>;

/// Discord's answers for the admin check. `None` values (unknown guild,
/// not a member) are cached too; errors are not.
#[derive(Default)]
struct AdminCache {
    guilds: CacheMap<String, Option<DiscordGuild>>,
    /// Role IDs per (guild, user).
    members: CacheMap<(String, String), Option<Vec<String>>>,
    /// Until when Discord is not asked after a failure.
    backoff: Mutex<Option<DateTime<Utc>>>,
}

impl AdminCache {
    fn backoff_until(&self, until: DateTime<Utc>) {
        *self.backoff.lock().unwrap_or_else(|e| e.into_inner()) = Some(until);
    }

    fn in_backoff(&self, now: DateTime<Utc>) -> bool {
        self.backoff
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|until| now < until)
    }
}

fn cache_get<K: Eq + Hash, V: Clone>(
    map: &CacheMap<K, V>,
    key: &K,
    now: DateTime<Utc>,
    ttl: Duration,
) -> Option<V> {
    let map = map.lock().unwrap_or_else(|e| e.into_inner());
    map.get(key)
        .filter(|c| now - c.at < ttl && c.at <= now)
        .map(|c| c.value.clone())
}

fn cache_put<K: Eq + Hash, V>(
    map: &CacheMap<K, V>,
    key: K,
    value: V,
    now: DateTime<Utc>,
    ttl: Duration,
) {
    let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() >= CACHE_PRUNE_AT {
        map.retain(|_, c| now - c.at < ttl);
    }
    map.insert(key, Cached { value, at: now });
}

// ---- routes -------------------------------------------------------------------

/// Routes `/token` and `/config`, to be nested under `/api/auth`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/config", get(config))
        .route("/token", post(token))
}

#[derive(Debug, Serialize)]
struct ConfigBody {
    client_id: String,
}

async fn config(State(st): State<AppState>) -> Json<ConfigBody> {
    Json(ConfigBody {
        client_id: st.auth.config().client_id.clone(),
    })
}

#[derive(Debug, Deserialize)]
struct TokenReq {
    code: String,
    #[serde(default)]
    guild_id: Option<String>,
}

/// The answer to a sign-in. Never cached.
#[derive(Serialize)]
pub(crate) struct SessionBody {
    session_token: String,
    expires_at: DateTime<Utc>,
    /// Absent for dev sessions, which involve no Discord.
    #[serde(skip_serializing_if = "Option::is_none")]
    access_token: Option<String>,
    me: MeBody,
}

pub(crate) fn session_response(
    session: NewSession,
    access_token: Option<String>,
    me: MeBody,
) -> Response {
    let mut res = Json(SessionBody {
        session_token: session.token,
        expires_at: session.expires_at,
        access_token,
        me,
    })
    .into_response();
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

async fn token(
    State(st): State<AppState>,
    Json(req): Json<TokenReq>,
) -> Result<Response, ApiError> {
    let login = st.auth.login(&req.code, req.guild_id.as_deref()).await?;
    let me = me_body(&st, &login.session.member).await?;
    Ok(session_response(
        login.session,
        Some(login.access_token),
        me,
    ))
}

#[cfg(test)]
mod tests;
