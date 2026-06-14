//! Durable fixed-window rate-limit buckets.
//!
//! The server may keep a process-local guard for cheap rejection, but this table
//! is the cross-process authority for hosted deployments that share SQLite.

use rusqlite::{Connection, OptionalExtension, params};

use crate::Result;

/// Outcome of one durable rate-limit check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableRateLimitDecision {
    /// Request may continue.
    Allowed,
    /// Request exceeded its durable bucket.
    Limited {
        /// Whole-second retry delay.
        retry_after_seconds: u64,
    },
}

/// Consume one request from a durable fixed-window bucket.
pub(crate) fn check(
    connection: &mut Connection,
    kind: &str,
    identity: &str,
    limit: u32,
    window_ms: u64,
    now_ms: i64,
) -> Result<DurableRateLimitDecision> {
    if limit == 0 || window_ms == 0 {
        return Ok(DurableRateLimitDecision::Limited {
            retry_after_seconds: 1,
        });
    }

    let transaction = connection.transaction()?;
    let existing = bucket(&transaction, kind, identity)?;

    let decision = match existing {
        Some(bucket) if inside_window(bucket.window_start_ms, window_ms, now_ms) => {
            if bucket.count >= limit {
                DurableRateLimitDecision::Limited {
                    retry_after_seconds: retry_after_seconds(
                        bucket.window_start_ms,
                        window_ms,
                        now_ms,
                    ),
                }
            } else {
                transaction.execute(
                    "UPDATE rate_limit_buckets
                     SET count = count + 1, updated_at_ms = ?4
                     WHERE kind = ?1 AND identity = ?2 AND window_start_ms = ?3",
                    params![kind, identity, bucket.window_start_ms, now_ms],
                )?;
                DurableRateLimitDecision::Allowed
            }
        }
        _ => {
            transaction.execute(
                "INSERT INTO rate_limit_buckets (
                    kind, identity, window_start_ms, window_ms, count, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, 1, ?3)
                 ON CONFLICT(kind, identity) DO UPDATE SET
                    window_start_ms = excluded.window_start_ms,
                    window_ms = excluded.window_ms,
                    count = excluded.count,
                    updated_at_ms = excluded.updated_at_ms",
                params![kind, identity, now_ms, window_ms],
            )?;
            DurableRateLimitDecision::Allowed
        }
    };

    transaction.commit()?;
    Ok(decision)
}

fn bucket(connection: &Connection, kind: &str, identity: &str) -> rusqlite::Result<Option<Bucket>> {
    connection
        .query_row(
            "SELECT window_start_ms, count
             FROM rate_limit_buckets
             WHERE kind = ?1 AND identity = ?2",
            params![kind, identity],
            |row| {
                Ok(Bucket {
                    window_start_ms: row.get(0)?,
                    count: row.get(1)?,
                })
            },
        )
        .optional()
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    window_start_ms: i64,
    count: u32,
}

fn inside_window(window_start_ms: i64, window_ms: u64, now_ms: i64) -> bool {
    now_ms.saturating_sub(window_start_ms) < window_ms as i64
}

fn retry_after_seconds(window_start_ms: i64, window_ms: u64, now_ms: i64) -> u64 {
    let elapsed_ms = now_ms.saturating_sub(window_start_ms);
    let remaining_ms = (window_ms as i64).saturating_sub(elapsed_ms).max(0) as u64;
    remaining_ms / 1_000 + 1
}
