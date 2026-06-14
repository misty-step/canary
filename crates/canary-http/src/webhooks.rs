//! Webhook signing and outbound header contracts.
//!
//! The delivery worker will own retries, circuit breaking, and persistence.
//! This module owns only the wire contract that responders verify.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Header name for the JSON content type.
pub const HEADER_CONTENT_TYPE: &str = "content-type";
/// Header name for the HMAC signature.
pub const HEADER_SIGNATURE: &str = "x-signature";
/// Header name for the timestamp-bound Canary HMAC signature.
pub const HEADER_CANARY_SIGNATURE: &str = "x-canary-signature";
/// Header name for the Canary event type.
pub const HEADER_EVENT: &str = "x-event";
/// Header name for the stable delivery id.
pub const HEADER_DELIVERY_ID: &str = "x-delivery-id";
/// Header name for the webhook subscription id used as signing-key id.
pub const HEADER_WEBHOOK_ID: &str = "x-webhook-id";
/// Header name for the signed event timestamp.
pub const HEADER_TIMESTAMP: &str = "x-timestamp";
/// Header name for the webhook contract version.
pub const HEADER_WEBHOOK_VERSION: &str = "x-webhook-version";
/// Header name for the payload sequence.
pub const HEADER_SEQUENCE: &str = "x-sequence";

/// JSON content type sent by the Phoenix webhook worker.
pub const APPLICATION_JSON: &str = "application/json";
/// Current outbound webhook contract version.
pub const WEBHOOK_VERSION: &str = "1";
/// Signature header prefix.
pub const SIGNATURE_PREFIX: &str = "sha256=";

/// Outbound webhook headers in stable wire order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookHeaders {
    /// `content-type`.
    pub content_type: &'static str,
    /// Legacy `x-signature`.
    pub signature: String,
    /// `x-canary-signature`.
    pub canary_signature: String,
    /// `x-event`.
    pub event: String,
    /// `x-delivery-id`.
    pub delivery_id: String,
    /// `x-webhook-id`.
    pub webhook_id: String,
    /// `x-timestamp`.
    pub timestamp: String,
    /// `x-webhook-version`.
    pub webhook_version: &'static str,
    /// `x-sequence`.
    pub sequence: String,
}

