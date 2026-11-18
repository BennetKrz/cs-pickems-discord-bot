//! Image proxy for avatars and team logos (T07).
//!
//! The Activity's CSP only lets it reach its own origin through Discord's
//! proxy, so images from Discord's CDN or a data source's CDN are served
//! from here:
//!
//! - `GET /api/img/avatar/{user_id}`: a known user's Discord avatar (or the
//!   default avatar);
//! - `GET /api/img/team/{team_id}`: a team's logo.
//!
//! Routes take internal IDs and look the upstream URL up in the database,
//! so the proxy is not open, and source URLs (which contain external IDs)
//! never reach the client. Upstream URLs must be HTTPS on an allow-listed
//! host, also across redirects. Only raster images (PNG, JPEG, GIF, WebP,
//! AVIF) up to 2 MB are served, recognized by their magic bytes; the
//! upstream content type is ignored.
//!
//! Images are cached on disk under `IMAGE_CACHE_DIR`, one file per upstream
//! URL (named by its SHA-256), and refetched after 7 days. When a refetch
//! fails, the stale copy is served. The routes need no session: `<img>`
//! cannot send a bearer token.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use reqwest::Url;
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::auth::is_snowflake;
use crate::error::ApiError;
use crate::hex;

/// Hosts the proxy fetches from: Discord's CDN and PandaScore's CDN.
pub const ALLOWED_HOSTS: [&str; 2] = ["cdn.discordapp.com", "cdn-api.pandascore.co"];
/// The largest image served.
pub const MAX_IMAGE_BYTES: usize = 2 * 1024 * 1024;
/// How long a cached image is served before it is fetched again.
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// How long browsers may keep an image. URLs carry a version, so a change
/// of avatar or logo gets a new URL anyway.
const BROWSER_CACHE: &str = "public, max-age=86400";
const MAX_REDIRECTS: usize = 3;

/// The avatar URL the API hands out. `v` changes with the avatar, so the
/// browser cache never shows an old one.
pub fn avatar_url(user_id: &str, avatar_hash: Option<&str>) -> String {
    format!("/api/img/avatar/{user_id}?v={}", avatar_hash.unwrap_or("0"))
}

/// The logo URL the API hands out (T12 onwards), or `None` without a logo.
/// `v` is derived from the upstream URL without revealing it.
pub fn team_logo_url(team_id: i64, logo_url: Option<&str>) -> Option<String> {
    let logo = logo_url?;
    let v = hex(&Sha256::digest(logo.as_bytes()));
    Some(format!("/api/img/team/{team_id}?v={}", &v[..12]))
}

/// Whether the proxy may fetch `url`: HTTPS, default port, allowed host.
pub fn is_allowed(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port().is_none()
        && url.host_str().is_some_and(|h| ALLOWED_HOSTS.contains(&h))
}

/// The content type of a supported raster image, by its magic bytes.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    let at = |range: std::ops::Range<usize>| bytes.get(range);
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if at(0..4) == Some(b"RIFF") && at(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if at(4..8) == Some(b"ftyp") && matches!(at(8..12), Some(b"avif" | b"avis")) {
        Some("image/avif")
    } else {
        None
    }
}

// ---- fetching -----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("HTTP {0}")]
    Status(u16),
    #[error("larger than {MAX_IMAGE_BYTES} bytes")]
    TooLarge,
    #[error("{0}")]
    Other(String),
}

/// Downloads one image. A trait so tests never touch the network.
#[async_trait]
pub trait ImageFetcher: Send + Sync {
    async fn fetch(&self, url: &Url) -> Result<Vec<u8>, FetchError>;
}

/// Fetches nothing; the default of [`crate::AppState::new`].
#[derive(Debug, Clone, Copy, Default)]
pub struct NoFetch;

#[async_trait]
impl ImageFetcher for NoFetch {
    async fn fetch(&self, _url: &Url) -> Result<Vec<u8>, FetchError> {
        Err(FetchError::Other("image fetching is disabled".to_owned()))
    }
}

