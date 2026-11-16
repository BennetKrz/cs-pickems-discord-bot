//! Sync worker: the only component that calls external data APIs.
//!
//! The scheduler, budget and reconciler arrive in T03 and T06. For now the
//! worker only idles until it is told to stop, so that the shutdown sequence
//! is already in place.

use tokio_util::sync::CancellationToken;

/// Runs the sync worker until `cancel` fires.
pub async fn run(cancel: CancellationToken) {
    tracing::info!("sync worker started (no jobs until T06)");
    cancel.cancelled().await;
    tracing::info!("sync worker stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn stops_on_cancel() {
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(run(cancel.clone()));
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("worker did not stop")
            .unwrap();
    }
}
