//! Server-side API-key enforcement for authenticated routes.
//!
//! `canary-http::auth` owns the wire contract for bearer parsing and Problem
//! Details. This module owns the server runtime concerns: store-backed key
//! lookup, scoped route authorization, route-family rate limits, and silent
//! invalid-key accounting.

use axum::http::{HeaderMap, HeaderName, HeaderValue, header::AUTHORIZATION};
use canary_http::{
    auth::{
        ApiKeyScope, BearerToken, Permission, extract_bearer, insufficient_scope_problem,
        invalid_api_key_problem, missing_authorization_problem,
    },
    problem_details::{ProblemDetails, internal_problem},
    rate_limit::{RateLimitKind, rate_limited_problem},
};
use canary_store::VerifiedApiKey;

use crate::rate_limit::RateLimitDecision;
use crate::{AuthFailIdentityConfig, IngestState};

pub(crate) const UNKNOWN_AUTH_FAIL_IDENTITY: &str = "unknown";

pub(crate) fn require_ingest_scope(
    state: &IngestState,
    headers: &HeaderMap,
) -> Result<(), Box<ProblemDetails>> {
    let key = require_scope(state, headers, Permission::Ingest)?;
    enforce_rate_limit(state, RateLimitKind::Ingest, &key.id)
}

pub(crate) fn require_read_scope(
    state: &IngestState,
    headers: &HeaderMap,
) -> Result<(), Box<ProblemDetails>> {
    let key = require_scope(state, headers, Permission::Read)?;
    enforce_rate_limit(state, RateLimitKind::Query, &key.id)
}

pub(crate) fn require_query_limited_admin_scope(
    state: &IngestState,
    headers: &HeaderMap,
) -> Result<(), Box<ProblemDetails>> {
    let key = require_scope(state, headers, Permission::Admin)?;
    enforce_rate_limit(state, RateLimitKind::Query, &key.id)
}

pub(crate) fn require_scope(
    state: &IngestState,
    headers: &HeaderMap,
    permission: Permission,
) -> Result<VerifiedApiKey, Box<ProblemDetails>> {
    let authorization_headers = headers
        .get_all(AUTHORIZATION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();

    let token = match extract_bearer(&authorization_headers) {
        BearerToken::Present(token) => token,
        BearerToken::Missing => return Err(Box::new(missing_authorization_problem(None))),
    };

    let store = state
        .lock_store()
        .map_err(|_| Box::new(internal_problem()))?;
    let Some(key) = store
        .verify_api_key(token)
        .map_err(|_| Box::new(internal_problem()))?
    else {
        account_auth_fail(state, headers)?;
        return Err(Box::new(invalid_api_key_problem(None)));
    };
    drop(store);

    let Some(scope) = ApiKeyScope::parse(&key.scope) else {
        account_auth_fail(state, headers)?;
        return Err(Box::new(invalid_api_key_problem(None)));
    };
    if scope.allows(permission) {
        Ok(key)
    } else {
        Err(Box::new(insufficient_scope_problem(
            scope, permission, None,
        )))
    }
}

fn account_auth_fail(state: &IngestState, headers: &HeaderMap) -> Result<(), Box<ProblemDetails>> {
    let identity = auth_fail_identity(headers, state.auth_fail_identity());
    enforce_rate_limit(state, RateLimitKind::AuthFail, &identity)
}

pub(crate) fn auth_fail_identity(headers: &HeaderMap, config: AuthFailIdentityConfig) -> String {
    if config.trust_proxy_headers
        && let Some(identity) = trusted_proxy_client_identity(headers)
    {
        return identity;
    }

    UNKNOWN_AUTH_FAIL_IDENTITY.to_owned()
}

fn trusted_proxy_client_identity(headers: &HeaderMap) -> Option<String> {
    header_proxy_token(headers, "fly-client-ip")
        .or_else(|| forwarded_for_identity(headers))
        .or_else(|| header_proxy_token(headers, "x-forwarded-for"))
        .filter(|identity| !identity.is_empty())
}

fn forwarded_for_identity(headers: &HeaderMap) -> Option<String> {
    let value = headers
        .get(HeaderName::from_static("forwarded"))
        .and_then(header_value_to_str)?;

    value
        .split(',')
        .next_back()
        .into_iter()
        .flat_map(|entry| entry.split(';'))
        .find_map(|part| {
            let (name, value) = part.split_once('=')?;
            if !name.trim().eq_ignore_ascii_case("for") {
                return None;
            }
            Some(normalize_forwarded_for(value))
        })
        .filter(|identity| !identity.is_empty())
}

fn header_proxy_token(headers: &HeaderMap, name: &'static str) -> Option<String> {
    headers
        .get(HeaderName::from_static(name))
        .and_then(header_value_to_str)
        .and_then(|value| value.split(',').next_back())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_forwarded_for)
}

fn header_value_to_str(value: &HeaderValue) -> Option<&str> {
    value.to_str().ok()
}

fn normalize_forwarded_for(value: &str) -> String {
    let value = value.trim().trim_matches('"').trim();

    if let Some(bracketed) = value.strip_prefix('[')
        && let Some((host, _rest)) = bracketed.split_once(']')
    {
        return host.trim().to_owned();
    }

    if let Some((host, port)) = value.rsplit_once(':')
        && !host.contains(':')
        && port.chars().all(|character| character.is_ascii_digit())
    {
        return host.trim().to_owned();
    }

    value.to_owned()
}

fn enforce_rate_limit(
    state: &IngestState,
    kind: RateLimitKind,
    identity: &str,
) -> Result<(), Box<ProblemDetails>> {
    let mut limiter = state
        .rate_limiter()
        .lock()
        .map_err(|_| Box::new(internal_problem()))?;

    match limiter.check(kind, identity) {
        RateLimitDecision::Allowed => Ok(()),
        RateLimitDecision::Limited {
            retry_after_seconds,
        } => Err(Box::new(rate_limited_problem(retry_after_seconds))),
    }
}
