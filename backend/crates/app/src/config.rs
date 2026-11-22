//! Typed configuration from environment variables plus an optional TOML file.
//!
//! Precedence per setting: environment variable, then `config.toml`, then the
//! built-in default. Secrets (`DISCORD_CLIENT_SECRET`, `DISCORD_BOT_TOKEN`,
//! `PANDASCORE_TOKEN`) come from the environment only; putting one into the
//! TOML file is a startup error. All problems are collected and reported
//! together, naming the environment variable to set.
//!
//! See `docs/adr/0002-config-and-runtime-skeleton.md`.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Default config file, relative to the working directory. Optional.
pub const DEFAULT_CONFIG_FILE: &str = "config.toml";

/// Default image cache of the proxy, relative to the working directory.
pub const DEFAULT_IMAGE_CACHE_DIR: &str = "data/img-cache";

const SECRET_KEYS: [&str; 3] = [
    "DISCORD_CLIENT_SECRET",
    "DISCORD_BOT_TOKEN",
    "PANDASCORE_TOKEN",
];

/// A secret value. Its `Debug` output is redacted and it has no `Display`,
/// so it cannot end up in logs by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Returns the secret value. Never log the result.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// A data source that can be enabled via `ENABLED_SOURCES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceId {
    Pandascore,
    Manual,
    Fixture,
}

impl SourceId {
    pub const ALL: [SourceId; 3] = [SourceId::Pandascore, SourceId::Manual, SourceId::Fixture];

    pub fn as_str(self) -> &'static str {
        match self {
            SourceId::Pandascore => "pandascore",
            SourceId::Manual => "manual",
            SourceId::Fixture => "fixture",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|id| id.as_str().eq_ignore_ascii_case(s.trim()))
    }
}

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable lines.
    Pretty,
    /// One JSON object per line.
    Json,
}

/// Request budget for one source (enforced by the sync scheduler, T06).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub per_hour: u32,
    pub per_minute: u32,
}

/// Discord application settings.
#[derive(Debug, Clone)]
pub struct DiscordConfig {
    pub client_id: String,
    pub client_secret: Secret,
    pub bot_token: Secret,
    /// When set, only this guild is accepted (T07).
    pub guild_id: Option<String>,
}

/// The complete, validated configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub discord: DiscordConfig,
    pub database_url: String,
    pub pandascore_token: Option<Secret>,
    pub enabled_sources: Vec<SourceId>,
    pub log_format: LogFormat,
    /// `tracing` filter directives, from `RUST_LOG`.
    pub log_filter: String,
    pub bind_addr: SocketAddr,
    /// Directory with the built frontend (`pnpm build` output).
    pub static_dir: PathBuf,
    /// Disk cache of the image proxy (T07): avatars and team logos.
    pub image_cache_dir: PathBuf,
    /// Request budget per source. Sources without an entry are unbudgeted
    /// (they make no external calls).
    pub budgets: BTreeMap<SourceId, Budget>,
    /// The only source that creates canonical entities (T03). `None` if
    /// only `manual` is enabled.
    pub primary_source: Option<SourceId>,
    /// Field precedence per field group, from `[precedence]` (T03).
    pub precedence: sync::Precedence,
    /// Cadences of the sync scheduler, from `[sync]` and `SYNC__*` (T06).
    pub sync: sync::SyncConfig,
    /// Backups (P4), from `BACKUP_*` and `TZ`.
    pub backup: backup::BackupConfig,
}

/// Everything wrong with the configuration, reported at once.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ConfigError {
    /// Required environment variables that are unset or empty.
    pub missing: Vec<&'static str>,
    /// Other problems, one human-readable line each.
    pub invalid: Vec<String>,
}

