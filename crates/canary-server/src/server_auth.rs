//! Server-side API-key enforcement for authenticated routes.
//!
//! `canary-http::auth` owns the wire contract for bearer parsing and Problem
//! Details. This module owns the server runtime concerns: store-backed key
//! lookup, scoped route authorization, route-family rate limits, and silent
//! invalid-key accounting.

use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header::AUTHORIZATION};
use canary_http::{
    auth::{
        ApiKeyScope, BearerToken, Permission, extract_bearer, insufficient_scope_problem,
        invalid_api_key_problem, missing_authorization_problem,
    },
    problem_details::{ProblemCode, ProblemDetails, internal_problem},
    rate_limit::{RateLimitKind, rate_limited_problem},
};
use canary_store::{DurableRateLimitDecision, VerifiedApiKey};
use serde_json::json;

use crate::rate_limit::RateLimitDecision;
use crate::server_time::current_unix_millis;
use crate::{AuthFailIdentityConfig, IngestState};

pub(crate) const UNKNOWN_AUTH_FAIL_IDENTITY: &str = "unknown";

pub(crate) fn require_ingest_scope(
    state: &IngestState,
    headers: &HeaderMap,
) -> Result<VerifiedApiKey, Box<ProblemDetails>> {
    let key = require_scope(state, headers, Permission::Ingest)?;
    enforce_rate_limit(state, RateLimitKind::Ingest, &key.id)?;
    Ok(key)
}

pub(crate) fn require_read_scope(
    state: &IngestState,
    headers: &HeaderMap,
) -> Result<VerifiedApiKey, Box<ProblemDetails>> {
    let key = require_scope(state, headers, Permission::Read)?;
    if ApiKeyScope::parse(&key.scope) == Some(ApiKeyScope::ResponderWrite)
        && key.service.as_deref().is_none()
    {
        return Err(Box::new(responder_service_binding_problem()));
    }
    enforce_rate_limit(state, RateLimitKind::Query, &key.id)?;
    Ok(key)
}

pub(crate) fn require_query_limited_admin_scope(
    state: &IngestState,
    headers: &HeaderMap,
) -> Result<VerifiedApiKey, Box<ProblemDetails>> {
    let key = require_scope(state, headers, Permission::Admin)?;
    enforce_rate_limit(state, RateLimitKind::Query, &key.id)?;
    Ok(key)
}

pub(crate) fn require_responder_write_scope(
    state: &IngestState,
    headers: &HeaderMap,
) -> Result<VerifiedApiKey, Box<ProblemDetails>> {
    let key = require_scope(state, headers, Permission::ResponderWrite)?;
    if ApiKeyScope::parse(&key.scope) == Some(ApiKeyScope::ResponderWrite)
        && key.service.as_deref().is_none()
    {
        return Err(Box::new(responder_service_binding_problem()));
    }
    enforce_rate_limit(state, RateLimitKind::Query, &key.id)?;
    Ok(key)
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
    let now_ms = current_unix_millis();
    let key = match state.auth_cache().get(token, now_ms) {
        Some(cached) => cached,
        None => {
            let auth_fail_identity = reject_limited_unknown_prefix(state, headers, token)?;

            // Fetch candidates under a short store lock, then run the
            // CPU-bound bcrypt loop AFTER dropping the guard: verification
            // under the single-writer lock serialized every authenticated
            // request behind ~230ms of bcrypt (canary-930). The epoch read
            // fences the insert against a concurrent revocation.
            let fetch_epoch = state.auth_cache().epoch();
            let candidates = {
                let store = state
                    .lock_store()
                    .map_err(|_| Box::new(internal_problem()))?;
                store
                    .api_key_verify_candidates(token)
                    .map_err(|_| Box::new(internal_problem()))?
            };
            let Some(key) = canary_store::verify_key_candidates(token, candidates) else {
                account_auth_fail_identity(state, &auth_fail_identity)?;
                return Err(Box::new(invalid_api_key_problem(None)));
            };
            state
                .auth_cache()
                .insert(token, key.clone(), now_ms, fetch_epoch);
            key
        }
    };

    let Some(scope) = ApiKeyScope::parse(&key.scope) else {
        let identity = auth_fail_identity(headers, state.auth_fail_identity());
        account_auth_fail_identity(state, &identity)?;
        return Err(Box::new(invalid_api_key_problem(None)));
    };
    if scope == ApiKeyScope::Admin
        && let Some(bound_service) = key.service.as_deref()
    {
        return Err(Box::new(service_authority_problem(bound_service, "*")));
    }
    if scope.allows(permission) {
        Ok(key)
    } else {
        Err(Box::new(insufficient_scope_problem(
            scope, permission, None,
        )))
    }
}

