//! HTTP authorization contracts for scoped Canary API keys.
//!
//! This module deliberately stops at the router boundary: it parses bearer
//! headers, models scope decisions, and builds Phoenix-compatible problem
//! responses. Key storage and hash verification belong in the persistence layer.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::problem_details::{ProblemCode, ProblemDetails};

/// Stable API-key scope stored on Canary keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApiKeyScope {
    /// Full access to admin, ingest, and read routes.
    Admin,
    /// Access to ingest routes only.
    IngestOnly,
    /// Access to read/query routes only.
    ReadOnly,
}

impl ApiKeyScope {
    /// Return the existing Phoenix wire value for the scope.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::IngestOnly => "ingest-only",
            Self::ReadOnly => "read-only",
        }
    }

    /// True when this scope permits the requested route permission.
    pub const fn allows(self, permission: Permission) -> bool {
        match permission {
            Permission::Admin => matches!(self, Self::Admin),
            Permission::Ingest => matches!(self, Self::Admin | Self::IngestOnly),
            Permission::Read => matches!(self, Self::Admin | Self::ReadOnly),
        }
    }
}

/// Route-level permission enforced by Canary's router pipelines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Administrative routes.
    Admin,
    /// Error and check-in ingest routes.
    Ingest,
    /// Query and reporting routes.
    Read,
}

impl Permission {
    /// Return scopes accepted by the existing Phoenix router for this permission.
    pub const fn allowed_scopes(self) -> &'static [ApiKeyScope] {
        match self {
            Self::Admin => &[ApiKeyScope::Admin],
            Self::Ingest => &[ApiKeyScope::Admin, ApiKeyScope::IngestOnly],
            Self::Read => &[ApiKeyScope::Admin, ApiKeyScope::ReadOnly],
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Ingest => "ingest",
            Self::Read => "read",
        }
    }
}

/// Result of parsing an HTTP Authorization header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BearerToken<'a> {
    /// A Phoenix-compatible `Bearer ...` token.
    Present(&'a str),
    /// Header is absent or not exactly a single `Bearer ` value.
    Missing,
}

/// Extract the same bearer-token shape accepted by `CanaryWeb.Plugs.Auth`.
pub fn extract_bearer<'a>(authorization_headers: &'a [&'a str]) -> BearerToken<'a> {
    match authorization_headers {
        [header] => header
            .strip_prefix("Bearer ")
            .map(str::trim)
            .map_or(BearerToken::Missing, BearerToken::Present),
        _ => BearerToken::Missing,
    }
}

/// Build the 401 response used when the Authorization header is missing.
pub fn missing_authorization_problem(request_id: Option<String>) -> ProblemDetails {
    ProblemDetails::new(
        401,
        ProblemCode::InvalidApiKey,
        "Missing Authorization header. Use: Bearer sk_...",
        request_id,
    )
}

/// Build the 401 response used when a supplied API key is invalid or revoked.
pub fn invalid_api_key_problem(request_id: Option<String>) -> ProblemDetails {
    ProblemDetails::new(
        401,
        ProblemCode::InvalidApiKey,
        "Invalid or revoked API key.",
        request_id,
    )
}

/// Build the 403 response used when a valid key has the wrong scope.
pub fn insufficient_scope_problem(
    scope: ApiKeyScope,
    permission: Permission,
    request_id: Option<String>,
) -> ProblemDetails {
    ProblemDetails::new(
        403,
        ProblemCode::InsufficientScope,
        insufficient_scope_detail(scope, permission),
        request_id,
    )
    .with_extra("scope", json!(scope.as_str()))
    .with_extra(
        "required_scopes",
        json!(
            permission
                .allowed_scopes()
                .iter()
                .map(|scope| scope.as_str())
                .collect::<Vec<_>>()
        ),
    )
}

fn insufficient_scope_detail(scope: ApiKeyScope, permission: Permission) -> String {
    let allowed = permission
        .allowed_scopes()
        .iter()
        .map(|scope| format!("`{}`", scope.as_str()))
        .collect::<Vec<_>>()
        .join(" or ");

    format!(
        "API key scope `{}` cannot access this {} endpoint. Use an {} key.",
        scope.as_str(),
        permission.label(),
        allowed
    )
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, to_value};

    use super::*;

    #[test]
    fn scope_matrix_matches_router_pipelines() {
        assert!(ApiKeyScope::Admin.allows(Permission::Admin));
        assert!(ApiKeyScope::Admin.allows(Permission::Ingest));
        assert!(ApiKeyScope::Admin.allows(Permission::Read));

        assert!(!ApiKeyScope::IngestOnly.allows(Permission::Admin));
        assert!(ApiKeyScope::IngestOnly.allows(Permission::Ingest));
        assert!(!ApiKeyScope::IngestOnly.allows(Permission::Read));

        assert!(!ApiKeyScope::ReadOnly.allows(Permission::Admin));
        assert!(!ApiKeyScope::ReadOnly.allows(Permission::Ingest));
        assert!(ApiKeyScope::ReadOnly.allows(Permission::Read));
    }

    #[test]
    fn bearer_parser_matches_phoenix_extract_key_shape() {
        assert_eq!(
            extract_bearer(&["Bearer sk_live_abc "]),
            BearerToken::Present("sk_live_abc")
        );
        assert_eq!(extract_bearer(&[]), BearerToken::Missing);
        assert_eq!(extract_bearer(&["Basic abc"]), BearerToken::Missing);
        assert_eq!(
            extract_bearer(&["Bearer one", "Bearer two"]),
            BearerToken::Missing
        );
    }

    #[test]
    fn auth_failure_problems_match_phoenix_wire_shape() {
        let missing = to_value(missing_authorization_problem(None)).unwrap_or(Value::Null);
        assert_eq!(missing["status"], 401);
        assert_eq!(missing["code"], "invalid_api_key");
        assert_eq!(
            missing["detail"],
            "Missing Authorization header. Use: Bearer sk_..."
        );
        assert!(missing["request_id"].is_null());

        let invalid =
            to_value(invalid_api_key_problem(Some("req-1".to_owned()))).unwrap_or(Value::Null);
        assert_eq!(invalid["status"], 401);
        assert_eq!(invalid["code"], "invalid_api_key");
        assert_eq!(invalid["detail"], "Invalid or revoked API key.");
        assert_eq!(invalid["request_id"], "req-1");
    }

    #[test]
    fn insufficient_scope_problem_matches_phoenix_wire_shape() {
        let encoded = to_value(insufficient_scope_problem(
            ApiKeyScope::ReadOnly,
            Permission::Ingest,
            Some("req-1".to_owned()),
        ))
        .unwrap_or(Value::Null);

        assert_eq!(
            encoded["type"],
            "https://canary.dev/problems/insufficient-scope"
        );
        assert_eq!(encoded["title"], "Insufficient Scope");
        assert_eq!(encoded["status"], 403);
        assert_eq!(encoded["code"], "insufficient_scope");
        assert_eq!(encoded["request_id"], "req-1");
        assert_eq!(encoded["scope"], "read-only");
        assert_eq!(encoded["required_scopes"], json!(["admin", "ingest-only"]));
        assert_eq!(
            encoded["detail"],
            "API key scope `read-only` cannot access this ingest endpoint. Use an `admin` or `ingest-only` key."
        );
    }
}
