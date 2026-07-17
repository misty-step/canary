//! Shared HTTP contract adapter for Canary's Rust server.
//!
//! Route modules own endpoint-specific translation. This module owns the
//! repeated Axum wire mechanics that must stay : response
//! content types, Problem Details serialization, request-size preflight, and
//! query-shape quirks that Axum's typed extractors intentionally hide.

use axum::{
    body::Body,
    http::{
        HeaderMap, HeaderValue, Response, StatusCode,
        header::{CONTENT_LENGTH, CONTENT_TYPE, RETRY_AFTER},
    },
};
use canary_http::{
    problem_details::{ProblemCode, ProblemDetails, payload_too_large_problem},
    public::PublicResponse,
    request::MAX_JSON_BODY_BYTES,
};
use serde::Serialize;

pub(crate) const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";
pub(crate) const PROBLEM_CONTENT_TYPE: &str = "application/problem+json; charset=utf-8";

pub(crate) fn check_content_length(headers: &HeaderMap) -> Result<(), Box<ProblemDetails>> {
    let Some(value) = headers.get(CONTENT_LENGTH) else {
        return Ok(());
    };

    let length = value
        .to_str()
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);

    if length > MAX_JSON_BODY_BYTES {
        Err(Box::new(payload_too_large_problem(
            "Request body exceeds 100KB limit.",
        )))
    } else {
        Ok(())
    }
}

pub(crate) fn json_response<T>(contract: PublicResponse<T>) -> Response<Body>
where
    T: Serialize,
{
    match serde_json::to_vec(&contract.body) {
        Ok(body) => response(contract.status, contract.content_type, Body::from(body)),
        Err(_) => internal_server_error(),
    }
}

pub(crate) fn json_status_response<T>(status: u16, body: T) -> Response<Body>
where
    T: Serialize,
{
    match serde_json::to_vec(&body) {
        Ok(body) => response(status, JSON_CONTENT_TYPE, Body::from(body)),
        Err(_) => internal_server_error(),
    }
}

pub(crate) fn problem_response(problem: ProblemDetails) -> Response<Body> {
    let status = problem.status;
    let retryable = problem.code == ProblemCode::Unavailable.as_str();
    match serde_json::to_vec(&problem) {
        Ok(body) => {
            let mut response = response(status, PROBLEM_CONTENT_TYPE, Body::from(body));
            if retryable {
                response
                    .headers_mut()
                    .insert(RETRY_AFTER, HeaderValue::from_static("5"));
            }
            response
        }
        Err(_) => internal_server_error(),
    }
}

/// Build the retryable problem used when SQLite's single writer is busy.
pub(crate) fn storage_busy_problem() -> ProblemDetails {
    ProblemDetails::new(
        StatusCode::SERVICE_UNAVAILABLE.as_u16(),
        ProblemCode::Unavailable,
        "Persistent storage is temporarily busy. Retry the request.",
        None,
    )
}

/// Build the retryable response used when SQLite's single writer is busy.
pub(crate) fn storage_busy_response() -> Response<Body> {
    problem_response(storage_busy_problem())
}

pub(crate) fn query_param_is_array(raw_query: Option<&str>, param: &str) -> bool {
    let Some(raw_query) = raw_query else {
        return false;
    };
    let bracket = format!("{param}[]");
    let encoded_bracket = format!("{param}%5B%5D");
    let mut seen = 0;

    for pair in raw_query.split('&') {
        let key = pair.split_once('=').map_or(pair, |(key, _)| key);
        if key == param {
            seen += 1;
            if seen > 1 {
                return true;
            }
        }
        if key == bracket || key.eq_ignore_ascii_case(&encoded_bracket) {
            return true;
        }
    }

    false
}

pub(crate) fn text_response(contract: PublicResponse<&'static str>) -> Response<Body> {
    response(
        contract.status,
        contract.content_type,
        Body::from(contract.body),
    )
}

pub(crate) fn response(status: u16, content_type: &'static str, body: Body) -> Response<Body> {
    let mut response = Response::new(body);
    *response.status_mut() = status_code(status);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

fn internal_server_error() -> Response<Body> {
    response(
        StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
        "text/plain; charset=utf-8",
        Body::from("internal server error"),
    )
}

fn status_code(status: u16) -> StatusCode {
    match StatusCode::from_u16(status) {
        Ok(status) => status,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
