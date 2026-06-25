//! Windowed service SLI read models.
//!
//! These projections derive service health from persisted observations only.
//! They attach deterministic default SLO class metadata, but do not own budget
//! burn, alert routing, or health-state transitions.

use std::collections::BTreeMap;

use canary_core::{
    query::QueryWindow,
    slo::{ServiceSloObjective, default_service_slo},
};
use rusqlite::{Connection, params_from_iter};
use time::OffsetDateTime;

use crate::query::{QueryError, QueryResult};

/// Per-service SLI projection returned by the unified report.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceSliSummary {
    /// Service name.
    pub service: String,
    /// Query window used to calculate the windowed fields.
    pub window: String,
    /// Default SLO class and objective metadata for this service.
    pub slo: ServiceSloObjective,
    /// HTTP-target availability and latency signals.
    pub targets: TargetSliSummary,
    /// Non-HTTP monitor availability signals.
    pub monitors: MonitorSliSummary,
    /// Error ingest signals.
    pub errors: ErrorSliSummary,
    /// Incident pressure signals.
    pub incidents: IncidentSliSummary,
    /// Direction of travel vs the prior equal-length window, when computed.
    ///
    /// `None` on read models that do not compute trajectory (e.g. the
    /// `service_sli_at` test helper); `Some` on the report path.
    pub trajectory: Option<ServiceSliTrajectory>,
}

/// Minimum check / check-in count, in both the current and prior window, before
/// an availability delta is trusted. Below this floor a single failed probe
/// would dominate the ratio, so the delta is nulled and the trajectory is
/// marked `InsufficientSamples`.
///
/// This is a deliberately coarse, fixed first-pass floor. A cadence-aware floor
/// (scaled to each target's probe interval) is a tracked follow-up on ticket
/// 047, not part of this slice.
pub const MIN_TRAJECTORY_SAMPLES: u64 = 20;

/// Whether a service's availability trajectory is backed by enough samples to
/// trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrajectoryStatus {
    /// At least one availability delta cleared the sample floor, or the service
    /// has no health surface to distrust (error-only services).
    Ok,
    /// The service has a configured health surface but every availability
    /// window was below `MIN_TRAJECTORY_SAMPLES`, so availability deltas are
    /// null.
    InsufficientSamples,
}

impl TrajectoryStatus {
    /// Wire value for this status.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::InsufficientSamples => "insufficient_samples",
        }
    }
}

/// Signed change in a service's SLIs vs the immediately preceding equal-length
/// window (this window minus the prior window).
///
/// A delta is evidence — a fact about two adjacent windows — never a burn-rate
/// budget claim or an urgency verdict. Consumers decide what is urgent.
/// Availability deltas are nulled below the sample floor (see
/// [`MIN_TRAJECTORY_SAMPLES`]); the error-count delta is always exact.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServiceSliTrajectory {
    /// HTTP-target availability ratio change (current − prior). `None` when
    /// either window is below the sample floor or has no target checks.
    pub targets_availability_delta: Option<f64>,
    /// Prior-window HTTP-target availability ratio, when computable.
    pub prior_targets_availability_ratio: Option<f64>,
    /// Non-HTTP monitor availability ratio change (current − prior). `None`
    /// when either window is below the sample floor or has no check-ins.
    pub monitors_availability_delta: Option<f64>,
    /// Prior-window monitor availability ratio, when computable.
    pub prior_monitors_availability_ratio: Option<f64>,
    /// Error-row count change vs the prior window (signed; current − prior).
    pub error_total_delta: i64,
    /// Prior-window error-row count.
    pub prior_error_total: u64,
    /// Smallest current/prior count backing a computed availability delta; 0
    /// when no availability delta cleared the floor.
    pub sample_basis: u64,
    /// Whether the availability deltas are trustworthy.
    pub status: TrajectoryStatus,
}

/// HTTP-target SLI aggregate for one service.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TargetSliSummary {
    /// Configured target count for this service.
    pub configured: u64,
    /// Target checks observed in the query window.
    pub checks: u64,
    /// Target checks whose result was `success`.
    pub successful_checks: u64,
    /// Target checks whose result was not `success`.
    pub failed_checks: u64,
    /// Successful checks divided by total checks, when checks exist.
    pub availability_ratio: Option<f64>,
    /// Average target latency in milliseconds, when any check reported latency.
    pub latency_ms_average: Option<f64>,
}

