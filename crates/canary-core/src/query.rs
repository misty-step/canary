//! Deterministic query read-model contracts.
//!
//! This module owns the small pieces of query behavior that are not SQLite:
//! accepted windows, cursor encoding, response DTOs, and summary templates.

use base64::{Engine, prelude::BASE64_STANDARD, prelude::BASE64_URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

/// Maximum number of error groups returned by group-list queries.
pub const MAX_ERROR_GROUPS: usize = 50;

/// User-facing validation detail for invalid query windows.
pub const INVALID_WINDOW_DETAIL: &str = "Invalid window. Allowed: 1h, 6h, 24h, 7d, 30d";

/// User-facing field error for invalid query windows.
pub const INVALID_WINDOW_FIELD_ERROR: &str = "must be one of: 1h, 6h, 24h, 7d, 30d";

/// Closed set of query windows accepted by the Phoenix service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryWindow {
    /// Last hour.
    OneHour,
    /// Last six hours.
    SixHours,
    /// Last 24 hours.
    TwentyFourHours,
    /// Last seven days.
    SevenDays,
    /// Last 30 days.
    ThirtyDays,
}

impl QueryWindow {
    /// Parse a Phoenix wire-value window.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "1h" => Some(Self::OneHour),
            "6h" => Some(Self::SixHours),
            "24h" => Some(Self::TwentyFourHours),
            "7d" => Some(Self::SevenDays),
            "30d" => Some(Self::ThirtyDays),
            _ => None,
        }
    }

    /// Return the Phoenix wire value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OneHour => "1h",
            Self::SixHours => "6h",
            Self::TwentyFourHours => "24h",
            Self::SevenDays => "7d",
            Self::ThirtyDays => "30d",
        }
    }

    /// Return the RFC3339 cutoff string for this window at `now`.
    pub fn cutoff_at(self, now: OffsetDateTime) -> String {
        let seconds = match self {
            Self::OneHour => 3_600,
            Self::SixHours => 21_600,
            Self::TwentyFourHours => 86_400,
            Self::SevenDays => 604_800,
            Self::ThirtyDays => 2_592_000,
        };

        (now - Duration::seconds(seconds))
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
    }
}

/// Cursor decoded from a group-list query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryCursor {
    /// Structured count/hash cursor emitted by current Canary.
    Structured(GroupCursor),
    /// Legacy cursor that only carries a group hash.
    LegacyGroupHash(String),
}

/// Structured cursor used by Phoenix for error-group pagination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupCursor {
    /// Last row count from the previous page.
    pub total_count: u64,
    /// Last row group hash from the previous page.
    pub group_hash: String,
}

/// Decode current structured cursors and legacy base64 group-hash cursors.
pub fn decode_cursor(cursor: &str) -> Option<QueryCursor> {
    if let Ok(json) = BASE64_URL_SAFE_NO_PAD.decode(cursor)
        && let Ok(decoded) = serde_json::from_slice::<GroupCursor>(&json)
    {
        return Some(QueryCursor::Structured(decoded));
    }

    BASE64_STANDARD
        .decode(cursor)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(QueryCursor::LegacyGroupHash)
}

/// Encode a current structured cursor.
pub fn encode_cursor(cursor: &GroupCursor) -> Option<String> {
    let json = serde_json::to_vec(cursor).ok()?;
    Some(BASE64_URL_SAFE_NO_PAD.encode(json))
}

/// Classification values attached to an error group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorClassification {
    /// Category classification.
    pub category: String,
    /// Persistence classification.
    pub persistence: String,
    /// Component classification.
    pub component: String,
}

impl ErrorClassification {
    /// Build a classification, replacing nil/empty database values with `unknown`.
    pub fn new(
        category: Option<String>,
        persistence: Option<String>,
        component: Option<String>,
    ) -> Self {
        Self {
            category: classification_value(category),
            persistence: classification_value(persistence),
            component: classification_value(component),
        }
    }
}

