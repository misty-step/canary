//! Background worker contracts for the Rust rewrite.
//!
//! This crate owns worker decisions that are independent of Axum and SQLite.
//! Persistence lives in `canary-store`; outbound webhook wire headers live in
//! `canary-http`.

pub mod health;
pub mod retention;
pub mod webhooks;
