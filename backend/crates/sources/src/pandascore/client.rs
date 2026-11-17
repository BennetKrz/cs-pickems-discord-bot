//! HTTP client for PandaScore's REST API: bearer auth, pagination, quota
//! headers, the hard request cap and 429 backoff.

use std::sync::Mutex;
use std::time::Duration;

use reqwest::header::{HeaderMap, LINK, RETRY_AFTER};
use reqwest::{StatusCode, Url};
use serde_json::Value;
use tokio::time::Instant;

use super::PandascoreConfig;
use super::limiter::Limiter;
use crate::{Quota, SourceError, SourceResult};

/// Items per page; PandaScore's maximum.
pub const PAGE_SIZE: usize = 100;

/// Pages followed per listing at most, as a guard against loops.
pub const MAX_PAGES: usize = 20;

const USER_AGENT: &str = concat!("cs-pickems-discord-bot/", env!("CARGO_PKG_VERSION"));

/// Retries after HTTP 429.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt.
    pub max_retries: u32,
    /// Delay before the first retry; doubles with each retry.
    pub base_delay: Duration,
    /// Upper bound of one delay. A `Retry-After` beyond it is not waited
    /// for: the call fails with `RateLimited` and the scheduler decides.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// Exponential backoff with "equal jitter": half the step is fixed, the
    /// other half random, so retries never hammer the API back to back.
    pub fn backoff(&self, retry: u32) -> Duration {
        let factor = 2u32.saturating_pow(retry);
        let step = self.base_delay.saturating_mul(factor).min(self.max_delay);
        let half = step / 2;
        let jitter = half.mul_f64(fastrand::f64());
        half + jitter
    }
}

pub struct Client {
    http: reqwest::Client,
    base: Url,
    token: String,
    retry: RetryPolicy,
    limiter: Mutex<Limiter>,
    quota: Mutex<Option<Quota>>,
}

impl Client {
    pub fn new(config: &PandascoreConfig) -> SourceResult<Self> {
        let base = Url::parse(config.base_url.trim_end_matches('/'))
            .map_err(|e| SourceError::Other(format!("invalid PandaScore base URL: {e}")))?;
        let mut http = reqwest::Client::builder().user_agent(USER_AGENT);
        if let Some(timeout) = config.timeout {
            http = http.timeout(timeout).connect_timeout(timeout);
        }
        let http = http
            .build()
            .map_err(|e| SourceError::Other(format!("cannot build HTTP client: {e}")))?;
        Ok(Self {
            http,
            base,
            token: config.token.clone(),
            retry: config.retry,
            limiter: Mutex::new(Limiter::new(config.limits)),
            quota: Mutex::new(None),
        })
    }

    pub fn quota(&self) -> Option<Quota> {
        *self.quota.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn url(&self, path: &str, query: &[(&str, String)]) -> Url {
        let mut url = self.base.clone();
        url.set_path(path);
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (*k, v.as_str())));
        }
        url
    }

    /// One JSON document.
    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> SourceResult<Value> {
        let (body, _) = self.send(self.url(path, query)).await?;
        Ok(body)
    }

    /// Every item of a paginated listing, following `Link: rel="next"`.
    pub async fn get_all(&self, path: &str, query: &[(&str, String)]) -> SourceResult<Vec<Value>> {
        let mut query = query.to_vec();
        if !query.iter().any(|(k, _)| *k == "per_page") {
            query.push(("per_page", PAGE_SIZE.to_string()));
        }
        let mut url = self.url(path, &query);
        let mut items = Vec::new();
        for _ in 0..MAX_PAGES {
            let (body, next) = self.send(url).await?;
            match body {
                Value::Array(page) => items.extend(page),
                other => {
                    return Err(SourceError::Decode(format!(
                        "{path}: expected a list, got {}",
                        kind_of(&other)
                    )));
                }
            }
            match next {
                Some(next) => url = next,
                None => return Ok(items),
            }
        }
        tracing::warn!(path, max = MAX_PAGES, "PandaScore listing truncated");
        Ok(items)
    }

    /// Sends one GET, with the hard cap checked before every attempt and
    /// retries after 429. Returns the body and the next page, if any.
    async fn send(&self, url: Url) -> SourceResult<(Value, Option<Url>)> {
        let mut retry = 0;
        loop {
            self.acquire()?;
            let resp = self
                .http
                .get(url.clone())
                .bearer_auth(&self.token)
                .send()
                .await
                .map_err(|e| SourceError::Http(format!("{}: {e}", url.path())))?;
            let status = resp.status();
            self.record_quota(resp.headers());

            if status == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = retry_after(resp.headers());
                let delay = match retry_after {
                    _ if retry >= self.retry.max_retries => None,
                    Some(d) if d > self.retry.max_delay => None,
                    Some(d) => Some(d),
                    None => Some(self.retry.backoff(retry)),
                };
                let Some(delay) = delay else {
                    return Err(SourceError::RateLimited { retry_after });
                };
                tracing::warn!(
                    path = url.path(),
                    retry = retry + 1,
                    delay_ms = delay.as_millis() as u64,
                    "PandaScore rate limit hit; backing off"
                );
                tokio::time::sleep(delay).await;
                retry += 1;
                continue;
            }

            let next = next_link(resp.headers()).map(|l| self.rebase(&l));
            if !status.is_success() {
                return Err(status_error(status, url.path()));
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| SourceError::Http(format!("{}: {e}", url.path())))?;
            let body = serde_json::from_slice(&bytes)
                .map_err(|e| SourceError::Decode(format!("{}: {e}", url.path())))?;
            return Ok((body, next));
        }
    }

    fn acquire(&self) -> SourceResult<()> {
        let mut limiter = self.limiter.lock().unwrap_or_else(|e| e.into_inner());
        limiter
            .try_acquire(Instant::now())
            .map_err(|wait| SourceError::RateLimited {
                retry_after: Some(wait),
            })
    }

    fn record_quota(&self, headers: &HeaderMap) {
        let num = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u32>().ok())
        };
        let Some(remaining) = num("x-rate-limit-remaining") else {
            return;
        };
        let used = num("x-rate-limit-used");
        *self.quota.lock().unwrap_or_else(|e| e.into_inner()) = Some(Quota { remaining, used });
        self.limiter
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .note_remaining(remaining, Instant::now());
    }

    /// The next-page link, re-based onto our base URL, so the token is
    /// never sent to another host.
    fn rebase(&self, link: &Url) -> Url {
        let mut url = self.base.clone();
        url.set_path(link.path());
        url.set_query(link.query());
        url
    }
}