/// Error group item returned by service and class queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorGroupSummary {
    /// Stable group hash.
    pub group_hash: String,
    /// Error class.
    pub error_class: String,
    /// Service name.
    pub service: String,
    /// Total count for the group.
    #[serde(rename = "count")]
    pub total_count: u64,
    /// First-seen timestamp.
    pub first_seen: String,
    /// Last-seen timestamp.
    pub last_seen: String,
    /// Sample message template.
    pub sample_message: Option<String>,
    /// Severity label.
    pub severity: String,
    /// Group status.
    pub status: String,
    /// Deterministic classification.
    pub classification: ErrorClassification,
}

/// Response for `GET /api/v1/query?service=...`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorsByService {
    /// Deterministic summary.
    pub summary: String,
    /// Service name.
    pub service: String,
    /// Query window wire value.
    pub window: String,
    /// Sum of returned group counts.
    pub total_errors: u64,
    /// Returned groups.
    pub groups: Vec<ErrorGroupSummary>,
    /// Cursor for the next page when exactly 50 groups were returned.
    pub cursor: Option<String>,
}

/// Response for `GET /api/v1/query?error_class=...`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorsByErrorClass {
    /// Deterministic summary.
    pub summary: String,
    /// Error class.
    pub error_class: String,
    /// Query window wire value.
    pub window: String,
    /// Sum of returned group counts.
    pub total_errors: u64,
    /// Returned groups.
    pub groups: Vec<ErrorGroupSummary>,
    /// Cursor for the next page when exactly 50 groups were returned.
    pub cursor: Option<String>,
}

/// Aggregate item returned by `GET /api/v1/query?group_by=error_class`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorClassAggregate {
    /// Error class.
    pub error_class: String,
    /// Sum of grouped error counts for the class.
    pub total_count: u64,
    /// Number of services represented by the class.
    pub service_count: u64,
}

/// Response for `GET /api/v1/query?group_by=error_class`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorsByClass {
    /// Deterministic summary.
    pub summary: String,
    /// Query window wire value.
    pub window: String,
    /// Sum of all matching group counts, including rows past the visible limit.
    pub total_errors: u64,
    /// Count of all matching error classes, including rows past the visible limit.
    pub total_error_classes: u64,
    /// True when more than 50 classes matched the query.
    pub truncated: bool,
    /// Top classes by total count.
    pub groups: Vec<ErrorClassAggregate>,
}

/// Signal item returned by `GET /api/v1/incidents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActiveIncidentSignal {
    /// Signal type, such as `error_group` or `health_transition`.
    pub signal_type: String,
    /// Stable signal reference.
    pub signal_ref: String,
    /// Signal attachment timestamp.
    pub attached_at: String,
    /// Signal resolution timestamp.
    pub resolved_at: Option<String>,
}

/// Incident item returned by `GET /api/v1/incidents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActiveIncident {
    /// Incident id.
    pub id: String,
    /// Service name.
    pub service: String,
    /// Active-list incidents are reported as investigating while any signal is active.
    pub state: String,
    /// Derived severity for the currently active signal set.
    pub severity: String,
    /// Incident title.
    pub title: Option<String>,
    /// Incident open timestamp.
    pub opened_at: String,
    /// Incident resolution timestamp.
    pub resolved_at: Option<String>,
    /// Count of active signals in this list item.
    pub signal_count: usize,
    /// Active signals.
    pub signals: Vec<ActiveIncidentSignal>,
}

/// Response for `GET /api/v1/incidents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActiveIncidents {
    /// Deterministic summary.
    pub summary: String,
    /// Active incident list.
    pub incidents: Vec<ActiveIncident>,
}

/// Error group attached to an error detail response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorDetailGroup {
    /// Total count for the group.
    pub total_count: u64,
    /// First-seen timestamp.
    pub first_seen_at: String,
    /// Last-seen timestamp.
    pub last_seen_at: String,
    /// Group status.
    pub status: String,
}