/// The real fetcher: follows at most 3 redirects, each to an allowed URL,
/// and stops reading at [`MAX_IMAGE_BYTES`].
pub struct HttpFetcher {
    http: reqwest::Client,
}

impl HttpFetcher {
    pub fn new() -> Result<Self, String> {
        Self::with_allow(is_allowed)
    }

    /// With another allow rule; tests use it to reach a local plain-HTTP
    /// server.
    fn with_allow(allow: fn(&Url) -> bool) -> Result<Self, String> {
        let policy = reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if allow(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("redirect to a host that is not allowed")
            }
        });
        let http = reqwest::Client::builder()
            .user_agent(concat!(
                "cs-pickems-discord-bot/",
                env!("CARGO_PKG_VERSION")
            ))
            .redirect(policy)
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("cannot build HTTP client: {e}"))?;
        Ok(Self { http })
    }
}

#[async_trait]
impl ImageFetcher for HttpFetcher {
    async fn fetch(&self, url: &Url) -> Result<Vec<u8>, FetchError> {
        let mut res = self
            .http
            .get(url.clone())
            .send()
            .await
            .map_err(|e| FetchError::Other(e.to_string()))?;
        if !res.status().is_success() {
            return Err(FetchError::Status(res.status().as_u16()));
        }
        if res
            .content_length()
            .is_some_and(|n| n > MAX_IMAGE_BYTES as u64)
        {
            return Err(FetchError::TooLarge);
        }
        let mut buf = Vec::new();
        while let Some(chunk) = res
            .chunk()
            .await
            .map_err(|e| FetchError::Other(e.to_string()))?
        {
            if buf.len() + chunk.len() > MAX_IMAGE_BYTES {
                return Err(FetchError::TooLarge);
            }
            buf.extend_from_slice(&chunk);
        }
        Ok(buf)
    }
}

// ---- proxy --------------------------------------------------------------------

/// A servable image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub content_type: &'static str,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImgError {
    /// The URL is not allowed (404).
    NotAllowed,
    /// The upstream failed or sent no supported image, and nothing is
    /// cached (502).
    Upstream,
}

/// Fetches allowed images through a disk cache.
pub struct ImageProxy {
    /// `None` disables the cache (and with [`NoFetch`], the proxy).
    cache_dir: Option<PathBuf>,
    fetcher: Arc<dyn ImageFetcher>,
    max_age: Duration,
}

impl ImageProxy {
    pub fn new(cache_dir: PathBuf, fetcher: Arc<dyn ImageFetcher>) -> Self {
        Self {
            cache_dir: Some(cache_dir),
            fetcher,
            max_age: DEFAULT_MAX_AGE,
        }
    }

    /// No cache, no fetching: every image is a 502.
    pub fn disabled() -> Self {
        Self {
            cache_dir: None,
            fetcher: Arc::new(NoFetch),
            max_age: DEFAULT_MAX_AGE,
        }
    }

    pub fn with_max_age(mut self, max_age: Duration) -> Self {
        self.max_age = max_age;
        self
    }

    /// The image at `url`, from the cache while fresh, else fetched.
    pub async fn get(&self, url: &str) -> Result<Image, ImgError> {
        let url = Url::parse(url).ok().filter(is_allowed).ok_or_else(|| {
            tracing::warn!(url, "image URL is not allowed");
            ImgError::NotAllowed
        })?;
        let path = self
            .cache_dir
            .as_ref()
            .map(|dir| dir.join(hex(&Sha256::digest(url.as_str().as_bytes()))));
        let cached = match &path {
            Some(p) => read_cached(p, self.max_age).await,
            None => None,
        };
        if let Some((image, true)) = cached {
            return Ok(image);
        }
        let stale = cached.map(|(image, _)| image);

        let fetched = match self.fetcher.fetch(&url).await {
            Ok(bytes) => match sniff(&bytes) {
                Some(content_type) => Ok(Image {
                    content_type,
                    bytes,
                }),
                None => Err("not a supported image".to_owned()),
            },
            Err(e) => Err(e.to_string()),
        };
        match fetched {
            Ok(image) => {
                if let Some(p) = &path
                    && let Err(e) = write_atomic(p, &image.bytes).await
                {
                    tracing::warn!(path = %p.display(), "cannot cache image: {e}");
                }
                Ok(image)
            }
            Err(e) => {
                tracing::warn!(url = %url, stale = stale.is_some(), "image fetch failed: {e}");
                stale.ok_or(ImgError::Upstream)
            }
        }
    }
}

