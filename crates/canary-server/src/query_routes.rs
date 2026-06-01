//! Axum adapter for Canary's authenticated query read routes.
//!
//! This module owns read-model route translation for errors, timelines, and
//! incidents. Route registration and shared auth/response primitives stay in
//! `lib.rs`; reporting stays separate because it has CSV and multi-surface
//! pagination concerns.

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Response, StatusCode},
};
use canary_http::problem_details::{
    internal_problem, invalid_cursor_problem, invalid_event_type_problem, invalid_limit_problem,
    invalid_window_problem, missing_query_problem, not_found_problem,
};
use canary_store::{
    IncidentListOptions, QueryError, ServiceQueryOptions, TimelineQueryError, TimelineQueryOptions,
};
use serde::Deserialize;

use crate::{
    IngestState,
    http_contract::{json_status_response, problem_response},
    require_read_scope,
};

enum QueryKind {
    Service {
        service: String,
    },
    ErrorClass {
        error_class: String,
        service: Option<String>,
    },
    ErrorClasses,
}

#[derive(Debug, Deserialize)]
pub(crate) struct QueryParams {
    service: Option<String>,
    error_class: Option<String>,
    group_by: Option<String>,
    window: Option<String>,
    cursor: Option<String>,
    with_annotation: Option<String>,
    without_annotation: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct TimelineParams {
    service: Option<String>,
    window: Option<String>,
    limit: Option<String>,
    cursor: Option<String>,
    after: Option<String>,
    event_type: Option<String>,
}

pub(crate) async fn query_errors(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(params): Query<QueryParams>,
) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
        return problem_response(*problem);
    }

    let query_kind = match (
        params.error_class.as_deref(),
        params.service.as_deref(),
        params.group_by.as_deref(),
    ) {
        (Some(error_class), service, _) => QueryKind::ErrorClass {
            error_class: error_class.to_owned(),
            service: service.map(ToOwned::to_owned),
        },
        (None, Some(service), _) => QueryKind::Service {
            service: service.to_owned(),
        },
        (None, None, Some("error_class")) => QueryKind::ErrorClasses,
        (None, None, _) => return problem_response(missing_query_problem()),
    };

    let default_window = match &query_kind {
        QueryKind::Service { .. } => "1h",
        QueryKind::ErrorClass { .. } | QueryKind::ErrorClasses => "24h",
    };
    let window = params.window.as_deref().unwrap_or(default_window);
    let options = ServiceQueryOptions {
        cursor: params.cursor,
        with_annotation: params.with_annotation,
        without_annotation: params.without_annotation,
    };

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match query_kind {
        QueryKind::Service { service } => {
            match store.errors_by_service(&service, window, options) {
                Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
                Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
                Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
            }
        }
        QueryKind::ErrorClass {
            error_class,
            service,
        } => match store.errors_by_error_class(&error_class, window, service.as_deref(), options) {
            Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
            Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
            Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
        },
        QueryKind::ErrorClasses => match store.errors_by_class(window) {
            Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
            Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
            Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
        },
    }
}

pub(crate) async fn timeline(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(params): Query<TimelineParams>,
) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
        return problem_response(*problem);
    }

    let window = params.window.as_deref().unwrap_or("24h");
    let cursor = params.after.or(params.cursor);
    let options = TimelineQueryOptions {
        service: params.service,
        limit: params.limit,
        cursor,
        event_type: params.event_type,
    };
    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.timeline(window, options) {
        Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
        Err(TimelineQueryError::InvalidWindow) => problem_response(invalid_window_problem()),
        Err(TimelineQueryError::InvalidLimit) => problem_response(invalid_limit_problem()),
        Err(TimelineQueryError::InvalidCursor) => problem_response(invalid_cursor_problem()),
        Err(TimelineQueryError::InvalidEventType(invalid)) => problem_response(
            invalid_event_type_problem(&invalid, canary_core::webhook_events::business()),
        ),
        Err(TimelineQueryError::Sqlite(_)) => problem_response(internal_problem()),
    }
}

pub(crate) async fn list_incidents(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(params): Query<QueryParams>,
) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
        return problem_response(*problem);
    }

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.active_incidents(IncidentListOptions {
        with_annotation: params.with_annotation,
        without_annotation: params.without_annotation,
    }) {
        Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
        Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
    }
}

pub(crate) async fn show_error(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
        return problem_response(*problem);
    }

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.error_detail(&id) {
        Ok(Some(result)) => json_status_response(StatusCode::OK.as_u16(), result),
        Ok(None) => problem_response(not_found_problem(format!("Error {id} not found."))),
        Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
    }
}

pub(crate) async fn show_incident(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    if let Err(problem) = require_read_scope(&state, &headers) {
        return problem_response(*problem);
    }

    let store = match state.store.lock() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.incident_detail(&id) {
        Ok(Some(result)) => json_status_response(StatusCode::OK.as_u16(), result),
        Ok(None) => problem_response(not_found_problem(format!("Incident {id} not found."))),
        Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
    }
}
