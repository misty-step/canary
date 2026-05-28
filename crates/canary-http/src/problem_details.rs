//! RFC 9457 Problem Details responses.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const BASE_URL: &str = "https://canary.dev/problems";

/// Stable Canary problem code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemCode {
    /// Malformed request or unsupported parameters.
    InvalidRequest,
    /// Missing, invalid, or revoked API key.
    InvalidApiKey,
    /// API key cannot access the requested scope.
    InsufficientScope,
    /// Requested resource does not exist.
    NotFound,
    /// Request payload is too large.
    PayloadTooLarge,
    /// Request failed validation.
    ValidationError,
    /// Rate limit exceeded.
    RateLimited,
    /// Internal error.
    InternalError,
    /// Service is temporarily unavailable.
    Unavailable,
    /// Forward-compatible custom code.
    Other(String),
}

impl ProblemCode {
    /// Return the stable snake_case code.
    pub fn as_str(&self) -> &str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidApiKey => "invalid_api_key",
            Self::InsufficientScope => "insufficient_scope",
            Self::NotFound => "not_found",
            Self::PayloadTooLarge => "payload_too_large",
            Self::ValidationError => "validation_error",
            Self::RateLimited => "rate_limited",
            Self::InternalError => "internal_error",
            Self::Unavailable => "unavailable",
            Self::Other(code) => code.as_str(),
        }
    }

    /// Return the title used by the existing Phoenix service.
    pub fn title(&self) -> String {
        match self {
            Self::InvalidRequest => "Invalid Request".to_owned(),
            Self::InvalidApiKey => "Invalid API Key".to_owned(),
            Self::InsufficientScope => "Insufficient Scope".to_owned(),
            Self::NotFound => "Not Found".to_owned(),
            Self::PayloadTooLarge => "Payload Too Large".to_owned(),
            Self::ValidationError => "Validation Error".to_owned(),
            Self::RateLimited => "Rate Limit Exceeded".to_owned(),
            Self::InternalError => "Internal Server Error".to_owned(),
            Self::Unavailable => "Service Unavailable".to_owned(),
            Self::Other(code) => titleize(code),
        }
    }

    fn problem_type(&self) -> String {
        format!("{BASE_URL}/{}", self.as_str().replace('_', "-"))
    }
}

/// RFC 9457 response body with Canary's stable `code` and `request_id` fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProblemDetails {
    /// Problem type URI.
    #[serde(rename = "type")]
    pub problem_type: String,
    /// Human-readable title.
    pub title: String,
    /// HTTP status code.
    pub status: u16,
    /// Human-readable detail.
    pub detail: String,
    /// Stable machine-readable Canary code.
    pub code: String,
    /// Request ID when available.
    pub request_id: Option<String>,
    /// Extra structured fields merged into the JSON body.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ProblemDetails {
    /// Build a problem response that preserves the Phoenix wire shape.
    pub fn new(
        status: u16,
        code: ProblemCode,
        detail: impl Into<String>,
        request_id: Option<String>,
    ) -> Self {
        Self {
            problem_type: code.problem_type(),
            title: code.title(),
            status,
            detail: detail.into(),
            code: code.as_str().to_owned(),
            request_id,
            extra: Map::new(),
        }
    }

    /// Add an extra RFC-compatible field.
    pub fn with_extra(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }
}

fn titleize(code: &str) -> String {
    let mut chars = code.replace('_', " ").chars().collect::<Vec<_>>();
    if let Some(first) = chars.first_mut() {
        first.make_ascii_uppercase();
    }
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_api_key_matches_existing_problem_shape() {
        let problem = ProblemDetails::new(
            401,
            ProblemCode::InvalidApiKey,
            "Missing Authorization header. Use: Bearer sk_...",
            Some("req-1".to_owned()),
        );

        let encoded = serde_json::to_value(problem).unwrap_or(Value::Null);

        assert_eq!(
            encoded["type"],
            "https://canary.dev/problems/invalid-api-key"
        );
        assert_eq!(encoded["title"], "Invalid API Key");
        assert_eq!(encoded["status"], 401);
        assert_eq!(encoded["code"], "invalid_api_key");
        assert_eq!(encoded["request_id"], "req-1");
    }

    #[test]
    fn insufficient_scope_can_carry_scope_metadata() {
        let problem = ProblemDetails::new(
            403,
            ProblemCode::InsufficientScope,
            "API key scope `ingest-only` cannot access this read endpoint.",
            None,
        )
        .with_extra("scope", Value::String("ingest-only".to_owned()));

        let encoded = serde_json::to_value(problem).unwrap_or(Value::Null);
        assert_eq!(encoded["scope"], "ingest-only");
        assert!(encoded.get("request_id").is_some());
        assert!(encoded["request_id"].is_null());
    }

    #[test]
    fn known_problem_codes_match_phoenix_titles_and_type_slugs() {
        let cases = [
            (
                ProblemCode::InvalidRequest,
                "invalid_request",
                "Invalid Request",
            ),
            (
                ProblemCode::InvalidApiKey,
                "invalid_api_key",
                "Invalid API Key",
            ),
            (
                ProblemCode::InsufficientScope,
                "insufficient_scope",
                "Insufficient Scope",
            ),
            (ProblemCode::NotFound, "not_found", "Not Found"),
            (
                ProblemCode::PayloadTooLarge,
                "payload_too_large",
                "Payload Too Large",
            ),
            (
                ProblemCode::ValidationError,
                "validation_error",
                "Validation Error",
            ),
            (
                ProblemCode::RateLimited,
                "rate_limited",
                "Rate Limit Exceeded",
            ),
            (
                ProblemCode::InternalError,
                "internal_error",
                "Internal Server Error",
            ),
            (
                ProblemCode::Unavailable,
                "unavailable",
                "Service Unavailable",
            ),
        ];

        for (code, expected_code, expected_title) in cases {
            let encoded = serde_json::to_value(ProblemDetails::new(400, code, "detail", None))
                .unwrap_or(Value::Null);

            assert_eq!(encoded["code"], expected_code);
            assert_eq!(encoded["title"], expected_title);
            assert_eq!(
                encoded["type"],
                format!(
                    "https://canary.dev/problems/{}",
                    expected_code.replace('_', "-")
                )
            );
            assert!(encoded["request_id"].is_null());
        }
    }
}
