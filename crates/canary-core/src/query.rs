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

/// Default number of webhook delivery ledger rows returned by Phoenix.
pub const DEFAULT_WEBHOOK_DELIVERY_LIMIT: usize = 50;

/// Maximum number of webhook delivery ledger rows accepted by Phoenix.
pub const MAX_WEBHOOK_DELIVERY_LIMIT: usize = 200;

/// Default number of annotations returned by Phoenix.
pub const DEFAULT_ANNOTATION_LIMIT: usize = 50;

/// Maximum number of annotations accepted by Phoenix.
pub const MAX_ANNOTATION_LIMIT: usize = 50;

/// Default number of remediation claims returned by subject-list APIs.
pub const DEFAULT_CLAIM_LIMIT: usize = 20;

/// Maximum number of remediation claims returned by subject-list APIs.
pub const MAX_CLAIM_LIMIT: usize = 50;

/// Subject types accepted by remediation claims.
pub const REMEDIATION_CLAIM_SUBJECT_TYPES: [&str; 4] =
    ["incident", "error_group", "target", "monitor"];

/// States accepted by remediation claims.
pub const REMEDIATION_CLAIM_STATES: [&str; 7] = [
    "claimed",
    "investigating",
    "fix_proposed",
    "verified",
    "dismissed",
    "expired",
    "released",
];

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

    /// Return the window length in seconds.
    pub const fn duration_seconds(self) -> i64 {
        match self {
            Self::OneHour => 3_600,
            Self::SixHours => 21_600,
            Self::TwentyFourHours => 86_400,
            Self::SevenDays => 604_800,
            Self::ThirtyDays => 2_592_000,
        }
    }

    /// Return the RFC3339 cutoff string for this window at `now`.
    pub fn cutoff_at(self, now: OffsetDateTime) -> String {
        (now - Duration::seconds(self.duration_seconds()))
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

/// Structured cursor used by Phoenix for webhook delivery ledger pagination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookDeliveryCursor {
    /// Last row timestamp from the previous page.
    pub created_at: String,
    /// Last row delivery id from the previous page.
    pub delivery_id: String,
}

/// Structured cursor used by Phoenix for annotation pagination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationCursor {
    /// Last row timestamp from the previous page.
    pub created_at: String,
    /// Last row id from the previous page.
    pub id: String,
}

/// Structured cursor used by remediation claim pagination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimCursor {
    /// Last row timestamp from the previous page.
    pub created_at: String,
    /// Last row id from the previous page.
    pub id: String,
}

/// Offset cursor used by Phoenix for the unified report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportCursor {
    /// Next target offset, or nil when targets are exhausted.
    pub targets_offset: Option<usize>,
    /// Next monitor offset, or nil when monitors are exhausted.
    pub monitor_offset: Option<usize>,
    /// Next error-group offset, or nil when error groups are exhausted.
    pub error_groups_offset: Option<usize>,
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

/// Decode a Phoenix webhook delivery cursor.
pub fn decode_webhook_delivery_cursor(cursor: &str) -> Option<WebhookDeliveryCursor> {
    let decoded = BASE64_URL_SAFE_NO_PAD.decode(cursor).ok()?;
    let cursor = serde_json::from_slice::<WebhookDeliveryCursor>(&decoded).ok()?;
    if cursor.created_at.is_empty()
        || cursor.delivery_id.is_empty()
        || OffsetDateTime::parse(&cursor.created_at, &Rfc3339).is_err()
    {
        return None;
    }
    Some(cursor)
}

/// Encode a Phoenix webhook delivery cursor.
pub fn encode_webhook_delivery_cursor(cursor: &WebhookDeliveryCursor) -> Option<String> {
    let json = serde_json::to_vec(cursor).ok()?;
    Some(BASE64_URL_SAFE_NO_PAD.encode(json))
}

/// Decode a Phoenix annotation cursor.
pub fn decode_annotation_cursor(cursor: &str) -> Option<AnnotationCursor> {
    let decoded = BASE64_URL_SAFE_NO_PAD.decode(cursor).ok()?;
    let cursor = serde_json::from_slice::<AnnotationCursor>(&decoded).ok()?;
    if cursor.created_at.is_empty() || cursor.id.is_empty() {
        return None;
    }
    Some(cursor)
}

