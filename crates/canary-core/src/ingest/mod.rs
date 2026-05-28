//! Error ingest domain logic.
//!
//! The HTTP/database layers should not own grouping, normalization, or
//! classification policy. Keeping those decisions here makes them testable and
//! gives agents one narrow place to update deterministic ingest behavior.

pub mod classification;
pub mod grouping;