/// Non-HTTP monitor SLI aggregate for one service.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MonitorSliSummary {
    /// Configured monitor count for this service.
    pub configured: u64,
    /// Monitor check-ins observed in the query window.
    pub check_ins: u64,
    /// Check-ins that map to an up health state.
    pub healthy_check_ins: u64,
    /// Check-ins that reported `error`.
    pub failed_check_ins: u64,
    /// Healthy check-ins divided by total check-ins, when check-ins exist.
    pub availability_ratio: Option<f64>,
}

/// Error ingest SLI aggregate for one service.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErrorSliSummary {
    /// Error rows observed in the query window.
    pub total: u64,
    /// Distinct error groups observed in the query window.
    pub groups: u64,
}

/// Incident SLI aggregate for one service.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IncidentSliSummary {
    /// Incidents opened in the query window.
    pub opened: u64,
    /// Incidents resolved in the query window.
    pub resolved: u64,
    /// Current unresolved incidents for this service.
    pub active: u64,
}

pub(crate) fn service_sli_scoped(
    connection: &Connection,
    window: &str,
    tenant_id: &str,
    project_id: &str,
) -> QueryResult<Vec<ServiceSliSummary>> {
    service_sli_with_trajectory_at_scoped(
        connection,
        window,
        OffsetDateTime::now_utc(),
        Some((tenant_id, project_id)),
    )
}

pub(crate) fn service_sli_at(
    connection: &Connection,
    window: &str,
    now: OffsetDateTime,
) -> QueryResult<Vec<ServiceSliSummary>> {
    service_sli_at_scoped(connection, window, now, None)
}

pub(crate) fn service_sli_with_trajectory_at(
    connection: &Connection,
    window: &str,
    now: OffsetDateTime,
) -> QueryResult<Vec<ServiceSliSummary>> {
    service_sli_with_trajectory_at_scoped(connection, window, now, None)
}

/// Compute current-window SLIs and attach a trajectory delta vs the prior
/// equal-length window.
///
/// The base read model filters `>= cutoff` with no upper bound, so we derive
/// the prior window `[now-2d, now-d)` by subtracting current-window additive
/// counts from a double-width window `[now-2d, ∞)`. Because the current window
/// is a subset of the double-width window, the subtraction is exactly the prior
/// window and cancels any future-dated rows the skew policy permits.
fn service_sli_with_trajectory_at_scoped(
    connection: &Connection,
    window: &str,
    now: OffsetDateTime,
    owner: Option<(&str, &str)>,
) -> QueryResult<Vec<ServiceSliSummary>> {
    let parsed = QueryWindow::parse(window).ok_or(QueryError::InvalidWindow)?;
    let mut current = service_sli_at_scoped(connection, window, now, owner)?;

    let prior_anchor = now - time::Duration::seconds(parsed.duration_seconds());
    let double_width = service_sli_at_scoped(connection, window, prior_anchor, owner)?;
    let double_by_service: BTreeMap<&str, &ServiceSliSummary> = double_width
        .iter()
        .map(|summary| (summary.service.as_str(), summary))
        .collect();

    for summary in &mut current {
        let wide = double_by_service.get(summary.service.as_str()).copied();
        summary.trajectory = Some(build_trajectory(summary, wide));
    }

    Ok(current)
}

