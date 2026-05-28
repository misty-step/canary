//! HTTP-facing contracts for the Rust rewrite of Canary.
//!
//! This crate owns wire shapes shared by handlers, tests, and generated API
//! documentation. The actual router should stay thin and delegate domain work
//! downward into `canary-core` and persistence modules.

pub mod auth;
pub mod problem_details;