impl ConfigError {
    fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.invalid.is_empty()
    }

    fn invalid(msg: impl Into<String>) -> Self {
        Self {
            missing: Vec::new(),
            invalid: vec![msg.into()],
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        if !self.missing.is_empty() {
            write!(
                f,
                "missing required environment variable(s): {}",
                self.missing.join(", ")
            )?;
            first = false;
        }
        for line in &self.invalid {
            if !first {
                f.write_str("\n")?;
            }
            f.write_str(line)?;
            first = false;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

/// Settings allowed in `config.toml`. Keys are the lowercase environment
/// variable names; secrets are not allowed here.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    discord_client_id: Option<StringOrInt>,
    discord_guild_id: Option<StringOrInt>,
    database_url: Option<String>,
    enabled_sources: Option<StringOrList>,
    log_format: Option<String>,
    rust_log: Option<String>,
    bind_addr: Option<String>,
    static_dir: Option<String>,
    image_cache_dir: Option<String>,
    primary_source: Option<String>,
    backup_dir: Option<String>,
    backup_extra_dir: Option<String>,
    backup_time: Option<String>,
    backup_keep_daily: Option<StringOrInt>,
    backup_keep_weekly: Option<StringOrInt>,
    backup_keep_special: Option<StringOrInt>,
    #[serde(default)]
    budget: BTreeMap<String, FileBudget>,
    /// Field group → sources, highest precedence first.
    #[serde(default)]
    precedence: BTreeMap<String, Vec<String>>,
    /// Sync cadences (T06), durations such as `30s`, `5m`, `6h`, `2d`.
    #[serde(default)]
    sync: BTreeMap<String, StringOrList>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileBudget {
    per_hour: Option<u32>,
    per_minute: Option<u32>,
}

/// Discord snowflakes may be written with or without quotes in TOML.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StringOrInt {
    Str(String),
    Int(u64),
}

impl StringOrInt {
    fn into_string(self) -> String {
        match self {
            StringOrInt::Str(s) => s,
            StringOrInt::Int(n) => n.to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StringOrList {
    Str(String),
    List(Vec<String>),
}

impl StringOrList {
    fn into_string(self) -> String {
        match self {
            StringOrList::Str(s) => s,
            StringOrList::List(v) => v.join(","),
        }
    }
}

/// Loads the configuration from the process environment and the config file.
///
/// The file is `$CONFIG_FILE` if set (then it must exist), else
/// `./config.toml` if present.
pub fn load() -> Result<Config, ConfigError> {
    let env: HashMap<String, String> = std::env::vars().collect();
    let (path, required) = match env.get("CONFIG_FILE").filter(|s| !s.trim().is_empty()) {
        Some(p) => (PathBuf::from(p), true),
        None => (PathBuf::from(DEFAULT_CONFIG_FILE), false),
    };
    let file = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => None,
        Err(e) => {
            return Err(ConfigError::invalid(format!(
                "cannot read config file {}: {e}",
                path.display()
            )));
        }
    };
    from_sources(&env, file.as_deref().map(|text| (path.as_path(), text)))
}

/// Builds the configuration from an environment map and optional TOML file
/// contents. Pure, so it is testable without touching the process
/// environment.
pub fn from_sources(
    env: &HashMap<String, String>,
    file: Option<(&Path, &str)>,
) -> Result<Config, ConfigError> {
    let file = match file {
        Some((path, text)) => parse_file(path, text)?,
        None => FileConfig::default(),
    };
    let mut err = ConfigError::default();

    // Empty values count as unset, so `FOO=` in a .env file is "missing".
    let env_get = |key: &str| {
        env.get(key)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let non_empty = |v: Option<String>| v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());

    let mut required = |key: &'static str, file_value: Option<String>| -> Option<String> {
        let value = env_get(key).or_else(|| non_empty(file_value));
        if value.is_none() {
            err.missing.push(key);
        }
        value
    };

    let client_id = required(
        "DISCORD_CLIENT_ID",
        file.discord_client_id.map(StringOrInt::into_string),
    );
    let client_secret = required("DISCORD_CLIENT_SECRET", None);
    let bot_token = required("DISCORD_BOT_TOKEN", None);
    let database_url = required("DATABASE_URL", file.database_url);

    if let Some(id) = &client_id {
        check_snowflake("DISCORD_CLIENT_ID", id, &mut err);
    }
    let guild_id = env_get("DISCORD_GUILD_ID")
        .or_else(|| non_empty(file.discord_guild_id.map(StringOrInt::into_string)));
    if let Some(id) = &guild_id {
        check_snowflake("DISCORD_GUILD_ID", id, &mut err);
    }

    // Sources
    let sources_raw = env_get("ENABLED_SOURCES")
        .or_else(|| non_empty(file.enabled_sources.map(StringOrList::into_string)))
        .unwrap_or_else(|| SourceId::Pandascore.as_str().to_owned());
    let mut enabled_sources = Vec::new();
    for name in sources_raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match SourceId::parse(name) {
            Some(id) if !enabled_sources.contains(&id) => enabled_sources.push(id),
            Some(_) => {}
            None => err.invalid.push(format!(
                "ENABLED_SOURCES: unknown source `{name}` (known: {})",
                known_sources()
            )),
        }
    }

    let pandascore_token = env_get("PANDASCORE_TOKEN").map(Secret);
    if enabled_sources.contains(&SourceId::Pandascore) && pandascore_token.is_none() {
        err.missing.push("PANDASCORE_TOKEN");
    }

    // Logging
    let log_format = match env_get("LOG_FORMAT")
        .or_else(|| non_empty(file.log_format))
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("pretty") => LogFormat::Pretty,
        Some("json") => LogFormat::Json,
        Some(other) => {
            err.invalid.push(format!(
                "LOG_FORMAT: expected `pretty` or `json`, got `{other}`"
            ));
            LogFormat::Pretty
        }
    };
    let log_filter = env_get("RUST_LOG")
        .or_else(|| non_empty(file.rust_log))
        .unwrap_or_else(|| "info".to_owned());
    if let Err(e) = tracing_subscriber::EnvFilter::try_new(&log_filter) {
        err.invalid.push(format!("RUST_LOG: invalid filter: {e}"));
    }

    // HTTP
    let bind_raw = env_get("BIND_ADDR")
        .or_else(|| non_empty(file.bind_addr))
        .unwrap_or_else(|| "0.0.0.0:8080".to_owned());
    let bind_addr = bind_raw.parse::<SocketAddr>().unwrap_or_else(|e| {
        err.invalid.push(format!(
            "BIND_ADDR: expected host:port such as 0.0.0.0:8080, got `{bind_raw}` ({e})"
        ));
        SocketAddr::from(([0, 0, 0, 0], 8080))
    });
    let static_dir = PathBuf::from(
        env_get("STATIC_DIR")
            .or_else(|| non_empty(file.static_dir))
            .unwrap_or_else(|| "../frontend/dist".to_owned()),
    );
    let image_cache_dir = PathBuf::from(
        env_get("IMAGE_CACHE_DIR")
            .or_else(|| non_empty(file.image_cache_dir))
            .unwrap_or_else(|| DEFAULT_IMAGE_CACHE_DIR.to_owned()),
    );

    let budgets = budgets(env, file.budget, &mut err);

    // Reconciliation (T03)
    let primary_source = primary_source(
        env_get("PRIMARY_SOURCE").or_else(|| non_empty(file.primary_source)),
        &enabled_sources,
        &mut err,
    );
    let precedence = precedence(file.precedence, &enabled_sources, &mut err);
    let sync = sync_config(env, file.sync, &mut err);
    let backup = backup_config(
        env,
        BackupFile {
            dir: file.backup_dir,
            extra_dir: file.backup_extra_dir,
            time: file.backup_time,
            keep_daily: file.backup_keep_daily.map(StringOrInt::into_string),
            keep_weekly: file.backup_keep_weekly.map(StringOrInt::into_string),
            keep_special: file.backup_keep_special.map(StringOrInt::into_string),
        },
        database_url.as_deref(),
        &mut err,
    );

    if !err.is_empty() {
        return Err(err);
    }

    // All `None`s above recorded an error, so these are present here.
    let (Some(client_id), Some(client_secret), Some(bot_token), Some(database_url)) =
        (client_id, client_secret, bot_token, database_url)
    else {
        unreachable!("missing required values are reported above");
    };

    Ok(Config {
        discord: DiscordConfig {
            client_id,
            client_secret: Secret(client_secret),
            bot_token: Secret(bot_token),
            guild_id,
        },
        database_url,
        pandascore_token,
        enabled_sources,
        log_format,
        log_filter,
        bind_addr,
        static_dir,
        image_cache_dir,
        budgets,
        primary_source,
        precedence,
        sync,
        backup,
    })
}

/// The primary source: as configured, else the first enabled source that is
/// not `manual`. It must be enabled and must not be `manual`.
fn primary_source(
    configured: Option<String>,
    enabled: &[SourceId],
    err: &mut ConfigError,
) -> Option<SourceId> {
    let Some(name) = configured else {
        return enabled.iter().copied().find(|s| *s != SourceId::Manual);
    };
    match SourceId::parse(&name) {
        None => err.invalid.push(format!(
            "PRIMARY_SOURCE: unknown source `{name}` (known: {})",
            known_sources()
        )),
        Some(SourceId::Manual) => err
            .invalid
            .push("PRIMARY_SOURCE: `manual` cannot be the primary source".to_owned()),
        Some(id) if !enabled.contains(&id) => err.invalid.push(format!(
            "PRIMARY_SOURCE: `{}` is not in ENABLED_SOURCES",
            id.as_str()
        )),
        Some(id) => return Some(id),
    }
    None
}

/// Builds the field precedence from `[precedence]`. Listed sources that are
/// not enabled are allowed (and ignored), so one config file works for dev
/// and production.
fn precedence(
    file: BTreeMap<String, Vec<String>>,
    enabled: &[SourceId],
    err: &mut ConfigError,
) -> sync::Precedence {
    let enabled: Vec<String> = enabled.iter().map(|s| s.as_str().to_owned()).collect();
    let mut groups = BTreeMap::new();
    for (key, list) in file {
        let Some(group) = sync::FieldGroup::parse(&key) else {
            err.invalid.push(format!(
                "config file: [precedence] `{key}`: unknown field group (known: {})",
                sync::FieldGroup::ALL.map(|g| g.as_str()).join(", ")
            ));
            continue;
        };
        let mut sources = Vec::new();
        for name in &list {
            match SourceId::parse(name) {
                Some(id) => sources.push(id.as_str().to_owned()),
                None => err.invalid.push(format!(
                    "config file: [precedence] {key}: unknown source `{name}` (known: {})",
                    known_sources()
                )),
            }
        }
        groups.insert(group, sources);
    }
    sync::Precedence::new(&enabled, &groups).unwrap_or_else(|e| {
        err.invalid.push(format!("config file: {e}"));
        sync::Precedence::from_enabled(&enabled)
    })
}

fn parse_file(path: &Path, text: &str) -> Result<FileConfig, ConfigError> {
    let table: toml::Table = toml::from_str(text)
        .map_err(|e| ConfigError::invalid(format!("config file {}: {e}", path.display())))?;

    let secrets: Vec<String> = SECRET_KEYS
        .iter()
        .filter(|k| table.contains_key(&k.to_ascii_lowercase()))
        .map(|k| {
            format!(
                "config file {}: `{}` is a secret; set the environment variable {k} instead",
                path.display(),
                k.to_ascii_lowercase()
            )
        })
        .collect();
    if !secrets.is_empty() {
        return Err(ConfigError {
            missing: Vec::new(),
            invalid: secrets,
        });
    }

    FileConfig::deserialize(toml::Value::Table(table))
        .map_err(|e| ConfigError::invalid(format!("config file {}: {e}", path.display())))
}

/// Default budgets: 50% of each plan (T06). PandaScore's free plan allows
/// 1,000 requests/hour and 60/minute.
fn default_budgets() -> BTreeMap<SourceId, Budget> {
    BTreeMap::from([(
        SourceId::Pandascore,
        Budget {
            per_hour: 500,
            per_minute: 30,
        },
    )])
}

/// Merges budgets: defaults, then `[budget.<source>]` in the file, then
/// `BUDGET__<SOURCE>__PER_HOUR` / `BUDGET__<SOURCE>__PER_MINUTE` from env.
fn budgets(
    env: &HashMap<String, String>,
    file: BTreeMap<String, FileBudget>,
    err: &mut ConfigError,
) -> BTreeMap<SourceId, Budget> {
    let mut budgets = default_budgets();

    for (name, fb) in file {
        let Some(id) = SourceId::parse(&name) else {
            err.invalid.push(format!(
                "config file: [budget.{name}]: unknown source (known: {})",
                known_sources()
            ));
            continue;
        };
        let entry = budgets.entry(id).or_insert(Budget {
            per_hour: 0,
            per_minute: 0,
        });
        if let Some(v) = fb.per_hour {
            entry.per_hour = v;
        }
        if let Some(v) = fb.per_minute {
            entry.per_minute = v;
        }
    }

    let mut env_keys: Vec<&String> = env.keys().filter(|k| k.starts_with("BUDGET__")).collect();
    env_keys.sort();
    for key in env_keys {
        let value = env[key].trim();
        if value.is_empty() {
            continue;
        }
        let parts: Vec<&str> = key["BUDGET__".len()..].split("__").collect();
        let (source, field) = match parts.as_slice() {
            [s, f] => (*s, *f),
            _ => {
                err.invalid.push(format!(
                    "{key}: expected BUDGET__<SOURCE>__PER_HOUR or BUDGET__<SOURCE>__PER_MINUTE"
                ));
                continue;
            }
        };
        let Some(id) = SourceId::parse(source) else {
            err.invalid.push(format!(
                "{key}: unknown source `{}` (known: {})",
                source.to_ascii_lowercase(),
                known_sources()
            ));
            continue;
        };
        let Ok(n) = value.parse::<u32>() else {
            err.invalid.push(format!(
                "{key}: expected a non-negative integer, got `{value}`"
            ));
            continue;
        };
        let entry = budgets.entry(id).or_insert(Budget {
            per_hour: 0,
            per_minute: 0,
        });
        match field.to_ascii_uppercase().as_str() {
            "PER_HOUR" => entry.per_hour = n,
            "PER_MINUTE" => entry.per_minute = n,
            _ => err.invalid.push(format!(
                "{key}: expected BUDGET__<SOURCE>__PER_HOUR or BUDGET__<SOURCE>__PER_MINUTE"
            )),
        }
    }

    for (id, b) in &budgets {
        if b.per_hour == 0 || b.per_minute == 0 {
            err.invalid.push(format!(
                "budget for {}: per_hour and per_minute must both be > 0",
                id.as_str()
            ));
        } else if b.per_minute > b.per_hour {
            err.invalid.push(format!(
                "budget for {}: per_minute ({}) exceeds per_hour ({})",
                id.as_str(),
                b.per_minute,
                b.per_hour
            ));
        }
    }
    budgets
}

/// The `[sync]` keys (lowercase) and `SYNC__<KEY>` variables (uppercase).
const SYNC_KEYS: [&str; 11] = [
    "structure_every",
    "upcoming_window",
    "upcoming_every",
    "imminent_window",
    "imminent_every",
    "overdue_window",
    "live_every",
    "corrections_after",
    "search_cache",
    "lock_every",
    "settle_every",
];

/// Parses a duration such as `30s`, `5m`, `6h` or `2d`.
pub fn parse_duration(s: &str) -> Option<chrono::Duration> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    let (n, unit) = s.split_at(split);
    let n: i64 = n.parse().ok()?;
    match unit.trim() {
        "s" => chrono::Duration::try_seconds(n),
        "m" => chrono::Duration::try_minutes(n),
        "h" => chrono::Duration::try_hours(n),
        "d" => chrono::Duration::try_days(n),
        _ => None,
    }
}

