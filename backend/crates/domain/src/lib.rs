//! Domain types, pick rules and scoring.
//!
//! This crate is pure: no I/O, no async, no database. The canonical model
//! lives in [`model`]; pick rules arrive in T10 and scoring in T11.

pub mod model;

pub use model::*;
