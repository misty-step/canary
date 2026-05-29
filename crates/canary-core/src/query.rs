//! Deterministic query read-model contracts.
//!
//! This module owns the small pieces of query behavior that are not SQLite:
//! accepted windows, cursor encoding, response DTOs, and summary templates.

use base64::{Engine, prelude::BASE64_STANDARD, prelude::BASE64_URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

/// Maximum number of error groups returned by group-list queries.
pub const MAX_ERROR_GROUPS: usize = 50;

/// Default number of timeline events returned by Phoenix.
pub const DEFAULT_TIMELINE_LIMIT: usize = 50;

/// Maximum number of timeline events accepted by Phoenix.
pub const MAX_TIMELINE_LIMIT: usize = 200;

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
///
/// This is a keyset anchor for the current `(total_count DESC, group_hash ASC)`
/// ordering, not a snapshot token. If ingest mutates a later group's
/// `total_count` above this anchor between page requests, that newly-promoted
/// group belongs to a fresh first page and will not be replayed on the next
/// page. The important agent-facing guarantee is stable continuation without
/// duplicating rows already observed under the old anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupCursor {
    /// Last row count from the previous page.
    pub total_count: u64,
    /// Last row group hash from the previous page.
    pub group_hash: String,
}

/// Structured cursor used by Phoenix for timeline pagination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineCursor {
    /// Last row timestamp from the previous page.
    pub created_at: String,
    /// Last row id from the previous page.
    pub id: String,
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

/// Decode a Phoenix timeline cursor.
pub fn decode_timeline_cursor(cursor: &str) -> Option<TimelineCursor> {
    let decoded = BASE64_URL_SAFE_NO_PAD.decode(cursor).ok()?;
    let cursor = serde_json::from_slice::<TimelineCursor>(&decoded).ok()?;
    if cursor.created_at.is_empty()
        || cursor.id.is_empty()
        || OffsetDateTime::parse(&cursor.created_at, &Rfc3339).is_err()
    {
        return None;
    }
    Some(cursor)
}

/// Encode a Phoenix timeline cursor.
pub fn encode_timeline_cursor(cursor: &TimelineCursor) -> Option<String> {
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

/// Incident row embedded in `GET /api/v1/incidents/:id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IncidentDetailIncident {
    /// Incident id.
    pub id: String,
    /// Service name.
    pub service: String,
    /// Stored incident state.
    pub state: String,
    /// Stored incident severity.
    pub severity: String,
    /// Incident title.
    pub title: Option<String>,
    /// Incident open timestamp.
    pub opened_at: String,
    /// Incident resolution timestamp.
    pub resolved_at: Option<String>,
    /// Total persisted signal count, including rows not visible in the bounded response.
    pub signal_count: usize,
}

/// Signal item embedded in `GET /api/v1/incidents/:id`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IncidentDetailSignal {
    /// Signal type.
    #[serde(rename = "type")]
    pub signal_type: String,
    /// Deterministic one-line signal summary.
    pub summary: String,
    /// Error group hash for error-group signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_hash: Option<String>,
    /// Error class for error-group signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    /// Total count for error-group signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_count: Option<u64>,
    /// First seen timestamp for error-group signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_seen_at: Option<String>,
    /// Last seen timestamp for error-group signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<String>,
    /// Classification for error-group signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification: Option<ErrorClassification>,
    /// Target id for target health signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    /// Target name for target health signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_name: Option<String>,
    /// Monitor id for monitor health signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_id: Option<String>,
    /// Monitor name for monitor health signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monitor_name: Option<String>,
    /// Current health state for health signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_state: Option<String>,
    /// Consecutive failure count for target health signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consecutive_failures: Option<u64>,
    /// Generic signal reference for fallback shapes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_ref: Option<String>,
    /// Signal attachment timestamp.
    pub attached_at: String,
    /// Signal resolution timestamp.
    pub resolved_at: Option<String>,
    /// Number of coordination annotations on the signal's underlying subject.
    pub annotation_count: u64,
}

/// Incident annotation view embedded in incident detail.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IncidentAnnotation {
    /// Annotation id.
    pub id: String,
    /// Canonical subject type.
    pub subject_type: Option<String>,
    /// Canonical subject id.
    pub subject_id: Option<String>,
    /// Legacy incident id.
    pub incident_id: Option<String>,
    /// Legacy error-group hash.
    pub group_hash: Option<String>,
    /// Agent that wrote the annotation.
    pub agent: String,
    /// Action label.
    pub action: String,
    /// Decoded annotation metadata.
    pub metadata: Option<Value>,
    /// Creation timestamp.
    pub created_at: String,
}

/// Recent incident timeline event embedded in incident detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IncidentTimelineEvent {
    /// Event id.
    pub id: String,
    /// Event name.
    pub event: String,
    /// Event severity.
    pub severity: Option<String>,
    /// Event summary.
    pub summary: String,
    /// Creation timestamp.
    pub created_at: String,
}

