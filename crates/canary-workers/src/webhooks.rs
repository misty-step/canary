//! Webhook delivery decisions.
//!
//! The Phoenix worker couples Oban, Req, ETS circuit breakers, cooldowns, and
//! the delivery ledger in one module. The Rust rewrite keeps the product
//! decisions here and lets the runtime provide persistence and transport.

use std::convert::Infallible;

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
    /// RFC3339 timestamp for this delivery attempt, supplied by runtimes with clocks.
    pub attempt_timestamp: Option<String>,
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

/// One scheduler enqueue decision for a subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookEnqueueDecision {
    /// Create a pending ledger row and schedule this delivery job.
    Schedule {
        /// Ledger row to create before scheduling.
        delivery: PlannedWebhookDelivery,
        /// Job arguments for the scheduler/runtime.
        job: WebhookJob,
        /// Cooldown key to mark after the scheduler accepts the job.
        cooldown_key: String,
    },
    /// Create a suppressed ledger row and do not schedule a job.
    Suppress {
        /// Ledger row to create as suppressed.
        delivery: PlannedWebhookDelivery,
        /// Suppression reason.
        reason: String,
    },
}

/// Minimal delivery ledger data produced by webhook enqueue planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedWebhookDelivery {
    /// Stable delivery id.
    pub delivery_id: String,
    /// Webhook subscription id.
    pub webhook_id: String,
    /// Event name.
    pub event: String,
}

/// Subscription lookup result for a scheduled delivery job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookLookup {
    /// No webhook row exists for the job's subscription id.
    Missing,
    /// A webhook row exists but is inactive.
    Inactive(WebhookEndpoint),
    /// A webhook row exists and is active.
    Active(WebhookEndpoint),
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

/// Per-subscription circuit-breaker decision supplied by the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitDecision {
    /// The circuit is closed and delivery may proceed normally.
    Closed,
    /// The circuit is open and this attempt should be suppressed.
    Open,
    /// The circuit is open but this attempt is the scheduled probe.
    Probe,
}

/// Circuit state update requested after one delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitEffect {
    /// No circuit state update is needed.
    None,
    /// Record a successful delivery for this webhook.
    RecordSuccess {
        /// Webhook id.
        webhook_id: String,
    },
    /// Record a failed delivery for this webhook.
    RecordFailure {
        /// Webhook id.
        webhook_id: String,
    },
}

/// Ledger update requested by delivery execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryLedgerAction {
    /// Ensure the delivery exists as pending.
    CreatePending(PlannedWebhookDelivery),
    /// Mark a delivery as attempted.
    MarkAttempt {
        /// Stable delivery id.
        delivery_id: String,
    },
    /// Mark a delivery as delivered.
    MarkDelivered {
        /// Stable delivery id.
        delivery_id: String,
    },
    /// Mark a delivery as discarded.
    MarkDiscarded {
        /// Stable delivery id.
        delivery_id: String,
        /// Discard reason.
        reason: String,
    },
    /// Mark a delivery as suppressed.
    CreateSuppressed {
        /// Ledger row to upsert.
        delivery: PlannedWebhookDelivery,
        /// Suppression reason.
        reason: String,
    },
}

/// Result of executing one scheduled webhook job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryExecution {
    /// Stable delivery id used on the wire and in the ledger.
    pub delivery_id: String,
    /// Product outcome for the scheduler.
    pub outcome: DeliveryOutcome,
    /// Ordered ledger updates for the runtime to persist.
    pub ledger_actions: Vec<DeliveryLedgerAction>,
    /// Circuit-breaker update requested by the attempt.
    pub circuit_effect: CircuitEffect,
    /// Backoff in seconds for retryable failures.
    pub retry_after_seconds: Option<u64>,
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
        endpoint.id.clone(),
        job.attempt_timestamp
            .clone()
            .or_else(|| timestamp(&job.payload)),
        sequence(&job.payload),
    );

    Some(WebhookRequest {
        url: endpoint.url.clone(),
        body,
        headers,
    })
}

