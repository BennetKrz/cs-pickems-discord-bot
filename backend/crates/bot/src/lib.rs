//! Discord bot (poise).
//!
//! Until T08 this is a stub without a gateway connection: it only waits for
//! cancellation, so the shutdown order (sync → gateway → HTTP → DB) is
//! already wired. T08 replaces the body with a poise framework and closes
//! the shard manager on cancel.

use tokio_util::sync::CancellationToken;

/// Runs the bot until `cancel` fires.
pub async fn run(cancel: CancellationToken) {
    tracing::info!("bot started (stub, no gateway connection until T08)");
    cancel.cancelled().await;
    tracing::info!("bot stopped");
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
            .expect("bot did not stop")
            .unwrap();
    }
}
