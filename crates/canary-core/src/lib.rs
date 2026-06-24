//! Core domain logic for the Rust rewrite of Canary.
//!
//! This crate intentionally owns domain invariants that should not be spread
//! across HTTP handlers or database code. Public modules are few and deep:
//! callers ask for typed outcomes, not implementation details.

pub mod health;
pub mod ids;
pub mod ingest;
pub mod metrics;
pub mod query;
pub mod secrets;
pub mod slo;
pub mod webhook_events;
