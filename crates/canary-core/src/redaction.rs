//! Shared redaction vocabulary for Canary-controlled payloads.
//!
//! This is a deliberately small pattern floor. It catches common credentials
//! and PII-shaped values before Canary repeats context to agent consumers; it
//! is not a full DLP engine.

use std::{borrow::Cow, sync::LazyLock};

use regex::Regex;
use serde_json::Value;

/// Replacement used when a value is sensitive by field name.
pub const REDACTED: &str = "[REDACTED]";

/// Stable names advertised by responder context envelopes for this redaction floor.
pub const REDACTION_RULE_NAMES: [&str; 9] = [
    "bearer_token",
    "canary_api_key",
    "jwt",
    "aws_access_key",
    "private_key_block",
    "provider_token",
    "credential_database_uri",
    "email",
    "sensitive_key_value",
];

static REDACTION_RULES: LazyLock<std::result::Result<Vec<(Regex, &'static str)>, regex::Error>> =
    LazyLock::new(|| {
        Ok(vec![
            (
                Regex::new(
                    r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
                )?,
                "[PRIVATE_KEY]",
            ),
            (
                Regex::new(
                    r"(?i)\b(postgres(?:ql)?|mysql|mongodb(?:\+srv)?|redis|rediss)://[^\s/@]+@",
                )?,
                "$1://[REDACTED]@",
            ),
            (
                Regex::new(r"\beyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\b")?,
                "[JWT]",
            ),
            (
                Regex::new(r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b")?,
                "[AWS_ACCESS_KEY]",
            ),
            (
                Regex::new(r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b")?,
                "[PROVIDER_TOKEN]",
            ),
            (
                Regex::new(r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b")?,
                "[PROVIDER_TOKEN]",
            ),
            (
                Regex::new(
                    r"\b(?:sk-(?:proj-|ant-(?:api\d+-)?|or-v1-)?[A-Za-z0-9_-]{16,}|AIza[0-9A-Za-z_-]{20,}|hf_[A-Za-z0-9]{20,}|glpat-[A-Za-z0-9_-]{20,}|npm_[A-Za-z0-9]{20,})\b",
                )?,
                "[PROVIDER_TOKEN]",
            ),
            (
                Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]+")?,
                "Bearer [REDACTED]",
            ),
            (
                Regex::new(r"\bsk_(?:live|test)_[A-Za-z0-9_=-]+")?,
                "[CANARY_API_KEY]",
            ),
            (
                Regex::new(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b")?,
                "[EMAIL]",
            ),
            (
                Regex::new(
                    r#"(?i)\b(password|secret|token|api[_-]?key)\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'&,;]+)"#,
                )?,
                "$1=[REDACTED]",
            ),
        ])
    });

/// Scrub common credential and PII-shaped strings.
pub fn scrub_string(value: &str) -> String {
    let Ok(rules) = REDACTION_RULES.as_ref() else {
        return REDACTED.to_owned();
    };
    let mut scrubbed = Cow::Borrowed(value);
    for (pattern, replacement) in rules {
        if pattern.is_match(&scrubbed) {
            scrubbed = Cow::Owned(pattern.replace_all(&scrubbed, *replacement).into_owned());
        }
    }
    scrubbed.into_owned()
}

/// Scrub all strings in a JSON value and replace sensitive-key values.
pub fn scrub_value(value: &Value) -> Value {
    match value {
        Value::String(value) => Value::String(scrub_string(value)),
        Value::Array(values) => Value::Array(values.iter().map(scrub_value).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let value = if sensitive_key(key) {
                        Value::String(REDACTED.to_owned())
                    } else {
                        scrub_value(value)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        value => value.clone(),
    }
}

/// Return true when a JSON/object key conventionally carries secret material.
pub fn sensitive_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().replace(['-', ' '], "_").as_str(),
        "authorization"
            | "cookie"
            | "set_cookie"
            | "password"
            | "passwd"
            | "secret"
            | "token"
            | "api_key"
            | "apikey"
            | "access_token"
            | "refresh_token"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn scrub_string_redacts_common_credential_shapes() {
        let input = "alice@example.com Authorization: Bearer abc.def token=sk_live_secret";

        let scrubbed = scrub_string(input);

        assert_eq!(
            scrubbed,
            "[EMAIL] Authorization: Bearer [REDACTED] token=[REDACTED]"
        );
    }

    #[test]
    fn scrub_value_redacts_sensitive_keys_recursively() {
        let value = json!({
            "authorization": "Bearer context-secret",
            "nested": {
                "email": "bob@example.com",
                "api_key": "sk_live_nested_secret"
            }
        });

        let scrubbed = scrub_value(&value);

        assert_eq!(scrubbed["authorization"], REDACTED);
        assert_eq!(scrubbed["nested"]["email"], "[EMAIL]");
        assert_eq!(scrubbed["nested"]["api_key"], REDACTED);
    }

    #[test]
    fn scrub_string_redacts_supported_secret_corpus_without_passthrough() {
        let secrets = [
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.signaturevalue",
            "AKIAIOSFODNN7EXAMPLE",
            "-----BEGIN PRIVATE KEY-----\nsecret-material\n-----END PRIVATE KEY-----",
            "ghp_0123456789abcdefghijklmnopqrstuv",
            "gho_0123456789abcdefghijklmnopqrstuv",
            "ghu_0123456789abcdefghijklmnopqrstuv",
            "ghs_0123456789abcdefghijklmnopqrstuv",
            "ghr_0123456789abcdefghijklmnopqrstuv",
            "github_pat_0123456789_abcdefghijklmnopqrstuv",
            concat!("xox", "b-123456789012-abcdefghijklmnop"),
            concat!("xox", "a-123456789012-abcdefghijklmnop"),
            concat!("xox", "p-123456789012-abcdefghijklmnop"),
            concat!("xox", "r-123456789012-abcdefghijklmnop"),
            concat!("xox", "s-123456789012-abcdefghijklmnop"),
            "sk-0123456789abcdefghijklmnop",
            "sk-proj-0123456789abcdefghijklmnop",
            "sk-ant-0123456789abcdefghijklmnop",
            "sk-ant-api03-0123456789abcdefghijklmnop",
            "sk-or-v1-0123456789abcdefghijklmnop",
            "AIza0123456789abcdefghijklmnopqrst",
            "hf_0123456789abcdefghijklmnopqrst",
            "glpat-0123456789abcdefghijklmnopqrst",
            "npm_0123456789abcdefghijklmnopqrst",
            "postgresql://canary:hunter2@db.internal/canary",
        ];

        for secret in secrets {
            let scrubbed = scrub_string(secret);
            assert_ne!(scrubbed, secret, "secret shape passed through: {secret}");
            assert!(
                !scrubbed.contains(secret),
                "secret shape remained in scrubbed output: {secret}"
            );
            assert_eq!(
                scrub_string(&scrubbed),
                scrubbed,
                "redaction is not idempotent"
            );
        }
    }
}
