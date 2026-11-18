//! Discord REST calls for sign-in and the admin check (T07): the OAuth2 code
//! exchange, the signed-in user, and guild data read with the bot token.
//!
//! [`DiscordApi`] is a trait so tests run against fakes; [`DiscordClient`]
//! is the real client. Tokens and secrets are never logged.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::AUTHORIZATION;
use reqwest::{StatusCode, Url};
use serde::Deserialize;

/// Discord's REST API, version 10.
pub const DISCORD_API: &str = "https://discord.com/api/v10";

/// Discord asks bots to identify as `DiscordBot ($url, $versionNumber)`.
const USER_AGENT: &str = concat!(
    "DiscordBot (https://github.com/BennetKrz/cs-pickems-discord-bot, ",
    env!("CARGO_PKG_VERSION"),
    ")"
);

/// A Discord user as `/users/@me` returns it (the fields we keep).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DiscordUser {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    /// Avatar hash; `None` means the default avatar.
    #[serde(default)]
    pub avatar: Option<String>,
}

/// A guild member as `GET /guilds/{g}/members/{u}` returns it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DiscordMember {
    #[serde(default)]
    pub nick: Option<String>,
    /// Role IDs, without `@everyone`.
    #[serde(default)]
    pub roles: Vec<String>,
}

/// The guild facts the admin rule needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscordGuild {
    pub owner_id: String,
    /// Permissions per role ID, including `@everyone` under the guild ID.
    pub role_permissions: HashMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiscordError {
    /// The authorization code is invalid, expired or already used.
    #[error("the authorization code was rejected")]
    InvalidGrant,
    /// The user's access token was rejected.
    #[error("the access token was rejected")]
    InvalidToken,
    /// The bot can't see this guild (it is not a member of it).
    #[error("the bot has no access to this server")]
    NoAccess,
    /// Discord could not be reached, rate-limited us, or rejected our own
    /// credentials. Details are in the log.
    #[error("Discord is unavailable")]
    Unavailable,
}

/// The Discord calls the API needs.
#[async_trait]
pub trait DiscordApi: Send + Sync {
    /// Exchanges an OAuth2 authorization code for an access token.
    async fn exchange_code(&self, code: &str) -> Result<String, DiscordError>;
    /// The user the access token belongs to.
    async fn current_user(&self, access_token: &str) -> Result<DiscordUser, DiscordError>;
    /// The member, or `None` if the user is not in the guild.
    async fn guild_member(
        &self,
        guild_id: &str,
        user_id: &str,
    ) -> Result<Option<DiscordMember>, DiscordError>;
    /// The guild's owner and roles, or `None` if the guild is unknown.
    async fn guild(&self, guild_id: &str) -> Result<Option<DiscordGuild>, DiscordError>;
}

/// Answers every call with [`DiscordError::Unavailable`]. The default in
/// [`crate::AppState::new`], so tests that don't sign in need no Discord.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unconfigured;

#[async_trait]
impl DiscordApi for Unconfigured {
    async fn exchange_code(&self, _code: &str) -> Result<String, DiscordError> {
        Err(DiscordError::Unavailable)
    }
    async fn current_user(&self, _token: &str) -> Result<DiscordUser, DiscordError> {
        Err(DiscordError::Unavailable)
    }
    async fn guild_member(
        &self,
        _guild_id: &str,
        _user_id: &str,
    ) -> Result<Option<DiscordMember>, DiscordError> {
        Err(DiscordError::Unavailable)
    }
    async fn guild(&self, _guild_id: &str) -> Result<Option<DiscordGuild>, DiscordError> {
        Err(DiscordError::Unavailable)
    }
}

/// Settings of [`DiscordClient`].
#[derive(Clone)]
pub struct DiscordClientConfig {
    /// Default [`DISCORD_API`]; tests point it at a local server.
    pub base_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub bot_token: String,
    pub timeout: Duration,
}

impl DiscordClientConfig {
    pub fn new(client_id: &str, client_secret: &str, bot_token: &str) -> Self {
        Self {
            base_url: DISCORD_API.to_owned(),
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
            bot_token: bot_token.to_owned(),
            timeout: Duration::from_secs(10),
        }
    }
}

impl fmt::Debug for DiscordClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiscordClientConfig")
            .field("base_url", &self.base_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"***")
            .field("bot_token", &"***")
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// The real Discord REST client.
pub struct DiscordClient {
    http: reqwest::Client,
    base: Url,
    config: DiscordClientConfig,
    /// Rejected credentials are logged as an error once, then quietly.
    credentials_logged: AtomicBool,
}