/// Encode a Phoenix annotation cursor.
pub fn encode_annotation_cursor(cursor: &AnnotationCursor) -> Option<String> {
    let json = serde_json::to_vec(cursor).ok()?;
    Some(BASE64_URL_SAFE_NO_PAD.encode(json))
}

/// Decode a remediation claim cursor.
pub fn decode_claim_cursor(cursor: &str) -> Option<ClaimCursor> {
    let decoded = BASE64_URL_SAFE_NO_PAD.decode(cursor).ok()?;
    let cursor = serde_json::from_slice::<ClaimCursor>(&decoded).ok()?;
    if cursor.created_at.is_empty() || cursor.id.is_empty() {
        return None;
    }
    Some(cursor)
}

/// Encode a remediation claim cursor.
pub fn encode_claim_cursor(cursor: &ClaimCursor) -> Option<String> {
    let json = serde_json::to_vec(cursor).ok()?;
    Some(BASE64_URL_SAFE_NO_PAD.encode(json))
}

/// Decode a Phoenix report cursor.
pub fn decode_report_cursor(cursor: &str) -> Option<ReportCursor> {
    if cursor.is_empty() {
        return Some(ReportCursor::default());
    }
    let decoded = BASE64_URL_SAFE_NO_PAD.decode(cursor).ok()?;
    serde_json::from_slice::<ReportCursor>(&decoded).ok()
}

/// Encode a Phoenix report cursor.
pub fn encode_report_cursor(cursor: &ReportCursor) -> Option<String> {
    if cursor.targets_offset.is_none()
        && cursor.monitor_offset.is_none()
        && cursor.error_groups_offset.is_none()
    {
        return None;
    }
    let json = serde_json::to_vec(cursor).ok()?;
    Some(BASE64_URL_SAFE_NO_PAD.encode(json))
}

impl Default for ReportCursor {
    fn default() -> Self {
        Self {
            targets_offset: Some(0),
            monitor_offset: Some(0),
            error_groups_offset: Some(0),
        }
    }
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
    /// Current active remediation claim for this group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_claim: Option<RemediationClaimSummary>,
}

/// Compact remediation-claim view embedded in agent read models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemediationClaimSummary {
    /// Claim id.
    pub id: String,
    /// Claimed subject type.
    pub subject_type: String,
    /// Claimed subject id.
    pub subject_id: String,
    /// Agent or automation owner.
    pub owner: String,
    /// Bounded claim state.
    pub state: String,
    /// Human-readable purpose.
    pub purpose: String,
    /// Expiration timestamp.
    pub expires_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Evidence links supplied by the owner.
    pub evidence_links: Vec<String>,
}

/// Escalation overlay state for one incident, returned by
/// `POST /api/v1/incidents/{id}/escalate` and `.../deescalate`.
///
/// Escalation is orthogonal to `incidents.state`: it never appears as a
/// value of that deterministic enum. `escalated_at` is `None` when the
/// incident is not currently escalated (including after deescalation or
/// after the incident resolves, which auto-clears any open escalation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncidentEscalation {
    /// Incident id.
    pub incident_id: String,
    /// Escalation timestamp, or `None` when the incident is not escalated.
    pub escalated_at: Option<String>,
    /// Owner who escalated (or last escalated) the incident.
    pub escalated_by: Option<String>,
    /// Reason given for the escalation.
    pub reason: Option<String>,
}

/// Full remediation claim row returned by claim routes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemediationClaim {
    /// Claim id.
    pub id: String,
    /// Tenant namespace.
    pub tenant_id: String,
    /// Project namespace.
    pub project_id: String,
    /// Service resolved from the subject.
    pub service: Option<String>,
    /// Claimed subject type.
    pub subject_type: String,
    /// Claimed subject id.
    pub subject_id: String,
    /// Agent or automation owner.
    pub owner: String,
    /// Human-readable purpose.
    pub purpose: String,
    /// Bounded claim state.
    pub state: String,
    /// Idempotency key for creation.
    pub idempotency_key: String,
    /// Evidence links supplied by the owner.
    pub evidence_links: Vec<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Expiration timestamp.
    pub expires_at: String,
    /// Release timestamp.
    pub released_at: Option<String>,
    /// Terminal timestamp.
    pub completed_at: Option<String>,
}

