//! RFC 9457 Problem Details responses.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

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

/// Build a validation problem with the standard `errors` object.
pub fn validation_problem(detail: impl Into<String>, errors: impl Serialize) -> ProblemDetails {
    ProblemDetails::new(422, ProblemCode::ValidationError, detail, None)
        .with_extra("errors", to_json_value(errors))
}

/// Build a validation problem that has only a detail string.
pub fn validation_detail_problem(detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new(422, ProblemCode::ValidationError, detail, None)
}

/// Build the synchronous webhook test-delivery failure problem.
pub fn webhook_delivery_failed_problem(reason: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new(
        502,
        ProblemCode::Other("webhook_delivery_failed".to_owned()),
        format!("Webhook test delivery failed: {}", reason.into()),
        None,
    )
}

/// Build the target URL validation problem.
pub fn invalid_target_url_problem(reason: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        format!("Invalid URL: {}", reason.into()),
        None,
    )
}

/// Build the Phoenix-compatible invalid observed-at problem.
pub fn invalid_observed_at_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Invalid observed_at timestamp.",
        None,
    )
    .with_extra(
        "errors",
        json!({"observed_at": ["must be an ISO8601 timestamp"]}),
    )
}

/// Build the future observed-at validation problem.
pub fn future_observed_at_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "observed_at is too far in the future.",
        None,
    )
    .with_extra(
        "errors",
        json!({"observed_at": ["must not be more than 5 minutes in the future"]}),
    )
}

/// Build a generic 404 problem.
pub fn not_found_problem(detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new(404, ProblemCode::NotFound, detail, None)
}

/// Build the default payload-too-large problem for router paths without a request ID.
pub fn payload_too_large_problem(detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new(413, ProblemCode::PayloadTooLarge, detail, None)
}

/// Build the query window validation problem.
pub fn invalid_window_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        canary_core::query::INVALID_WINDOW_DETAIL,
        None,
    )
    .with_extra(
        "errors",
        json!({"window": [canary_core::query::INVALID_WINDOW_FIELD_ERROR]}),
    )
}

/// Build the query endpoint validation problem for missing grouping inputs.
pub fn missing_query_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Provide 'service', 'error_class', or 'group_by=error_class' parameter.",
        None,
    )
}

/// Build the standard paginated-query limit validation problem.
pub fn invalid_limit_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Invalid limit. Expected a positive integer up to 200.",
        None,
    )
    .with_extra(
        "errors",
        json!({"limit": ["must be a positive integer no greater than 200"]}),
    )
}

/// Build the standard paginated-query cursor validation problem.
pub fn invalid_cursor_problem() -> ProblemDetails {
    ProblemDetails::new(422, ProblemCode::ValidationError, "Invalid cursor.", None).with_extra(
        "errors",
        json!({"cursor": ["must be a valid pagination cursor"]}),
    )
}

/// Build the invalid annotation validation problem.
pub fn invalid_annotation_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Invalid annotation.",
        None,
    )
}

/// Build the missing annotation subject validation problem.
pub fn annotation_missing_subject_problem(field: &str) -> ProblemDetails {
    let mut errors = Map::new();
    errors.insert(field.to_owned(), json!(["is required"]));
    ProblemDetails::new(422, ProblemCode::ValidationError, "Missing subject.", None)
        .with_extra("errors", Value::Object(errors))
}

/// Build the invalid annotation subject-type validation problem.
pub fn invalid_annotation_subject_type_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Unknown subject_type.",
        None,
    )
    .with_extra(
        "errors",
        json!({"subject_type": ["must be one of incident, error_group, target, monitor"]}),
    )
}

/// Build the annotation limit validation problem.
pub fn invalid_annotation_limit_problem() -> ProblemDetails {
    ProblemDetails::new(422, ProblemCode::ValidationError, "Invalid limit.", None).with_extra(
        "errors",
        json!({"limit": ["must be an integer between 1 and 50"]}),
    )
}

/// Build the annotation cursor validation problem.
pub fn annotation_invalid_cursor_problem() -> ProblemDetails {
    ProblemDetails::new(422, ProblemCode::ValidationError, "Invalid cursor.", None)
        .with_extra("errors", json!({"cursor": ["is invalid"]}))
}

/// Build the timeline event-type validation problem.
pub fn invalid_event_type_problem(invalid: &[String], allowed: &[&str]) -> ProblemDetails {
    let allowed = allowed.join(", ");
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        format!(
            "Invalid event_type: {}. Allowed: {allowed}",
            invalid.join(", ")
        ),
        None,
    )
    .with_extra(
        "errors",
        json!({"event_type": [format!("must be one or more of: {allowed}")]}),
    )
}

/// Build the webhook-delivery status validation problem.
pub fn invalid_webhook_delivery_status_problem(allowed: &[&str]) -> ProblemDetails {
    let allowed = allowed.join(", ");
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        format!("Invalid status. Allowed: {allowed}"),
        None,
    )
    .with_extra(
        "errors",
        json!({"status": [format!("must be one of: {allowed}")]}),
    )
}

/// Build the query parameter type validation problem.
pub fn invalid_string_param_problem(param: &str) -> ProblemDetails {
    let mut errors = Map::new();
    errors.insert(param.to_owned(), json!(["must be a string"]));

    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        format!("Invalid {param} parameter. Must be a string."),
        None,
    )
    .with_extra("errors", Value::Object(errors))
}

/// Build the report limit validation problem.
pub fn invalid_report_limit_problem() -> ProblemDetails {
    ProblemDetails::new(
        422,
        ProblemCode::ValidationError,
        "Invalid limit. Expected a positive integer.",
        None,
    )
    .with_extra("errors", json!({"limit": ["must be a positive integer"]}))
}