/// Response for `GET /api/v1/errors/:id`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ErrorDetail {
    /// Deterministic summary.
    pub summary: String,
    /// Error row id.
    pub id: String,
    /// Service name.
    pub service: String,
    /// Error class.
    pub error_class: String,
    /// Error message.
    pub message: String,
    /// Optional message template.
    pub message_template: Option<String>,
    /// Optional stack trace.
    pub stack_trace: Option<String>,
    /// Optional decoded context value.
    pub context: Option<serde_json::Value>,
    /// Severity label.
    pub severity: String,
    /// Environment label.
    pub environment: String,
    /// Stable group hash.
    pub group_hash: String,
    /// Creation timestamp.
    pub created_at: String,
    /// Optional group summary.
    pub group: Option<ErrorDetailGroup>,
    /// Sorted incident ids correlated to the error group.
    pub incident_ids: Vec<String>,
}

/// Build a Phoenix-compatible service query response.
pub fn errors_by_service_response(
    service: String,
    window: QueryWindow,
    groups: Vec<ErrorGroupSummary>,
) -> ErrorsByService {
    let total_errors = groups.iter().map(|group| group.total_count).sum();
    let cursor = if groups.len() == MAX_ERROR_GROUPS {
        groups.last().and_then(|last| {
            encode_cursor(&GroupCursor {
                total_count: last.total_count,
                group_hash: last.group_hash.clone(),
            })
        })
    } else {
        None
    };
    let summary = error_query_summary(total_errors, &service, window.as_str(), &groups);

    ErrorsByService {
        summary,
        service,
        window: window.as_str().to_owned(),
        total_errors,
        groups,
        cursor,
    }
}

/// Build a Phoenix-compatible error-class query response.
pub fn errors_by_error_class_response(
    error_class: String,
    window: QueryWindow,
    groups: Vec<ErrorGroupSummary>,
) -> ErrorsByErrorClass {
    let total_errors = groups.iter().map(|group| group.total_count).sum();
    let cursor = if groups.len() == MAX_ERROR_GROUPS {
        groups.last().and_then(|last| {
            encode_cursor(&GroupCursor {
                total_count: last.total_count,
                group_hash: last.group_hash.clone(),
            })
        })
    } else {
        None
    };
    let summary = error_class_query_summary(total_errors, &error_class, window.as_str(), &groups);

    ErrorsByErrorClass {
        summary,
        error_class,
        window: window.as_str().to_owned(),
        total_errors,
        groups,
        cursor,
    }
}

/// Build a Phoenix-compatible error-class aggregate response.
pub fn errors_by_class_response(
    window: QueryWindow,
    groups: Vec<ErrorClassAggregate>,
    total_errors: u64,
    total_error_classes: u64,
) -> ErrorsByClass {
    let truncated = total_error_classes as usize > groups.len();
    let summary = error_class_aggregate_summary(
        total_errors,
        total_error_classes,
        window.as_str(),
        &groups,
        truncated,
    );

    ErrorsByClass {
        summary,
        window: window.as_str().to_owned(),
        total_errors,
        total_error_classes,
        truncated,
        groups,
    }
}

/// Build a Phoenix-compatible active incidents response.
pub fn active_incidents_response(incidents: Vec<ActiveIncident>) -> ActiveIncidents {
    ActiveIncidents {
        summary: incidents_list_summary(&incidents),
        incidents,
    }
}

/// Build a Phoenix-compatible error detail response.
pub fn error_detail_response(
    mut detail: ErrorDetail,
    count: u64,
    first_seen: String,
    last_seen: String,
) -> ErrorDetail {
    detail.summary = error_detail_summary(
        &detail.error_class,
        &detail.service,
        count,
        &first_seen,
        &last_seen,
    );
    detail
}

fn error_query_summary(
    total: u64,
    service: &str,
    window: &str,
    groups: &[ErrorGroupSummary],
) -> String {
    let base = format!(
        "{total} errors in {service} in the last {window}. {} unique classes.",
        groups.len()
    );
    match groups.first() {
        Some(top) => format!(
            "{base} Most frequent: {} ({} occurrences).",
            top.error_class, top.total_count
        ),
        None => base,
    }
}