impl RemediationClaim {
    /// Return the compact read-model shape for this claim.
    pub fn summary(&self) -> RemediationClaimSummary {
        RemediationClaimSummary {
            id: self.id.clone(),
            subject_type: self.subject_type.clone(),
            subject_id: self.subject_id.clone(),
            owner: self.owner.clone(),
            state: self.state.clone(),
            purpose: self.purpose.clone(),
            expires_at: self.expires_at.clone(),
            updated_at: self.updated_at.clone(),
            evidence_links: self.evidence_links.clone(),
        }
    }
}

/// Response for `GET /api/v1/claims`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemediationClaimsResponse {
    /// Deterministic summary.
    pub summary: String,
    /// Matching claims.
    pub claims: Vec<RemediationClaim>,
    /// Effective page limit.
    pub limit: usize,
    /// Current active claim for the subject, if one exists.
    pub current_claim: Option<RemediationClaimSummary>,
    /// Next-page cursor, or null when all matching claims are visible.
    pub cursor: Option<String>,
    /// Whether more claims exist past this response.
    pub truncated: bool,
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
    /// Current active remediation claim for this incident.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_claim: Option<RemediationClaimSummary>,
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
    /// Current active remediation claim for the signal's underlying subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_claim: Option<RemediationClaimSummary>,
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
    /// Current remediation claim on this incident, if one exists.
    pub current_claim: Option<RemediationClaimSummary>,
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
    /// Bounded remediation claim list for the incident.
    pub claims: Vec<RemediationClaim>,
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
    /// Signal family for agent routing.
    pub signal_kind: String,
    /// Consumer-supplied signal name, when this is a telemetry event.
    pub signal_name: Option<String>,
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
    /// Decoded event attributes.
    pub attributes: Value,
    /// Retention class applied to this signal.
    pub retention_class: String,
    /// Privacy policy applied before persistence.
    pub privacy_policy: String,
    /// Sampling policy reported by the producer.
    pub sampling_policy: String,
    /// Creation timestamp.
    pub created_at: String,
}

/// Successful response for `POST /api/v1/events`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TelemetryEvent {
    /// Event row id.
    pub id: String,
    /// Service name.
    pub service: String,
    /// Stable timeline event family, currently `telemetry.event`.
    pub event: String,
    /// Consumer event name.
    pub name: String,
    /// Event severity.
    pub severity: String,
    /// Deterministic bounded summary.
    pub summary: String,
    /// Decoded event attributes.
    pub attributes: Value,
    /// Retention class applied to this signal.
    pub retention_class: String,
    /// Privacy policy applied before persistence.
    pub privacy_policy: String,
    /// Sampling policy reported by the producer.
    pub sampling_policy: String,
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

/// Delivery item returned by `GET /api/v1/webhook-deliveries`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebhookDelivery {
    /// Stable delivery id.
    pub delivery_id: String,
    /// Webhook subscription id.
    pub webhook_id: String,
    /// Tenant that owns the delivery row.
    pub tenant_id: String,
    /// Project that owns the delivery row.
    pub project_id: String,
    /// Optional service scope for the delivery row.
    pub service: Option<String>,
    /// Event name.
    pub event: String,
    /// Current delivery status.
    pub status: String,
    /// Number of HTTP attempts.
    pub attempt_count: i64,
    /// Suppression or discard reason.
    pub reason: Option<String>,
    /// First attempt timestamp.
    pub first_attempt_at: Option<String>,
    /// Last attempt timestamp.
    pub last_attempt_at: Option<String>,
    /// Success timestamp.
    pub delivered_at: Option<String>,
    /// Permanent-discard timestamp.
    pub discarded_at: Option<String>,
    /// Completion timestamp for terminal statuses.
    pub completed_at: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
}

/// Response for `GET /api/v1/webhook-deliveries`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebhookDeliveriesResponse {
    /// Count of returned rows.
    pub returned_count: usize,
    /// Next-page cursor.
    pub cursor: Option<String>,
    /// Delivery page.
    pub deliveries: Vec<WebhookDelivery>,
}

/// Annotation subject types accepted by the public API.
pub const ANNOTATION_SUBJECT_TYPES: [&str; 4] = ["incident", "error_group", "target", "monitor"];

/// Annotation item returned by public annotation routes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Annotation {
    /// Stable annotation id.
    pub id: String,
    /// Canonical subject type.
    pub subject_type: String,
    /// Canonical subject id.
    pub subject_id: String,
    /// Legacy incident id field for incident annotations.
    pub incident_id: Option<String>,
    /// Legacy group hash field for error-group annotations.
    pub group_hash: Option<String>,
    /// Agent that wrote the annotation.
    pub agent: String,
    /// Opaque consumer-authored action label.
    pub action: String,
    /// Decoded metadata or original malformed string.
    pub metadata: Option<Value>,
    /// Creation timestamp.
    pub created_at: String,
}