impl fmt::Debug for DiscordClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiscordClient")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct OAuthError {
    #[serde(default)]
    error: String,
}

#[derive(Deserialize)]
struct RawRole {
    id: String,
    permissions: String,
}

#[derive(Deserialize)]
struct RawGuild {
    owner_id: String,
    #[serde(default)]
    roles: Vec<RawRole>,
}

impl DiscordClient {
    pub fn new(config: DiscordClientConfig) -> Result<Self, String> {
        let base = Url::parse(config.base_url.trim_end_matches('/'))
            .map_err(|e| format!("invalid Discord API URL: {e}"))?;
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(config.timeout)
            .connect_timeout(config.timeout)
            .build()
            .map_err(|e| format!("cannot build HTTP client: {e}"))?;
        Ok(Self {
            http,
            base,
            config,
            credentials_logged: AtomicBool::new(false),
        })
    }

    /// `base` plus path segments; each segment is percent-encoded, so an ID
    /// can never change the path.
    fn url(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .expect("an http(s) URL has path segments")
            .pop_if_empty()
            .extend(segments);
        url
    }

    /// Logs a configuration problem: as an error the first time, then at
    /// debug level, so a wrong token does not flood the log.
    fn credentials_rejected(&self, msg: &str) {
        if self.credentials_logged.swap(true, Ordering::Relaxed) {
            tracing::debug!("{msg}");
        } else {
            tracing::error!("{msg}");
        }
    }

    fn bot_auth(&self) -> String {
        format!("Bot {}", self.config.bot_token)
    }

    /// GET with the bot token: `Ok(None)` on 404, [`DiscordError::NoAccess`]
    /// on 403.
    async fn bot_get<T: serde::de::DeserializeOwned>(
        &self,
        what: &'static str,
        url: Url,
    ) -> Result<Option<T>, DiscordError> {
        let res = self
            .http
            .get(url)
            .header(AUTHORIZATION, self.bot_auth())
            .send()
            .await
            .map_err(|e| unavailable(what, &e))?;
        match res.status() {
            s if s.is_success() => res
                .json::<T>()
                .await
                .map(Some)
                .map_err(|e| unavailable(what, &e)),
            StatusCode::NOT_FOUND => Ok(None),
            StatusCode::FORBIDDEN => Err(DiscordError::NoAccess),
            StatusCode::UNAUTHORIZED => {
                self.credentials_rejected(&format!(
                    "Discord rejected the bot token ({what}); check DISCORD_BOT_TOKEN"
                ));
                Err(DiscordError::Unavailable)
            }
            s => Err(bad_status(what, s)),
        }
    }
}

fn unavailable(what: &str, e: &reqwest::Error) -> DiscordError {
    // Request errors never contain headers, so no token can leak here.
    tracing::warn!("Discord request failed ({what}): {e}");
    DiscordError::Unavailable
}

fn bad_status(what: &str, status: StatusCode) -> DiscordError {
    tracing::warn!("Discord answered HTTP {status} ({what})");
    DiscordError::Unavailable
}