/// Recommended next action for an incident detail response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IncidentActionRecommendation {
    /// Machine-friendly action label.
    pub action: String,
    /// Deterministic reason.
    pub reason: String,
}

/// Visible and total signal counts for an action brief.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IncidentActionSignalCounts {
    /// Active visible signals.
    pub active: usize,
    /// Resolved visible signals.
    pub resolved: usize,
    /// Visible signal count.
    pub visible: usize,
    /// Total persisted signal count.
    pub total: usize,
}

/// Newest incident annotation summary embedded in the action brief.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LatestIncidentAnnotation {
    /// Annotation id.
    pub id: String,
    /// Agent that wrote the annotation.
    pub agent: String,
    /// Action label.
    pub action: String,
    /// Creation timestamp.
    pub created_at: String,
}

/// Action brief embedded in incident detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IncidentActionBrief {
    /// Deterministic summary.
    pub summary: String,
    /// Recommended next action.
    pub recommendation: IncidentActionRecommendation,
    /// Signal counts.
    pub signal_counts: IncidentActionSignalCounts,
    /// Whether the signal list is truncated.
    pub signals_truncated: bool,
    /// Newest incident annotation, if one exists.
    pub latest_annotation: Option<LatestIncidentAnnotation>,
}

/// Response for `GET /api/v1/incidents/:id`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IncidentDetail {
    /// Deterministic summary.
    pub summary: String,
    /// Incident row.
    pub incident: IncidentDetailIncident,
    /// Bounded signal list.
    pub signals: Vec<IncidentDetailSignal>,
    /// Whether more signals exist past the visible list.
    pub signals_truncated: bool,
    /// Bounded incident annotation list.
    pub annotations: Vec<IncidentAnnotation>,
    /// Whether more annotations exist past the visible list.
    pub annotations_truncated: bool,
    /// Recent timeline events for this incident.
    pub recent_timeline_events: Vec<IncidentTimelineEvent>,
    /// Deterministic next-action brief.
    pub action_brief: IncidentActionBrief,
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

/// Event item returned by `GET /api/v1/timeline`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineEvent {
    /// Event row id.
    pub id: String,
    /// Service name.
    pub service: String,
    /// Business event name.
    pub event: String,
    /// Subject type.
    pub entity_type: String,
    /// Subject id, when present.
    pub entity_ref: Option<String>,
    /// Event severity, when present.
    pub severity: Option<String>,
    /// Deterministic event summary.
    pub summary: String,
    /// Decoded JSON payload.
    pub payload: Value,
    /// Creation timestamp.
    pub created_at: String,
}

/// Response for `GET /api/v1/timeline`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineResponse {
    /// Deterministic summary.
    pub summary: String,
    /// Count of returned events.
    pub returned_count: usize,
    /// Query window.
    pub window: String,
    /// Optional service filter.
    pub service: Option<String>,
    /// Event page.
    pub events: Vec<TimelineEvent>,
    /// Next-page cursor.
    pub cursor: Option<String>,
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

