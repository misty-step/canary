//! Target probe request configuration validation.
//!
//! Admin routes and probe execution both need the same target URL, method, and
//! configured-header policy. This module owns that request-shape validation so
//! target probing can focus on SSRF resolution, transport, persistence, and
//! health transitions.

use std::collections::BTreeMap;

use reqwest::{
    Url,
    header::{HeaderName, HeaderValue},
};
use serde_json::Value;

const MAX_TARGET_HEADERS: usize = 64;
const MAX_TARGET_HEADER_BYTES: usize = 8 * 1024;
const FORBIDDEN_TARGET_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "expect",
    "host",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

pub(crate) fn validate_url(raw_url: &str) -> Result<Url, String> {
    let url = Url::parse(raw_url).map_err(|error| format!("invalid target URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("target URL scheme must be http or https".to_owned());
    }
    if url.host_str().is_none() {
        return Err("target URL must include a host".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("target URL must not include credentials".to_owned());
    }
    Ok(url)
}

pub(crate) fn validate_method(method: &str) -> Result<&str, String> {
    match method {
        "GET" | "HEAD" => Ok(method),
        _ => Err(format!("unsupported target probe method: {method}")),
    }
}

pub(crate) fn parse_headers(headers: Option<&str>) -> Result<BTreeMap<String, String>, String> {
    let Some(headers) = headers else {
        return Ok(BTreeMap::new());
    };
    let value: Value = serde_json::from_str(headers)
        .map_err(|error| format!("invalid target headers JSON: {error}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| "target headers must be a JSON object".to_owned())?;
    if object.len() > MAX_TARGET_HEADERS {
        return Err(format!(
            "target headers exceed {MAX_TARGET_HEADERS} configured entries"
        ));
    }
    let mut parsed = BTreeMap::new();
    let mut serialized_bytes = 0_usize;
    for (name, value) in object {
        let Some(value) = value.as_str() else {
            return Err(format!("target header {name} must be a string"));
        };
        let normalized_name = validate_header_name(name)?;
        validate_header_value(&normalized_name, value)?;
        serialized_bytes = serialized_bytes
            .saturating_add(normalized_name.len())
            .saturating_add(2)
            .saturating_add(value.len())
            .saturating_add(2);
        if serialized_bytes > MAX_TARGET_HEADER_BYTES {
            return Err(format!(
                "target headers exceed {MAX_TARGET_HEADER_BYTES} serialized bytes"
            ));
        }
        if parsed
            .insert(normalized_name.clone(), value.to_owned())
            .is_some()
        {
            return Err(format!(
                "duplicate target header {normalized_name} after case normalization"
            ));
        }
    }
    Ok(parsed)
}

fn validate_header_name(name: &str) -> Result<String, String> {
    let header_name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| format!("invalid target header name: {name}"))?;
    let normalized = header_name.as_str().to_owned();
    if FORBIDDEN_TARGET_HEADERS.contains(&normalized.as_str()) {
        return Err(format!(
            "target header {normalized} is managed by Canary probe transport"
        ));
    }
    Ok(normalized)
}

fn validate_header_value(name: &str, value: &str) -> Result<(), String> {
    HeaderValue::from_str(value).map_err(|_| format!("invalid value for target header {name}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn configured_headers_are_validated_and_normalized() -> Result<(), Box<dyn Error>> {
        let headers = parse_headers(Some(
            r#"{"X-Canary-Probe":"true","Authorization":"Bearer health-token"}"#,
        ))?;

        assert_eq!(
            headers
                .get("x-canary-probe")
                .ok_or("missing normalized custom header")?,
            "true"
        );
        assert_eq!(
            headers
                .get("authorization")
                .ok_or("missing normalized authorization header")?,
            "Bearer health-token"
        );
        Ok(())
    }

    #[test]
    fn configured_headers_reject_transport_owned_or_malformed_values() -> Result<(), Box<dyn Error>>
    {
        for (headers, expected) in [
            (
                r#"{"Host":"evil.example"}"#,
                "target header host is managed by Canary probe transport",
            ),
            (
                r#"{"Content-Length":"100"}"#,
                "target header content-length is managed by Canary probe transport",
            ),
            (
                r#"{"Expect":"100-continue"}"#,
                "target header expect is managed by Canary probe transport",
            ),
            (
                r#"{"Proxy-Connection":"keep-alive"}"#,
                "target header proxy-connection is managed by Canary probe transport",
            ),
            (
                r#"{"Bad Header":"value"}"#,
                "invalid target header name: Bad Header",
            ),
            (
                r#"{"X-Canary":"bad\r\nsplit"}"#,
                "invalid value for target header x-canary",
            ),
            (
                r#"{"X-Canary":"one","x-canary":"two"}"#,
                "duplicate target header x-canary after case normalization",
            ),
        ] {
            assert_parse_header_error(headers, expected)?;
        }
        Ok(())
    }

    #[test]
    fn configured_headers_reject_unbounded_count_or_size() -> Result<(), Box<dyn Error>> {
        let too_many = (0..=MAX_TARGET_HEADERS)
            .map(|index| format!(r#""X-Test-{index}":"ok""#))
            .collect::<Vec<_>>()
            .join(",");
        let too_many_json = format!("{{{too_many}}}");
        assert_parse_header_error(
            &too_many_json,
            "target headers exceed 64 configured entries",
        )?;

        let oversized = format!(r#"{{"X-Large":"{}"}}"#, "x".repeat(MAX_TARGET_HEADER_BYTES));
        assert_parse_header_error(&oversized, "target headers exceed 8192 serialized bytes")?;
        Ok(())
    }

    fn assert_parse_header_error(headers: &str, expected: &str) -> Result<(), Box<dyn Error>> {
        match parse_headers(Some(headers)) {
            Ok(parsed) => Err(format!("expected {headers} to fail, parsed {parsed:?}").into()),
            Err(error) => {
                assert_eq!(error, expected);
                Ok(())
            }
        }
    }
}
