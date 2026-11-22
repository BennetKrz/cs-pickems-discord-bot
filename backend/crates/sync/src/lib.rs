//! Sync worker: the only component that calls external data APIs.
//!
//! - [`scheduler`]: decides which external calls to make, by match state and
//!   within each source's [`budget`], and keeps followed events fresh (T06).
//! - [`lock`]: the lock ticker that locks picks once a match starts (T06).
//! - [`follow`]: following and unfollowing events per guild (T09).
//! - [`handle`]: how other components trigger a sync, search events and
//!   read the counters.
//! - [`reconcile`]: turns source DTOs into canonical data, by field
//!   precedence ([`precedence`]), and emits domain events on the [`bus`].
//! - [`manual`]: admin overrides (the `manual` source), void and reopen
//!   (T05).
//! - [`settle`]: settlement, which recomputes `pick_results` from match
//!   results on domain events and by periodic sweep (T11).

pub mod budget;
pub mod bus;
pub mod config;
pub mod follow;
pub mod handle;
pub mod lock;
pub mod manual;
pub mod precedence;
pub mod reconcile;
pub mod scheduler;
pub mod settle;

pub use budget::Priority;
pub use bus::EventBus;
pub use config::{BudgetLimits, SyncConfig};
pub use follow::{FollowError, FollowOutcome, UnfollowOutcome};
pub use handle::{JobStatus, SearchHit, SearchOutcome, SourceStatus, SyncHandle, SyncStatus};
pub use lock::run_lock_ticker;
pub use manual::{ManualError, ManualService, Outcome, Override, OverrideField};
pub use precedence::{FieldGroup, MANUAL, Precedence, PrecedenceError};
pub use reconcile::{ReconcileError, Reconciler, Report};
pub use scheduler::{Scheduler, SchedulerParts, SourceEntry, SyncError, TickReport};
pub use settle::{RulesMode, SettleError, SettleReport, run_settler};