/// Build a Phoenix-compatible incident detail response.
pub fn incident_detail_response(
    incident: IncidentDetailIncident,
    signals: Vec<IncidentDetailSignal>,
    signals_truncated: bool,
    annotations: Vec<IncidentAnnotation>,
    annotations_truncated: bool,
    recent_timeline_events: Vec<IncidentTimelineEvent>,
) -> IncidentDetail {
    let summary = incident_detail_summary(&incident, annotations.len());
    let action_brief = incident_action_brief(
        &incident,
        &signals,
        signals_truncated,
        annotations
            .first()
            .map(|annotation| LatestIncidentAnnotation {
                id: annotation.id.clone(),
                agent: annotation.agent.clone(),
                action: annotation.action.clone(),
                created_at: annotation.created_at.clone(),
            }),
    );

    IncidentDetail {
        summary,
        incident,
        signals,
        signals_truncated,
        annotations,
        annotations_truncated,
        recent_timeline_events,
        action_brief,
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

/// Build a Phoenix-compatible timeline response.
pub fn timeline_response(
    events: Vec<TimelineEvent>,
    service: Option<String>,
    window: QueryWindow,
    cursor: Option<String>,
) -> TimelineResponse {
    let summary = match service.as_deref() {
        Some(service) => format!(
            "Returned {} timeline events for {service} in the last {}.",
            events.len(),
            window.as_str()
        ),
        None => format!(
            "Returned {} timeline events in the last {}.",
            events.len(),
            window.as_str()
        ),
    };

    TimelineResponse {
        summary,
        returned_count: events.len(),
        window: window.as_str().to_owned(),
        service,
        events,
        cursor,
    }
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

fn incident_detail_summary(incident: &IncidentDetailIncident, annotation_count: usize) -> String {
    let state_label = if incident.state == "resolved" {
        "Resolved"
    } else {
        "Investigating"
    };
    let signal_part = match incident.signal_count {
        0 => "No active signals.".to_owned(),
        n => format!(
            "{n} correlated {}.",
            pluralize_usize(n, "signal", "signals")
        ),
    };
    let annotation_part = match annotation_count {
        0 => " No prior triage annotations.".to_owned(),
        n => format!(
            " {n} prior triage {}.",
            pluralize_usize(n, "annotation", "annotations")
        ),
    };

    format!(
        "{state_label}. {}-severity incident opened at {} on service {}. {signal_part}{annotation_part}",
        incident.severity, incident.opened_at, incident.service
    )
}

fn incident_action_brief(
    incident: &IncidentDetailIncident,
    signals: &[IncidentDetailSignal],
    signals_truncated: bool,
    latest_annotation: Option<LatestIncidentAnnotation>,
) -> IncidentActionBrief {
    let active = signals
        .iter()
        .filter(|signal| signal.resolved_at.is_none())
        .count();
    let resolved = signals.len() - active;
    let recommendation = incident_action_recommendation(signals, signals_truncated);
    let scope = if signals_truncated { " visible" } else { "" };
    let summary = format!(
        "{} action brief: {active}{scope} {}, {resolved}{scope} {}. Recommended action: {}.",
        incident.service,
        pluralize_usize(active, "active signal", "active signals"),
        pluralize_usize(resolved, "resolved signal", "resolved signals"),
        recommendation.action
    );

    IncidentActionBrief {
        summary,
        recommendation,
        signal_counts: IncidentActionSignalCounts {
            active,
            resolved,
            visible: signals.len(),
            total: incident.signal_count,
        },
        signals_truncated,
        latest_annotation,
    }
}

fn incident_action_recommendation(
    signals: &[IncidentDetailSignal],
    signals_truncated: bool,
) -> IncidentActionRecommendation {
    if signals_truncated {
        return IncidentActionRecommendation {
            action: "inspect-truncated-signals".to_owned(),
            reason: "Signal state is truncated; a complete recommendation cannot be derived from the visible signal set.".to_owned(),
        };
    }

    let active = signals
        .iter()
        .filter(|signal| signal.resolved_at.is_none())
        .collect::<Vec<_>>();

    if active.is_empty() {
        let resolved = signals.len();
        return IncidentActionRecommendation {
            action: "verify-recovery".to_owned(),
            reason: format!(
                "No active signals remain in the visible signal set ({resolved} resolved {}).",
                pluralize_usize(resolved, "signal", "signals")
            ),
        };
    }

    let unannotated = active
        .iter()
        .filter(|signal| signal.annotation_count == 0)
        .count();

    if unannotated > 0 {
        IncidentActionRecommendation {
            action: "triage".to_owned(),
            reason: format!(
                "{unannotated} active {} lack coordination annotations.",
                pluralize_usize(unannotated, "signal", "signals")
            ),
        }
    } else {
        IncidentActionRecommendation {
            action: "watch".to_owned(),
            reason: "Active signals already have coordination annotations.".to_owned(),
        }
    }
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

    #[test]
    fn timeline_cursor_round_trip_matches_phoenix_shape() -> Result<(), Box<dyn std::error::Error>>
    {
        let cursor = TimelineCursor {
            created_at: "2026-05-28T20:59:50Z".to_owned(),
            id: "EVT-b".to_owned(),
        };
        let Some(encoded) = encode_timeline_cursor(&cursor) else {
            return Err("timeline cursor should encode".into());
        };

        assert_eq!(decode_timeline_cursor(&encoded), Some(cursor));
        assert_eq!(decode_timeline_cursor("bogus"), None);

        let malformed = BASE64_URL_SAFE_NO_PAD.encode(r#"{"created_at":1,"id":2}"#);
        assert_eq!(decode_timeline_cursor(&malformed), None);
        Ok(())
    }

    #[test]
    fn timeline_response_summary_matches_phoenix_templates() {
        assert_eq!(
            timeline_response(vec![], None, QueryWindow::TwentyFourHours, None).summary,
            "Returned 0 timeline events in the last 24h."
        );
        assert_eq!(
            timeline_response(
                vec![TimelineEvent {
                    id: "EVT-a".to_owned(),
                    service: "alpha".to_owned(),
                    event: "error.new_class".to_owned(),
                    entity_type: "error_group".to_owned(),
                    entity_ref: Some("group-a".to_owned()),
                    severity: Some("error".to_owned()),
                    summary: "summary".to_owned(),
                    payload: serde_json::json!({"event": "error.new_class"}),
                    created_at: "2026-05-28T20:59:50Z".to_owned(),
                }],
                Some("alpha".to_owned()),
                QueryWindow::OneHour,
                None
            )
            .summary,
            "Returned 1 timeline events for alpha in the last 1h."
        );
    }
}
