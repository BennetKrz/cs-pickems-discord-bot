//! The CS2 Pick'em application: configuration, wiring of HTTP API, Discord
//! bot and sync worker, and graceful shutdown. `main.rs` is a thin wrapper.

pub mod config;
pub mod shutdown;
pub mod telemetry;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::{Config, SourceId};

/// Upper bound for the whole shutdown sequence. Docker's default grace period
/// is 10 s (compose uses 20 s, T16); stay well inside it.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(8);

/// The reconciler as configured: precedence and primary source.
pub fn reconciler(
    config: &Config,
    pool: store::SqlitePool,
    bus: sync::EventBus,
) -> sync::Reconciler {
    sync::Reconciler::new(
        pool,
        config.precedence.clone(),
        config.primary_source.map(|s| s.as_str().to_owned()),
        bus,
    )
}

/// The sources the sync scheduler polls, with their budgets. `manual` has
/// no adapter and `fixture` is driven by the dev clock (T05), so today this
/// is PandaScore when enabled.
pub fn scheduled_sources(config: &Config) -> anyhow::Result<Vec<sync::SourceEntry>> {
    let mut out = Vec::new();
    for id in &config.enabled_sources {
        match id {
            SourceId::Pandascore => {
                let token = config
                    .pandascore_token
                    .as_ref()
                    .context("PANDASCORE_TOKEN is not set")?;
                let adapter = sources::pandascore::PandascoreAdapter::new(
                    sources::pandascore::PandascoreConfig::new(token.expose()),
                )
                .context("building the PandaScore client")?;
                out.push(sync::SourceEntry {
                    adapter: Arc::new(adapter),
                    budget: budget_of(config, *id),
                });
            }
            SourceId::Manual | SourceId::Fixture => {}
        }
    }
    Ok(out)
}

fn budget_of(config: &Config, id: SourceId) -> Option<sync::BudgetLimits> {
    config.budgets.get(&id).map(|b| sync::BudgetLimits {
        per_hour: b.per_hour,
        per_minute: b.per_minute,
    })
}

/// `app reconcile --replay`: rebuilds canonical data from `source_records`
/// without any API calls, then exits. Domain events go nowhere, because no
/// consumer runs in this process; settlement is a recompute (T11), so a
/// later run catches up.
pub async fn replay(config: Config) -> anyhow::Result<sync::Report> {
    let pool = store::open(&config.database_url)
        .await
        .context("opening the database")?;
    let report = reconciler(&config, pool.clone(), sync::EventBus::new())
        .replay(chrono::Utc::now())
        .await
        .context("replaying source records")?;
    tracing::info!(
        created = report.created,
        linked = report.linked,
        updated = report.updated,
        queued = report.queued,
        failed = report.failed,
        events = report.events,
        "replay finished"
    );
    pool.close().await;
    if report.failed > 0 {
        anyhow::bail!(
            "{} record(s) could not be reconciled; see the warnings above",
            report.failed
        );
    }
    Ok(report)
}

