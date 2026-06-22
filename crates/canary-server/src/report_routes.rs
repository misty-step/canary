//! Axum adapter for Canary's report route.
//!
//! Reports are a read surface with their own CSV format and pagination cursor.
//! The route strings and shared HTTP primitives stay in `lib.rs`; canonical
//! health projections stay in `health_routes` and are reused here.

use axum::{
    body::Body,
    extract::{Query, RawQuery, State},
    http::{HeaderMap, HeaderName, Response, StatusCode},
};
use canary_core::query::{
    ErrorGroupSummary, ReportCursor, decode_report_cursor, encode_report_cursor,
};
use canary_http::problem_details::{
    ProblemDetails, internal_problem, invalid_report_cursor_problem, invalid_report_limit_problem,
    invalid_string_param_problem, invalid_window_problem,
};
use canary_store::{
    IncidentListOptions, QueryError, RecentTransition, SearchResult, ServiceSliSummary,
    TimelineQueryError, TimelineQueryOptions,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::health_routes::{
    combined_overall, combined_status_summary, health_monitor_response, health_target_response,
};
use crate::{
    IngestState,
    http_contract::{json_status_response, problem_response, query_param_is_array, response},
    require_read_scope,
};

#[derive(Deserialize)]
pub(crate) struct ReportParams {
    window: Option<String>,
    q: Option<String>,
    limit: Option<String>,
    cursor: Option<String>,
}

pub(crate) async fn report(
    State(state): State<IngestState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    Query(params): Query<ReportParams>,
) -> Response<Body> {
    let key = match require_read_scope(&state, &headers) {
        Ok(key) => key,
        Err(problem) => return problem_response(*problem),
    };
    if query_param_is_array(raw_query.as_deref(), "q") {
        return problem_response(invalid_string_param_problem("q"));
    }

    let window = params.window.as_deref().unwrap_or("1h");
    let limit = match parse_report_limit(params.limit.as_deref()) {
        Ok(limit) => limit,
        Err(problem) => return problem_response(*problem),
    };
    let cursor = match parse_report_cursor(params.cursor.as_deref()) {
        Ok(cursor) => cursor,
        Err(problem) => return problem_response(*problem),
    };

    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    let mut targets = match store.health_targets_scoped(&key.tenant_id, &key.project_id) {
        Ok(targets) => targets,
        Err(_) => return problem_response(internal_problem()),
    };
    let mut monitors = match store.health_monitors_scoped(&key.tenant_id, &key.project_id) {
        Ok(monitors) => monitors,
        Err(_) => return problem_response(internal_problem()),
    };
    let mut error_summary =
        match store.error_summary_scoped(window, &key.tenant_id, &key.project_id) {
            Ok(summary) => summary,
            Err(QueryError::InvalidWindow) => return problem_response(invalid_window_problem()),
            Err(QueryError::Sqlite(_)) => return problem_response(internal_problem()),
        };
    let mut service_sli = match store.service_sli_scoped(window, &key.tenant_id, &key.project_id) {
        Ok(service_sli) => service_sli,
        Err(QueryError::InvalidWindow) => return problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => return problem_response(internal_problem()),
    };
    let mut error_groups =
        match store.report_error_groups_scoped(window, &key.tenant_id, &key.project_id) {
            Ok(groups) => groups,
            Err(QueryError::InvalidWindow) => return problem_response(invalid_window_problem()),
            Err(QueryError::Sqlite(_)) => return problem_response(internal_problem()),
        };
    let mut incidents = match store.active_incidents(IncidentListOptions {
        tenant_id: Some(key.tenant_id.clone()),
        project_id: Some(key.project_id.clone()),
        ..IncidentListOptions::default()
    }) {
        Ok(incidents) => incidents.incidents,
        Err(QueryError::InvalidWindow) => return problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => return problem_response(internal_problem()),
    };
    let mut transitions =
        match store.recent_transitions_scoped(window, &key.tenant_id, &key.project_id) {
            Ok(transitions) => transitions,
            Err(QueryError::InvalidWindow) => return problem_response(invalid_window_problem()),
            Err(QueryError::Sqlite(_)) => return problem_response(internal_problem()),
        };
    let recent_events = match store.timeline(
        window,
        TimelineQueryOptions {
            tenant_id: Some(key.tenant_id.clone()),
            project_id: Some(key.project_id.clone()),
            service: key.service.clone(),
            limit: Some("10".to_owned()),
            event_type: Some("telemetry.event".to_owned()),
            ..TimelineQueryOptions::default()
        },
    ) {
        Ok(events) => events.events,
        Err(TimelineQueryError::InvalidWindow) => return problem_response(invalid_window_problem()),
        Err(TimelineQueryError::InvalidLimit)
        | Err(TimelineQueryError::InvalidCursor)
        | Err(TimelineQueryError::InvalidEventType(_))
        | Err(TimelineQueryError::Sqlite(_)) => return problem_response(internal_problem()),
    };
    let mut search_results = match params.q.as_deref() {
        Some(query) => {
            match store.search_errors_scoped(query, window, &key.tenant_id, &key.project_id) {
                Ok(results) => Some(results),
                Err(QueryError::InvalidWindow) => return problem_response(invalid_window_problem()),
                Err(QueryError::Sqlite(_)) => Some(Vec::new()),
            }
        }
        None => None,
    };
    if let Some(bound_service) = key.service.as_deref() {
        targets.retain(|target| target.service == bound_service);
        monitors.retain(|monitor| monitor.service == bound_service);
        error_summary.retain(|summary| summary.service == bound_service);
        service_sli.retain(|summary| summary.service == bound_service);
        error_groups.retain(|group| group.service == bound_service);
        incidents.retain(|incident| incident.service == bound_service);
        transitions.retain(|transition| transition.service == bound_service);
        if let Some(results) = search_results.as_mut() {
            results.retain(|result| result.service == bound_service);
        }
    }

    let overall = combined_overall(&targets, &monitors, &error_summary);
    let summary = combined_status_summary(&overall, &targets, &monitors, &error_summary, window);
    let (targets, next_targets_offset) =
        paginate_report_items(targets, limit, cursor.targets_offset);
    let (monitors, next_monitor_offset) =
        paginate_report_items(monitors, limit, cursor.monitor_offset);
    let (error_groups, next_error_groups_offset) = paginate_report_items(
        error_groups,
        Some(limit.unwrap_or(25)),
        cursor.error_groups_offset,
    );
    // service_sli is a compact per-service snapshot; report cursors advance
    // only repeated detail sections.
    let next_cursor = ReportCursor {
        targets_offset: next_targets_offset,
        monitor_offset: next_monitor_offset,
        error_groups_offset: next_error_groups_offset,
    };
    let cursor = encode_report_cursor(&next_cursor);
    let truncated = cursor.is_some();

    let mut body = json!({
        "status": overall,
        "summary": summary,
        "targets": targets.iter().map(health_target_response).collect::<Vec<_>>(),
        "monitors": monitors.iter().map(health_monitor_response).collect::<Vec<_>>(),
        "service_sli": service_sli.iter().map(service_sli_response).collect::<Vec<_>>(),
        "error_groups": error_groups.iter().map(error_group_response).collect::<Vec<_>>(),
        "incidents": incidents,
        "recent_transitions": transitions.iter().map(recent_transition_response).collect::<Vec<_>>(),
        "recent_events": recent_events,
        "truncated": truncated,
        "cursor": cursor,
    });
    if let (Some(results), Some(object)) = (search_results, body.as_object_mut()) {
        object.insert(
            "search_results".to_owned(),
            Value::Array(results.iter().map(search_result_response).collect()),
        );
    }

    if accepts_csv(&headers) {
        response(
            StatusCode::OK.as_u16(),
            "text/csv; charset=utf-8",
            Body::from(report_csv(&body)),
        )
    } else {
        json_status_response(StatusCode::OK.as_u16(), body)
    }
}

fn error_group_response(group: &ErrorGroupSummary) -> Value {
    json!({
        "group_hash": group.group_hash,
        "error_class": group.error_class,
        "service": group.service,
        "count": group.total_count,
        "first_seen": group.first_seen,
        "last_seen": group.last_seen,
        "sample_message": group.sample_message,
        "severity": group.severity,
        "status": group.status,
        "classification": {
            "category": group.classification.category,
            "persistence": group.classification.persistence,
            "component": group.classification.component,
        },
        "current_claim": group.current_claim,
    })
}

fn recent_transition_response(transition: &RecentTransition) -> Value {
    json!({
        "entity_type": transition.entity_type,
        "entity_ref": transition.entity_ref,
        "name": transition.name,
        "service": transition.service,
        "state": transition.state,
        "transitioned_at": transition.transitioned_at,
    })
}

fn search_result_response(result: &SearchResult) -> Value {
    json!({
        "id": result.id,
        "service": result.service,
        "error_class": result.error_class,
        "message": result.message,
        "group_hash": result.group_hash,
        "created_at": result.created_at,
        "score": result.score,
    })
}

fn service_sli_response(summary: &ServiceSliSummary) -> Value {
    json!({
        "service": summary.service,
        "window": summary.window,
        "slo": {
            "class": summary.slo.class,
            "source": summary.slo.source,
            "availability_target": summary.slo.availability_target,
            "latency_ms_average_target": summary.slo.latency_ms_average_target,
            "error_budget_events_per_hour": summary.slo.error_budget_events_per_hour,
        },
        "targets": {
            "configured": summary.targets.configured,
            "checks": summary.targets.checks,
            "successful_checks": summary.targets.successful_checks,
            "failed_checks": summary.targets.failed_checks,
            "availability_ratio": summary.targets.availability_ratio,
            "latency_ms_average": summary.targets.latency_ms_average,
        },
        "monitors": {
            "configured": summary.monitors.configured,
            "check_ins": summary.monitors.check_ins,
            "healthy_check_ins": summary.monitors.healthy_check_ins,
            "failed_check_ins": summary.monitors.failed_check_ins,
            "availability_ratio": summary.monitors.availability_ratio,
        },
        "errors": {
            "total": summary.errors.total,
            "groups": summary.errors.groups,
        },
        "incidents": {
            "opened": summary.incidents.opened,
            "resolved": summary.incidents.resolved,
            "active": summary.incidents.active,
        },
    })
}

fn parse_report_limit(limit: Option<&str>) -> Result<Option<usize>, Box<ProblemDetails>> {
    match limit {
        None => Ok(None),
        Some(raw) => match raw.parse::<usize>() {
            Ok(value) if value > 0 => Ok(Some(value)),
            _ => Err(Box::new(invalid_report_limit_problem())),
        },
    }
}

fn parse_report_cursor(cursor: Option<&str>) -> Result<ReportCursor, Box<ProblemDetails>> {
    match cursor {
        None => Ok(ReportCursor::default()),
        Some(cursor) => {
            decode_report_cursor(cursor).ok_or_else(|| Box::new(invalid_report_cursor_problem()))
        }
    }
}

fn paginate_report_items<T: Clone>(
    items: Vec<T>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> (Vec<T>, Option<usize>) {
    let Some(offset) = offset else {
        return (Vec::new(), None);
    };
    let remaining = items.into_iter().skip(offset).collect::<Vec<_>>();
    let Some(limit) = limit else {
        return (remaining, None);
    };
    let page = remaining.iter().take(limit).cloned().collect::<Vec<_>>();
    let next_offset = if remaining.len() > page.len() {
        Some(offset + page.len())
    } else {
        None
    };
    (page, next_offset)
}

fn accepts_csv(headers: &HeaderMap) -> bool {
    headers
        .get(HeaderName::from_static("accept"))
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().starts_with("text/csv"))
        })
}

