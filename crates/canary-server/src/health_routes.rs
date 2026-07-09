//! Authenticated health read routes.
//!
//! Public liveness/readiness probes stay in `public_routes`; target mutation
//! stays in `admin_targets`; probe execution stays in `target_probes`.

use std::collections::BTreeMap;

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Response, StatusCode},
};
use canary_http::problem_details::{
    internal_problem, invalid_window_problem, target_checks_window_problem,
};
use canary_store::{
    ErrorSummaryItem, HealthMonitorStatus, HealthTargetStatus, QueryError, TargetCheckRead,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    IngestState,
    http_contract::{json_status_response, problem_response},
    require_read_scope,
};

#[derive(Deserialize)]
pub(crate) struct StatusParams {
    window: Option<String>,
}

pub(crate) async fn health_status(
    State(state): State<IngestState>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match require_read_scope(&state, &headers) {
        Ok(key) => key,
        Err(problem) => return problem_response(*problem),
    };

    let reader = match state.read_source() {
        Ok(reader) => reader,
        Err(_) => return problem_response(internal_problem()),
    };
    let mut targets = match reader.health_targets_scoped(&key.tenant_id, &key.project_id) {
        Ok(targets) => targets,
        Err(_) => return problem_response(internal_problem()),
    };
    let mut monitors = match reader.health_monitors_scoped(&key.tenant_id, &key.project_id) {
        Ok(monitors) => monitors,
        Err(_) => return problem_response(internal_problem()),
    };
    if let Some(bound_service) = key.service.as_deref() {
        targets.retain(|target| target.service == bound_service);
        monitors.retain(|monitor| monitor.service == bound_service);
    }

    json_status_response(
        StatusCode::OK.as_u16(),
        json!({
            "summary": health_status_summary(&targets, &monitors),
            "targets": targets.iter().map(health_target_response).collect::<Vec<_>>(),
            "monitors": monitors.iter().map(health_monitor_response).collect::<Vec<_>>(),
        }),
    )
}

pub(crate) async fn status(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(params): Query<StatusParams>,
) -> Response<Body> {
    let key = match require_read_scope(&state, &headers) {
        Ok(key) => key,
        Err(problem) => return problem_response(*problem),
    };

    let window = params.window.as_deref().unwrap_or("1h");
    let reader = match state.read_source() {
        Ok(reader) => reader,
        Err(_) => return problem_response(internal_problem()),
    };
    let mut targets = match reader.health_targets_scoped(&key.tenant_id, &key.project_id) {
        Ok(targets) => targets,
        Err(_) => return problem_response(internal_problem()),
    };
    let mut monitors = match reader.health_monitors_scoped(&key.tenant_id, &key.project_id) {
        Ok(monitors) => monitors,
        Err(_) => return problem_response(internal_problem()),
    };
    let mut error_summary =
        match reader.error_summary_scoped(window, &key.tenant_id, &key.project_id) {
            Ok(summary) => summary,
            Err(QueryError::InvalidWindow) => return problem_response(invalid_window_problem()),
            Err(QueryError::Sqlite(_)) => return problem_response(internal_problem()),
        };
    if let Some(bound_service) = key.service.as_deref() {
        targets.retain(|target| target.service == bound_service);
        monitors.retain(|monitor| monitor.service == bound_service);
        error_summary.retain(|summary| summary.service == bound_service);
    }
    let overall = combined_overall(&targets, &monitors, &error_summary);

    json_status_response(
        StatusCode::OK.as_u16(),
        json!({
            "overall": overall,
            "summary": combined_status_summary(&overall, &targets, &monitors, &error_summary, window),
            "targets": targets.iter().map(status_target_response).collect::<Vec<_>>(),
            "monitors": monitors.iter().map(status_monitor_response).collect::<Vec<_>>(),
            "error_summary": error_summary.iter().map(error_summary_response).collect::<Vec<_>>(),
        }),
    )
}

