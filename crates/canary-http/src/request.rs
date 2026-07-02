//! Request-body contracts shared by HTTP adapters.

use serde_json::{Map, Value};

use crate::problem_details::{ProblemCode, ProblemDetails};

/// JSON request body limit.
pub const MAX_JSON_BODY_BYTES: u64 = 102_400;

/// Result type for request decoding helpers.
pub type RequestResult<T> = std::result::Result<T, Box<ProblemDetails>>;

/// Decode a JSON request body that must be an object.
pub fn decode_json_object(
    body: &[u8],
    request_id: Option<String>,
) -> RequestResult<Map<String, Value>> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(attrs)) => Ok(attrs),
        Ok(_) | Err(_) => Err(Box::new(ProblemDetails::new(
            400,
            ProblemCode::InvalidRequest,
            "Request body must be a JSON object.",
            request_id,
        ))),
    }
}

/// Build the Problem Details response used when a request body is too large.
pub fn payload_too_large_problem(
    detail: impl Into<String>,
    request_id: Option<String>,
) -> ProblemDetails {
    ProblemDetails::new(413, ProblemCode::PayloadTooLarge, detail, request_id)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn decode_json_object_accepts_objects_only() {
        assert!(decode_json_object(br#"{"service":"api"}"#, None).is_ok());

        let malformed = decode_json_object(b"{", Some("req-1".to_owned()))
            .err()
            .map(|problem| *problem);
        assert_eq!(
            malformed.map(|problem| json!(problem)),
            Some(json!({
                "type": "https://canary.dev/problems/invalid-request",
                "title": "Invalid Request",
                "status": 400,
                "detail": "Request body must be a JSON object.",
                "code": "invalid_request",
                "request_id": "req-1"
            }))
        );

        let non_object = decode_json_object(br#"[]"#, None)
            .err()
            .map(|problem| problem.code);
        assert_eq!(non_object.as_deref(), Some("invalid_request"));
    }
}
