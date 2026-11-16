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
    /// Request budget per source. Sources without an entry are unbudgeted
    /// (they make no external calls).
    pub budgets: BTreeMap<SourceId, Budget>,
    /// The only source that creates canonical entities (T03). `None` if
    /// only `manual` is enabled.
    pub primary_source: Option<SourceId>,
    /// Field precedence per field group, from `[precedence]` (T03).
    pub precedence: sync::Precedence,
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
    primary_source: Option<String>,
    #[serde(default)]
    budget: BTreeMap<String, FileBudget>,
    /// Field group → sources, highest precedence first.
    #[serde(default)]
    precedence: BTreeMap<String, Vec<String>>,
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

    let budgets = budgets(env, file.budget, &mut err);

    // Reconciliation (T03)
    let primary_source = primary_source(
        env_get("PRIMARY_SOURCE").or_else(|| non_empty(file.primary_source)),
        &enabled_sources,
        &mut err,
    );
    let precedence = precedence(file.precedence, &enabled_sources, &mut err);

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
        budgets,
        primary_source,
        precedence,
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
}
