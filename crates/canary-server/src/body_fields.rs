//! Shared JSON object field decoding for request bodies.
//!
//! Route modules should express endpoint-specific policy. This module owns the
//! repeated scalar rules: missing/null defaults, positive
//! integer validation text, required non-empty strings, string arrays, and
//! optional scalar extraction from decoded JSON objects.

use canary_ingest::ValidationErrors;
use serde_json::{Map, Value};

pub(crate) fn optional_positive_i64(
    attrs: &Map<String, Value>,
    key: &str,
    default: i64,
    errors: &mut ValidationErrors,
) -> i64 {
    match attrs.get(key) {
        Some(Value::Number(number)) => match number.as_i64().filter(|value| *value > 0) {
            Some(value) => value,
            None => {
                errors.insert(key.to_owned(), vec!["must be greater than 0".to_owned()]);
                default
            }
        },
        Some(Value::Null) | None => default,
        Some(_) => {
            errors.insert(key.to_owned(), vec!["must be an integer".to_owned()]);
            default
        }
    }
}

pub(crate) fn optional_positive_u32(
    attrs: &Map<String, Value>,
    key: &str,
    default: u32,
    errors: &mut ValidationErrors,
) -> u32 {
    match attrs.get(key) {
        Some(Value::Number(number)) => match number.as_u64().and_then(|value| {
            if value > 0 {
                u32::try_from(value).ok()
            } else {
                None
            }
        }) {
            Some(value) => value,
            None => {
                errors.insert(key.to_owned(), vec!["must be greater than 0".to_owned()]);
                default
            }
        },
        Some(Value::Null) | None => default,
        Some(_) => {
            errors.insert(key.to_owned(), vec!["must be an integer".to_owned()]);
            default
        }
    }
}

pub(crate) fn required_string(
    attrs: &Map<String, Value>,
    key: &str,
    errors: &mut ValidationErrors,
) -> Option<String> {
    match attrs.get(key) {
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        _ => {
            errors.insert(
                key.to_owned(),
                vec!["must be a non-empty string".to_owned()],
            );
            None
        }
    }
}

pub(crate) fn required_string_array(
    attrs: &Map<String, Value>,
    key: &str,
    errors: &mut ValidationErrors,
) -> Option<Vec<String>> {
    match attrs.get(key) {
        Some(Value::Array(values)) => {
            let mut strings = Vec::new();
            for (index, value) in values.iter().enumerate() {
                match value {
                    Value::String(event) if !event.is_empty() => strings.push(event.clone()),
                    _ => {
                        errors.insert(
                            format!("{key}.{index}"),
                            vec!["must be a non-empty string".to_owned()],
                        );
                    }
                }
            }
            if errors
                .keys()
                .any(|field| field.starts_with(&format!("{key}.")))
            {
                None
            } else {
                Some(strings)
            }
        }
        _ => {
            errors.insert(key.to_owned(), vec!["must be an array".to_owned()]);
            None
        }
    }
}

pub(crate) fn optional_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn object(value: Value) -> Result<Map<String, Value>, String> {
        match value {
            Value::Object(object) => Ok(object),
            _ => Err("fixture is not an object".to_owned()),
        }
    }

    #[test]
    fn optional_positive_numbers_default_on_missing_or_null_and_record_errors() -> Result<(), String>
    {
        let attrs = object(json!({
            "valid_i64": 42,
            "valid_u32": 7,
            "nullish": null,
            "zero": 0,
            "negative": -1,
            "float": 1.5,
            "too_big": u64::from(u32::MAX) + 1,
            "text": "10"
        }))?;
        let mut errors = ValidationErrors::new();

        assert_eq!(
            optional_positive_i64(&attrs, "valid_i64", 60_000, &mut errors),
            42
        );
        assert_eq!(
            optional_positive_u32(&attrs, "valid_u32", 3, &mut errors),
            7
        );
        assert_eq!(
            optional_positive_i64(&attrs, "missing", 60_000, &mut errors),
            60_000
        );
        assert_eq!(
            optional_positive_i64(&attrs, "nullish", 60_000, &mut errors),
            60_000
        );
        assert_eq!(
            optional_positive_i64(&attrs, "zero", 60_000, &mut errors),
            60_000
        );
        assert_eq!(
            optional_positive_i64(&attrs, "negative", 60_000, &mut errors),
            60_000
        );
        assert_eq!(
            optional_positive_i64(&attrs, "float", 60_000, &mut errors),
            60_000
        );
        assert_eq!(optional_positive_u32(&attrs, "too_big", 3, &mut errors), 3);
        assert_eq!(optional_positive_u32(&attrs, "text", 3, &mut errors), 3);

        assert_eq!(errors["zero"], vec!["must be greater than 0"]);
        assert_eq!(errors["negative"], vec!["must be greater than 0"]);
        assert_eq!(errors["float"], vec!["must be greater than 0"]);
        assert_eq!(errors["too_big"], vec!["must be greater than 0"]);
        assert_eq!(errors["text"], vec!["must be an integer"]);
        assert!(!errors.contains_key("missing"));
        assert!(!errors.contains_key("nullish"));
        Ok(())
    }

    #[test]
    fn required_strings_and_arrays_preserve_nested_error_keys() -> Result<(), String> {
        let attrs = object(json!({
            "name": "target",
            "events": ["error.created", "", 1],
            "missing_array": "not-array"
        }))?;
        let mut errors = ValidationErrors::new();

        assert_eq!(
            required_string(&attrs, "name", &mut errors),
            Some("target".to_owned())
        );
        assert_eq!(required_string(&attrs, "blank", &mut errors), None);
        assert_eq!(required_string_array(&attrs, "events", &mut errors), None);
        assert_eq!(
            required_string_array(&attrs, "missing_array", &mut errors),
            None
        );

        assert_eq!(errors["blank"], vec!["must be a non-empty string"]);
        assert_eq!(errors["events.1"], vec!["must be a non-empty string"]);
        assert_eq!(errors["events.2"], vec!["must be a non-empty string"]);
        assert_eq!(errors["missing_array"], vec!["must be an array"]);
        Ok(())
    }

    #[test]
    fn optional_string_accepts_only_non_empty_strings() -> Result<(), String> {
        let attrs = object(json!({
            "string": "svc",
            "empty": ""
        }))?;

        assert_eq!(optional_string(attrs.get("string")), Some("svc".to_owned()));
        assert_eq!(optional_string(attrs.get("empty")), None);
        assert_eq!(optional_string(attrs.get("missing")), None);
        Ok(())
    }
}