/// Build a [`ServiceSliTrajectory`] from a current-window summary and the
/// matching double-width (`[now-2d, ∞)`) summary. Prior-window counts are
/// `double_width − current`.
///
/// INVARIANT: only ADDITIVE, cutoff-bounded fields may be subtracted this way —
/// target/monitor check counts and `errors.total`. Do NOT extend this to
/// non-additive aggregates: `errors.groups` is `COUNT(DISTINCT ...)`,
/// `latency_ms_average` is an `AVG`, and `incidents.active` is a point-in-time
/// gauge whose query has an unbounded `OR state != 'resolved'` branch. For any
/// of those, `double_width − current` is silently wrong.
fn build_trajectory(
    current: &ServiceSliSummary,
    double_width: Option<&ServiceSliSummary>,
) -> ServiceSliTrajectory {
    let (targets_availability_delta, prior_targets_availability_ratio, targets_basis) =
        availability_delta(
            current.targets.successful_checks,
            current.targets.checks,
            double_width.map_or(0, |wide| wide.targets.successful_checks),
            double_width.map_or(0, |wide| wide.targets.checks),
        );
    let (monitors_availability_delta, prior_monitors_availability_ratio, monitors_basis) =
        availability_delta(
            current.monitors.healthy_check_ins,
            current.monitors.check_ins,
            double_width.map_or(0, |wide| wide.monitors.healthy_check_ins),
            double_width.map_or(0, |wide| wide.monitors.check_ins),
        );

    let prior_error_total = double_width
        .map_or(0, |wide| wide.errors.total)
        .saturating_sub(current.errors.total);
    let error_total_delta = current.errors.total as i64 - prior_error_total as i64;

    let sample_basis = [targets_basis, monitors_basis]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(0);

    let has_health_surface = current.targets.configured > 0 || current.monitors.configured > 0;
    let any_availability =
        targets_availability_delta.is_some() || monitors_availability_delta.is_some();
    let status = if has_health_surface && !any_availability {
        TrajectoryStatus::InsufficientSamples
    } else {
        TrajectoryStatus::Ok
    };

    ServiceSliTrajectory {
        targets_availability_delta,
        prior_targets_availability_ratio,
        monitors_availability_delta,
        prior_monitors_availability_ratio,
        error_total_delta,
        prior_error_total,
        sample_basis,
        status,
    }
}

/// Compute the availability delta for one signal: `(delta, prior_ratio,
/// sample_basis)`. Returns `(None, None, None)` when either window is below the
/// sample floor (prior counts are `double_width − current`).
fn availability_delta(
    current_successful: u64,
    current_total: u64,
    double_successful: u64,
    double_total: u64,
) -> (Option<f64>, Option<f64>, Option<u64>) {
    let prior_total = double_total.saturating_sub(current_total);
    let prior_successful = double_successful.saturating_sub(current_successful);

    if current_total < MIN_TRAJECTORY_SAMPLES || prior_total < MIN_TRAJECTORY_SAMPLES {
        return (None, None, None);
    }

    match (
        ratio(current_successful, current_total),
        ratio(prior_successful, prior_total),
    ) {
        (Some(current_ratio), Some(prior_ratio)) => (
            Some(current_ratio - prior_ratio),
            Some(prior_ratio),
            Some(current_total.min(prior_total)),
        ),
        _ => (None, None, None),
    }
}

fn service_sli_at_scoped(
    connection: &Connection,
    window: &str,
    now: OffsetDateTime,
    owner: Option<(&str, &str)>,
) -> QueryResult<Vec<ServiceSliSummary>> {
    let window = QueryWindow::parse(window).ok_or(QueryError::InvalidWindow)?;
    let cutoff = window.cutoff_at(now);
    let mut summaries = BTreeMap::new();

    add_target_sli(connection, owner, window, &mut summaries)?;
    add_target_check_sli(connection, &cutoff, owner, window, &mut summaries)?;
    add_monitor_sli(connection, owner, window, &mut summaries)?;
    add_monitor_check_in_sli(connection, &cutoff, owner, window, &mut summaries)?;
    add_error_sli(connection, &cutoff, owner, window, &mut summaries)?;
    add_incident_sli(connection, &cutoff, owner, window, &mut summaries)?;
    apply_default_slo(&mut summaries);

    Ok(summaries.into_values().collect())
}

fn service_sli_entry<'a>(
    summaries: &'a mut BTreeMap<String, ServiceSliSummary>,
    service: &str,
    window: QueryWindow,
) -> &'a mut ServiceSliSummary {
    summaries
        .entry(service.to_owned())
        .or_insert_with(|| ServiceSliSummary {
            service: service.to_owned(),
            window: window.as_str().to_owned(),
            slo: default_service_slo(false),
            targets: TargetSliSummary::default(),
            monitors: MonitorSliSummary::default(),
            errors: ErrorSliSummary::default(),
            incidents: IncidentSliSummary::default(),
            trajectory: None,
        })
}

