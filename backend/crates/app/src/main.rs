//! CS2 Pick'em: one binary running the HTTP API, the Discord bot and the sync
//! worker as tokio tasks, plus maintenance subcommands.

use std::net::SocketAddr;
use std::process::ExitCode;

use app::{config, healthcheck as probe, telemetry};
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
    /// Re-settle picks (T11).
    Rescore {
        /// Re-settle every match with picks using each guild's current
        /// scoring rules, then exit. Matches otherwise keep the rules that
        /// were active when they locked.
        #[arg(long, required = true)]
        all: bool,
    },
    /// Show or change a guild's scoring rules (T11).
    Rules {
        #[command(subcommand)]
        command: RulesCommand,
    },
    /// Probe `GET /healthz` of the running app; exit 0 only on 200. For the
    /// Docker HEALTHCHECK. Needs no configuration besides `BIND_ADDR`.
    Healthcheck {
        /// Address to probe. Default: `BIND_ADDR` (wildcard → loopback).
        #[arg(long)]
        addr: Option<SocketAddr>,
    },
}

#[derive(Subcommand)]
enum RulesCommand {
    /// Print the current rules (defaults filled in) and their history as
    /// JSON.
    Show {
        /// Discord guild ID.
        #[arg(long)]
        guild: String,
    },
    /// Make a rules JSON file the current rules, as a new version. Fields:
    /// winner, series, map_winner, map_score; missing ones take the
    /// defaults 3, 3, 1, 2.
    Set {
        /// Discord guild ID.
        #[arg(long)]
        guild: String,
        /// Path to the rules JSON file.
        file: std::path::PathBuf,
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

    if let Some(Command::Healthcheck { addr }) = cli.command {
        return healthcheck(addr);
    }

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
        Some(Command::Rescore { all: _ }) => app::rescore(config).await.map(|_| ()),
        Some(Command::Rules {
            command: RulesCommand::Show { guild },
        }) => app::rules_show(config, &guild).await.and_then(|v| {
            println!("{}", serde_json::to_string_pretty(&v)?);
            Ok(())
        }),
        Some(Command::Rules {
            command: RulesCommand::Set { guild, file },
        }) => app::rules_set(config, &guild, &file)
            .await
            .map(|v| println!("scoring rules version {v} saved for guild {guild}")),
        Some(Command::Healthcheck { .. }) => unreachable!("handled before loading the config"),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

/// `app healthcheck`: one line on stdout or stderr, exit 0 only when healthy.
fn healthcheck(addr: Option<SocketAddr>) -> ExitCode {
    let bind_addr = std::env::var("BIND_ADDR").ok();
    let result = probe::target(addr, bind_addr.as_deref())
        .and_then(|target| probe::check(target, probe::TIMEOUT));
    match result {
        Ok(status) => {
            println!("healthy: {status}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("unhealthy: {e}");
            ExitCode::FAILURE
        }
    }
}
