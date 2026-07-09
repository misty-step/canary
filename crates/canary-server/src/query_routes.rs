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
use canary_core::query::active_incidents_response;
use canary_http::problem_details::{
    internal_problem, invalid_cursor_problem, invalid_event_type_problem, invalid_limit_problem,
    invalid_window_problem, missing_query_problem, not_found_problem,
};
use canary_store::{
    ErrorGroupQueryError, IncidentListOptions, QueryError, ServiceQueryOptions, TimelineQueryError,
    TimelineQueryOptions,
};
use serde::Deserialize;

use crate::{
    IngestState, enforce_service_authority,
    http_contract::{json_status_response, problem_response},
    require_read_scope, service_authority_problem,
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
    let key = match require_read_scope(&state, &headers) {
        Ok(key) => key,
        Err(problem) => return problem_response(*problem),
    };

    let mut query_kind = match (
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
    if let Some(bound_service) = key.service.as_deref() {
        match &mut query_kind {
            QueryKind::Service { service } => {
                if let Err(problem) = enforce_service_authority(&key, service) {
                    return problem_response(*problem);
                }
            }
            QueryKind::ErrorClass { service, .. } => match service {
                Some(service) => {
                    if let Err(problem) = enforce_service_authority(&key, service) {
                        return problem_response(*problem);
                    }
                }
                None => {
                    *service = Some(bound_service.to_owned());
                }
            },
            QueryKind::ErrorClasses => {
                return problem_response(service_authority_problem(bound_service, "*"));
            }
        }
    }

    let default_window = match &query_kind {
        QueryKind::Service { .. } => "1h",
        QueryKind::ErrorClass { .. } | QueryKind::ErrorClasses => "24h",
    };
    let window = params.window.as_deref().unwrap_or(default_window);
    let options = ServiceQueryOptions {
        tenant_id: Some(key.tenant_id.clone()),
        project_id: Some(key.project_id.clone()),
        cursor: params.cursor,
        with_annotation: params.with_annotation,
        without_annotation: params.without_annotation,
    };

    // `errors_by_service` and `errors_by_error_class` fuse a claim-expiry
    // write into the read (Store::errors_by_service/_by_error_class take
    // `&mut self`), so those two branches stay on the writer. Only the
    // class-listing branch is a pure read and can use the read pool.
    match query_kind {
        QueryKind::Service { service } => {
            let mut store = match state.lock_store() {
                Ok(store) => store,
                Err(_) => return problem_response(internal_problem()),
            };
            match store.errors_by_service(&service, window, options) {
                Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
                Err(ErrorGroupQueryError::InvalidWindow) => {
                    problem_response(invalid_window_problem())
                }
                Err(ErrorGroupQueryError::InvalidCursor) => {
                    problem_response(invalid_cursor_problem())
                }
                Err(ErrorGroupQueryError::Sqlite(_)) => problem_response(internal_problem()),
            }
        }
        QueryKind::ErrorClass {
            error_class,
            service,
        } => {
            let mut store = match state.lock_store() {
                Ok(store) => store,
                Err(_) => return problem_response(internal_problem()),
            };
            match store.errors_by_error_class(&error_class, window, service.as_deref(), options) {
                Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
                Err(ErrorGroupQueryError::InvalidWindow) => {
                    problem_response(invalid_window_problem())
                }
                Err(ErrorGroupQueryError::InvalidCursor) => {
                    problem_response(invalid_cursor_problem())
                }
                Err(ErrorGroupQueryError::Sqlite(_)) => problem_response(internal_problem()),
            }
        }
        QueryKind::ErrorClasses => {
            let reader = match state.read_source() {
                Ok(reader) => reader,
                Err(_) => return problem_response(internal_problem()),
            };
            match reader.errors_by_class_scoped(window, &key.tenant_id, &key.project_id) {
                Ok(result) => json_status_response(StatusCode::OK.as_u16(), result),
                Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
                Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
            }
        }
    }
}

pub(crate) async fn timeline(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Query(params): Query<TimelineParams>,
) -> Response<Body> {
    let key = match require_read_scope(&state, &headers) {
        Ok(key) => key,
        Err(problem) => return problem_response(*problem),
    };

    let window = params.window.as_deref().unwrap_or("24h");
    let cursor = params.after.or(params.cursor);
    let service = match (key.service.as_deref(), params.service) {
        (Some(_bound_service), Some(service)) => {
            if let Err(problem) = enforce_service_authority(&key, &service) {
                return problem_response(*problem);
            }
            Some(service)
        }
        (Some(bound_service), None) => Some(bound_service.to_owned()),
        (None, service) => service,
    };
    let options = TimelineQueryOptions {
        tenant_id: Some(key.tenant_id),
        project_id: Some(key.project_id),
        service,
        limit: params.limit,
        cursor,
        event_type: params.event_type,
    };
    let reader = match state.read_source() {
        Ok(reader) => reader,
        Err(_) => return problem_response(internal_problem()),
    };

    match reader.timeline(window, options) {
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
    let key = match require_read_scope(&state, &headers) {
        Ok(key) => key,
        Err(problem) => return problem_response(*problem),
    };

    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.active_incidents(IncidentListOptions {
        tenant_id: Some(key.tenant_id),
        project_id: Some(key.project_id),
        with_annotation: params.with_annotation,
        without_annotation: params.without_annotation,
    }) {
        Ok(mut result) => {
            if let Some(bound_service) = key.service.as_deref() {
                result
                    .incidents
                    .retain(|incident| incident.service == bound_service);
                result = active_incidents_response(result.incidents);
            }
            json_status_response(StatusCode::OK.as_u16(), result)
        }
        Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
    }
}

pub(crate) async fn show_error(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    let key = match require_read_scope(&state, &headers) {
        Ok(key) => key,
        Err(problem) => return problem_response(*problem),
    };

    let reader = match state.read_source() {
        Ok(reader) => reader,
        Err(_) => return problem_response(internal_problem()),
    };

    let response = match reader.error_detail_scoped(&id, &key.tenant_id, &key.project_id) {
        Ok(Some(result)) => {
            if let Some(bound_service) = key.service.as_deref()
                && result.service != bound_service
            {
                return problem_response(not_found_problem(format!("Error {id} not found.")));
            }
            json_status_response(StatusCode::OK.as_u16(), result)
        }
        Ok(None) => problem_response(not_found_problem(format!("Error {id} not found."))),
        Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
    };
    drop(reader);

    // The audit event is a write, so it stays on the writer; the heavy read
    // above already ran off the writer connection.
    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };
    crate::read_audit::record_read_audit(&mut store, &key, "GET /api/v1/errors/{id}");
    response
}

pub(crate) async fn show_incident(
    State(state): State<IngestState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    let key = match require_read_scope(&state, &headers) {
        Ok(key) => key,
        Err(problem) => return problem_response(*problem),
    };

    let mut store = match state.lock_store() {
        Ok(store) => store,
        Err(_) => return problem_response(internal_problem()),
    };

    match store.incident_detail_scoped(&id, &key.tenant_id, &key.project_id) {
        Ok(Some(result)) => {
            if let Some(bound_service) = key.service.as_deref()
                && result.incident.service != bound_service
            {
                return problem_response(crate::service_authority_problem(
                    bound_service,
                    &result.incident.service,
                ));
            }
            let audit_event_id = match crate::read_audit::record_context_read_audit(
                &mut store,
                &key,
                crate::read_audit::ReadAuditContext {
                    route: "GET /api/v1/incidents/{id}",
                    subject_type: "incident",
                    subject_id: &result.incident.id,
                    subject_service: &result.incident.service,
                    context_schema: crate::responder_context::INCIDENT_CONTEXT_SCHEMA,
                },
            ) {
                Ok(audit_event_id) => audit_event_id,
                Err(_) => return problem_response(internal_problem()),
            };
            json_status_response(
                StatusCode::OK.as_u16(),
                crate::responder_context::incident_context_response(result, &key, audit_event_id),
            )
        }
        Ok(None) => problem_response(not_found_problem(format!("Incident {id} not found."))),
        Err(QueryError::InvalidWindow) => problem_response(invalid_window_problem()),
        Err(QueryError::Sqlite(_)) => problem_response(internal_problem()),
    }
}
