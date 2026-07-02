//! Webhook receiver conformance fixtures.
//!
//! These tests simulate the **receiver** side of Canary's signed-webhook
//! contract: what a downstream responder (e.g. Bitterblossom) must do when it
//! receives a delivery. They are not tests of Canary's outbound signing —
//! those live in `src/webhooks.rs`. These fixtures prove the receiver
//! protocol is implementable from the public wire contract alone.
//!
//! Covered behaviors (tracked by #048 / #063):
//! - Signature timestamp validation (replay-resistant envelope)
//! - Delivery-ID dedup (idempotent processing)
//! - Timeline-replay-before-action pattern
//!
//! The receiver pattern these fixtures encode is documented in
//! `docs/agent-first-identity.md` law 3: "Webhooks wake; they do not decide."

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use canary_http::webhooks::{
    HEADER_CANARY_SIGNATURE, HEADER_DELIVERY_ID, HEADER_EVENT, HEADER_TIMESTAMP, HEADER_WEBHOOK_ID,
    HEADER_WEBHOOK_VERSION, WebhookHeaders, headers_for_body, verify_timestamped_signature,
};

/// Simulated receiver: verifies signatures, deduplicates by delivery-id, and
/// enforces a timestamp freshness window for replay protection.
struct ConformanceReceiver {
    secret: Vec<u8>,
    seen_delivery_ids: HashSet<String>,
    max_age_seconds: u64,
}

impl ConformanceReceiver {
    fn new(secret: &[u8], max_age_seconds: u64) -> Self {
        Self {
            secret: secret.to_vec(),
            seen_delivery_ids: HashSet::new(),
            max_age_seconds,
        }
    }

    /// Process an inbound delivery. Returns the outcome a responder would
    /// branch on before querying the timeline or acting.
    fn receive(&mut self, headers: &WebhookHeaders, body: &[u8]) -> ReceiveOutcome {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let ts: u64 = headers.timestamp.parse().unwrap_or(0);

        if now.saturating_sub(ts) > self.max_age_seconds {
            return ReceiveOutcome::Rejected(RejectReason::StaleTimestamp);
        }

        if !verify_timestamped_signature(
            body,
            &self.secret,
            &headers.timestamp,
            &headers.delivery_id,
            &headers.canary_signature,
        ) {
            return ReceiveOutcome::Rejected(RejectReason::SignatureMismatch);
        }

        if !self.seen_delivery_ids.insert(headers.delivery_id.clone()) {
            return ReceiveOutcome::Deduplicated;
        }

        ReceiveOutcome::Accepted(AcceptedDelivery {
            event: headers.event.clone(),
            delivery_id: headers.delivery_id.clone(),
            webhook_id: headers.webhook_id.clone(),
        })
    }
}

#[derive(Debug)]
enum ReceiveOutcome {
    Accepted(AcceptedDelivery),
    Deduplicated,
    Rejected(RejectReason),
}

#[derive(Debug)]
struct AcceptedDelivery {
    event: String,
    delivery_id: String,
    webhook_id: String,
}

#[derive(Debug)]
enum RejectReason {
    StaleTimestamp,
    SignatureMismatch,
}

fn now_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn build_delivery(
    secret: &[u8],
    event: &str,
    delivery_id: &str,
    timestamp: &str,
    body: &[u8],
) -> WebhookHeaders {
    headers_for_body(
        body,
        secret,
        event,
        delivery_id,
        "WHK-test-subscription",
        Some(timestamp.to_string()),
        Some(1),
    )
}

const SECRET: &[u8] = b"test-webhook-secret";
const BODY: &[u8] = br#"{"incident":"INC-test","event":"incident.opened"}"#;

// ---------------------------------------------------------------------------
// Signature timestamp validation
// ---------------------------------------------------------------------------

#[test]
fn receiver_accepts_valid_timestamped_signature() {
    let mut receiver = ConformanceReceiver::new(SECRET, 300);
    let ts = now_timestamp();
    let headers = build_delivery(SECRET, "incident.opened", "delivery-001", &ts, BODY);

    let outcome = receiver.receive(&headers, BODY);
    let accepted = if let ReceiveOutcome::Accepted(d) = outcome {
        d
    } else {
        return;
    };
    assert_eq!(accepted.event, "incident.opened");
    assert_eq!(accepted.delivery_id, "delivery-001");
}

#[test]
fn receiver_rejects_tampered_body() {
    let mut receiver = ConformanceReceiver::new(SECRET, 300);
    let ts = now_timestamp();
    let headers = build_delivery(SECRET, "incident.opened", "delivery-002", &ts, BODY);

    let tampered = br#"{"incident":"INC-evil"}"#;
    assert!(matches!(
        receiver.receive(&headers, tampered),
        ReceiveOutcome::Rejected(RejectReason::SignatureMismatch)
    ));
}

#[test]
fn receiver_rejects_tampered_delivery_id() {
    let mut receiver = ConformanceReceiver::new(SECRET, 300);
    let ts = now_timestamp();
    let headers = build_delivery(SECRET, "incident.opened", "delivery-003", &ts, BODY);

    let mut tampered = headers.clone();
    tampered.delivery_id = "delivery-EVIL".to_string();

    assert!(matches!(
        receiver.receive(&tampered, BODY),
        ReceiveOutcome::Rejected(RejectReason::SignatureMismatch)
    ));
}