fn status_error(status: StatusCode, path: &str) -> SourceError {
    match status {
        StatusCode::NOT_FOUND => SourceError::NotFound(path.to_owned()),
        StatusCode::UNAUTHORIZED => {
            SourceError::Http(format!("{path}: 401 unauthorized (check PANDASCORE_TOKEN)"))
        }
        StatusCode::FORBIDDEN => SourceError::Http(format!(
            "{path}: 403 forbidden (not included in the PandaScore plan)"
        )),
        other => SourceError::Http(format!("{path}: HTTP {}", other.as_u16())),
    }
}

/// `Retry-After` in seconds; the HTTP-date form is ignored.
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// The `rel="next"` URL of a `Link` header.
fn next_link(headers: &HeaderMap) -> Option<Url> {
    headers
        .get_all(LINK)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .find_map(|part| {
            let (target, params) = part.split_once(';')?;
            let is_next = params
                .split(';')
                .any(|p| p.trim().replace(' ', "") == "rel=\"next\"");
            if !is_next {
                return None;
            }
            let target = target.trim().strip_prefix('<')?.strip_suffix('>')?;
            Url::parse(target).ok()
        })
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn parses_next_link() {
        let mut h = HeaderMap::new();
        h.insert(
            LINK,
            HeaderValue::from_static(
                "<https://api.pandascore.co/csgo/matches/past?page=101712&per_page=1>; rel=\"last\", \
                 <https://api.pandascore.co/csgo/matches/past?page=2&per_page=1>; rel=\"next\"",
            ),
        );
        let next = next_link(&h).unwrap();
        assert_eq!(next.path(), "/csgo/matches/past");
        assert_eq!(next.query(), Some("page=2&per_page=1"));

        h.insert(
            LINK,
            HeaderValue::from_static("<https://x/y?page=1>; rel=\"first\""),
        );
        assert_eq!(next_link(&h), None);
    }

    #[test]
    fn backoff_grows_with_jitter_and_cap() {
        let p = RetryPolicy {
            max_retries: 5,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
        };
        for _ in 0..100 {
            let d0 = p.backoff(0);
            assert!(d0 >= Duration::from_millis(500) && d0 <= Duration::from_secs(1));
            let d2 = p.backoff(2);
            assert!(d2 >= Duration::from_secs(2) && d2 <= Duration::from_secs(4));
            let d9 = p.backoff(9);
            assert!(d9 >= Duration::from_secs(15) && d9 <= Duration::from_secs(30));
        }
        let samples: std::collections::HashSet<_> = (0..20).map(|_| p.backoff(3)).collect();
        assert!(samples.len() > 1, "backoff is jittered");
    }

    #[test]
    fn retry_after_seconds_only() {
        let mut h = HeaderMap::new();
        assert_eq!(retry_after(&h), None);
        h.insert(RETRY_AFTER, HeaderValue::from_static("7"));
        assert_eq!(retry_after(&h), Some(Duration::from_secs(7)));
        h.insert(
            RETRY_AFTER,
            HeaderValue::from_static("Wed, 21 Oct 2026 07:28:00 GMT"),
        );
        assert_eq!(retry_after(&h), None);
    }
}
