//! Logging setup: `RUST_LOG` filter, `LOG_FORMAT` pretty or JSON.

use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;

use crate::config::LogFormat;

/// Installs the global tracing subscriber. `filter` was validated by the
/// config loader.
pub fn init(format: LogFormat, filter: &str) {
    let filter = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match format {
        LogFormat::Pretty => builder.with_ansi(std::io::stdout().is_terminal()).init(),
        LogFormat::Json => builder.json().flatten_event(true).init(),
    }
}