fn apply_default_slo(summaries: &mut BTreeMap<String, ServiceSliSummary>) {
    for summary in summaries.values_mut() {
        let has_health_surface = summary.targets.configured > 0 || summary.monitors.configured > 0;
        summary.slo = default_service_slo(has_health_surface);
    }
}

fn add_target_sli(
    connection: &Connection,
    owner: Option<(&str, &str)>,
    window: QueryWindow,
    summaries: &mut BTreeMap<String, ServiceSliSummary>,
) -> QueryResult<()> {
    let sql = format!(
        "SELECT COALESCE(NULLIF(t.service, ''), t.name), COUNT(*)
         FROM targets t
         WHERE 1 = 1
         {}
         GROUP BY 1",
        owner_clause("t", 1, owner)
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(owner_params(owner)), configured_sli_row)?;
    for row in rows {
        let (service, configured) = row?;
        service_sli_entry(summaries, &service, window)
            .targets
            .configured = configured;
    }
    Ok(())
}

fn add_target_check_sli(
    connection: &Connection,
    cutoff: &str,
    owner: Option<(&str, &str)>,
    window: QueryWindow,
    summaries: &mut BTreeMap<String, ServiceSliSummary>,
) -> QueryResult<()> {
    let sql = format!(
        "SELECT
            COALESCE(NULLIF(t.service, ''), t.name),
            COUNT(*),
            SUM(CASE WHEN c.result = 'success' THEN 1 ELSE 0 END),
            SUM(CASE WHEN c.result != 'success' THEN 1 ELSE 0 END),
            AVG(c.latency_ms)
         FROM target_checks c
         JOIN targets t ON t.id = c.target_id
         WHERE c.checked_at >= ?1
         {}
         GROUP BY 1",
        owner_clause("t", 2, owner)
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(
        params_from_iter(window_params(cutoff, owner)),
        target_sli_row,
    )?;
    for row in rows {
        let row = row?;
        let target = &mut service_sli_entry(summaries, &row.service, window).targets;
        target.checks = row.total;
        target.successful_checks = row.successful;
        target.failed_checks = row.failed;
        target.availability_ratio = ratio(row.successful, row.total);
        target.latency_ms_average = row.latency_ms_average;
    }
    Ok(())
}

fn add_monitor_sli(
    connection: &Connection,
    owner: Option<(&str, &str)>,
    window: QueryWindow,
    summaries: &mut BTreeMap<String, ServiceSliSummary>,
) -> QueryResult<()> {
    let sql = format!(
        "SELECT COALESCE(NULLIF(m.service, ''), m.name), COUNT(*)
         FROM monitors m
         WHERE 1 = 1
         {}
         GROUP BY 1",
        owner_clause("m", 1, owner)
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(owner_params(owner)), configured_sli_row)?;
    for row in rows {
        let (service, configured) = row?;
        service_sli_entry(summaries, &service, window)
            .monitors
            .configured = configured;
    }
    Ok(())
}

fn add_monitor_check_in_sli(
    connection: &Connection,
    cutoff: &str,
    owner: Option<(&str, &str)>,
    window: QueryWindow,
    summaries: &mut BTreeMap<String, ServiceSliSummary>,
) -> QueryResult<()> {
    let sql = format!(
        "SELECT
            COALESCE(NULLIF(m.service, ''), m.name),
            COUNT(*),
            SUM(CASE WHEN c.status IN ('alive', 'ok', 'in_progress') THEN 1 ELSE 0 END),
            SUM(CASE WHEN c.status = 'error' THEN 1 ELSE 0 END)
         FROM monitor_check_ins c
         JOIN monitors m ON m.id = c.monitor_id
         WHERE c.observed_at >= ?1
         {}
         GROUP BY 1",
        owner_clause("m", 2, owner)
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(
        params_from_iter(window_params(cutoff, owner)),
        monitor_sli_row,
    )?;
    for row in rows {
        let row = row?;
        let monitor = &mut service_sli_entry(summaries, &row.service, window).monitors;
        monitor.check_ins = row.total;
        monitor.healthy_check_ins = row.successful;
        monitor.failed_check_ins = row.failed;
        monitor.availability_ratio = ratio(row.successful, row.total);
    }
    Ok(())
}

fn add_error_sli(
    connection: &Connection,
    cutoff: &str,
    owner: Option<(&str, &str)>,
    window: QueryWindow,
    summaries: &mut BTreeMap<String, ServiceSliSummary>,
) -> QueryResult<()> {
    let sql = format!(
        "SELECT e.service, COUNT(*), COUNT(DISTINCT e.group_hash)
         FROM errors e
         WHERE e.created_at >= ?1
         {}
         GROUP BY e.service",
        owner_clause("e", 2, owner)
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(
        params_from_iter(window_params(cutoff, owner)),
        error_sli_row,
    )?;
    for row in rows {
        let (service, total, groups) = row?;
        let errors = &mut service_sli_entry(summaries, &service, window).errors;
        errors.total = total;
        errors.groups = groups;
    }
    Ok(())
}

fn add_incident_sli(
    connection: &Connection,
    cutoff: &str,
    owner: Option<(&str, &str)>,
    window: QueryWindow,
    summaries: &mut BTreeMap<String, ServiceSliSummary>,
) -> QueryResult<()> {
    let sql = format!(
        "SELECT
            i.service,
            SUM(CASE WHEN i.opened_at >= ?1 THEN 1 ELSE 0 END),
            SUM(CASE WHEN i.resolved_at >= ?1 THEN 1 ELSE 0 END),
            SUM(CASE WHEN i.state != 'resolved' THEN 1 ELSE 0 END)
         FROM incidents i
         WHERE (i.opened_at >= ?1 OR i.resolved_at >= ?1 OR i.state != 'resolved')
         {}
         GROUP BY i.service",
        owner_clause("i", 2, owner)
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(
        params_from_iter(window_params(cutoff, owner)),
        incident_sli_row,
    )?;
    for row in rows {
        let row = row?;
        let incidents = &mut service_sli_entry(summaries, &row.service, window).incidents;
        incidents.opened = row.opened;
        incidents.resolved = row.resolved;
        incidents.active = row.active;
    }
    Ok(())
}

struct HealthSliRow {
    service: String,
    total: u64,
    successful: u64,
    failed: u64,
    latency_ms_average: Option<f64>,
}

fn configured_sli_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, u64)> {
    Ok((row.get(0)?, row.get(1)?))
}

fn target_sli_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HealthSliRow> {
    Ok(HealthSliRow {
        service: row.get(0)?,
        total: row.get(1)?,
        successful: row.get(2)?,
        failed: row.get(3)?,
        latency_ms_average: row.get(4)?,
    })
}

fn monitor_sli_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HealthSliRow> {
    Ok(HealthSliRow {
        service: row.get(0)?,
        total: row.get(1)?,
        successful: row.get(2)?,
        failed: row.get(3)?,
        latency_ms_average: None,
    })
}

fn error_sli_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, u64, u64)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

struct IncidentSliRow {
    service: String,
    opened: u64,
    resolved: u64,
    active: u64,
}

fn incident_sli_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IncidentSliRow> {
    Ok(IncidentSliRow {
        service: row.get(0)?,
        opened: row.get(1)?,
        resolved: row.get(2)?,
        active: row.get(3)?,
    })
}

fn ratio(successful: u64, total: u64) -> Option<f64> {
    (total > 0).then_some(successful as f64 / total as f64)
}

fn owner_clause(alias: &str, first_parameter: usize, owner: Option<(&str, &str)>) -> String {
    owner
        .map(|_| {
            format!(
                "AND {alias}.tenant_id = ?{first_parameter} AND {alias}.project_id = ?{}",
                first_parameter + 1
            )
        })
        .unwrap_or_default()
}

fn owner_params<'a>(owner: Option<(&'a str, &'a str)>) -> Vec<&'a str> {
    owner
        .map(|(tenant_id, project_id)| vec![tenant_id, project_id])
        .unwrap_or_default()
}

fn window_params<'a>(cutoff: &'a str, owner: Option<(&'a str, &'a str)>) -> Vec<&'a str> {
    let mut values = vec![cutoff];
    if let Some((tenant_id, project_id)) = owner {
        values.push(tenant_id);
        values.push(project_id);
    }
    values
}