const REPORT_CSV_HEADERS: [&str; 17] = [
    "section",
    "position",
    "id",
    "name",
    "service",
    "error_class",
    "url",
    "state",
    "count",
    "first_seen",
    "last_seen",
    "severity",
    "status",
    "consecutive_failures",
    "last_checked_at",
    "cursor",
    "truncated",
];

fn report_csv(report: &Value) -> String {
    let mut rows = vec![REPORT_CSV_HEADERS.map(str::to_owned).to_vec()];
    rows.extend(report_csv_targets(report));
    rows.extend(report_csv_monitors(report));
    rows.extend(report_csv_error_groups(report));
    rows.into_iter()
        .map(|row| {
            row.into_iter()
                .map(|value| csv_value(&value))
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn report_csv_targets(report: &Value) -> Vec<Vec<String>> {
    report
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, target)| {
            vec![
                "targets".to_owned(),
                (index + 1).to_string(),
                csv_field(target, "id"),
                csv_field(target, "name"),
                String::new(),
                String::new(),
                csv_field(target, "url"),
                csv_field(target, "state"),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                csv_field(target, "consecutive_failures"),
                csv_field(target, "last_checked_at"),
                csv_field(report, "cursor"),
                csv_field(report, "truncated"),
            ]
        })
        .collect()
}

fn report_csv_monitors(report: &Value) -> Vec<Vec<String>> {
    report
        .get("monitors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, monitor)| {
            vec![
                "monitors".to_owned(),
                (index + 1).to_string(),
                csv_field(monitor, "id"),
                csv_field(monitor, "name"),
                csv_field(monitor, "service"),
                String::new(),
                String::new(),
                csv_field(monitor, "state"),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                csv_field(monitor, "last_check_in_status"),
                String::new(),
                csv_field(monitor, "last_check_in_at"),
                csv_field(report, "cursor"),
                csv_field(report, "truncated"),
            ]
        })
        .collect()
}

fn report_csv_error_groups(report: &Value) -> Vec<Vec<String>> {
    report
        .get("error_groups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, group)| {
            vec![
                "error_groups".to_owned(),
                (index + 1).to_string(),
                String::new(),
                String::new(),
                csv_field(group, "service"),
                csv_field(group, "error_class"),
                String::new(),
                String::new(),
                csv_field(group, "count"),
                csv_field(group, "first_seen"),
                csv_field(group, "last_seen"),
                csv_field(group, "severity"),
                csv_field(group, "status"),
                String::new(),
                String::new(),
                csv_field(report, "cursor"),
                csv_field(report, "truncated"),
            ]
        })
        .collect()
}

fn csv_field(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn csv_value(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}