pub(crate) fn enforce_service_authority(
    key: &VerifiedApiKey,
    requested_service: &str,
) -> Result<(), Box<ProblemDetails>> {
    let Some(bound_service) = key.service.as_deref() else {
        return Ok(());
    };
    if requested_service == bound_service {
        return Ok(());
    }

    Err(Box::new(service_authority_problem(
        bound_service,
        requested_service,
    )))
}

pub(crate) fn service_authority_problem(
    bound_service: &str,
    requested_service: &str,
) -> ProblemDetails {
    ProblemDetails::new(
        StatusCode::FORBIDDEN.as_u16(),
        ProblemCode::InsufficientScope,
        format!(
            "API key is bound to service `{bound_service}` and cannot access `{requested_service}`."
        ),
        None,
    )
    .with_extra("bound_service", json!(bound_service))
    .with_extra("requested_service", json!(requested_service))
}

pub(crate) fn responder_service_binding_problem() -> ProblemDetails {
    ProblemDetails::new(
        StatusCode::FORBIDDEN.as_u16(),
        ProblemCode::InsufficientScope,
        "API key scope `responder-write` must be bound to one service.",
        None,
    )
    .with_extra("scope", json!("responder-write"))
    .with_extra("required_service_binding", json!(true))
}

fn reject_limited_unknown_prefix(
    state: &IngestState,
    headers: &HeaderMap,
    token: &str,
) -> Result<String, Box<ProblemDetails>> {
    let identity = auth_fail_identity(headers, state.auth_fail_identity());
    let mut limiter = state
        .rate_limiter()
        .lock()
        .map_err(|_| Box::new(internal_problem()))?;

    let retry_after_seconds = match limiter.peek(RateLimitKind::AuthFail, &identity) {
        RateLimitDecision::Allowed => return Ok(identity),
        RateLimitDecision::Limited {
            retry_after_seconds,
        } => retry_after_seconds,
    };
    drop(limiter);

    let store = state
        .lock_store()
        .map_err(|_| Box::new(internal_problem()))?;
    if store
        .active_api_key_prefix_exists(token)
        .map_err(|_| Box::new(internal_problem()))?
    {
        Ok(identity)
    } else {
        Err(Box::new(rate_limited_problem(retry_after_seconds)))
    }
}

fn account_auth_fail_identity(
    state: &IngestState,
    identity: &str,
) -> Result<(), Box<ProblemDetails>> {
    enforce_rate_limit(state, RateLimitKind::AuthFail, identity)
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
    {
        let mut limiter = state
            .rate_limiter()
            .lock()
            .map_err(|_| Box::new(internal_problem()))?;

        match limiter.check(kind, identity) {
            RateLimitDecision::Allowed => {}
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => return Err(Box::new(rate_limited_problem(retry_after_seconds))),
        }
    }

    let policy = kind.policy();
    let mut store = state
        .lock_store()
        .map_err(|_| Box::new(internal_problem()))?;
    match store
        .check_rate_limit(
            rate_limit_kind_name(kind),
            identity,
            policy.limit,
            policy.window_ms,
            current_unix_millis(),
        )
        .map_err(|_| Box::new(internal_problem()))?
    {
        DurableRateLimitDecision::Allowed => Ok(()),
        DurableRateLimitDecision::Limited {
            retry_after_seconds,
        } => Err(Box::new(rate_limited_problem(retry_after_seconds))),
    }
}

fn rate_limit_kind_name(kind: RateLimitKind) -> &'static str {
    match kind {
        RateLimitKind::Ingest => "ingest",
        RateLimitKind::Query => "query",
        RateLimitKind::AuthFail => "auth_fail",
    }
}
