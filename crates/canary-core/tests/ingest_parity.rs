//! Phoenix-observed parity fixtures for deterministic ingest helpers.
//!
//! These tests pin observable behavior from `Canary.Errors.Grouping` and
//! `Canary.Errors.Classification` so the Rust rewrite can refactor internals
//! without silently changing production grouping or classification contracts.

use canary_core::ingest::classification::{
    Category, Classification, Component, Persistence, classify,
};
use canary_core::ingest::grouping::{GroupingInput, GroupingStrategy, compute};

#[derive(Debug)]
struct GroupingCase<'a> {
    name: &'a str,
    service: &'a str,
    error_class: &'a str,
    message: &'a str,
    stack_trace: Option<&'a str>,
    fingerprint: Option<&'a [&'a str]>,
    expected_strategy: GroupingStrategy,
    expected_hash: &'a str,
    expected_template: &'a str,
}

#[derive(Debug)]
struct ClassificationCase<'a> {
    name: &'a str,
    error_class: &'a str,
    message: &'a str,
    expected: Classification,
}

#[test]
fn grouping_matches_phoenix_fixtures() {
    let module_prefixes = vec![String::from("MyApp.")];
    let path_prefixes = vec![String::from("/app/")];

    for case in grouping_cases() {
        let fingerprint = case.fingerprint.map(|parts| {
            parts
                .iter()
                .map(|part| (*part).to_owned())
                .collect::<Vec<_>>()
        });

        let actual = compute(GroupingInput {
            service: case.service,
            error_class: case.error_class,
            message: case.message,
            stack_trace: case.stack_trace,
            fingerprint: fingerprint.as_deref(),
            module_prefixes: &module_prefixes,
            path_prefixes: &path_prefixes,
        });

        assert_eq!(
            actual.group_hash, case.expected_hash,
            "{} group hash drifted",
            case.name
        );
        assert_eq!(
            actual.message_template, case.expected_template,
            "{} template drifted",
            case.name
        );
        assert_eq!(
            actual.strategy, case.expected_strategy,
            "{} strategy drifted",
            case.name
        );
    }
}

#[test]
fn classification_matches_phoenix_fixtures() {
    for case in classification_cases() {
        assert_eq!(
            classify(case.error_class, case.message),
            case.expected,
            "{} classification drifted",
            case.name
        );
    }
}

fn grouping_cases<'a>() -> Vec<GroupingCase<'a>> {
    vec![
        GroupingCase {
            name: "message_template_path_values",
            service: "svc",
            error_class: "RuntimeError",
            message: "user test@example.com failed at 2026-05-28T19:00:00Z path /tmp/file id 123456",
            stack_trace: None,
            fingerprint: None,
            expected_strategy: GroupingStrategy::MessageTemplate,
            expected_hash: "4e61909df2b64e403a0119be9f11d16c16362372d0d717c4b9bd1bd1b8b21278",
            expected_template: "user <email> failed at <timestamp> path<path> id <int>",
        },
        GroupingCase {
            name: "client_fingerprint_priority",
            service: "svc",
            error_class: "RuntimeError",
            message: "different 123456",
            stack_trace: None,
            fingerprint: Some(&["tenant", "route"]),
            expected_strategy: GroupingStrategy::ClientFingerprint,
            expected_hash: "e8c426c2a8ad757b72a018ccc58bc49b8ca53511bfc23769dbc8b38aed04e22d",
            expected_template: "different <int>",
        },
        GroupingCase {
            name: "empty_fingerprint_is_still_explicit",
            service: "svc",
            error_class: "RuntimeError",
            message: "empty fp 123456",
            stack_trace: None,
            fingerprint: Some(&[]),
            expected_strategy: GroupingStrategy::ClientFingerprint,
            expected_hash: "348c658682ae8701d3e9d21f191872491cf15e6acbb1681770b1cb787c1cf7ff",
            expected_template: "empty fp <int>",
        },
        GroupingCase {
            name: "stack_trace_in_project_priority",
            service: "svc",
            error_class: "RuntimeError",
            message: "fallback 123456",
            stack_trace: Some(
                "lib/foo.ex:10: MyApp.Worker.run/1\n\
                 /app/lib/bar.ex:22: MyApp.Bar.call/0\n\
                 /usr/lib/other.ex:99: Other.call/0",
            ),
            fingerprint: None,
            expected_strategy: GroupingStrategy::StackTrace,
            expected_hash: "71db0aba58c866c1c9b62fcda8af1c1b869a52eae9f827ec7540fd12a52fa36d",
            expected_template: "fallback <int>",
        },
        GroupingCase {
            name: "stack_trace_fallback_frames",
            service: "svc",
            error_class: "RuntimeError",
            message: "fallback 123456",
            stack_trace: Some("deps/foo.ex:10: Dep.run/1\ndeps/bar.ex:22: Dep.call/0"),
            fingerprint: None,
            expected_strategy: GroupingStrategy::StackTrace,
            expected_hash: "d5a892319682648233fb52a763c8b80ab7a3325afb7cc5580f0c7ac18cef0734",
            expected_template: "fallback <int>",
        },
    ]
}

fn classification_cases<'a>() -> Vec<ClassificationCase<'a>> {
    vec![
        ClassificationCase {
            name: "db_connection",
            error_class: "DBConnection.ConnectionError",
            message: "pool timed out",
            expected: Classification {
                category: Category::Infrastructure,
                persistence: Persistence::Transient,
                component: Component::Database,
            },
        },
        ClassificationCase {
            name: "function_clause_precedes_timeout",
            error_class: "FunctionClauseError",
            message: "request timed out while pattern matching",
            expected: Classification {
                category: Category::Application,
                persistence: Persistence::Persistent,
                component: Component::Runtime,
            },
        },
        ClassificationCase {
            name: "auth_secret",
            error_class: "RuntimeError",
            message: "CRON_SECRET not configured",
            expected: Classification {
                category: Category::Application,
                persistence: Persistence::Persistent,
                component: Component::Runtime,
            },
        },
        ClassificationCase {
            name: "unknown",
            error_class: "SomethingElse",
            message: "odd",
            expected: Classification::UNKNOWN,
        },
    ]
}