/// Execute one scheduled delivery job through injected lookup, circuit, and transport.
///
/// Runtime code owns endpoint lookup, circuit state, HTTP transport, persistence,
/// and job rescheduling. This function owns the Phoenix product contract:
/// stable delivery ids, pending rows before decisions, attempt marking before
/// transport, final discard reasons, and circuit suppression.
pub fn execute_delivery(
    job: &WebhookJob,
    lookup: WebhookLookup,
    circuit: CircuitDecision,
    mut record_ledger: impl FnMut(DeliveryLedgerAction),
    transport: impl FnMut(WebhookRequest) -> TransportResult,
) -> DeliveryExecution {
    match try_execute_delivery(
        job,
        lookup,
        circuit,
        |action| {
            record_ledger(action);
            Ok::<(), Infallible>(())
        },
        transport,
    ) {
        Ok(execution) => execution,
        Err(error) => match error {},
    }
}

/// Execute one scheduled delivery job with fallible ledger persistence.
///
/// Runtimes should use this variant so a failed pending or attempt ledger write
/// stops execution before an outbound request can be sent.
pub fn try_execute_delivery<E>(
    job: &WebhookJob,
    lookup: WebhookLookup,
    circuit: CircuitDecision,
    mut record_ledger: impl FnMut(DeliveryLedgerAction) -> Result<(), E>,
    transport: impl FnMut(WebhookRequest) -> TransportResult,
) -> Result<DeliveryExecution, E> {
    let delivery_id = delivery_id_from_job(job);
    let delivery = PlannedWebhookDelivery {
        delivery_id: delivery_id.clone(),
        webhook_id: job.webhook_id.clone(),
        event: job.event.clone(),
    };
    let mut ledger_actions = Vec::new();
    let mut push_ledger = |action: DeliveryLedgerAction| {
        record_ledger(action.clone())?;
        ledger_actions.push(action);
        Ok(())
    };
    push_ledger(DeliveryLedgerAction::CreatePending(delivery.clone()))?;

    Ok(match lookup {
        WebhookLookup::Missing => {
            push_ledger(DeliveryLedgerAction::MarkDiscarded {
                delivery_id: delivery_id.clone(),
                reason: "webhook_not_found".to_owned(),
            })?;
            DeliveryExecution {
                delivery_id,
                outcome: DeliveryOutcome::Discarded {
                    reason: "webhook_not_found".to_owned(),
                },
                ledger_actions,
                circuit_effect: CircuitEffect::None,
                retry_after_seconds: None,
            }
        }
        WebhookLookup::Inactive(_) => {
            push_ledger(DeliveryLedgerAction::MarkDiscarded {
                delivery_id: delivery_id.clone(),
                reason: "webhook_inactive".to_owned(),
            })?;
            DeliveryExecution {
                delivery_id,
                outcome: DeliveryOutcome::Discarded {
                    reason: "webhook_inactive".to_owned(),
                },
                ledger_actions,
                circuit_effect: CircuitEffect::None,
                retry_after_seconds: None,
            }
        }
        WebhookLookup::Active(endpoint) => {
            if circuit == CircuitDecision::Open {
                push_ledger(DeliveryLedgerAction::CreateSuppressed {
                    delivery,
                    reason: "circuit_open".to_owned(),
                })?;
                return Ok(DeliveryExecution {
                    delivery_id,
                    outcome: DeliveryOutcome::Suppressed {
                        reason: "circuit_open".to_owned(),
                    },
                    ledger_actions,
                    circuit_effect: CircuitEffect::None,
                    retry_after_seconds: None,
                });
            }

            push_ledger(DeliveryLedgerAction::MarkAttempt {
                delivery_id: delivery_id.clone(),
            })?;

            let outcome = build_request(&endpoint, job).map(transport).map_or_else(
                || DeliveryOutcome::Discarded {
                    reason: "webhook_inactive".to_owned(),
                },
                |result| classify_result(job, result),
            );

            let circuit_effect = match outcome {
                DeliveryOutcome::Delivered => {
                    push_ledger(DeliveryLedgerAction::MarkDelivered {
                        delivery_id: delivery_id.clone(),
                    })?;
                    CircuitEffect::RecordSuccess {
                        webhook_id: endpoint.id,
                    }
                }
                DeliveryOutcome::Retry { .. } => CircuitEffect::RecordFailure {
                    webhook_id: endpoint.id,
                },
                DeliveryOutcome::Discarded { ref reason } => {
                    push_ledger(DeliveryLedgerAction::MarkDiscarded {
                        delivery_id: delivery_id.clone(),
                        reason: reason.clone(),
                    })?;
                    CircuitEffect::RecordFailure {
                        webhook_id: endpoint.id,
                    }
                }
                DeliveryOutcome::Suppressed { .. } => CircuitEffect::None,
            };
            let retry_after_seconds = match outcome {
                DeliveryOutcome::Retry { .. } => Some(backoff_seconds(job.effective_attempt())),
                _ => None,
            };

            DeliveryExecution {
                delivery_id,
                outcome,
                ledger_actions,
                circuit_effect,
                retry_after_seconds,
            }
        }
    })
}

