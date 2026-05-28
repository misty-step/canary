//! Webhook delivery decisions.
//!
//! The Phoenix worker couples Oban, Req, ETS circuit breakers, cooldowns, and
//! the delivery ledger in one module. The Rust rewrite keeps the product
//! decisions here and lets the runtime provide persistence and transport.

use canary_http::webhooks::{WebhookHeaders, headers_for_body};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Number of delivery attempts configured by the Phoenix Oban worker.
pub const MAX_ATTEMPTS: u32 = 4;

/// One outbound webhook subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookEndpoint {
    /// Webhook row id.
    pub id: String,
    /// Destination URL.
    pub url: String,
    /// Shared signing secret.
    pub secret: String,
    /// Whether the subscription is active.
    pub active: bool,
}

/// Delivery job arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookJob {
    /// Webhook row id.
    pub webhook_id: String,
    /// Event payload.
    pub payload: Value,
    /// Event name.
    pub event: String,
    /// Stable delivery id carried by modern jobs.
    pub delivery_id: Option<String>,
    /// Legacy Oban job id, used only when no delivery id exists.
    pub legacy_job_id: Option<i64>,
    /// Current one-based attempt.
    pub attempt: u32,
    /// Maximum attempts before final discard.
    pub max_attempts: u32,
}

impl WebhookJob {
    /// Return the effective max-attempt count, matching the Phoenix default.
    pub fn effective_max_attempts(&self) -> u32 {
        if self.max_attempts == 0 {
            MAX_ATTEMPTS
        } else {
            self.max_attempts
        }
    }

    /// Return the current one-based attempt, matching the Phoenix fallback.
    pub fn effective_attempt(&self) -> u32 {
        if self.attempt == 0 { 1 } else { self.attempt }
    }
}

/// Request produced for the HTTP transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookRequest {
    /// Destination URL.
    pub url: String,
    /// Exact JSON body bytes to send and sign.
    pub body: String,
    /// Phoenix-compatible outbound headers.
    pub headers: WebhookHeaders,
}

/// Transport-level response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportResult {
    /// HTTP response with status code.
    HttpStatus(u16),
    /// Request could not be completed.
    RequestError(String),
}

/// Product outcome of one delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// Request succeeded with a 2xx status.
    Delivered,
    /// Request failed but may be retried by the scheduler.
    Retry {
        /// Human-readable reason.
        reason: String,
    },
    /// Request failed on the final attempt.
    Discarded {
        /// Ledger reason.
        reason: String,
    },
    /// Request should not be sent.
    Suppressed {
        /// Ledger reason.
        reason: String,
    },
}

/// Build the request body and headers for a webhook delivery attempt.
pub fn build_request(endpoint: &WebhookEndpoint, job: &WebhookJob) -> Option<WebhookRequest> {
    if !endpoint.active {
        return None;
    }

    let body = serde_json::to_string(&job.payload).ok()?;
    let delivery_id = delivery_id_from_job(job);
    let headers = headers_for_body(
        &body,
        &endpoint.secret,
        job.event.clone(),
        delivery_id,
        sequence(&job.payload),
    );

    Some(WebhookRequest {
        url: endpoint.url.clone(),
        body,
        headers,
    })
}

/// Classify the transport result for ledger updates and retry handling.
pub fn classify_result(job: &WebhookJob, result: TransportResult) -> DeliveryOutcome {
    match result {
        TransportResult::HttpStatus(status) if (200..=299).contains(&status) => {
            DeliveryOutcome::Delivered
        }
        TransportResult::HttpStatus(status) => failed_attempt(job, format!("HTTP {status}")),
        TransportResult::RequestError(reason) => failed_attempt(job, reason),
    }
}

/// Phoenix-compatible retry backoff in seconds.
pub fn backoff_seconds(attempt: u32) -> u64 {
    match attempt {
        1 => 1,
        2 => 5,
        3 => 30,
        _ => 60,
    }
}