#[async_trait]
impl DiscordApi for DiscordClient {
    async fn exchange_code(&self, code: &str) -> Result<String, DiscordError> {
        const WHAT: &str = "oauth2 token";
        // The token endpoint takes form data only. Embedded apps send no
        // redirect_uri: the SDK's authorize command has none.
        let res = self
            .http
            .post(self.url(&["oauth2", "token"]))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", self.config.client_id.as_str()),
                ("client_secret", self.config.client_secret.as_str()),
            ])
            .send()
            .await
            .map_err(|e| unavailable(WHAT, &e))?;
        let status = res.status();
        if status.is_success() {
            return res
                .json::<TokenResponse>()
                .await
                .map(|t| t.access_token)
                .map_err(|e| unavailable(WHAT, &e));
        }
        let error = res
            .json::<OAuthError>()
            .await
            .map(|e| e.error)
            .unwrap_or_default();
        match (status, error.as_str()) {
            (StatusCode::BAD_REQUEST, "invalid_grant") => Err(DiscordError::InvalidGrant),
            (_, "invalid_client") | (StatusCode::UNAUTHORIZED, _) => {
                self.credentials_rejected(
                    "Discord rejected the client credentials; check DISCORD_CLIENT_ID and \
                     DISCORD_CLIENT_SECRET",
                );
                Err(DiscordError::Unavailable)
            }
            (StatusCode::BAD_REQUEST, other) => {
                tracing::warn!("Discord rejected the code exchange: {other}");
                Err(DiscordError::InvalidGrant)
            }
            (s, _) => Err(bad_status(WHAT, s)),
        }
    }

    async fn current_user(&self, access_token: &str) -> Result<DiscordUser, DiscordError> {
        const WHAT: &str = "users/@me";
        let res = self
            .http
            .get(self.url(&["users", "@me"]))
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| unavailable(WHAT, &e))?;
        match res.status() {
            s if s.is_success() => res.json().await.map_err(|e| unavailable(WHAT, &e)),
            StatusCode::UNAUTHORIZED => Err(DiscordError::InvalidToken),
            s => Err(bad_status(WHAT, s)),
        }
    }

    async fn guild_member(
        &self,
        guild_id: &str,
        user_id: &str,
    ) -> Result<Option<DiscordMember>, DiscordError> {
        self.bot_get(
            "guild member",
            self.url(&["guilds", guild_id, "members", user_id]),
        )
        .await
    }

    async fn guild(&self, guild_id: &str) -> Result<Option<DiscordGuild>, DiscordError> {
        let Some(raw) = self
            .bot_get::<RawGuild>("guild", self.url(&["guilds", guild_id]))
            .await?
        else {
            return Ok(None);
        };
        let mut role_permissions = HashMap::with_capacity(raw.roles.len());
        for role in raw.roles {
            match domain::admin::parse_permissions(&role.permissions) {
                Some(p) => {
                    role_permissions.insert(role.id, p);
                }
                None => tracing::warn!(
                    role = %role.id,
                    "Discord sent unparsable role permissions; treating them as none"
                ),
            }
        }
        Ok(Some(DiscordGuild {
            owner_id: raw.owner_id,
            role_permissions,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::extract::{Form, Path, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::json;

    use super::*;

    #[derive(Default)]
    struct Seen {
        token_form: Vec<(String, String)>,
        headers: Vec<(String, String, String)>, // path, authorization, user-agent
    }

    type Shared = Arc<Mutex<Seen>>;

    fn record(seen: &Shared, path: &str, h: &HeaderMap) {
        let get = |k: &str| {
            h.get(k)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        seen.lock().unwrap().headers.push((
            path.to_owned(),
            get("authorization"),
            get("user-agent"),
        ));
    }

    /// A local stand-in for Discord's API.
    async fn mock() -> (DiscordClient, Shared) {
        let seen: Shared = Arc::default();
        let app = Router::new()
            .route(
                "/api/v10/oauth2/token",
                post(
                    |State(s): State<Shared>,
                     h: HeaderMap,
                     Form(f): Form<Vec<(String, String)>>| async move {
                        record(&s, "token", &h);
                        let code = f.iter().find(|(k, _)| k == "code").map(|(_, v)| v.clone());
                        s.lock().unwrap().token_form = f;
                        match code.as_deref() {
                            Some("good") => (
                                StatusCode::OK,
                                Json(json!({
                                    "access_token": "at-1", "token_type": "Bearer",
                                    "expires_in": 604800, "refresh_token": "rt", "scope": "identify"
                                })),
                            ),
                            _ => (
                                StatusCode::BAD_REQUEST,
                                Json(json!({ "error": "invalid_grant" })),
                            ),
                        }
                    },
                ),
            )
            .route(
                "/api/v10/users/@me",
                get(|State(s): State<Shared>, h: HeaderMap| async move {
                    record(&s, "me", &h);
                    if h.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer at-1") {
                        (
                            StatusCode::OK,
                            Json(json!({
                                "id": "200", "username": "alice", "global_name": "Alice",
                                "avatar": "abc", "discriminator": "0"
                            })),
                        )
                    } else {
                        (StatusCode::UNAUTHORIZED, Json(json!({ "code": 0 })))
                    }
                }),
            )
            .route(
                "/api/v10/guilds/{g}/members/{u}",
                get(
                    |State(s): State<Shared>,
                     h: HeaderMap,
                     Path((g, u)): Path<(String, String)>| async move {
                        record(&s, "member", &h);
                        match (g.as_str(), u.as_str()) {
                            ("100", "200") => (
                                StatusCode::OK,
                                Json(json!({ "nick": "ali", "roles": ["11"], "joined_at": "x" })),
                            ),
                            ("100", _) => (
                                StatusCode::NOT_FOUND,
                                Json(json!({ "code": 10007, "message": "Unknown Member" })),
                            ),
                            _ => (
                                StatusCode::FORBIDDEN,
                                Json(json!({ "code": 50001, "message": "Missing Access" })),
                            ),
                        }
                    },
                ),
            )
            .route(
                "/api/v10/guilds/{g}",
                get(
                    |State(s): State<Shared>, h: HeaderMap, Path(g): Path<String>| async move {
                        record(&s, "guild", &h);
                        if g == "100" {
                            (
                                StatusCode::OK,
                                Json(json!({
                                    "id": "100", "owner_id": "1",
                                    "roles": [
                                        { "id": "100", "permissions": "1024" },
                                        { "id": "11", "permissions": "32" },
                                        { "id": "12", "permissions": "garbage" }
                                    ]
                                })),
                            )
                        } else {
                            (StatusCode::NOT_FOUND, Json(json!({ "code": 10004 })))
                        }
                    },
                ),
            )
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = DiscordClientConfig::new("42", "s3cret", "bot-tok");
        config.base_url = format!("http://{addr}/api/v10");
        (DiscordClient::new(config).unwrap(), seen)
    }

    #[tokio::test]
    async fn exchanges_codes_as_form_data_with_client_credentials() {
        let (client, seen) = mock().await;
        assert_eq!(client.exchange_code("good").await.unwrap(), "at-1");
        let form = seen.lock().unwrap().token_form.clone();
        let mut form: Vec<(&str, &str)> =
            form.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        form.sort();
        assert_eq!(
            form,
            [
                ("client_id", "42"),
                ("client_secret", "s3cret"),
                ("code", "good"),
                ("grant_type", "authorization_code"),
            ]
        );
        assert_eq!(
            client.exchange_code("used").await,
            Err(DiscordError::InvalidGrant)
        );
    }

    #[tokio::test]
    async fn reads_the_user_with_the_access_token() {
        let (client, _) = mock().await;
        let user = client.current_user("at-1").await.unwrap();
        assert_eq!(
            user,
            DiscordUser {
                id: "200".into(),
                username: "alice".into(),
                global_name: Some("Alice".into()),
                avatar: Some("abc".into()),
            }
        );
        assert_eq!(
            client.current_user("stale").await,
            Err(DiscordError::InvalidToken)
        );
    }

    #[tokio::test]
    async fn reads_guild_data_with_the_bot_token() {
        let (client, seen) = mock().await;
        let member = client.guild_member("100", "200").await.unwrap().unwrap();
        assert_eq!(member.nick.as_deref(), Some("ali"));
        assert_eq!(member.roles, ["11"]);
        assert_eq!(client.guild_member("100", "999").await, Ok(None));
        assert_eq!(
            client.guild_member("555", "200").await,
            Err(DiscordError::NoAccess)
        );

        let guild = client.guild("100").await.unwrap().unwrap();
        assert_eq!(guild.owner_id, "1");
        assert_eq!(
            guild.role_permissions,
            HashMap::from([("100".to_owned(), 1024), ("11".to_owned(), 32)]),
            "unparsable permissions are dropped"
        );
        assert_eq!(client.guild("555").await, Ok(None));

        // Bot calls carry the bot token; every call names the app.
        for (path, auth, ua) in &seen.lock().unwrap().headers {
            assert_eq!(auth, "Bot bot-tok", "{path}");
            assert!(ua.starts_with("DiscordBot (https://"), "{ua}");
        }
    }

    #[tokio::test]
    async fn path_segments_are_encoded() {
        let (client, _) = mock().await;
        let url = client.url(&["guilds", "1/../2", "members", "?x"]);
        assert!(
            url.path().ends_with("/guilds/1%2F..%2F2/members/%3Fx"),
            "{url}"
        );
        assert_eq!(url.query(), None);
    }

    #[tokio::test]
    async fn unreachable_discord_is_unavailable() {
        let mut config = DiscordClientConfig::new("42", "s", "b");
        // A port nobody listens on.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        config.base_url = format!("http://{}/api/v10", listener.local_addr().unwrap());
        drop(listener);
        let client = DiscordClient::new(config).unwrap();
        assert_eq!(client.guild("100").await, Err(DiscordError::Unavailable));
    }

    #[test]
    fn debug_output_hides_secrets() {
        let config = DiscordClientConfig::new("42", "s3cret", "bot-tok");
        let text = format!("{config:?}");
        assert!(
            !text.contains("s3cret") && !text.contains("bot-tok"),
            "{text}"
        );
    }
}