pub(crate) async fn target_checks(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<StatusParams>,
) -> Response<Body> {
    let key = match require_read_scope(&state, &headers) {
        Ok(key) => key,
        Err(problem) => return problem_response(*problem),
    };

    let window = params.window.as_deref().unwrap_or("24h");
    let reader = match state.read_source() {
        Ok(reader) => reader,
        Err(_) => return problem_response(internal_problem()),
    };
    let checks = match key.service.as_deref() {
        Some(service) => reader.target_checks_scoped_for_service(
            &id,
            window,
            &key.tenant_id,
            &key.project_id,
            service,
        ),
        None => reader.target_checks_scoped(&id, window, &key.tenant_id, &key.project_id),
    };
    let checks = match checks {
        Ok(checks) => checks,
        Err(QueryError::InvalidWindow) => return problem_response(target_checks_window_problem()),
        Err(QueryError::Sqlite(_)) => return problem_response(internal_problem()),
    };

    json_status_response(
        StatusCode::OK.as_u16(),
        json!({
            "target_id": id,
            "window": window,
            "checks": checks.iter().map(target_check_response).collect::<Vec<_>>(),
        }),
    )
}

pub(crate) fn health_target_response(target: &HealthTargetStatus) -> Value {
    json!({
        "id": target.id,
        "name": target.name,
        "service": target.service,
        "url": target.url,
        "state": target.state,
        "consecutive_failures": target.consecutive_failures,
        "last_checked_at": target.last_checked_at,
        "last_success_at": target.last_success_at,
        "latency_ms": target.latency_ms,
        "tls_expires_at": target.tls_expires_at,
        "recent_checks": target.recent_checks.iter().map(|check| json!({
            "checked_at": check.checked_at,
            "result": check.result,
            "status_code": check.status_code,
            "latency_ms": check.latency_ms,
        })).collect::<Vec<_>>(),
    })
}

pub(crate) fn health_monitor_response(monitor: &HealthMonitorStatus) -> Value {
    json!({
        "id": monitor.id,
        "name": monitor.name,
        "service": monitor.service,
        "mode": monitor.mode,
        "expected_every_ms": monitor.expected_every_ms,
        "grace_ms": monitor.grace_ms,
        "state": monitor.state,
        "last_check_in_status": monitor.last_check_in_status,
        "last_check_in_at": monitor.last_check_in_at,
        "last_success_at": monitor.last_success_at,
        "last_failure_at": monitor.last_failure_at,
        "deadline_at": monitor.deadline_at,
    })
}

pub(crate) fn combined_overall(
    targets: &[HealthTargetStatus],
    monitors: &[HealthMonitorStatus],
    error_summary: &[ErrorSummaryItem],
) -> String {
    if targets.is_empty() && monitors.is_empty() && error_summary.is_empty() {
        return "empty".to_owned();
    }
    if targets.is_empty() && monitors.is_empty() {
        return "warning".to_owned();
    }

    let has_down = targets.iter().any(|target| target.state == "down")
        || monitors.iter().any(|monitor| monitor.state == "down");
    let has_non_up = targets.iter().any(|target| target.state != "up")
        || monitors.iter().any(|monitor| monitor.state != "up");

    if has_down {
        "unhealthy".to_owned()
    } else if has_non_up {
        "degraded".to_owned()
    } else if !error_summary.is_empty() {
        "warning".to_owned()
    } else {
        "healthy".to_owned()
    }
}