/// Merges the sync cadences: defaults, then `[sync]` in the file, then
/// `SYNC__<KEY>` from env. `corrections_after` is a list (comma-separated
/// in env).
fn sync_config(
    env: &HashMap<String, String>,
    file: BTreeMap<String, StringOrList>,
    err: &mut ConfigError,
) -> sync::SyncConfig {
    let mut values: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (key, v) in file {
        values.insert(
            key.to_ascii_lowercase(),
            (format!("config file: [sync] {key}"), v.into_string()),
        );
    }
    let mut env_keys: Vec<&String> = env.keys().filter(|k| k.starts_with("SYNC__")).collect();
    env_keys.sort();
    for key in env_keys {
        let value = env[key].trim();
        if !value.is_empty() {
            values.insert(
                key["SYNC__".len()..].to_ascii_lowercase(),
                (key.clone(), value.to_owned()),
            );
        }
    }

    let mut c = sync::SyncConfig::default();
    for (key, (origin, raw)) in values {
        let parse_one = |raw: &str, err: &mut ConfigError| {
            let d = parse_duration(raw);
            if d.is_none() {
                err.invalid.push(format!(
                    "{origin}: expected a duration such as 30s, 5m, 6h or 2d, got `{raw}`"
                ));
            }
            d
        };
        let slot = match key.as_str() {
            "structure_every" => &mut c.structure_every,
            "upcoming_window" => &mut c.upcoming_window,
            "upcoming_every" => &mut c.upcoming_every,
            "imminent_window" => &mut c.imminent_window,
            "imminent_every" => &mut c.imminent_every,
            "overdue_window" => &mut c.overdue_window,
            "live_every" => &mut c.live_every,
            "search_cache" => &mut c.search_cache,
            "lock_every" => &mut c.lock_every,
            "settle_every" => &mut c.settle_every,
            "corrections_after" => {
                let parsed: Vec<_> = raw
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| parse_one(s, err))
                    .collect();
                if let Some(list) = parsed.into_iter().collect::<Option<Vec<_>>>() {
                    c.corrections_after = list;
                }
                continue;
            }
            _ => {
                err.invalid.push(format!(
                    "{origin}: unknown sync setting (known: {})",
                    SYNC_KEYS.join(", ")
                ));
                continue;
            }
        };
        if let Some(d) = parse_one(&raw, err) {
            *slot = d;
        }
    }
    for problem in c.validate() {
        err.invalid.push(format!("sync settings: {problem}"));
    }
    c
}

