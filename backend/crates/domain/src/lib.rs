//! Domain types, pick rules and scoring.
//!
//! This crate is pure: no I/O, no async, no database. The canonical model
//! lives in [`model`], domain events in [`events`], pick rules in [`picks`]
//! (T10), scoring in [`scoring`] (T11). [`clock`] is the single injected source of
//! "now", so everything that depends on time can run on a fake clock (T05).
//! [`admin`] decides who counts as a guild admin (T07). [`leaderboard`]
//! ranks members (T14); [`localtime`] finds local calendar days (T08, T14).
//! [`bracket`] lays out stages for the event view (T13).

pub mod admin;
pub mod bracket;
pub mod clock;
pub mod events;
pub mod leaderboard;
pub mod localtime;
pub mod model;
pub mod picks;
pub mod scoring;

pub use clock::{Clock, FakeClock, SystemClock};
pub use events::DomainEvent;
pub use model::*;