fn error_class_query_summary(
    total: u64,
    error_class: &str,
    window: &str,
    groups: &[ErrorGroupSummary],
) -> String {
    let mut services = groups
        .iter()
        .map(|group| group.service.as_str())
        .collect::<Vec<_>>();
    services.sort_unstable();
    services.dedup();

    format!(
        "{total} errors matching {error_class} in the last {window}. {} groups across {} services.",
        groups.len(),
        services.len()
    )
}

fn error_class_aggregate_summary(
    total: u64,
    class_count: u64,
    window: &str,
    groups: &[ErrorClassAggregate],
    truncated: bool,
) -> String {
    let class_label = pluralize(class_count, "error class", "error classes");
    let base = format!("{total} errors across {class_count} {class_label} in the last {window}.");
    let top_part = match groups.first() {
        Some(top) => format!(
            " Most frequent: {} ({} occurrences).",
            top.error_class, top.total_count
        ),
        None => String::new(),
    };
    let truncated_part = if truncated {
        format!(" Response truncated to top {} classes.", groups.len())
    } else {
        String::new()
    };

    format!("{base}{top_part}{truncated_part}")
}

fn incidents_list_summary(incidents: &[ActiveIncident]) -> String {
    if incidents.is_empty() {
        return "No active incidents.".to_owned();
    }

    let count = incidents.len();
    let mut services = incidents
        .iter()
        .map(|incident| incident.service.as_str())
        .collect::<Vec<_>>();
    services.sort_unstable();
    services.dedup();
    let service_count = services.len();
    let high = incidents
        .iter()
        .filter(|incident| incident.severity == "high")
        .count();

    let severity_part = if high > 0 {
        format!(
            " {high} high-severity {}.",
            pluralize_usize(high, "incident", "incidents")
        )
    } else {
        String::new()
    };

    let newest_part = incidents
        .iter()
        .max_by_key(|incident| incident.opened_at.as_str())
        .map(|incident| format!(" Newest: {} at {}.", incident.service, incident.opened_at))
        .unwrap_or_default();

    format!(
        "{count} open {} across {service_count} {}.{severity_part}{newest_part}",
        pluralize_usize(count, "incident", "incidents"),
        pluralize_usize(service_count, "service", "services")
    )
}

fn error_detail_summary(
    error_class: &str,
    service: &str,
    count: u64,
    first_seen: &str,
    last_seen: &str,
) -> String {
    format!(
        "{error_class} in {service}. Seen {count} times since {first_seen}. Last occurrence: {last_seen}."
    )
}

fn pluralize<'a>(count: u64, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

fn pluralize_usize<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

fn classification_value(value: Option<String>) -> String {
    match value {
        Some(value) if !value.is_empty() => value,
        _ => "unknown".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_match_phoenix_wire_values() {
        for (wire, window) in [
            ("1h", QueryWindow::OneHour),
            ("6h", QueryWindow::SixHours),
            ("24h", QueryWindow::TwentyFourHours),
            ("7d", QueryWindow::SevenDays),
            ("30d", QueryWindow::ThirtyDays),
        ] {
            assert_eq!(QueryWindow::parse(wire), Some(window));
            assert_eq!(window.as_str(), wire);
        }
        assert_eq!(QueryWindow::parse("99h"), None);
    }

    #[test]
    fn cursor_round_trip_matches_phoenix_shape() -> Result<(), Box<dyn std::error::Error>> {
        let cursor = GroupCursor {
            total_count: 42,
            group_hash: "group-a".to_owned(),
        };
        let Some(encoded) = encode_cursor(&cursor) else {
            return Err("cursor should encode".into());
        };

        assert_eq!(
            decode_cursor(&encoded),
            Some(QueryCursor::Structured(cursor))
        );
        Ok(())
    }

    #[test]
    fn cursor_decoder_accepts_legacy_group_hash() {
        let encoded = BASE64_STANDARD.encode("group-legacy");

        assert_eq!(
            decode_cursor(&encoded),
            Some(QueryCursor::LegacyGroupHash("group-legacy".to_owned()))
        );
    }
}