/// Resolve the delivery id from modern or legacy job fields.
pub fn delivery_id_from_job(job: &WebhookJob) -> String {
    if let Some(delivery_id) = &job.delivery_id {
        return delivery_id.clone();
    }

    if let Some(job_id) = job.legacy_job_id {
        return format!("DLV-legacy-{job_id}");
    }

    format!("DLV-legacy-{}", stable_args_hash(job))
}

/// Return the cooldown identity for an event payload.
pub fn cooldown_key(webhook_id: &str, event: &str, payload: &Value) -> String {
    let identity = payload_identity(payload).unwrap_or_else(|| "payload".to_owned());
    format!("{webhook_id}:{event}:{identity}")
}

fn failed_attempt(job: &WebhookJob, reason: String) -> DeliveryOutcome {
    if job.effective_attempt() >= job.effective_max_attempts() {
        DeliveryOutcome::Discarded {
            reason: discard_reason(&reason),
        }
    } else {
        DeliveryOutcome::Retry { reason }
    }
}

fn discard_reason(reason: &str) -> String {
    if let Some(status) = reason.strip_prefix("HTTP ") {
        format!("http_{status}")
    } else {
        "request_error".to_owned()
    }
}

fn sequence(payload: &Value) -> Option<u64> {
    payload.get("sequence").and_then(Value::as_u64)
}

fn payload_identity(payload: &Value) -> Option<String> {
    let object = payload.as_object()?;

    object
        .get("error")
        .and_then(|error| error.get("group_hash"))
        .and_then(Value::as_str)
        .map(|group_hash| format!("error_group:{group_hash}"))
        .or_else(|| {
            object
                .get("incident")
                .and_then(|incident| incident.get("id"))
                .and_then(Value::as_str)
                .map(|id| format!("incident:{id}"))
        })
        .or_else(|| {
            object.get("target").and_then(|target| {
                let target = target.as_object()?;
                let service = target.get("service").and_then(Value::as_str).unwrap_or("");
                let name = target.get("name").and_then(Value::as_str).unwrap_or("");
                let url = target.get("url").and_then(Value::as_str).unwrap_or("");
                Some(format!("target:{service}:{name}:{url}"))
            })
        })
        .or_else(|| Some(format!("payload:{}", stable_payload_hash(payload))))
}

fn stable_payload_hash(payload: &Value) -> String {
    let mut scrubbed = payload.clone();
    if let Some(object) = scrubbed.as_object_mut() {
        object.remove("timestamp");
        object.remove("sequence");
    }
    stable_hash(&scrubbed)
}

fn stable_args_hash(job: &WebhookJob) -> String {
    let value = serde_json::json!({
        "event": job.event,
        "payload": job.payload,
        "webhook_id": job.webhook_id,
    });
    stable_hash(&value).chars().take(24).collect()
}

fn stable_hash(value: &Value) -> String {
    let canonical = canonical_json(value);
    let digest = Sha256::digest(canonical.as_bytes());
    encode_hex_lower(&digest)
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_owned()),
        Value::Array(values) => {
            let nested = values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{nested}]")
        }
        Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let nested = entries
                .into_iter()
                .map(|(key, value)| {
                    let key = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_owned());
                    format!("{key}:{}", canonical_json(value))
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{nested}}}")
        }
    }
}