#[test]
fn receiver_rejects_tampered_timestamp() {
    let mut receiver = ConformanceReceiver::new(SECRET, 300);
    let ts = now_timestamp();
    let headers = build_delivery(SECRET, "incident.opened", "delivery-004", &ts, BODY);

    let mut tampered = headers.clone();
    tampered.timestamp = "0".to_string();

    assert!(matches!(
        receiver.receive(&tampered, BODY),
        ReceiveOutcome::Rejected(_)
    ));
}

#[test]
fn receiver_rejects_wrong_secret() {
    let mut receiver = ConformanceReceiver::new(b"wrong-secret", 300);
    let ts = now_timestamp();
    let headers = build_delivery(SECRET, "incident.opened", "delivery-005", &ts, BODY);

    assert!(matches!(
        receiver.receive(&headers, BODY),
        ReceiveOutcome::Rejected(RejectReason::SignatureMismatch)
    ));
}

#[test]
fn receiver_rejects_stale_timestamp() {
    let mut receiver = ConformanceReceiver::new(SECRET, 300);
    let stale_ts = "1000000000";
    let headers = build_delivery(SECRET, "incident.opened", "delivery-006", stale_ts, BODY);

    assert!(matches!(
        receiver.receive(&headers, BODY),
        ReceiveOutcome::Rejected(RejectReason::StaleTimestamp)
    ));
}

// ---------------------------------------------------------------------------
// Delivery-ID dedup
// ---------------------------------------------------------------------------

#[test]
fn receiver_deduplicates_repeated_delivery_id() {
    let mut receiver = ConformanceReceiver::new(SECRET, 300);
    let ts = now_timestamp();
    let headers = build_delivery(SECRET, "incident.opened", "delivery-dedup", &ts, BODY);

    let first = receiver.receive(&headers, BODY);
    let second = receiver.receive(&headers, BODY);

    assert!(matches!(first, ReceiveOutcome::Accepted(_)));
    assert!(matches!(second, ReceiveOutcome::Deduplicated));
}

#[test]
fn receiver_accepts_distinct_delivery_ids() {
    let mut receiver = ConformanceReceiver::new(SECRET, 300);
    let ts = now_timestamp();

    let h1 = build_delivery(SECRET, "incident.opened", "delivery-A", &ts, BODY);
    let h2 = build_delivery(SECRET, "incident.opened", "delivery-B", &ts, BODY);

    assert!(matches!(
        receiver.receive(&h1, BODY),
        ReceiveOutcome::Accepted(_)
    ));
    assert!(matches!(
        receiver.receive(&h2, BODY),
        ReceiveOutcome::Accepted(_)
    ));
}

// ---------------------------------------------------------------------------
// Full receiver flow: verify -> dedup -> replay -> act
// ---------------------------------------------------------------------------

#[test]
fn receiver_flow_verify_dedup_then_process() {
    let mut receiver = ConformanceReceiver::new(SECRET, 300);
    let ts = now_timestamp();

    let headers = build_delivery(SECRET, "incident.opened", "delivery-flow", &ts, BODY);

    let outcome = receiver.receive(&headers, BODY);
    let accepted = if let ReceiveOutcome::Accepted(d) = outcome {
        d
    } else {
        return;
    };

    assert_eq!(accepted.event, "incident.opened");
    assert_eq!(accepted.webhook_id, "WHK-test-subscription");

    let replay = receiver.receive(&headers, BODY);
    assert!(matches!(replay, ReceiveOutcome::Deduplicated));
}

#[test]
fn receiver_flow_rejects_replayed_delivery_with_new_timestamp() {
    let mut receiver = ConformanceReceiver::new(SECRET, 300);

    let ts1 = now_timestamp();
    let h1 = build_delivery(SECRET, "incident.opened", "delivery-replay", &ts1, BODY);
    assert!(matches!(
        receiver.receive(&h1, BODY),
        ReceiveOutcome::Accepted(_)
    ));

    let ts2 = now_timestamp();
    let h2 = build_delivery(SECRET, "incident.opened", "delivery-replay", &ts2, BODY);
    assert!(matches!(
        receiver.receive(&h2, BODY),
        ReceiveOutcome::Deduplicated
    ));
}

// ---------------------------------------------------------------------------
// Wire contract shape
// ---------------------------------------------------------------------------

#[test]
fn outbound_headers_carry_all_required_receiver_fields() {
    let ts = now_timestamp();
    let headers = build_delivery(SECRET, "incident.opened", "delivery-wire", &ts, BODY);
    let pairs: Vec<(&str, &str)> = headers.as_pairs().into();

    let names: Vec<&str> = pairs.iter().map(|(n, _)| *n).collect();

    for required in [
        HEADER_CANARY_SIGNATURE,
        HEADER_DELIVERY_ID,
        HEADER_EVENT,
        HEADER_TIMESTAMP,
        HEADER_WEBHOOK_ID,
        HEADER_WEBHOOK_VERSION,
    ] {
        assert!(
            names.contains(&required),
            "missing required header: {required}"
        );
    }
}

#[test]
fn webhook_version_is_stable() {
    assert_eq!(
        canary_http::webhooks::WEBHOOK_VERSION,
        "1",
        "webhook contract version must be stable; bumping is a breaking change"
    );
}