/// Build the report cursor validation problem.
pub fn invalid_report_cursor_problem() -> ProblemDetails {
    ProblemDetails::new(422, ProblemCode::ValidationError, "Invalid cursor.", None).with_extra(
        "errors",
        json!({"cursor": ["must be a valid pagination cursor"]}),
    )
}

/// Build the target checks window validation problem.
pub fn target_checks_window_problem() -> ProblemDetails {
    ProblemDetails::new(422, ProblemCode::ValidationError, "Invalid window.", None)
}

/// Build the generic internal error problem.
pub fn internal_problem() -> ProblemDetails {
    ProblemDetails::new(
        500,
        ProblemCode::InternalError,
        "An unexpected error occurred.",
        None,
    )
}

fn to_json_value(value: impl Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
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
    use std::collections::BTreeMap;

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

    #[test]
    fn validation_factories_preserve_phoenix_details_and_errors() {
        let mut errors = BTreeMap::new();
        errors.insert("name".to_owned(), vec!["can't be blank".to_owned()]);

        let cases = [
            (
                serde_json::to_value(validation_problem(
                    "Request body has invalid fields.",
                    errors.clone(),
                ))
                .unwrap_or(Value::Null),
                "Request body has invalid fields.",
            ),
            (
                serde_json::to_value(validation_problem(
                    "Invalid check-in payload.",
                    errors.clone(),
                ))
                .unwrap_or(Value::Null),
                "Invalid check-in payload.",
            ),
            (
                serde_json::to_value(validation_problem(
                    "Invalid monitor configuration.",
                    errors.clone(),
                ))
                .unwrap_or(Value::Null),
                "Invalid monitor configuration.",
            ),
            (
                serde_json::to_value(validation_problem(
                    "Invalid API key request.",
                    errors.clone(),
                ))
                .unwrap_or(Value::Null),
                "Invalid API key request.",
            ),
            (
                serde_json::to_value(validation_problem(
                    "Invalid target configuration.",
                    errors.clone(),
                ))
                .unwrap_or(Value::Null),
                "Invalid target configuration.",
            ),
            (
                serde_json::to_value(validation_problem(
                    "Invalid service onboarding request.",
                    errors,
                ))
                .unwrap_or(Value::Null),
                "Invalid service onboarding request.",
            ),
        ];

        for (encoded, detail) in cases {
            assert_eq!(
                encoded["type"],
                "https://canary.dev/problems/validation-error"
            );
            assert_eq!(encoded["title"], "Validation Error");
            assert_eq!(encoded["status"], 422);
            assert_eq!(encoded["detail"], detail);
            assert_eq!(encoded["code"], "validation_error");
            assert_eq!(encoded["errors"]["name"], json!(["can't be blank"]));
            assert!(encoded["request_id"].is_null());
        }
    }

    #[test]
    fn query_and_annotation_problem_factories_preserve_wire_shape() {
        let event = serde_json::to_value(invalid_event_type_problem(
            &["bad.event".to_owned()],
            &["error.new_class", "canary.ping"],
        ))
        .unwrap_or(Value::Null);
        assert_eq!(
            event["detail"],
            "Invalid event_type: bad.event. Allowed: error.new_class, canary.ping"
        );
        assert_eq!(
            event["errors"]["event_type"],
            json!(["must be one or more of: error.new_class, canary.ping"])
        );

        let delivery_status = serde_json::to_value(invalid_webhook_delivery_status_problem(&[
            "pending",
            "delivered",
        ]))
        .unwrap_or(Value::Null);
        assert_eq!(
            delivery_status["detail"],
            "Invalid status. Allowed: pending, delivered"
        );
        assert_eq!(
            delivery_status["errors"]["status"],
            json!(["must be one of: pending, delivered"])
        );

        let missing_subject =
            serde_json::to_value(annotation_missing_subject_problem("subject_id"))
                .unwrap_or(Value::Null);
        assert_eq!(missing_subject["detail"], "Missing subject.");
        assert_eq!(
            missing_subject["errors"]["subject_id"],
            json!(["is required"])
        );

        let report_limit =
            serde_json::to_value(invalid_report_limit_problem()).unwrap_or(Value::Null);
        assert_eq!(
            report_limit["detail"],
            "Invalid limit. Expected a positive integer."
        );
        assert_eq!(
            report_limit["errors"]["limit"],
            json!(["must be a positive integer"])
        );
    }

    #[test]
    fn operational_problem_factories_preserve_status_codes() {
        let not_found =
            serde_json::to_value(not_found_problem("Target not found.")).unwrap_or(Value::Null);
        assert_eq!(not_found["status"], 404);
        assert_eq!(not_found["code"], "not_found");
        assert_eq!(not_found["detail"], "Target not found.");

        let payload = serde_json::to_value(payload_too_large_problem(
            "Request body exceeds 100KB limit.",
        ))
        .unwrap_or(Value::Null);
        assert_eq!(payload["status"], 413);
        assert_eq!(payload["code"], "payload_too_large");

        let webhook = serde_json::to_value(webhook_delivery_failed_problem("HTTP 500"))
            .unwrap_or(Value::Null);
        assert_eq!(webhook["status"], 502);
        assert_eq!(webhook["code"], "webhook_delivery_failed");
        assert_eq!(webhook["detail"], "Webhook test delivery failed: HTTP 500");

        let internal = serde_json::to_value(internal_problem()).unwrap_or(Value::Null);
        assert_eq!(internal["status"], 500);
        assert_eq!(internal["code"], "internal_error");
        assert_eq!(internal["detail"], "An unexpected error occurred.");
    }
}