pub(crate) fn combined_status_summary(
    overall: &str,
    targets: &[HealthTargetStatus],
    monitors: &[HealthMonitorStatus],
    error_summary: &[ErrorSummaryItem],
    window: &str,
) -> String {
    let surface_count = targets.len() + monitors.len();
    if overall == "empty" {
        return "No services configured.".to_owned();
    }
    if overall == "healthy" {
        return format!(
            "All {surface_count} health surfaces healthy. No errors in the last {}.",
            window_label(window)
        );
    }

    let mut states = BTreeMap::<&str, Vec<&str>>::new();
    for target in targets {
        states
            .entry(target.state.as_str())
            .or_default()
            .push(target.name.as_str());
    }
    for monitor in monitors {
        states
            .entry(monitor.state.as_str())
            .or_default()
            .push(monitor.name.as_str());
    }
    let total_errors = error_summary
        .iter()
        .map(|item| item.total_count)
        .sum::<i64>();

    let mut summary = format!("{surface_count} health surfaces monitored.");
    if let Some(part) = describe_status_state_group(states.get("down"), "down") {
        summary.push_str(&part);
    }
    if let Some(part) = describe_status_state_group(states.get("degraded"), "degraded") {
        summary.push_str(&part);
    }
    if total_errors > 0 {
        let service_count = error_summary.len();
        let service_word = if service_count == 1 {
            "service"
        } else {
            "services"
        };
        summary.push_str(&format!(
            " {total_errors} errors across {service_count} {service_word} in the last {}.",
            window_label(window)
        ));
    }

    summary
}

fn status_target_response(target: &HealthTargetStatus) -> Value {
    json!({
        "id": target.id,
        "name": target.name,
        "url": target.url,
        "state": target.state,
        "consecutive_failures": target.consecutive_failures,
        "last_checked_at": target.last_checked_at,
    })
}

fn status_monitor_response(monitor: &HealthMonitorStatus) -> Value {
    json!({
        "id": monitor.id,
        "name": monitor.name,
        "service": monitor.service,
        "mode": monitor.mode,
        "state": monitor.state,
        "last_check_in_status": monitor.last_check_in_status,
        "last_check_in_at": monitor.last_check_in_at,
        "deadline_at": monitor.deadline_at,
    })
}

fn error_summary_response(item: &ErrorSummaryItem) -> Value {
    json!({
        "service": item.service,
        "total_count": item.total_count,
        "unique_classes": item.unique_classes,
    })
}

fn target_check_response(check: &TargetCheckRead) -> Value {
    json!({
        "checked_at": check.checked_at,
        "result": check.result,
        "status_code": check.status_code,
        "latency_ms": check.latency_ms,
        "tls_expires_at": check.tls_expires_at,
        "error_detail": check.error_detail,
    })
}

fn health_status_summary(
    targets: &[HealthTargetStatus],
    monitors: &[HealthMonitorStatus],
) -> String {
    let mut states = BTreeMap::<&str, Vec<&str>>::new();
    for target in targets {
        states
            .entry(target.state.as_str())
            .or_default()
            .push(target.name.as_str());
    }
    for monitor in monitors {
        states
            .entry(monitor.state.as_str())
            .or_default()
            .push(monitor.name.as_str());
    }

    summarize_health(targets.len() + monitors.len(), &states, "health surfaces")
}

fn summarize_health(total: usize, states: &BTreeMap<&str, Vec<&str>>, label: &str) -> String {
    let up = states.get("up").map_or(0, Vec::len);
    let mut parts = vec![format!("{total} {label} monitored. {up} up")];
    if let Some(part) = describe_health_state_group(states.get("degraded"), "degraded") {
        parts.push(part);
    }
    if let Some(part) = describe_health_state_group(states.get("down"), "down") {
        parts.push(part);
    }

    format!("{}.", parts.join(", "))
}

fn describe_health_state_group(names: Option<&Vec<&str>>, label: &str) -> Option<String> {
    let names = names?;
    if names.is_empty() {
        return None;
    }
    Some(format!("{} {label} ({})", names.len(), names.join(", ")))
}

fn describe_status_state_group(names: Option<&Vec<&str>>, label: &str) -> Option<String> {
    describe_health_state_group(names, label).map(|part| format!(" {part}."))
}

fn window_label(window: &str) -> &'static str {
    match window {
        "1h" => "hour",
        "6h" => "6 hours",
        "24h" => "24 hours",
        "7d" => "7 days",
        "30d" => "30 days",
        _ => "requested window",
    }
}