/// A cached image and whether it is still fresh. Unreadable or invalid
/// files count as absent.
async fn read_cached(path: &Path, max_age: Duration) -> Option<(Image, bool)> {
    let modified = tokio::fs::metadata(path).await.ok()?.modified().ok()?;
    let bytes = tokio::fs::read(path).await.ok()?;
    let content_type = sniff(&bytes)?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO);
    Some((
        Image {
            content_type,
            bytes,
        },
        age < max_age,
    ))
}

/// Writes through a temp file and a rename, so readers never see half a
/// file.
async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let suffix = getrandom::u64().map_err(std::io::Error::other)?;
    let tmp = path.with_extension(format!("tmp{suffix:016x}"));
    tokio::fs::write(&tmp, bytes).await?;
    if let Err(e) = tokio::fs::rename(&tmp, path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    Ok(())
}

// ---- routes -------------------------------------------------------------------

/// Routes `/avatar/{user_id}` and `/team/{team_id}`, to be nested under
/// `/api/img`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/avatar/{user_id}", get(avatar))
        .route("/team/{team_id}", get(team))
}

/// Discord's CDN URL for a user's avatar. Without a hash (or with an
/// unexpected one), the default avatar `(id >> 22) % 6`.
fn discord_avatar(user_id: &str, avatar_hash: Option<&str>) -> String {
    match avatar_hash
        .filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
    {
        Some(hash) => format!("https://cdn.discordapp.com/avatars/{user_id}/{hash}.png?size=128"),
        None => {
            let index = user_id.parse::<u64>().map_or(0, |id| (id >> 22) % 6);
            format!("https://cdn.discordapp.com/embed/avatars/{index}.png")
        }
    }
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such image")
}

async fn avatar(
    State(st): State<AppState>,
    UrlPath(user_id): UrlPath<String>,
) -> Result<Response, ApiError> {
    if !is_snowflake(&user_id) {
        return Err(not_found());
    }
    let user = store::users::get_user(&st.pool, &user_id)
        .await
        .map_err(|e| ApiError::db("avatar", e))?
        .ok_or_else(not_found)?;
    serve(
        &st,
        &discord_avatar(&user.discord_user_id, user.avatar_hash.as_deref()),
    )
    .await
}

async fn team(
    State(st): State<AppState>,
    UrlPath(team_id): UrlPath<i64>,
) -> Result<Response, ApiError> {
    let logo = store::teams::get(&st.pool, domain::TeamId(team_id))
        .await
        .map_err(|e| ApiError::db("team logo", e))?
        .and_then(|t| t.logo_url)
        .ok_or_else(not_found)?;
    serve(&st, &logo).await
}

async fn serve(st: &AppState, url: &str) -> Result<Response, ApiError> {
    match st.images.get(url).await {
        Ok(image) => Ok((
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static(image.content_type),
                ),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_static(BROWSER_CACHE),
                ),
                (
                    header::X_CONTENT_TYPE_OPTIONS,
                    HeaderValue::from_static("nosniff"),
                ),
            ],
            image.bytes,
        )
            .into_response()),
        Err(ImgError::NotAllowed) => Err(not_found()),
        Err(ImgError::Upstream) => Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "image_unavailable",
            "the image could not be loaded",
        )),
    }
}

#[cfg(test)]
mod tests;
