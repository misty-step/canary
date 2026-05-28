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
/// Header name for the Canary event type.
pub const HEADER_EVENT: &str = "x-event";
/// Header name for the stable delivery id.
pub const HEADER_DELIVERY_ID: &str = "x-delivery-id";
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

/// Outbound webhook headers in the same order as the Phoenix worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookHeaders {
    /// `content-type`.
    pub content_type: &'static str,
    /// `x-signature`.
    pub signature: String,
    /// `x-event`.
    pub event: String,
    /// `x-delivery-id`.
    pub delivery_id: String,
    /// `x-webhook-version`.
    pub webhook_version: &'static str,
    /// `x-sequence`.
    pub sequence: String,
}

impl WebhookHeaders {
    /// Return headers as lowercase wire-name/value pairs.
    pub fn as_pairs(&self) -> [(&'static str, &str); 6] {
        [
            (HEADER_CONTENT_TYPE, self.content_type),
            (HEADER_SIGNATURE, self.signature.as_str()),
            (HEADER_EVENT, self.event.as_str()),
            (HEADER_DELIVERY_ID, self.delivery_id.as_str()),
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

/// Build the outbound webhook header set for already-encoded body bytes.
pub fn headers_for_body(
    body: impl AsRef<[u8]>,
    secret: impl AsRef<[u8]>,
    event: impl Into<String>,
    delivery_id: impl Into<String>,
    sequence: Option<u64>,
) -> WebhookHeaders {
    WebhookHeaders {
        content_type: APPLICATION_JSON,
        signature: signature_header(body, secret),
        event: event.into(),
        delivery_id: delivery_id.into(),
        webhook_version: WEBHOOK_VERSION,
        sequence: sequence.unwrap_or(0).to_string(),
    }
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
    fn headers_match_phoenix_worker_wire_shape() {
        let headers = headers_for_body(
            BODY,
            SECRET,
            "error.new_class",
            "DLV-test-delivery",
            Some(42),
        );

        assert_eq!(
            headers.as_pairs(),
            [
                (HEADER_CONTENT_TYPE, APPLICATION_JSON),
                (HEADER_SIGNATURE, PHOENIX_SIGNATURE_HEADER),
                (HEADER_EVENT, "error.new_class"),
                (HEADER_DELIVERY_ID, "DLV-test-delivery"),
                (HEADER_WEBHOOK_VERSION, WEBHOOK_VERSION),
                (HEADER_SEQUENCE, "42"),
            ]
        );
    }

    #[test]
    fn headers_default_missing_sequence_to_zero() {
        let headers = headers_for_body(BODY, SECRET, "error.new_class", "DLV-test-delivery", None);

        assert_eq!(headers.sequence, "0");
        assert_eq!(headers.as_pairs()[5], (HEADER_SEQUENCE, "0"));
    }

    #[test]
    fn retry_attempts_keep_signature_and_delivery_id_stable() {
        let first = headers_for_body(BODY, SECRET, "error.new_class", "DLV-stable", Some(7));
        let second = headers_for_body(BODY, SECRET, "error.new_class", "DLV-stable", Some(7));

        assert_eq!(first.signature, second.signature);
        assert_eq!(first.delivery_id, second.delivery_id);
    }
}