/// Response for legacy annotation list routes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnnotationListResponse {
    /// Matching annotations.
    pub annotations: Vec<Annotation>,
}

/// Response for `GET /api/v1/annotations`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnnotationPageResponse {
    /// Deterministic summary.
    pub summary: String,
    /// Matching annotations.
    pub annotations: Vec<Annotation>,
    /// Current active remediation claim for the annotated subject.
    pub current_claim: Option<RemediationClaimSummary>,
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
    claims: Vec<RemediationClaim>,
    recent_timeline_events: Vec<IncidentTimelineEvent>,
) -> IncidentDetail {
    let summary = incident_detail_summary(&incident, annotations.len());
    let current_claim = claims
        .iter()
        .find(|claim| claim_state_is_active(&claim.state))
        .map(RemediationClaim::summary);
    let action_brief = incident_action_brief(
        &incident,
        &signals,
        signals_truncated,
        current_claim.clone(),
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
        claims,
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

/// Build a Phoenix-compatible webhook delivery page response.
pub fn webhook_deliveries_response(
    deliveries: Vec<WebhookDelivery>,
    cursor: Option<String>,
) -> WebhookDeliveriesResponse {
    WebhookDeliveriesResponse {
        returned_count: deliveries.len(),
        cursor,
        deliveries,
    }
}

/// Build a Phoenix-compatible annotation list response.
pub fn annotation_list_response(annotations: Vec<Annotation>) -> AnnotationListResponse {
    AnnotationListResponse { annotations }
}

/// Build a Phoenix-compatible annotation page response.
pub fn annotation_page_response(
    subject_type: &str,
    subject_id: &str,
    total_count: u64,
    latest: Option<(&str, &str)>,
    annotations: Vec<Annotation>,
    current_claim: Option<RemediationClaimSummary>,
    cursor: Option<String>,
) -> AnnotationPageResponse {
    AnnotationPageResponse {
        summary: annotation_page_summary(subject_type, subject_id, total_count, latest),
        annotations,
        current_claim,
        cursor,
    }
}

/// Build a remediation-claim list response.
pub fn remediation_claims_response(
    subject_type: &str,
    subject_id: &str,
    claims: Vec<RemediationClaim>,
    limit: usize,
    current_claim: Option<RemediationClaimSummary>,
    cursor: Option<String>,
) -> RemediationClaimsResponse {
    let claim_count = claims.len();
    let truncated = cursor.is_some();
    RemediationClaimsResponse {
        summary: format!("{claim_count} remediation claims on {subject_type} {subject_id}."),
        claims,
        limit,
        current_claim,
        cursor,
        truncated,
    }
}

/// Return whether a claim state represents active ownership.
pub fn claim_state_is_active(state: &str) -> bool {
    matches!(state, "claimed" | "investigating" | "fix_proposed")
}

/// Return whether a value is an accepted claim state.
pub fn claim_state_is_valid(state: &str) -> bool {
    REMEDIATION_CLAIM_STATES.contains(&state)
}

fn annotation_page_summary(
    subject_type: &str,
    subject_id: &str,
    total_count: u64,
    latest: Option<(&str, &str)>,
) -> String {
    let label = format!(
        "{total_count} {}",
        pluralize(total_count, "annotation", "annotations")
    );
    let subject = format!("{subject_type} {}", truncate_subject_id(subject_id));
    match latest {
        Some((agent, created_at)) => {
            format!("{label} on {subject}; latest from {agent} at {created_at}.")
        }
        None => format!("{label} on {subject}."),
    }
}

fn truncate_subject_id(subject_id: &str) -> String {
    if subject_id.len() > 16 {
        format!("{}…", &subject_id[..12])
    } else {
        subject_id.to_owned()
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
    current_claim: Option<RemediationClaimSummary>,
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
        current_claim,
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
    fn webhook_delivery_cursor_round_trip_matches_phoenix_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let cursor = WebhookDeliveryCursor {
            created_at: "2026-05-28T20:59:50Z".to_owned(),
            delivery_id: "DLV-b".to_owned(),
        };
        let Some(encoded) = encode_webhook_delivery_cursor(&cursor) else {
            return Err("webhook delivery cursor should encode".into());
        };

        assert_eq!(decode_webhook_delivery_cursor(&encoded), Some(cursor));
        assert_eq!(decode_webhook_delivery_cursor("bogus"), None);

        let malformed = BASE64_URL_SAFE_NO_PAD.encode(r#"{"created_at":1,"delivery_id":2}"#);
        assert_eq!(decode_webhook_delivery_cursor(&malformed), None);
        Ok(())
    }

    #[test]
    fn webhook_deliveries_response_counts_returned_rows() {
        let response = webhook_deliveries_response(
            vec![WebhookDelivery {
                delivery_id: "DLV-a".to_owned(),
                webhook_id: "WHK-a".to_owned(),
                tenant_id: "TENANT-bootstrap".to_owned(),
                project_id: "PROJECT-bootstrap".to_owned(),
                service: None,
                event: "error.new_class".to_owned(),
                status: "suppressed".to_owned(),
                attempt_count: 0,
                reason: Some("cooldown".to_owned()),
                first_attempt_at: None,
                last_attempt_at: None,
                delivered_at: None,
                discarded_at: None,
                completed_at: Some("2026-05-28T20:59:50Z".to_owned()),
                created_at: "2026-05-28T20:59:50Z".to_owned(),
                updated_at: "2026-05-28T20:59:50Z".to_owned(),
            }],
            Some("cursor".to_owned()),
        );

        assert_eq!(response.returned_count, 1);
        assert_eq!(response.cursor.as_deref(), Some("cursor"));
    }

    #[test]
    fn annotation_cursor_and_page_response_match_phoenix_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let cursor = AnnotationCursor {
            created_at: "2026-05-28T20:59:50Z".to_owned(),
            id: "ANN-b".to_owned(),
        };
        let Some(encoded) = encode_annotation_cursor(&cursor) else {
            return Err("annotation cursor should encode".into());
        };
        assert_eq!(decode_annotation_cursor(&encoded), Some(cursor));
        assert_eq!(decode_annotation_cursor("bogus"), None);

        let response = annotation_page_response(
            "target",
            "TGT-api",
            2,
            Some(("beta", "2026-05-28T20:59:50Z")),
            vec![Annotation {
                id: "ANN-b".to_owned(),
                subject_type: "target".to_owned(),
                subject_id: "TGT-api".to_owned(),
                incident_id: None,
                group_hash: None,
                agent: "beta".to_owned(),
                action: "ack".to_owned(),
                metadata: Some(serde_json::json!({"ticket": "OPS-1"})),
                created_at: "2026-05-28T20:59:50Z".to_owned(),
            }],
            None,
            Some("cursor".to_owned()),
        );

        assert_eq!(response.annotations.len(), 1);
        assert_eq!(response.cursor.as_deref(), Some("cursor"));
        assert!(response.summary.contains("2 annotations"));
        assert!(response.summary.contains("target"));
        assert!(response.summary.contains("beta"));
        assert!(response.summary.contains("2026-05-28T20:59:50Z"));
        Ok(())
    }

    #[test]
    fn report_cursor_round_trip_matches_phoenix_offset_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let cursor = ReportCursor {
            targets_offset: Some(5),
            monitor_offset: None,
            error_groups_offset: Some(10),
        };

        let encoded = encode_report_cursor(&cursor).ok_or("cursor should encode")?;
        assert_eq!(decode_report_cursor(&encoded), Some(cursor));
        assert_eq!(decode_report_cursor(""), Some(ReportCursor::default()));
        assert_eq!(decode_report_cursor("W10"), None);
        assert_eq!(
            encode_report_cursor(&ReportCursor {
                targets_offset: None,
                monitor_offset: None,
                error_groups_offset: None,
            }),
            None
        );
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
                    signal_kind: "operational".to_owned(),
                    signal_name: None,
                    entity_type: "error_group".to_owned(),
                    entity_ref: Some("group-a".to_owned()),
                    severity: Some("error".to_owned()),
                    summary: "summary".to_owned(),
                    payload: serde_json::json!({"event": "error.new_class"}),
                    attributes: serde_json::json!({}),
                    retention_class: "standard".to_owned(),
                    privacy_policy: "system".to_owned(),
                    sampling_policy: "unsampled".to_owned(),
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