/// Runs all components until SIGTERM/SIGINT, then stops them in order: sync
/// worker, bot gateway, HTTP (draining in-flight requests), database.
pub async fn run(config: Config) -> anyhow::Result<()> {
    tracing::info!(
        version = api::VERSION,
        git_sha = api::GIT_SHA,
        sources = ?config.enabled_sources.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        "starting"
    );

    let fixture = config.enabled_sources.contains(&SourceId::Fixture);
    if fixture && !cfg!(feature = "dev") {
        anyhow::bail!(
            "the fixture source exists only in dev builds: run `cargo run --features dev`, \
             or remove `fixture` from ENABLED_SOURCES"
        );
    }

    let pool = store::open(&config.database_url)
        .await
        .context("opening the database")?;

    // Domain events from the reconciler; T08, T10 and T11 subscribe here.
    let bus = sync::EventBus::new();
    let reconciler = Arc::new(reconciler(&config, pool.clone(), bus.clone()));
    #[cfg_attr(not(feature = "dev"), allow(unused_mut))]
    let mut state = api::AppState::new(reconciler.clone(), Arc::new(domain::SystemClock));
    #[cfg_attr(not(feature = "dev"), allow(unused_mut))]
    let mut extra_api = axum::Router::new();
    #[cfg(feature = "dev")]
    {
        tracing::warn!("dev build: admin requests are trusted from the X-Dev-User header");
        state = state.with_admin_auth(Arc::new(api::admin::DevHeaderAuth));
        if fixture {
            let driver = Arc::new(
                api::dev::FixtureDriver::open(
                    pool.clone(),
                    reconciler.clone(),
                    api::dev::Scenario::default_scenario(),
                    chrono::Utc::now(),
                )
                .await
                .context("opening the fixture scenario")?,
            );
            let report = driver
                .sync()
                .await
                .context("loading the fixture scenario")?;
            tracing::info!(
                anchor = %driver.anchor(),
                now = %domain::Clock::now(driver.clock().as_ref()),
                created = report.created,
                failed = report.failed,
                "fixture scenario loaded; advance with POST /api/dev/clock/advance"
            );
            state.clock = driver.clock();
            extra_api = api::dev::routes(driver);
        }
    }

    // Sync scheduler (T06) over the sources that make external calls. The
    // fixture source is driven by the dev clock instead (T05).
    let (scheduler, sync_handle) = sync::Scheduler::open(sync::SchedulerParts {
        reconciler: reconciler.clone(),
        clock: Arc::new(domain::SystemClock),
        config: config.sync.clone(),
        sources: scheduled_sources(&config)?,
        seed: None,
    })
    .await
    .context("starting the sync scheduler")?;
    state = state.with_sync(sync_handle);
    // The lock ticker follows the app clock, so in fixture mode it locks
    // matches as the dev clock moves.
    let lock_clock = state.clock.clone();
    let lock_every = config
        .sync
        .lock_every
        .to_std()
        .context("SYNC__LOCK_EVERY")?;

    let listener = TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("binding {}", config.bind_addr))?;
    tracing::info!(addr = %listener.local_addr()?, "HTTP listening");

    // One token per component, so shutdown can stop them in a fixed order.
    let sync_stop = CancellationToken::new();
    let bot_stop = CancellationToken::new();
    let http_stop = CancellationToken::new();

    let sync_task = {
        let stop = sync_stop.clone();
        let pool = pool.clone();
        tokio::spawn(async move {
            tokio::join!(
                scheduler.run(stop.clone()),
                sync::run_lock_ticker(pool, lock_clock, lock_every, stop),
            );
        })
    };
    let bot_task = tokio::spawn(bot::run(bot_stop.clone()));
    let app = api::router_with(state, &config.static_dir, extra_api);
    let mut http_task = tokio::spawn(api::serve(listener, app, http_stop.clone()));

    let mut http_finished = false;
    let outcome: anyhow::Result<()> = tokio::select! {
        name = shutdown::signal() => {
            tracing::info!(signal = name, "shutting down");
            Ok(())
        }
        res = &mut http_task => {
            http_finished = true;
            Err(match res {
                Ok(Ok(())) => anyhow::anyhow!("HTTP server stopped unexpectedly"),
                Ok(Err(e)) => anyhow::Error::new(e).context("HTTP server failed"),
                Err(e) => anyhow::Error::new(e).context("HTTP server task panicked"),
            })
        }
    };

    let sequence = async {
        sync_stop.cancel();
        if let Err(e) = sync_task.await {
            tracing::warn!("sync worker task ended abnormally: {e}");
        }
        bot_stop.cancel();
        if let Err(e) = bot_task.await {
            tracing::warn!("bot task ended abnormally: {e}");
        }
        http_stop.cancel();
        if !http_finished {
            match http_task.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!("HTTP server ended with an error: {e}"),
                Err(e) => tracing::warn!("HTTP task ended abnormally: {e}"),
            }
        }
        pool.close().await;
    };

    match tokio::time::timeout(SHUTDOWN_TIMEOUT, sequence).await {
        Ok(()) => tracing::info!("shutdown complete"),
        Err(_) => {
            return Err(outcome.err().unwrap_or_else(|| {
                anyhow::anyhow!("shutdown did not finish within {SHUTDOWN_TIMEOUT:?}")
            }));
        }
    }
    outcome
}