/// Plan enqueue work for one event across active subscriptions.
///
/// This function deliberately does not mutate cooldown state or schedule work.
/// The runtime must first persist the returned ledger decision, then schedule
/// the returned job, then mark cooldown only after scheduling succeeds.
pub fn plan_enqueue_for_event<I, F>(
    event: &str,
    payload: &Value,
    endpoints: I,
    mut next_delivery_id: F,
    mut in_cooldown: impl FnMut(&str) -> bool,
) -> Vec<WebhookEnqueueDecision>
where
    I: IntoIterator<Item = WebhookEndpoint>,
    F: FnMut() -> String,
{
    endpoints
        .into_iter()
        .filter(|endpoint| endpoint.active)
        .map(|endpoint| {
            let delivery_id = next_delivery_id();
            let cooldown_key = cooldown_key(&endpoint.id, event, payload);
            let delivery = PlannedWebhookDelivery {
                delivery_id: delivery_id.clone(),
                webhook_id: endpoint.id.clone(),
                event: event.to_owned(),
            };

            if in_cooldown(&cooldown_key) {
                WebhookEnqueueDecision::Suppress {
                    delivery,
                    reason: "cooldown".to_owned(),
                }
            } else {
                WebhookEnqueueDecision::Schedule {
                    delivery,
                    job: WebhookJob {
                        webhook_id: endpoint.id,
                        payload: payload.clone(),
                        event: event.to_owned(),
                        delivery_id: Some(delivery_id),
                        legacy_job_id: None,
                        attempt: 1,
                        max_attempts: MAX_ATTEMPTS,
                        attempt_timestamp: None,
                    },
                    cooldown_key,
                }
            }
        })
        .collect()
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

fn timestamp(payload: &Value) -> Option<String> {
    payload
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_owned)
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
            entries.sort_by_key(|(left, _)| *left);
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
    use canary_http::webhooks::{
        HEADER_DELIVERY_ID, HEADER_EVENT, HEADER_SEQUENCE, HEADER_TIMESTAMP, HEADER_WEBHOOK_ID,
    };
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
            headers: canary_http::webhooks::headers_for_body("", "", "", "", "", None, None),
        });
        assert_eq!(request.url, "https://example.test/hook");
        assert_eq!(
            request.headers.as_pairs()[3],
            (HEADER_EVENT, "error.new_class")
        );
        assert_eq!(
            request.headers.as_pairs()[4],
            (HEADER_DELIVERY_ID, "DLV-stable")
        );
        assert_eq!(
            request.headers.as_pairs()[5],
            (HEADER_WEBHOOK_ID, "WHK-123456789abc")
        );
        assert_eq!(
            request.headers.as_pairs()[6],
            (HEADER_TIMESTAMP, "2026-05-28T20:00:00Z")
        );
        assert_eq!(request.headers.as_pairs()[8], (HEADER_SEQUENCE, "7"));
    }

    #[test]
    fn inactive_webhook_does_not_build_request() {
        assert!(build_request(&endpoint(false), &job(None, 1, 4)).is_none());
    }

    #[test]
    fn enqueue_plan_creates_pending_jobs_and_cooldown_suppressions() {
        let payload = json!({
            "event": "error.new_class",
            "error": {"group_hash": "grp-stable"},
            "sequence": 7
        });
        let decisions = plan_enqueue_for_event(
            "error.new_class",
            &payload,
            [
                endpoint(true),
                WebhookEndpoint {
                    id: "WHK-cooldown".to_owned(),
                    url: "https://example.test/quiet".to_owned(),
                    secret: "secret".to_owned(),
                    active: true,
                },
            ],
            {
                let mut ids = ["DLV-first", "DLV-second"].into_iter();
                move || ids.next().unwrap_or("DLV-extra").to_owned()
            },
            |key| key.starts_with("WHK-cooldown:"),
        );

        assert_eq!(decisions.len(), 2);
        assert!(matches!(
            &decisions[0],
            WebhookEnqueueDecision::Schedule { delivery, job, cooldown_key }
                if delivery.delivery_id == "DLV-first"
                    && delivery.webhook_id == "WHK-123456789abc"
                    && job.delivery_id.as_deref() == Some("DLV-first")
                    && job.attempt == 1
                    && job.max_attempts == MAX_ATTEMPTS
                    && cooldown_key.ends_with("error_group:grp-stable")
        ));
        assert!(matches!(
            &decisions[1],
            WebhookEnqueueDecision::Suppress { delivery, reason }
                if delivery.delivery_id == "DLV-second"
                    && delivery.webhook_id == "WHK-cooldown"
                    && reason == "cooldown"
        ));
    }

    #[test]
    fn executor_delivers_and_requests_success_ledger_and_circuit_updates() {
        let job = job(Some("DLV-stable".to_owned()), 1, 4);
        let mut recorded_actions = Vec::new();
        let execution = execute_delivery(
            &job,
            WebhookLookup::Active(endpoint(true)),
            CircuitDecision::Closed,
            |action| recorded_actions.push(action),
            |request| {
                assert_eq!(request.headers.delivery_id, "DLV-stable");
                TransportResult::HttpStatus(204)
            },
        );

        assert_eq!(execution.delivery_id, "DLV-stable");
        assert_eq!(execution.outcome, DeliveryOutcome::Delivered);
        assert_eq!(
            execution.ledger_actions,
            vec![
                DeliveryLedgerAction::CreatePending(PlannedWebhookDelivery {
                    delivery_id: "DLV-stable".to_owned(),
                    webhook_id: "WHK-123456789abc".to_owned(),
                    event: "error.new_class".to_owned(),
                }),
                DeliveryLedgerAction::MarkAttempt {
                    delivery_id: "DLV-stable".to_owned(),
                },
                DeliveryLedgerAction::MarkDelivered {
                    delivery_id: "DLV-stable".to_owned(),
                },
            ]
        );
        assert_eq!(recorded_actions, execution.ledger_actions);
        assert_eq!(
            execution.circuit_effect,
            CircuitEffect::RecordSuccess {
                webhook_id: "WHK-123456789abc".to_owned()
            }
        );
        assert_eq!(execution.retry_after_seconds, None);
    }

    #[test]
    fn executor_retries_non_final_failures_with_backoff() {
        let mut recorded_actions = Vec::new();
        let execution = execute_delivery(
            &job(Some("DLV-retry".to_owned()), 2, 4),
            WebhookLookup::Active(endpoint(true)),
            CircuitDecision::Closed,
            |action| recorded_actions.push(action),
            |_| TransportResult::HttpStatus(500),
        );

        assert_eq!(
            execution.outcome,
            DeliveryOutcome::Retry {
                reason: "HTTP 500".to_owned()
            }
        );
        assert_eq!(
            execution.ledger_actions,
            vec![
                DeliveryLedgerAction::CreatePending(PlannedWebhookDelivery {
                    delivery_id: "DLV-retry".to_owned(),
                    webhook_id: "WHK-123456789abc".to_owned(),
                    event: "error.new_class".to_owned(),
                }),
                DeliveryLedgerAction::MarkAttempt {
                    delivery_id: "DLV-retry".to_owned(),
                },
            ]
        );
        assert_eq!(recorded_actions, execution.ledger_actions);
        assert_eq!(
            execution.circuit_effect,
            CircuitEffect::RecordFailure {
                webhook_id: "WHK-123456789abc".to_owned()
            }
        );
        assert_eq!(execution.retry_after_seconds, Some(5));
    }

    #[test]
    fn executor_discards_final_http_and_request_failures() {
        let mut http_actions = Vec::new();
        let http = execute_delivery(
            &job(Some("DLV-http".to_owned()), 4, 4),
            WebhookLookup::Active(endpoint(true)),
            CircuitDecision::Closed,
            |action| http_actions.push(action),
            |_| TransportResult::HttpStatus(500),
        );
        assert_eq!(
            http.outcome,
            DeliveryOutcome::Discarded {
                reason: "http_500".to_owned()
            }
        );
        assert!(http.ledger_actions.iter().any(|action| matches!(
            action,
            DeliveryLedgerAction::MarkDiscarded { delivery_id, reason }
                if delivery_id == "DLV-http" && reason == "http_500"
        )));
        assert_eq!(http_actions, http.ledger_actions);

        let mut request_actions = Vec::new();
        let request = execute_delivery(
            &job(Some("DLV-request".to_owned()), 4, 4),
            WebhookLookup::Active(endpoint(true)),
            CircuitDecision::Probe,
            |action| request_actions.push(action),
            |_| TransportResult::RequestError("connection refused".to_owned()),
        );
        assert_eq!(
            request.outcome,
            DeliveryOutcome::Discarded {
                reason: "request_error".to_owned()
            }
        );
        assert!(request.ledger_actions.iter().any(|action| matches!(
            action,
            DeliveryLedgerAction::MarkDiscarded { delivery_id, reason }
                if delivery_id == "DLV-request" && reason == "request_error"
        )));
        assert_eq!(request_actions, request.ledger_actions);
    }

    #[test]
    fn executor_suppresses_open_circuit_without_transport_or_attempt() {
        let mut called = false;
        let mut recorded_actions = Vec::new();
        let execution = execute_delivery(
            &job(Some("DLV-open".to_owned()), 1, 4),
            WebhookLookup::Active(endpoint(true)),
            CircuitDecision::Open,
            |action| recorded_actions.push(action),
            |_| {
                called = true;
                TransportResult::HttpStatus(204)
            },
        );

        assert!(!called);
        assert_eq!(
            execution.outcome,
            DeliveryOutcome::Suppressed {
                reason: "circuit_open".to_owned()
            }
        );
        assert_eq!(
            execution.ledger_actions,
            vec![
                DeliveryLedgerAction::CreatePending(PlannedWebhookDelivery {
                    delivery_id: "DLV-open".to_owned(),
                    webhook_id: "WHK-123456789abc".to_owned(),
                    event: "error.new_class".to_owned(),
                }),
                DeliveryLedgerAction::CreateSuppressed {
                    delivery: PlannedWebhookDelivery {
                        delivery_id: "DLV-open".to_owned(),
                        webhook_id: "WHK-123456789abc".to_owned(),
                        event: "error.new_class".to_owned(),
                    },
                    reason: "circuit_open".to_owned(),
                },
            ]
        );
        assert_eq!(recorded_actions, execution.ledger_actions);
        assert_eq!(execution.circuit_effect, CircuitEffect::None);
    }

    #[test]
    fn executor_discards_missing_or_inactive_webhooks_without_transport() {
        let mut missing_actions = Vec::new();
        let missing = execute_delivery(
            &job(Some("DLV-missing".to_owned()), 1, 4),
            WebhookLookup::Missing,
            CircuitDecision::Closed,
            |action| missing_actions.push(action),
            |_| TransportResult::HttpStatus(204),
        );
        assert_eq!(
            missing.outcome,
            DeliveryOutcome::Discarded {
                reason: "webhook_not_found".to_owned()
            }
        );
        assert_eq!(missing_actions, missing.ledger_actions);

        let mut inactive_actions = Vec::new();
        let inactive = execute_delivery(
            &job(Some("DLV-inactive".to_owned()), 1, 4),
            WebhookLookup::Inactive(endpoint(false)),
            CircuitDecision::Closed,
            |action| inactive_actions.push(action),
            |_| TransportResult::HttpStatus(204),
        );
        assert_eq!(
            inactive.outcome,
            DeliveryOutcome::Discarded {
                reason: "webhook_inactive".to_owned()
            }
        );
        assert_eq!(inactive_actions, inactive.ledger_actions);
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
                "timestamp": "2026-05-28T20:00:00Z",
                "sequence": 7,
                "error": {"group_hash": "grp-stable"}
            }),
            event: "error.new_class".to_owned(),
            delivery_id,
            legacy_job_id: None,
            attempt,
            max_attempts,
            attempt_timestamp: None,
        }
    }
}