/// Backup settings from the config file (keys `backup_*`).
struct BackupFile {
    dir: Option<String>,
    extra_dir: Option<String>,
    time: Option<String>,
    keep_daily: Option<String>,
    keep_weekly: Option<String>,
    keep_special: Option<String>,
}

/// The backup directory when `BACKUP_DIR` is unset: `backups/` next to the
/// database file (`/data/backups` in the image).
fn default_backup_dir(database_url: Option<&str>) -> PathBuf {
    database_url
        .and_then(|url| store::db_path(url).ok().flatten())
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default()
        .join("backups")
}

/// Reads `TZ` as glibc does, as far as an IANA zone is concerned: a bare
/// name (`Europe/Berlin`), optionally with a leading `:`, or a path into a
/// zoneinfo directory (`:/usr/share/zoneinfo/Europe/Berlin`). A path such as
/// `/etc/localtime` is followed when it is a symlink into zoneinfo, as on
/// most hosts. POSIX rules like `CET-1CEST` are not supported.
fn parse_tz_env(raw: &str) -> Option<chrono_tz::Tz> {
    let s = raw.trim().trim_start_matches(':');
    if let Ok(tz) = s.parse() {
        return Some(tz);
    }
    if !s.starts_with('/') {
        return None;
    }
    let path = std::fs::read_link(s)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| s.to_owned());
    path.split_once("zoneinfo/")?.1.parse().ok()
}

