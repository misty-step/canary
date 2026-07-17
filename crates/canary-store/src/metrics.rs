//! Store-backed metrics snapshot queries.

use std::time::{Duration, Instant};

use canary_core::metrics::{HealthStateMetric, LabeledCount, MetricsSnapshot};
use rusqlite::Connection;

use crate::{Result, health};

const HEALTH_STATES: [&str; 6] = ["unknown", "up", "degraded", "down", "paused", "flapping"];
const METRICS_QUERY_BUDGET: Duration = Duration::from_millis(250);
const PROGRESS_CALLBACK_OPS: i32 = 1_000;

pub(crate) fn snapshot(connection: &Connection) -> Result<MetricsSnapshot> {
    let deadline = Instant::now() + METRICS_QUERY_BUDGET;
    connection.progress_handler(
        PROGRESS_CALLBACK_OPS,
        Some(move || Instant::now() >= deadline),
    );
    let result = snapshot_with_budget(connection);
    connection.progress_handler(0, None::<fn() -> bool>);
    result
}

fn snapshot_with_budget(connection: &Connection) -> Result<MetricsSnapshot> {
    Ok(MetricsSnapshot {
        errors_total: count_errors(connection)?,
        webhook_queue_depth: webhook_queue_depth(connection)?,
        webhook_delivery_totals: labeled_counts(
            connection,
            "SELECT status, count(*) FROM webhook_deliveries GROUP BY status ORDER BY status",
        )?,
        oban_queue_depths: labeled_counts(
            connection,
            "SELECT queue, count(*)
             FROM oban_jobs
             WHERE state IN ('available', 'scheduled', 'executing')
             GROUP BY queue
             ORDER BY queue",
        )?,
        target_states: target_state_gauges(connection)?,
        monitor_states: monitor_state_gauges(connection)?,
    })
}

fn count_errors(connection: &Connection) -> Result<u64> {
    Ok(connection.query_row("SELECT count(*) FROM errors", [], |row| row.get(0))?)
}

fn webhook_queue_depth(connection: &Connection) -> Result<u64> {
    Ok(connection.query_row(
        "SELECT count(*) FROM webhook_deliveries WHERE status IN ('pending', 'retrying')",
        [],
        |row| row.get(0),
    )?)
}

fn labeled_counts(connection: &Connection, sql: &str) -> Result<Vec<LabeledCount>> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([], |row| {
        Ok(LabeledCount {
            label: row.get(0)?,
            count: row.get(1)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn target_state_gauges(connection: &Connection) -> Result<Vec<HealthStateMetric>> {
    let targets = health::health_targets(connection)?;
    let mut gauges = Vec::with_capacity(targets.len() * HEALTH_STATES.len());
    for target in targets {
        for state in HEALTH_STATES {
            gauges.push(HealthStateMetric {
                id: target.id.clone(),
                service: target.service.clone(),
                state: state.to_owned(),
                value: u8::from(target.state == state),
            });
        }
    }
    Ok(gauges)
}

fn monitor_state_gauges(connection: &Connection) -> Result<Vec<HealthStateMetric>> {
    let monitors = health::health_monitors(connection)?;
    let mut gauges = Vec::with_capacity(monitors.len() * HEALTH_STATES.len());
    for monitor in monitors {
        for state in HEALTH_STATES {
            gauges.push(HealthStateMetric {
                id: monitor.id.clone(),
                service: monitor.service.clone(),
                state: state.to_owned(),
                value: u8::from(monitor.state == state),
            });
        }
    }
    Ok(gauges)
}
