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

static REDACTION_RULES: LazyLock<std::result::Result<Vec<(Regex, &'static str)>, regex::Error>> =
    LazyLock::new(|| {
        Ok(vec![
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
}