/// Backups (P4): `BACKUP_DIR`, `BACKUP_EXTRA_DIR`, `BACKUP_TIME` (`HH:MM`
/// or `off`), `BACKUP_KEEP_DAILY`/`_WEEKLY`/`_SPECIAL`, and the zone of
/// `BACKUP_TIME` from `TZ` (an IANA name; UTC when unset).
fn backup_config(
    env: &HashMap<String, String>,
    file: BackupFile,
    database_url: Option<&str>,
    err: &mut ConfigError,
) -> backup::BackupConfig {
    let get = |key: &str, file_value: Option<String>| {
        env.get(key)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .or_else(|| {
                file_value
                    .map(|v| v.trim().to_owned())
                    .filter(|v| !v.is_empty())
            })
    };
    let mut c = backup::BackupConfig::new(
        get("BACKUP_DIR", file.dir)
            .map(PathBuf::from)
            .unwrap_or_else(|| default_backup_dir(database_url)),
    );
    c.extra_dir = get("BACKUP_EXTRA_DIR", file.extra_dir).map(PathBuf::from);

    if let Some(raw) = get("BACKUP_TIME", file.time) {
        if raw.eq_ignore_ascii_case("off") {
            c.time = None;
        } else {
            match backup::schedule::parse_hhmm(&raw) {
                Some(t) => c.time = Some(t),
                None => err.invalid.push(format!(
                    "BACKUP_TIME: expected HH:MM such as 04:00, or `off`, got `{raw}`"
                )),
            }
        }
    }

    if let Some(raw) = get("TZ", None) {
        match parse_tz_env(&raw) {
            Some(tz) => c.tz = tz,
            None => err.invalid.push(format!(
                "TZ: expected an IANA time zone name such as Europe/Berlin, got `{raw}`"
            )),
        }
    }

    for (key, file_value, slot) in [
        (
            "BACKUP_KEEP_DAILY",
            file.keep_daily,
            &mut c.policy.keep_daily,
        ),
        (
            "BACKUP_KEEP_WEEKLY",
            file.keep_weekly,
            &mut c.policy.keep_weekly,
        ),
        (
            "BACKUP_KEEP_SPECIAL",
            file.keep_special,
            &mut c.policy.keep_special,
        ),
    ] {
        if let Some(raw) = get(key, file_value) {
            match raw.parse::<usize>() {
                // Keeping no daily backup would delete each new one at once.
                Ok(0) if key == "BACKUP_KEEP_DAILY" => {
                    err.invalid.push(format!("{key}: must be at least 1"));
                }
                Ok(n) => *slot = n,
                Err(_) => err
                    .invalid
                    .push(format!("{key}: expected a whole number, got `{raw}`")),
            }
        }
    }
    c
}

fn check_snowflake(key: &str, value: &str, err: &mut ConfigError) {
    if value.parse::<u64>().is_err() {
        err.invalid.push(format!(
            "{key}: expected a numeric Discord ID, got `{value}`"
        ));
    }
}