#[cfg(test)]
mod tests {
    use rusqlite::params;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    use super::*;
    use crate::{MonitorInsert, Store, TargetInsert};

    #[test]
    fn service_sli_counts_windowed_health_errors_and_incidents_by_service()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        store.insert_target(TargetInsert {
            id: "TGT-api".to_owned(),
            url: "https://api.example.test/health".to_owned(),
            name: "API".to_owned(),
            service: "api".to_owned(),
            method: "GET".to_owned(),
            headers: None,
            interval_ms: 60_000,
            timeout_ms: 10_000,
            expected_status: "200".to_owned(),
            body_contains: None,
            degraded_after: 2,
            down_after: 3,
            up_after: 1,
            active: true,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
        })?;
        store.insert_monitor(MonitorInsert {
            id: "MON-api".to_owned(),
            name: "API worker".to_owned(),
            service: "api".to_owned(),
            mode: "ttl".to_owned(),
            expected_every_ms: 60_000,
            grace_ms: 5_000,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
        })?;
        store.insert_monitor(MonitorInsert {
            id: "MON-worker".to_owned(),
            name: "Worker".to_owned(),
            service: "worker".to_owned(),
            mode: "ttl".to_owned(),
            expected_every_ms: 60_000,
            grace_ms: 5_000,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
        })?;
        for (checked_at, result, latency_ms) in [
            ("2026-05-28T20:55:00Z", "success", 50),
            ("2026-05-28T20:50:00Z", "error", 100),
            ("2026-05-28T20:45:00Z", "success", 150),
            ("2026-05-28T14:00:00Z", "error", 999),
        ] {
            store.connection.execute(
                "INSERT INTO target_checks (
                    target_id, checked_at, status_code, latency_ms, result
                 ) VALUES ('TGT-api', ?1, 200, ?2, ?3)",
                params![checked_at, latency_ms, result],
            )?;
        }
        for (id, monitor_id, status, observed_at) in [
            ("CHK-api-alive", "MON-api", "alive", "2026-05-28T20:58:00Z"),
            ("CHK-api-error", "MON-api", "error", "2026-05-28T20:57:00Z"),
            (
                "CHK-api-in-progress-old",
                "MON-api",
                "in_progress",
                "2026-05-28T14:00:00Z",
            ),
            (
                "CHK-worker-in-progress",
                "MON-worker",
                "in_progress",
                "2026-05-28T20:56:00Z",
            ),
        ] {
            store.connection.execute(
                "INSERT INTO monitor_check_ins (id, monitor_id, status, observed_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, monitor_id, status, observed_at],
            )?;
        }
        for (id, group_hash, service, created_at) in [
            (
                "ERR-sliapi000001",
                "group-sli-api-a",
                "api",
                "2026-05-28T20:54:00Z",
            ),
            (
                "ERR-sliapi000002",
                "group-sli-api-b",
                "api",
                "2026-05-28T20:53:00Z",
            ),
            (
                "ERR-sliapiold01",
                "group-sli-api-old",
                "api",
                "2026-05-28T14:00:00Z",
            ),
            (
                "ERR-slibatch001",
                "group-sli-batch-a",
                "batch-worker",
                "2026-05-28T20:52:30Z",
            ),
        ] {
            store.connection.execute(
                "INSERT INTO errors (
                    id, service, error_class, message, group_hash, created_at
                 ) VALUES (?1, ?2, 'RuntimeError', 'boom', ?3, ?4)",
                params![id, service, group_hash, created_at],
            )?;
        }
        for (id, service, state, opened_at, resolved_at) in [
            (
                "INC-sliapi000001",
                "api",
                "investigating",
                "2026-05-28T20:52:00Z",
                None,
            ),
            (
                "INC-sliapi000002",
                "api",
                "resolved",
                "2026-05-28T20:51:00Z",
                Some("2026-05-28T20:59:00Z"),
            ),
            (
                "INC-sliapiold01",
                "api",
                "resolved",
                "2026-05-28T14:00:00Z",
                Some("2026-05-28T14:30:00Z"),
            ),
        ] {
            store.connection.execute(
                "INSERT INTO incidents (id, service, state, severity, title, opened_at, resolved_at)
                 VALUES (?1, ?2, ?3, 'medium', 'sli incident', ?4, ?5)",
                params![id, service, state, opened_at, resolved_at],
            )?;
        }
        let now = OffsetDateTime::parse("2026-05-28T21:00:00Z", &Rfc3339)?;

        let summaries = store.service_sli_at("6h", now)?;

        assert_eq!(summaries.len(), 3);
        let api = summaries
            .iter()
            .find(|summary| summary.service == "api")
            .ok_or("missing api SLI")?;
        assert_eq!(api.window, "6h");
        assert_eq!(api.slo.class, "standard");
        assert_eq!(api.slo.source, "default_health_surface");
        assert_ratio(Some(api.slo.availability_target), 0.995);
        assert_eq!(api.slo.latency_ms_average_target, 1_000);
        assert_eq!(api.slo.error_budget_events_per_hour, 5);
        assert_eq!(api.targets.configured, 1);
        assert_eq!(api.targets.checks, 3);
        assert_eq!(api.targets.successful_checks, 2);
        assert_eq!(api.targets.failed_checks, 1);
        assert_ratio(api.targets.availability_ratio, 2.0 / 3.0);
        assert_eq!(api.targets.latency_ms_average, Some(100.0));
        assert_eq!(api.monitors.configured, 1);
        assert_eq!(api.monitors.check_ins, 2);
        assert_eq!(api.monitors.healthy_check_ins, 1);
        assert_eq!(api.monitors.failed_check_ins, 1);
        assert_ratio(api.monitors.availability_ratio, 0.5);
        assert_eq!(api.errors.total, 2);
        assert_eq!(api.errors.groups, 2);
        assert_eq!(api.incidents.opened, 2);
        assert_eq!(api.incidents.resolved, 1);
        assert_eq!(api.incidents.active, 1);

        let worker = summaries
            .iter()
            .find(|summary| summary.service == "worker")
            .ok_or("missing worker SLI")?;
        assert_eq!(worker.targets.configured, 0);
        assert_eq!(worker.monitors.configured, 1);
        assert_eq!(worker.monitors.check_ins, 1);
        assert_eq!(worker.monitors.healthy_check_ins, 1);
        assert_ratio(worker.monitors.availability_ratio, 1.0);
        assert_eq!(worker.errors.total, 0);
        assert_eq!(worker.incidents.active, 0);
        let batch = summaries
            .iter()
            .find(|summary| summary.service == "batch-worker")
            .ok_or("missing batch-worker SLI")?;
        assert_eq!(batch.slo.class, "best_effort");
        assert_eq!(batch.slo.source, "default_signal_only");
        assert_ratio(Some(batch.slo.availability_target), 0.99);
        assert_eq!(batch.slo.latency_ms_average_target, 2_500);
        assert_eq!(batch.slo.error_budget_events_per_hour, 20);
        assert_eq!(batch.errors.total, 1);
        assert_eq!(batch.targets.configured, 0);
        assert_eq!(batch.monitors.configured, 0);
        assert!(matches!(
            store.service_sli_at("99h", now),
            Err(QueryError::InvalidWindow)
        ));

        Ok(())
    }

    fn insert_traj_target(store: &mut Store, id: &str, service: &str) -> crate::Result<()> {
        store.insert_target(TargetInsert {
            id: id.to_owned(),
            url: format!("https://{service}.example.test/health"),
            name: service.to_owned(),
            service: service.to_owned(),
            method: "GET".to_owned(),
            headers: None,
            interval_ms: 60_000,
            timeout_ms: 10_000,
            expected_status: "200".to_owned(),
            body_contains: None,
            degraded_after: 2,
            down_after: 3,
            up_after: 1,
            active: true,
            created_at: "2026-05-28T18:00:00Z".to_owned(),
        })
    }

    fn insert_check(
        store: &Store,
        target_id: &str,
        checked_at: &str,
        result: &str,
    ) -> crate::Result<()> {
        store.connection.execute(
            "INSERT INTO target_checks (target_id, checked_at, status_code, latency_ms, result)
             VALUES (?1, ?2, 200, 50, ?3)",
            params![target_id, checked_at, result],
        )?;
        Ok(())
    }

    #[test]
    fn service_sli_trajectory_reports_availability_and_error_deltas()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        insert_traj_target(&mut store, "TGT-traj", "traj")?;

        // Current window [20:00, 21:00): 30 checks, 27 success -> 0.9 availability.
        for minute in 0..30 {
            let result = if minute < 27 { "success" } else { "error" };
            insert_check(
                &store,
                "TGT-traj",
                &format!("2026-05-28T20:{minute:02}:00Z"),
                result,
            )?;
        }
        // Prior window [19:00, 20:00): 25 checks, all success -> 1.0 availability.
        for minute in 0..25 {
            insert_check(
                &store,
                "TGT-traj",
                &format!("2026-05-28T19:{minute:02}:00Z"),
                "success",
            )?;
        }
        // Errors: 5 in the current window, 2 in the prior window.
        for (idx, created_at) in [
            "2026-05-28T20:05:00Z",
            "2026-05-28T20:10:00Z",
            "2026-05-28T20:15:00Z",
            "2026-05-28T20:20:00Z",
            "2026-05-28T20:25:00Z",
            "2026-05-28T19:05:00Z",
            "2026-05-28T19:10:00Z",
        ]
        .into_iter()
        .enumerate()
        {
            store.connection.execute(
                "INSERT INTO errors (id, service, error_class, message, group_hash, created_at)
                 VALUES (?1, 'traj', 'RuntimeError', 'boom', ?2, ?3)",
                params![
                    format!("ERR-traj{idx:08}"),
                    format!("group-traj-{idx}"),
                    created_at
                ],
            )?;
        }

        let now = OffsetDateTime::parse("2026-05-28T21:00:00Z", &Rfc3339)?;
        let summaries = store.service_sli_with_trajectory_at("1h", now)?;
        let traj = summaries
            .iter()
            .find(|summary| summary.service == "traj")
            .and_then(|summary| summary.trajectory)
            .ok_or("missing traj trajectory")?;

        assert_eq!(traj.status, TrajectoryStatus::Ok);
        assert_ratio(traj.prior_targets_availability_ratio, 1.0);
        let delta = traj
            .targets_availability_delta
            .ok_or("missing availability delta")?;
        assert!(
            (delta - (-0.1)).abs() < 1e-9,
            "expected -0.1 delta, got {delta}"
        );
        assert_eq!(traj.prior_error_total, 2);
        assert_eq!(traj.error_total_delta, 3);
        assert_eq!(traj.sample_basis, 25);
        Ok(())
    }

    #[test]
    fn service_sli_trajectory_marks_insufficient_samples_below_floor()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        insert_traj_target(&mut store, "TGT-thin", "thin")?;

        // Ten checks per window, below MIN_TRAJECTORY_SAMPLES (20).
        for minute in 0..10 {
            insert_check(
                &store,
                "TGT-thin",
                &format!("2026-05-28T20:{minute:02}:00Z"),
                "success",
            )?;
            insert_check(
                &store,
                "TGT-thin",
                &format!("2026-05-28T19:{minute:02}:00Z"),
                "success",
            )?;
        }

        let now = OffsetDateTime::parse("2026-05-28T21:00:00Z", &Rfc3339)?;
        let summaries = store.service_sli_with_trajectory_at("1h", now)?;
        let traj = summaries
            .iter()
            .find(|summary| summary.service == "thin")
            .and_then(|summary| summary.trajectory)
            .ok_or("missing thin trajectory")?;

        assert_eq!(traj.status, TrajectoryStatus::InsufficientSamples);
        assert!(traj.targets_availability_delta.is_none());
        assert!(traj.prior_targets_availability_ratio.is_none());
        assert_eq!(traj.sample_basis, 0);
        Ok(())
    }

    fn migrated_store() -> crate::Result<Store> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        Ok(store)
    }

    fn assert_ratio(actual: Option<f64>, expected: f64) {
        let within_epsilon = actual.is_some_and(|actual| (actual - expected).abs() < f64::EPSILON);
        assert!(within_epsilon, "expected ratio {expected}, got {actual:?}");
    }
}
