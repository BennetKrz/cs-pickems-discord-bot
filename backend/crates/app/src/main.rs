//! CS2 Pick'em: one binary running the HTTP API, the Discord bot and the sync
//! worker as tokio tasks, plus maintenance subcommands.

use std::process::ExitCode;

use app::{config, telemetry};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "app",
    version,
    about = "CS2 Pick'em: Discord bot, HTTP API and sync worker"
)]
struct Cli {
    /// Without a subcommand, the app runs all components until SIGTERM.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Reconcile canonical data from stored source data.
    Reconcile {
        /// Rebuild canonical data from `source_records` without any API
        /// calls, then exit.
        #[arg(long, required = true)]
        replay: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

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

    let result = match cli.command {
        None => app::run(config).await,
        Some(Command::Reconcile { replay: _ }) => app::replay(config).await.map(|_| ()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}
