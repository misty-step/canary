//! Deterministic error classification.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Error category assigned during ingest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Infrastructure-originated failure.
    Infrastructure,
    /// Application-originated failure.
    Application,
    /// Unknown category.
    Unknown,
}

/// Error persistence assigned during ingest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Persistence {
    /// Likely transient.
    Transient,
    /// Likely persistent until code/config changes.
    Persistent,
    /// Unknown persistence.
    Unknown,
}

/// Component assigned during ingest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    /// Database component.
    Database,
    /// Network component.
    Network,
    /// Runtime/application component.
    Runtime,
    /// Unknown component.
    Unknown,
}

/// Table-driven classification output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Classification {
    /// Broad category.
    pub category: Category,
    /// Persistence expectation.
    pub persistence: Persistence,
    /// Responsible component.
    pub component: Component,
}

impl Classification {
    /// Unknown fallback used when no rule matches.
    pub const UNKNOWN: Self = Self {
        category: Category::Unknown,
        persistence: Persistence::Unknown,
        component: Component::Unknown,
    };

    const fn infrastructure_transient(component: Component) -> Self {
        Self {
            category: Category::Infrastructure,
            persistence: Persistence::Transient,
            component,
        }
    }

    const fn application_persistent(component: Component) -> Self {
        Self {
            category: Category::Application,
            persistence: Persistence::Persistent,
            component,
        }
    }
}

#[derive(Debug)]
struct Rule {
    error_class: Option<Regex>,
    message: Option<Regex>,
    classification: Classification,
}

static RULES: LazyLock<Result<Vec<Rule>, regex::Error>> = LazyLock::new(build_rules);

fn build_rules() -> Result<Vec<Rule>, regex::Error> {
    Ok(vec![
        Rule {
            error_class: Some(Regex::new(r"(^|\.)DBConnection\.ConnectionError$")?),
            message: None,
            classification: Classification::infrastructure_transient(Component::Database),
        },
        Rule {
            error_class: None,
            message: Some(Regex::new(
                r"(?i)(CRON_SECRET not configured|unauthorized|forbidden|invalid[_ -]?api[_ -]?key|missing .*secret|token expired)",
            )?),
            classification: Classification::application_persistent(Component::Runtime),
        },
        Rule {
            error_class: Some(Regex::new(r"(^|\.)EmbeddingError$")?),
            message: Some(Regex::new(r"(?i)(timed out|timeout|deadline exceeded)")?),
            classification: Classification::infrastructure_transient(Component::Network),
        },
        Rule {
            error_class: Some(Regex::new(r"(^|\.)(Mint|Req)\.TransportError$")?),
            message: None,
            classification: Classification::infrastructure_transient(Component::Network),
        },
        Rule {
            error_class: Some(Regex::new(r"(^|\.)FunctionClauseError$")?),
            message: None,
            classification: Classification::application_persistent(Component::Runtime),
        },
        Rule {
            error_class: None,
            message: Some(Regex::new(
                r"(?i)(timed out|timeout|deadline exceeded|connection reset|connection refused|ECONNRESET|ECONNREFUSED|fetch failed|nxdomain|socket closed)",
            )?),
            classification: Classification::infrastructure_transient(Component::Network),
        },
    ])
}

/// Classify an error using the deterministic rule table.
pub fn classify(error_class: &str, message: &str) -> Classification {
    let Ok(rules) = RULES.as_ref() else {
        return Classification::UNKNOWN;
    };

    rules
        .iter()
        .find(|rule| {
            rule.error_class
                .as_ref()
                .is_none_or(|pattern| pattern.is_match(error_class))
                && rule
                    .message
                    .as_ref()
                    .is_none_or(|pattern| pattern.is_match(message))
        })
        .map_or(Classification::UNKNOWN, |rule| rule.classification)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_db_connection_as_transient_database_infrastructure() {
        assert_eq!(
            classify("DBConnection.ConnectionError", "pool timed out"),
            Classification::infrastructure_transient(Component::Database)
        );
    }

    #[test]
    fn classifies_secret_errors_as_persistent_runtime_application() {
        assert_eq!(
            classify("RuntimeError", "CRON_SECRET not configured"),
            Classification::application_persistent(Component::Runtime)
        );
    }

    #[test]
    fn classifies_transport_errors_as_transient_network_infrastructure() {
        assert_eq!(
            classify("Req.TransportError", "connection refused"),
            Classification::infrastructure_transient(Component::Network)
        );
    }

    #[test]
    fn classifies_function_clause_as_persistent_runtime_application() {
        assert_eq!(
            classify("FunctionClauseError", "no function clause matching"),
            Classification::application_persistent(Component::Runtime)
        );
    }

    #[test]
    fn falls_back_to_unknown() {
        assert_eq!(classify("SomethingElse", "odd"), Classification::UNKNOWN);
    }
}