impl WebhookHeaders {
    /// Return headers as lowercase wire-name/value pairs.
    pub fn as_pairs(&self) -> [(&'static str, &str); 9] {
        [
            (HEADER_CONTENT_TYPE, self.content_type),
            (HEADER_SIGNATURE, self.signature.as_str()),
            (HEADER_CANARY_SIGNATURE, self.canary_signature.as_str()),
            (HEADER_EVENT, self.event.as_str()),
            (HEADER_DELIVERY_ID, self.delivery_id.as_str()),
            (HEADER_WEBHOOK_ID, self.webhook_id.as_str()),
            (HEADER_TIMESTAMP, self.timestamp.as_str()),
            (HEADER_WEBHOOK_VERSION, self.webhook_version),
            (HEADER_SEQUENCE, self.sequence.as_str()),
        ]
    }
}

/// Sign exact outbound body bytes with HMAC-SHA256.
pub fn sign(body: impl AsRef<[u8]>, secret: impl AsRef<[u8]>) -> String {
    match hmac_digest(body.as_ref(), secret.as_ref()) {
        Some(digest) => encode_hex_lower(&digest),
        None => String::new(),
    }
}

/// Build a Phoenix-compatible `x-signature` value.
pub fn signature_header(body: impl AsRef<[u8]>, secret: impl AsRef<[u8]>) -> String {
    format!("{}{}", SIGNATURE_PREFIX, sign(body, secret))
}

/// Verify a Phoenix-compatible `x-signature` value.
pub fn verify_signature(body: impl AsRef<[u8]>, secret: impl AsRef<[u8]>, signature: &str) -> bool {
    let Some(hex) = signature.strip_prefix(SIGNATURE_PREFIX) else {
        return false;
    };
    let Some(expected) = decode_hex_sha256(hex) else {
        return false;
    };
    let Some(mut mac) = new_hmac(secret.as_ref()) else {
        return false;
    };

    mac.update(body.as_ref());
    mac.verify_slice(&expected).is_ok()
}

/// Sign a replay-resistant webhook envelope: `timestamp.delivery_id.body`.
pub fn sign_timestamped(
    body: impl AsRef<[u8]>,
    secret: impl AsRef<[u8]>,
    timestamp: &str,
    delivery_id: &str,
) -> String {
    match hmac_digest(
        &timestamped_signature_base(body.as_ref(), timestamp, delivery_id),
        secret.as_ref(),
    ) {
        Some(digest) => encode_hex_lower(&digest),
        None => String::new(),
    }
}

/// Build a timestamp-bound `x-canary-signature` value.
pub fn timestamped_signature_header(
    body: impl AsRef<[u8]>,
    secret: impl AsRef<[u8]>,
    timestamp: &str,
    delivery_id: &str,
) -> String {
    format!(
        "{}{}",
        SIGNATURE_PREFIX,
        sign_timestamped(body, secret, timestamp, delivery_id)
    )
}

/// Verify a timestamp-bound `x-canary-signature` value.
pub fn verify_timestamped_signature(
    body: impl AsRef<[u8]>,
    secret: impl AsRef<[u8]>,
    timestamp: &str,
    delivery_id: &str,
    signature: &str,
) -> bool {
    let Some(hex) = signature.strip_prefix(SIGNATURE_PREFIX) else {
        return false;
    };
    let Some(expected) = decode_hex_sha256(hex) else {
        return false;
    };
    let Some(mut mac) = new_hmac(secret.as_ref()) else {
        return false;
    };

    mac.update(&timestamped_signature_base(
        body.as_ref(),
        timestamp,
        delivery_id,
    ));
    mac.verify_slice(&expected).is_ok()
}

/// Build the outbound webhook header set for already-encoded body bytes.
pub fn headers_for_body(
    body: impl AsRef<[u8]>,
    secret: impl AsRef<[u8]>,
    event: impl Into<String>,
    delivery_id: impl Into<String>,
    webhook_id: impl Into<String>,
    timestamp: Option<String>,
    sequence: Option<u64>,
) -> WebhookHeaders {
    let body = body.as_ref();
    let secret = secret.as_ref();
    let delivery_id = delivery_id.into();
    let timestamp = timestamp.unwrap_or_default();
    WebhookHeaders {
        content_type: APPLICATION_JSON,
        signature: signature_header(body, secret),
        canary_signature: timestamped_signature_header(body, secret, &timestamp, &delivery_id),
        event: event.into(),
        delivery_id,
        webhook_id: webhook_id.into(),
        timestamp,
        webhook_version: WEBHOOK_VERSION,
        sequence: sequence.unwrap_or(0).to_string(),
    }
}

fn timestamped_signature_base(body: &[u8], timestamp: &str, delivery_id: &str) -> Vec<u8> {
    let mut base = Vec::with_capacity(timestamp.len() + delivery_id.len() + body.len() + 2);
    base.extend_from_slice(timestamp.as_bytes());
    base.push(b'.');
    base.extend_from_slice(delivery_id.as_bytes());
    base.push(b'.');
    base.extend_from_slice(body);
    base
}

fn hmac_digest(body: &[u8], secret: &[u8]) -> Option<[u8; 32]> {
    let mut mac = new_hmac(secret)?;
    mac.update(body);
    Some(mac.finalize().into_bytes().into())
}

fn new_hmac(secret: &[u8]) -> Option<HmacSha256> {
    HmacSha256::new_from_slice(secret).ok()
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

fn decode_hex_sha256(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }

    let mut out = [0u8; 32];
    for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        out[index] = (high << 4) | low;
    }

    Some(out)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{"event":"error.new_class","data":"test"}"#;
    const SECRET: &str = "test-webhook-secret";
    const TIMESTAMP: &str = "2026-05-28T20:00:00Z";
    const PHOENIX_SIGNATURE: &str =
        "fca9576b9dd9dad8ba9cc597354dba19a3a040b4f35f5217358b444b1462f006";
    const PHOENIX_SIGNATURE_HEADER: &str =
        "sha256=fca9576b9dd9dad8ba9cc597354dba19a3a040b4f35f5217358b444b1462f006";

    #[test]
    fn sign_matches_phoenix_hmac_fixture() {
        let signature = sign(BODY, SECRET);

        assert_eq!(signature, PHOENIX_SIGNATURE);
        assert_eq!(signature.len(), 64);
    }

    #[test]
    fn signature_header_matches_phoenix_prefix_and_digest() {
        assert_eq!(signature_header(BODY, SECRET), PHOENIX_SIGNATURE_HEADER);
    }