fn encode_hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use canary_http::webhooks::{HEADER_DELIVERY_ID, HEADER_EVENT, HEADER_SEQUENCE};
    use serde_json::json;

    use super::*;

    #[test]
    fn request_uses_stable_delivery_id_and_phoenix_headers() {
        let endpoint = endpoint(true);
        let job = job(Some("DLV-stable".to_owned()), 1, 4);
        let request = build_request(&endpoint, &job);

        assert!(request.is_some());
        let request = request.unwrap_or_else(|| WebhookRequest {
            url: String::new(),
            body: String::new(),
            headers: canary_http::webhooks::headers_for_body("", "", "", "", None),
        });
        assert_eq!(request.url, "https://example.test/hook");
        assert_eq!(
            request.headers.as_pairs()[2],
            (HEADER_EVENT, "error.new_class")
        );
        assert_eq!(
            request.headers.as_pairs()[3],
            (HEADER_DELIVERY_ID, "DLV-stable")
        );
        assert_eq!(request.headers.as_pairs()[5], (HEADER_SEQUENCE, "7"));
    }

    #[test]
    fn inactive_webhook_does_not_build_request() {
        assert!(build_request(&endpoint(false), &job(None, 1, 4)).is_none());
    }

    #[test]
    fn classify_delivery_retry_and_final_discard() {
        assert_eq!(
            classify_result(
                &job(Some("DLV-one".to_owned()), 1, 4),
                TransportResult::HttpStatus(204)
            ),
            DeliveryOutcome::Delivered
        );
        assert_eq!(
            classify_result(
                &job(Some("DLV-one".to_owned()), 2, 4),
                TransportResult::HttpStatus(500)
            ),
            DeliveryOutcome::Retry {
                reason: "HTTP 500".to_owned()
            }
        );
        assert_eq!(
            classify_result(
                &job(Some("DLV-one".to_owned()), 4, 4),
                TransportResult::HttpStatus(500)
            ),
            DeliveryOutcome::Discarded {
                reason: "http_500".to_owned()
            }
        );
        assert_eq!(
            classify_result(
                &job(Some("DLV-one".to_owned()), 4, 4),
                TransportResult::RequestError("connection refused".to_owned())
            ),
            DeliveryOutcome::Discarded {
                reason: "request_error".to_owned()
            }
        );
    }

    #[test]
    fn backoff_matches_phoenix_worker() {
        assert_eq!(backoff_seconds(1), 1);
        assert_eq!(backoff_seconds(2), 5);
        assert_eq!(backoff_seconds(3), 30);
        assert_eq!(backoff_seconds(4), 60);
        assert_eq!(backoff_seconds(5), 60);
    }

    #[test]
    fn legacy_delivery_ids_are_stable() {
        let from_id = WebhookJob {
            legacy_job_id: Some(42),
            ..job(None, 1, 4)
        };
        assert_eq!(delivery_id_from_job(&from_id), "DLV-legacy-42");

        let first = delivery_id_from_job(&job(None, 1, 4));
        let second = delivery_id_from_job(&job(None, 2, 4));
        assert_eq!(first, second);
        assert!(first.starts_with("DLV-legacy-"));
    }

    #[test]
    fn cooldown_identity_keeps_distinct_error_groups_separate() {
        let first = cooldown_key(
            "WHK-one",
            "error.new_class",
            &json!({"error": {"group_hash": "grp-a"}, "timestamp": "2026-05-28T20:00:00Z"}),
        );
        let second = cooldown_key(
            "WHK-one",
            "error.new_class",
            &json!({"error": {"group_hash": "grp-b"}, "timestamp": "2026-05-28T20:00:00Z"}),
        );

        assert_ne!(first, second);
        assert!(first.ends_with("error_group:grp-a"));
    }

    #[test]
    fn fallback_cooldown_identity_is_canonical_and_ignores_timestamp_sequence() {
        let first = cooldown_key(
            "WHK-one",
            "error.new_class",
            &json!({
                "alpha": 1,
                "nested": {"a": 1, "b": 2},
                "timestamp": "2026-05-28T20:00:00Z",
                "sequence": 1
            }),
        );
        let second = cooldown_key(
            "WHK-one",
            "error.new_class",
            &json!({
                "nested": {"b": 2, "a": 1},
                "sequence": 9,
                "timestamp": "2026-05-28T20:05:00Z",
                "alpha": 1
            }),
        );

        assert_eq!(first, second);
    }

    fn endpoint(active: bool) -> WebhookEndpoint {
        WebhookEndpoint {
            id: "WHK-123456789abc".to_owned(),
            url: "https://example.test/hook".to_owned(),
            secret: "test-webhook-secret".to_owned(),
            active,
        }
    }

    fn job(delivery_id: Option<String>, attempt: u32, max_attempts: u32) -> WebhookJob {
        WebhookJob {
            webhook_id: "WHK-123456789abc".to_owned(),
            payload: json!({
                "event": "error.new_class",
                "sequence": 7,
                "error": {"group_hash": "grp-stable"}
            }),
            event: "error.new_class".to_owned(),
            delivery_id,
            legacy_job_id: None,
            attempt,
            max_attempts,
        }
    }
}