fn known_sources() -> String {
    SourceId::ALL
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn full_env() -> HashMap<String, String> {
        env(&[
            ("DISCORD_CLIENT_ID", "123456789012345678"),
            ("DISCORD_CLIENT_SECRET", "client-secret"),
            ("DISCORD_BOT_TOKEN", "bot-token"),
            ("DATABASE_URL", "sqlite://data/dev.db"),
            ("PANDASCORE_TOKEN", "ps-token"),
        ])
    }

    fn with_file(text: &str) -> Option<(&'static Path, &str)> {
        Some((Path::new("config.toml"), text))
    }

    #[test]
    fn full_env_loads_with_defaults() {
        let c = from_sources(&full_env(), None).unwrap();
        assert_eq!(c.discord.client_id, "123456789012345678");
        assert_eq!(c.discord.client_secret.expose(), "client-secret");
        assert_eq!(c.discord.bot_token.expose(), "bot-token");
        assert_eq!(c.discord.guild_id, None);
        assert_eq!(c.database_url, "sqlite://data/dev.db");
        assert_eq!(c.enabled_sources, vec![SourceId::Pandascore]);
        assert_eq!(c.log_format, LogFormat::Pretty);
        assert_eq!(c.log_filter, "info");
        assert_eq!(c.bind_addr, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(c.static_dir, PathBuf::from("../frontend/dist"));
        assert_eq!(c.image_cache_dir, PathBuf::from("data/img-cache"));
        assert_eq!(
            c.budgets[&SourceId::Pandascore],
            Budget {
                per_hour: 500,
                per_minute: 30
            }
        );
    }

    #[test]
    fn missing_variable_is_named() {
        let mut e = full_env();
        e.remove("DISCORD_BOT_TOKEN");
        let err = from_sources(&e, None).unwrap_err();
        assert_eq!(err.missing, vec!["DISCORD_BOT_TOKEN"]);
        assert_eq!(
            err.to_string(),
            "missing required environment variable(s): DISCORD_BOT_TOKEN"
        );
    }

    #[test]
    fn all_missing_variables_are_listed() {
        let err = from_sources(&HashMap::new(), None).unwrap_err();
        assert_eq!(
            err.missing,
            vec![
                "DISCORD_CLIENT_ID",
                "DISCORD_CLIENT_SECRET",
                "DISCORD_BOT_TOKEN",
                "DATABASE_URL",
                "PANDASCORE_TOKEN",
            ]
        );
    }

    #[test]
    fn empty_value_counts_as_missing() {
        let mut e = full_env();
        e.insert("DATABASE_URL".into(), "  ".into());
        let err = from_sources(&e, None).unwrap_err();
        assert_eq!(err.missing, vec!["DATABASE_URL"]);
    }

    #[test]
    fn pandascore_token_required_only_when_enabled() {
        let mut e = full_env();
        e.remove("PANDASCORE_TOKEN");
        assert_eq!(
            from_sources(&e, None).unwrap_err().missing,
            vec!["PANDASCORE_TOKEN"]
        );

        e.insert("ENABLED_SOURCES".into(), "fixture, manual".into());
        let c = from_sources(&e, None).unwrap();
        assert_eq!(c.enabled_sources, vec![SourceId::Fixture, SourceId::Manual]);
        assert!(c.pandascore_token.is_none());
    }

    #[test]
    fn unknown_source_is_rejected() {
        let mut e = full_env();
        e.insert("ENABLED_SOURCES".into(), "pandascore,hltv".into());
        let err = from_sources(&e, None).unwrap_err();
        assert!(err.missing.is_empty());
        assert_eq!(err.invalid.len(), 1);
        assert!(err.invalid[0].contains("ENABLED_SOURCES"));
        assert!(err.invalid[0].contains("hltv"));
    }

    #[test]
    fn invalid_values_are_rejected() {
        let mut e = full_env();
        e.insert("LOG_FORMAT".into(), "xml".into());
        e.insert("BIND_ADDR".into(), "localhost".into());
        e.insert("DISCORD_GUILD_ID".into(), "my-server".into());
        e.insert("RUST_LOG".into(), "app=notalevel".into());
        let err = from_sources(&e, None).unwrap_err();
        let all = err.invalid.join("\n");
        for key in ["LOG_FORMAT", "BIND_ADDR", "DISCORD_GUILD_ID", "RUST_LOG"] {
            assert!(all.contains(key), "{key} not reported in:\n{all}");
        }
    }

    #[test]
    fn file_values_apply_and_env_overrides_them() {
        let file = r#"
            discord_guild_id = 987654321098765432
            log_format = "json"
            bind_addr = "127.0.0.1:9000"
            enabled_sources = ["pandascore", "manual"]
            static_dir = "/srv/dist"
            image_cache_dir = "/data/img-cache"

            [budget.pandascore]
            per_hour = 400
        "#;
        let mut e = full_env();
        e.insert("BIND_ADDR".into(), "127.0.0.1:9100".into());
        e.insert("BUDGET__PANDASCORE__PER_MINUTE".into(), "20".into());

        let c = from_sources(&e, with_file(file)).unwrap();
        assert_eq!(c.discord.guild_id.as_deref(), Some("987654321098765432"));
        assert_eq!(c.log_format, LogFormat::Json);
        assert_eq!(c.bind_addr, "127.0.0.1:9100".parse().unwrap());
        assert_eq!(
            c.enabled_sources,
            vec![SourceId::Pandascore, SourceId::Manual]
        );
        assert_eq!(c.static_dir, PathBuf::from("/srv/dist"));
        assert_eq!(c.image_cache_dir, PathBuf::from("/data/img-cache"));
        e.insert("IMAGE_CACHE_DIR".into(), "/tmp/img".into());
        let c = from_sources(&e, with_file(file)).unwrap();
        assert_eq!(c.image_cache_dir, PathBuf::from("/tmp/img"), "env wins");
        assert_eq!(
            c.budgets[&SourceId::Pandascore],
            Budget {
                per_hour: 400,
                per_minute: 20
            }
        );
    }

    #[test]
    fn required_non_secret_may_come_from_file() {
        let mut e = full_env();
        e.remove("DATABASE_URL");
        let c = from_sources(&e, with_file(r#"database_url = "sqlite://x.db""#)).unwrap();
        assert_eq!(c.database_url, "sqlite://x.db");
    }

    #[test]
    fn secrets_in_file_are_rejected() {
        let file = r#"
            discord_bot_token = "oops"
            pandascore_token = "oops"
        "#;
        let err = from_sources(&full_env(), with_file(file)).unwrap_err();
        assert_eq!(err.invalid.len(), 2);
        assert!(err.invalid.iter().any(|l| l.contains("DISCORD_BOT_TOKEN")));
        assert!(err.invalid.iter().any(|l| l.contains("PANDASCORE_TOKEN")));
        assert!(!err.to_string().contains("oops"));
    }

    #[test]
    fn unknown_file_keys_and_bad_toml_are_rejected() {
        let err = from_sources(&full_env(), with_file("colour = \"blue\"")).unwrap_err();
        assert!(err.to_string().contains("colour"), "{err}");
        assert!(from_sources(&full_env(), with_file("not toml [")).is_err());
    }

    #[test]
    fn budget_validation() {
        let mut e = full_env();
        e.insert("BUDGET__PANDASCORE__PER_MINUTE".into(), "600".into());
        let err = from_sources(&e, None).unwrap_err();
        assert!(err.to_string().contains("exceeds per_hour"), "{err}");

        let mut e = full_env();
        e.insert("BUDGET__NOPE__PER_HOUR".into(), "1".into());
        e.insert("BUDGET__PANDASCORE__PER_DAY".into(), "1".into());
        e.insert("BUDGET__PANDASCORE__PER_HOUR".into(), "lots".into());
        let err = from_sources(&e, None).unwrap_err();
        assert_eq!(err.invalid.len(), 3, "{err}");
    }

    #[test]
    fn secrets_are_redacted_in_debug() {
        let c = from_sources(&full_env(), None).unwrap();
        let dbg = format!("{c:?}");
        for secret in ["client-secret", "bot-token", "ps-token"] {
            assert!(!dbg.contains(secret), "{secret} leaked in {dbg}");
        }
        assert!(dbg.contains("Secret(***)"));
    }

    #[test]
    fn precedence_defaults_to_manual_then_enabled_sources() {
        let mut e = full_env();
        e.insert("ENABLED_SOURCES".into(), "fixture,pandascore".into());
        let c = from_sources(&e, None).unwrap();
        assert_eq!(c.primary_source, Some(SourceId::Fixture));
        for g in sync::FieldGroup::ALL {
            assert_eq!(c.precedence.order(g), ["manual", "fixture", "pandascore"]);
        }
    }

    #[test]
    fn precedence_from_the_config_file() {
        let mut e = full_env();
        e.insert("ENABLED_SOURCES".into(), "pandascore,fixture".into());
        let file = "primary_source = \"fixture\"\n\
                    [precedence]\n\
                    schedule = [\"manual\", \"Fixture\"]\n\
                    map_rounds = [\"fixture\", \"pandascore\"]\n";
        let c = from_sources(&e, with_file(file)).unwrap();
        assert_eq!(c.primary_source, Some(SourceId::Fixture));
        assert_eq!(
            c.precedence.order(sync::FieldGroup::Schedule),
            ["manual", "fixture", "pandascore"]
        );
        assert_eq!(
            c.precedence.order(sync::FieldGroup::Series),
            ["manual", "pandascore", "fixture"]
        );

        // PRIMARY_SOURCE from the environment wins over the file.
        e.insert("PRIMARY_SOURCE".into(), "pandascore".into());
        let c = from_sources(&e, with_file(file)).unwrap();
        assert_eq!(c.primary_source, Some(SourceId::Pandascore));
    }

    #[test]
    fn precedence_validation() {
        let file = "[precedence]\n\
                    schedule = [\"pandascore\", \"manual\"]\n\
                    teams = [\"hltv\"]\n\
                    rounds = [\"pandascore\"]\n";
        let err = from_sources(&full_env(), with_file(file)).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("unknown field group"), "{text}");
        assert!(text.contains("unknown source `hltv`"), "{text}");
        assert!(text.contains("`manual` must come first"), "{text}");

        for (primary, needle) in [
            ("manual", "cannot be the primary"),
            ("fixture", "not in ENABLED_SOURCES"),
            ("nope", "unknown source `nope`"),
        ] {
            let mut e = full_env();
            e.insert("PRIMARY_SOURCE".into(), primary.into());
            let err = from_sources(&e, None).unwrap_err();
            assert!(err.to_string().contains(needle), "{err}");
        }

        let mut e = full_env();
        e.insert("ENABLED_SOURCES".into(), "manual".into());
        assert_eq!(from_sources(&e, None).unwrap().primary_source, None);
    }

    #[test]
    fn durations_parse_with_units() {
        use chrono::Duration;
        assert_eq!(parse_duration("30s"), Some(Duration::seconds(30)));
        assert_eq!(parse_duration(" 5m "), Some(Duration::minutes(5)));
        assert_eq!(parse_duration("6h"), Some(Duration::hours(6)));
        assert_eq!(parse_duration("2d"), Some(Duration::days(2)));
        for bad in ["", "5", "m", "5x", "-5m", "1.5h"] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
    }

    #[test]
    fn sync_cadences_default_then_file_then_env() {
        use chrono::Duration;
        let c = from_sources(&full_env(), None).unwrap();
        assert_eq!(c.sync, sync::SyncConfig::default());

        let file = "[sync]\n\
                    live_every = \"3m\"\n\
                    structure_every = \"12h\"\n\
                    corrections_after = [\"15m\", \"1h\", \"6h\"]\n";
        let mut e = full_env();
        e.insert("SYNC__LIVE_EVERY".into(), "1m".into());
        e.insert("SYNC__SEARCH_CACHE".into(), "2d".into());
        let c = from_sources(&e, with_file(file)).unwrap();
        assert_eq!(c.sync.live_every, Duration::minutes(1), "env wins");
        assert_eq!(c.sync.structure_every, Duration::hours(12));
        assert_eq!(c.sync.search_cache, Duration::days(2));
        assert_eq!(
            c.sync.corrections_after,
            [
                Duration::minutes(15),
                Duration::hours(1),
                Duration::hours(6)
            ]
        );

        e.insert("SYNC__CORRECTIONS_AFTER".into(), "5m, 30m".into());
        let c = from_sources(&e, None).unwrap();
        assert_eq!(
            c.sync.corrections_after,
            [Duration::minutes(5), Duration::minutes(30)]
        );
    }

    #[test]
    fn bad_sync_settings_are_reported() {
        let mut e = full_env();
        e.insert("SYNC__LIVE_EVERY".into(), "fast".into());
        e.insert("SYNC__NAP_TIME".into(), "5m".into());
        e.insert("SYNC__IMMINENT_WINDOW".into(), "3d".into());
        let err = from_sources(&e, with_file("[sync]\nlock_every = \"0s\"\n")).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("SYNC__LIVE_EVERY: expected a duration"),
            "{text}"
        );
        assert!(
            text.contains("SYNC__NAP_TIME: unknown sync setting"),
            "{text}"
        );
        assert!(text.contains("imminent_window must be shorter"), "{text}");
        assert!(
            text.contains("lock_every must be greater than zero"),
            "{text}"
        );
    }

    #[test]
    fn backup_defaults_sit_next_to_the_database() {
        let c = from_sources(&full_env(), None).unwrap();
        assert_eq!(c.backup, backup::BackupConfig::new("data/backups"));
        assert_eq!(c.backup.time, chrono::NaiveTime::from_hms_opt(4, 0, 0));
        assert_eq!(c.backup.tz, chrono_tz::Tz::UTC);
        assert_eq!(c.backup.policy, backup::Policy::default());

        let mut e = full_env();
        e.insert("DATABASE_URL".into(), "sqlite:///data/pickem.db".into());
        let c = from_sources(&e, None).unwrap();
        assert_eq!(c.backup.dir, PathBuf::from("/data/backups"));
    }

    #[test]
    fn backup_settings_from_env_and_file() {
        let mut e = full_env();
        e.extend(env(&[
            ("BACKUP_TIME", "03:30"),
            ("TZ", "Europe/Berlin"),
            ("BACKUP_EXTRA_DIR", "/mnt/nas"),
            ("BACKUP_KEEP_WEEKLY", "4"),
        ]));
        let file = "backup_dir = \"/srv/bk\"\nbackup_time = \"05:00\"\nbackup_keep_daily = 7\n";
        let c = from_sources(&e, with_file(file)).unwrap();
        assert_eq!(c.backup.dir, PathBuf::from("/srv/bk"));
        assert_eq!(c.backup.extra_dir, Some(PathBuf::from("/mnt/nas")));
        // The environment wins over the file.
        assert_eq!(c.backup.time, chrono::NaiveTime::from_hms_opt(3, 30, 0));
        assert_eq!(c.backup.tz, chrono_tz::Europe::Berlin);
        assert_eq!(c.backup.policy.keep_daily, 7);
        assert_eq!(c.backup.policy.keep_weekly, 4);
        assert_eq!(c.backup.policy.keep_special, 5);

        let mut e = full_env();
        e.insert("BACKUP_TIME".into(), "OFF".into());
        e.insert("TZ".into(), ":UTC".into());
        let c = from_sources(&e, None).unwrap();
        assert_eq!(c.backup.time, None);
    }

    #[cfg(unix)]
    #[test]
    fn tz_accepts_the_forms_glibc_users_set() {
        use chrono_tz::{Europe::Berlin, Tz};
        assert_eq!(parse_tz_env("Europe/Berlin"), Some(Berlin));
        assert_eq!(parse_tz_env(":Europe/Berlin"), Some(Berlin));
        assert_eq!(parse_tz_env("UTC"), Some(Tz::UTC));
        assert_eq!(
            parse_tz_env(":/usr/share/zoneinfo/Europe/Berlin"),
            Some(Berlin)
        );
        // A symlink into zoneinfo, like /etc/localtime on most hosts.
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("localtime");
        std::os::unix::fs::symlink("/usr/share/zoneinfo/Europe/Berlin", &link).unwrap();
        assert_eq!(parse_tz_env(&format!(":{}", link.display())), Some(Berlin));
        for bad in ["CET-1CEST", "Europe/Atlantis", "/nonexistent/localtime", ""] {
            assert_eq!(parse_tz_env(bad), None, "{bad}");
        }
    }

    #[test]
    fn bad_backup_settings_are_reported_together() {
        let mut e = full_env();
        e.extend(env(&[
            ("BACKUP_TIME", "4am"),
            ("TZ", "CET-1CEST"),
            ("BACKUP_KEEP_DAILY", "0"),
            ("BACKUP_KEEP_SPECIAL", "-1"),
        ]));
        let text = from_sources(&e, None).unwrap_err().to_string();
        assert!(text.contains("BACKUP_TIME: expected HH:MM"), "{text}");
        assert!(text.contains("TZ: expected an IANA time zone"), "{text}");
        assert!(
            text.contains("BACKUP_KEEP_DAILY: must be at least 1"),
            "{text}"
        );
        assert!(
            text.contains("BACKUP_KEEP_SPECIAL: expected a whole number"),
            "{text}"
        );
    }

    #[test]
    fn example_backup_keys_are_accepted() {
        // The commented backup keys in config.example.toml, uncommented.
        let text = include_str!("../../../../config.example.toml");
        let keys: String = text
            .lines()
            .filter(|l| l.starts_with("# backup_"))
            .map(|l| l.trim_start_matches("# "))
            .map(|l| l.split("  #").next().unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(keys.lines().count(), 6, "{keys}");
        let c = from_sources(&full_env(), with_file(&keys)).unwrap();
        assert_eq!(c.backup.policy, backup::Policy::default());
        assert_eq!(c.backup.time, chrono::NaiveTime::from_hms_opt(4, 0, 0));
    }

    #[test]
    fn example_sync_block_matches_the_defaults() {
        // The commented `[sync]` block in config.example.toml, uncommented.
        let text = include_str!("../../../../config.example.toml");
        let block: String = text
            .lines()
            .skip_while(|l| *l != "# [sync]")
            .take_while(|l| l.starts_with('#'))
            .map(|l| l.trim_start_matches("# "))
            .map(|l| l.split("  #").next().unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(block.starts_with("[sync]"), "{block}");
        let c = from_sources(&full_env(), with_file(&block)).unwrap();
        assert_eq!(c.sync, sync::SyncConfig::default());
    }
}
