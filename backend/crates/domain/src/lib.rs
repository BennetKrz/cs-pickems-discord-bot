//! Domain types, pick rules and scoring.
//!
//! This crate is pure: no I/O, no async, no database. The canonical model
//! lives in [`model`], domain events in [`events`]; pick rules arrive in T10
//! and scoring in T11. [`clock`] is the single injected source of "now", so
//! everything that depends on time can run on a fake clock (T05).

pub mod clock;
pub mod events;
pub mod model;

pub use clock::{Clock, FakeClock, SystemClock};
pub use events::DomainEvent;
pub use model::*;
