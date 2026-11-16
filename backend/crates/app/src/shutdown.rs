//! Waiting for a shutdown signal.

/// Resolves on SIGTERM or SIGINT (Ctrl-C) and returns the signal's name.
#[cfg(unix)]
pub async fn signal() -> &'static str {
    use tokio::signal::unix::{SignalKind, signal};

    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = term.recv() => "SIGTERM",
        _ = int.recv() => "SIGINT",
    }
}

/// Resolves on Ctrl-C.
#[cfg(not(unix))]
pub async fn signal() -> &'static str {
    tokio::signal::ctrl_c()
        .await
        .expect("install Ctrl-C handler");
    "Ctrl-C"
}
