//! Error grouping and message-template normalization.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_STACK_FRAMES: usize = 5;

static NORMALIZATION_RULES: LazyLock<Result<Vec<(Regex, &'static str)>, regex::Error>> =
    LazyLock::new(build_normalization_rules);

fn build_normalization_rules() -> Result<Vec<(Regex, &'static str)>, regex::Error> {
    Ok(vec![
        (
            Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")?,
            "<uuid>",
        ),
        (
            Regex::new(
                r"\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?(?:Z|[+-]\d{2}:?\d{2})?\b",
            )?,
            "<timestamp>",
        ),
        (
            Regex::new(r"\b[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}\b")?,
            "<email>",
        ),
        (Regex::new(r#"(?:^|[\s('"=])(/[^\s'")\]]+)"#)?, "<path>"),
        (Regex::new(r"(?i)\b(?:0x)?[0-9a-f]{9,}\b")?, "<hex>"),
        (Regex::new(r"\b\d{4,}\b")?, "<int>"),
        (Regex::new(r"\s+")?, " "),
    ])
}

/// Input fields needed to compute an error group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupingInput<'a> {
    /// Service name.
    pub service: &'a str,
    /// Error class.
    pub error_class: &'a str,
    /// Original message.
    pub message: &'a str,
    /// Optional stack trace.
    pub stack_trace: Option<&'a str>,
    /// Optional client-provided fingerprint.
    pub fingerprint: Option<&'a [String]>,
    /// In-project module prefixes used for stack fingerprints.
    pub module_prefixes: &'a [String],
    /// In-project path prefixes used for stack fingerprints.
    pub path_prefixes: &'a [String],
}

/// Output of grouping policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grouping {
    /// Stable SHA-256 lowercase hex group hash.
    pub group_hash: String,
    /// Normalized message template.
    pub message_template: String,
    /// Strategy used to compute the group hash.
    pub strategy: GroupingStrategy,
}

/// Strategy used for grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupingStrategy {
    /// Client-provided fingerprint.
    ClientFingerprint,
    /// In-project stack trace frames.
    StackTrace,
    /// Message template fallback.
    MessageTemplate,
}

/// Compute the same grouping priority as the Phoenix service:
/// client fingerprint, stack trace, then normalized message template.
pub fn compute(input: GroupingInput<'_>) -> Grouping {
    let message_template = strip_template(input.message);

    if let Some(fingerprint) = input.fingerprint {
        return Grouping {
            group_hash: fingerprint_hash(input.service, fingerprint),
            message_template,
            strategy: GroupingStrategy::ClientFingerprint,
        };
    }

    if let Some(stack_hash) = stack_trace_hash(&input) {
        return Grouping {
            group_hash: stack_hash,
            message_template,
            strategy: GroupingStrategy::StackTrace,
        };
    }

    Grouping {
        group_hash: template_hash(input.service, input.error_class, &message_template),
        message_template,
        strategy: GroupingStrategy::MessageTemplate,
    }
}

/// Strip unstable values out of an error message.
pub fn strip_template(message: &str) -> String {
    let Ok(rules) = NORMALIZATION_RULES.as_ref() else {
        return message.trim().to_owned();
    };

    let normalized = rules
        .iter()
        .fold(message.to_owned(), |acc, (pattern, replacement)| {
            pattern.replace_all(&acc, *replacement).into_owned()
        });
    normalized.trim().to_owned()
}

fn fingerprint_hash(service: &str, fingerprint: &[String]) -> String {
    let mut input = String::from(service);
    input.push_str(&fingerprint.join(":"));
    sha256_hex(input.as_bytes())
}

fn stack_trace_hash(input: &GroupingInput<'_>) -> Option<String> {
    let stack_trace = input.stack_trace?;
    let frames = extract_in_project_frames(stack_trace, input.module_prefixes, input.path_prefixes);
    if frames.len() < 2 {
        return None;
    }

    let frame_key = frames
        .iter()
        .take(MAX_STACK_FRAMES)
        .map(|frame| strip_line_number(frame))
        .collect::<Vec<_>>()
        .join("|");

    Some(sha256_hex(
        format!("{}{}{}", input.service, input.error_class, frame_key).as_bytes(),
    ))
}

fn template_hash(service: &str, error_class: &str, template: &str) -> String {
    sha256_hex(format!("{service}{error_class}{template}").as_bytes())
}

fn extract_in_project_frames(
    stack_trace: &str,
    module_prefixes: &[String],
    path_prefixes: &[String],
) -> Vec<String> {
    let frames = stack_trace
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    let in_project = frames
        .iter()
        .filter(|frame| {
            module_prefixes.iter().any(|prefix| frame.contains(prefix))
                || path_prefixes.iter().any(|prefix| frame.contains(prefix))
        })
        .cloned()
        .collect::<Vec<_>>();

    if in_project.len() >= 2 {
        in_project
    } else if frames.len() >= 2 {
        frames.into_iter().take(MAX_STACK_FRAMES).collect()
    } else {
        Vec::new()
    }
}

fn strip_line_number(frame: &str) -> String {
    static LINE_NUMBER: LazyLock<Result<Regex, regex::Error>> =
        LazyLock::new(|| Regex::new(r":\d+"));

    match LINE_NUMBER.as_ref() {
        Ok(pattern) => pattern.replace_all(frame, "").into_owned(),
        Err(_) => frame.to_owned(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(message: &'a str) -> GroupingInput<'a> {
        GroupingInput {
            service: "svc",
            error_class: "RuntimeError",
            message,
            stack_trace: None,
            fingerprint: None,
            module_prefixes: &[],
            path_prefixes: &[],
        }
    }

    #[test]
    fn strips_unstable_message_values() {
        assert_eq!(
            strip_template(
                "user test@example.com failed at 2026-05-28T19:00:00Z path /tmp/file id 123456"
            ),
            "user <email> failed at <timestamp> path<path> id <int>"
        );
    }

    #[test]
    fn client_fingerprint_takes_priority() {
        let fingerprint = vec!["route".to_owned(), "handler".to_owned()];
        let result = compute(GroupingInput {
            fingerprint: Some(&fingerprint),
            ..input("boom 1234")
        });

        assert_eq!(result.strategy, GroupingStrategy::ClientFingerprint);
        assert_eq!(result.group_hash.len(), 64);
    }

    #[test]
    fn stack_trace_takes_priority_over_template_when_no_fingerprint() {
        let stack = "app/lib/foo.ex:10\napp/lib/bar.ex:20\n";
        let prefixes = vec!["app/lib".to_owned()];

        let result = compute(GroupingInput {
            stack_trace: Some(stack),
            path_prefixes: &prefixes,
            ..input("boom 1234")
        });

        assert_eq!(result.strategy, GroupingStrategy::StackTrace);
        assert_eq!(result.message_template, "boom <int>");
    }

    #[test]
    fn message_template_is_fallback() {
        let first = compute(input("failed for user 1234"));
        let second = compute(input("failed for user 5678"));

        assert_eq!(first.strategy, GroupingStrategy::MessageTemplate);
        assert_eq!(first.message_template, "failed for user <int>");
        assert_eq!(first.group_hash, second.group_hash);
    }

    proptest::proptest! {
        #[test]
        fn strip_template_is_idempotent(message in ".*") {
            let once = strip_template(&message);
            let twice = strip_template(&once);
            proptest::prop_assert_eq!(once, twice);
        }
    }
}
