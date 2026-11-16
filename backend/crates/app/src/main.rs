//! CS2 Pick'em: one binary running the HTTP API, the Discord bot and the sync
//! worker as tokio tasks.

use std::process::ExitCode;

use app::{config, telemetry};

#[tokio::main]
async fn main() -> ExitCode {
    // A local .env is a dev convenience; real environment variables win.
    let dotenv_error = match dotenvy::dotenv() {
        Err(e) if !e.not_found() => Some(e),
        _ => None,
    };

    let config = match config::load() {
        Ok(c) => c,
        Err(e) => {
            if let Some(dotenv_error) = &dotenv_error {
                eprintln!("error: cannot load .env: {dotenv_error}");
            }
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    telemetry::init(config.log_format, &config.log_filter);
    if let Some(e) = dotenv_error {
        tracing::warn!("cannot load .env: {e}");
    }

    match app::run(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}
