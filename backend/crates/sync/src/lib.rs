//! Sync worker: the only component that calls external data APIs.
//!
//! - [`reconcile`]: turns source DTOs into canonical data, by field
//!   precedence ([`precedence`]), and emits domain events on the [`bus`].
//! - The scheduler and budget arrive in T06. For now the worker only idles
//!   until it is told to stop, so that the shutdown sequence is in place.

pub mod bus;
pub mod precedence;
pub mod reconcile;

pub use bus::EventBus;
pub use precedence::{FieldGroup, MANUAL, Precedence, PrecedenceError};
pub use reconcile::{ReconcileError, Reconciler, Report};

use tokio_util::sync::CancellationToken;

/// Runs the sync worker until `cancel` fires. `bus` is where the reconciler
/// publishes domain events; consumers subscribe to it.
pub async fn run(bus: EventBus, cancel: CancellationToken) {
    tracing::info!("sync worker started (no jobs until T06)");
    let _bus = bus;
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
        let handle = tokio::spawn(run(EventBus::new(), cancel.clone()));
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("worker did not stop")
            .unwrap();
    }
}