    #[test]
    fn verify_signature_accepts_valid_header() {
        assert!(verify_signature(BODY, SECRET, PHOENIX_SIGNATURE_HEADER));
    }

    #[test]
    fn verify_signature_rejects_wrong_secret() {
        assert!(!verify_signature(
            BODY,
            "wrong-secret",
            PHOENIX_SIGNATURE_HEADER
        ));
    }

    #[test]
    fn verify_signature_rejects_tampered_body() {
        assert!(!verify_signature(
            format!("{BODY}tampered"),
            SECRET,
            PHOENIX_SIGNATURE_HEADER
        ));
    }

    #[test]
    fn verify_signature_rejects_wrong_prefix_or_bad_hex() {
        assert!(!verify_signature(BODY, SECRET, PHOENIX_SIGNATURE));
        assert!(!verify_signature(BODY, SECRET, "sha1=abc"));
        assert!(!verify_signature(BODY, SECRET, "sha256=not-hex"));
    }

    #[test]
    fn timestamped_signature_binds_timestamp_delivery_id_and_body() {
        let signature = timestamped_signature_header(BODY, SECRET, TIMESTAMP, "DLV-test-delivery");

        assert!(verify_timestamped_signature(
            BODY,
            SECRET,
            TIMESTAMP,
            "DLV-test-delivery",
            &signature
        ));
        assert!(!verify_timestamped_signature(
            BODY,
            SECRET,
            "2026-05-28T20:00:01Z",
            "DLV-test-delivery",
            &signature
        ));
        assert!(!verify_timestamped_signature(
            BODY,
            SECRET,
            TIMESTAMP,
            "DLV-other",
            &signature
        ));
        assert!(!verify_timestamped_signature(
            format!("{BODY}tampered"),
            SECRET,
            TIMESTAMP,
            "DLV-test-delivery",
            &signature
        ));
    }

    #[test]
    fn headers_keep_legacy_signature_and_add_replay_resistant_envelope() {
        let headers = headers_for_body(
            BODY,
            SECRET,
            "error.new_class",
            "DLV-test-delivery",
            "WHK-test-webhook",
            Some(TIMESTAMP.to_owned()),
            Some(42),
        );

        let pairs = headers.as_pairs();
        assert!(pairs.contains(&(HEADER_CONTENT_TYPE, APPLICATION_JSON)));
        assert!(pairs.contains(&(HEADER_SIGNATURE, PHOENIX_SIGNATURE_HEADER)));
        assert!(pairs.contains(&(HEADER_EVENT, "error.new_class")));
        assert!(pairs.contains(&(HEADER_DELIVERY_ID, "DLV-test-delivery")));
        assert!(pairs.contains(&(HEADER_WEBHOOK_ID, "WHK-test-webhook")));
        assert!(pairs.contains(&(HEADER_TIMESTAMP, TIMESTAMP)));
        assert!(pairs.contains(&(HEADER_WEBHOOK_VERSION, WEBHOOK_VERSION)));
        assert!(pairs.contains(&(HEADER_SEQUENCE, "42")));
        assert!(verify_timestamped_signature(
            BODY,
            SECRET,
            TIMESTAMP,
            "DLV-test-delivery",
            &headers.canary_signature
        ));
    }

    #[test]
    fn headers_default_missing_sequence_to_zero() {
        let headers = headers_for_body(
            BODY,
            SECRET,
            "error.new_class",
            "DLV-test-delivery",
            "WHK-test-webhook",
            None,
            None,
        );

        assert_eq!(headers.sequence, "0");
        assert_eq!(headers.timestamp, "");
        assert_eq!(headers.as_pairs()[8], (HEADER_SEQUENCE, "0"));
    }

    #[test]
    fn retry_attempts_keep_signature_and_delivery_id_stable() {
        let first = headers_for_body(
            BODY,
            SECRET,
            "error.new_class",
            "DLV-stable",
            "WHK-test-webhook",
            Some(TIMESTAMP.to_owned()),
            Some(7),
        );
        let second = headers_for_body(
            BODY,
            SECRET,
            "error.new_class",
            "DLV-stable",
            "WHK-test-webhook",
            Some(TIMESTAMP.to_owned()),
            Some(7),
        );

        assert_eq!(first.signature, second.signature);
        assert_eq!(first.canary_signature, second.canary_signature);
        assert_eq!(first.delivery_id, second.delivery_id);
    }
}
