use canary_core::{
    health::state_machine::HealthState,
    query::{
        ActiveIncident, ActiveIncidentSignal, ActiveIncidents, ErrorClassAggregate,
        ErrorClassification, ErrorDetail, ErrorDetailGroup, ErrorGroupSummary, ErrorsByClass,
        ErrorsByErrorClass, ErrorsByService, IncidentAnnotation, IncidentDetail,
        IncidentDetailIncident, IncidentDetailSignal, IncidentTimelineEvent, QueryCursor,
        QueryWindow, TimelineCursor, TimelineEvent, TimelineResponse, active_incidents_response,
        decode_cursor, decode_timeline_cursor, encode_timeline_cursor, error_detail_response,
        errors_by_class_response, errors_by_error_class_response, errors_by_service_response,
        incident_detail_response, timeline_response,
    },
    webhook_events,
};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_INCIDENT_SIGNALS: usize = 25;
const MAX_INCIDENT_ANNOTATIONS: usize = 20;
const MAX_INCIDENT_TIMELINE_EVENTS: usize = 5;

/// Optional filters for service error queries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceQueryOptions {
    /// Tenant namespace for read isolation.
    pub tenant_id: Option<String>,
    /// Project namespace for read isolation.
    pub project_id: Option<String>,
    /// Optional pagination cursor.
    pub cursor: Option<String>,
    /// Optional annotation action that must exist for the group.
    pub with_annotation: Option<String>,
    /// Optional annotation action that must not exist for the group.
    pub without_annotation: Option<String>,
}

/// Optional filters for active incident queries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IncidentListOptions {
    /// Tenant namespace for read isolation.
    pub tenant_id: Option<String>,
    /// Project namespace for read isolation.
    pub project_id: Option<String>,
    /// Optional annotation action that must exist for the incident.
    pub with_annotation: Option<String>,
    /// Optional annotation action that must not exist for the incident.
    pub without_annotation: Option<String>,
}

/// Error-summary row returned by combined service status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorSummaryItem {
    /// Service name.
    pub service: String,
    /// Total active errors for the window.
    pub total_count: i64,
    /// Number of active error groups for the service.
    pub unique_classes: i64,
}

/// Recent health transition row returned by the unified report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentTransition {
    /// Surface type, `target` or `monitor`.
    pub entity_type: String,
    /// Surface id.
    pub entity_ref: String,
    /// Surface display name.
    pub name: String,
    /// Service name resolved through Phoenix's fallback.
    pub service: String,
    /// Current state.
    pub state: String,
    /// Transition timestamp.
    pub transitioned_at: String,
}

/// Error full-text search result embedded in the unified report.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    /// Error id.
    pub id: String,
    /// Service name.
    pub service: String,
    /// Error class.
    pub error_class: String,
    /// Error message.
    pub message: String,
    /// Error-group hash.
    pub group_hash: String,
    /// Error timestamp.
    pub created_at: String,
    /// FTS score.
    pub score: f64,
}

/// Query read-model failure.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    /// Query window is outside Canary's closed set.
    #[error("invalid query window")]
    InvalidWindow,
    /// SQLite rejected a read query.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Result type returned by query read models.
pub type QueryResult<T> = std::result::Result<T, QueryError>;

/// Error-group query failure.
#[derive(Debug, thiserror::Error)]
pub enum ErrorGroupQueryError {
    /// Query window is outside Canary's closed set.
    #[error("invalid query window")]
    InvalidWindow,
    /// Cursor is not a valid Phoenix query cursor.
    #[error("invalid query cursor")]
    InvalidCursor,
    /// SQLite rejected a read query.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Result type returned by error-group read models.
pub type ErrorGroupQueryResult<T> = std::result::Result<T, ErrorGroupQueryError>;

/// Timeline read-model failure.
#[derive(Debug, thiserror::Error)]
pub enum TimelineQueryError {
    /// Query window is outside Canary's closed set.
    #[error("invalid query window")]
    InvalidWindow,
    /// Requested limit is outside Phoenix's accepted range.
    #[error("invalid timeline limit")]
    InvalidLimit,
    /// Cursor is not a valid Phoenix timeline cursor.
    #[error("invalid timeline cursor")]
    InvalidCursor,
    /// Event type filter includes one or more non-business events.
    #[error("invalid timeline event types: {0:?}")]
    InvalidEventType(Vec<String>),
    /// SQLite rejected a read query.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Result type returned by timeline read models.
pub type TimelineQueryResult<T> = std::result::Result<T, TimelineQueryError>;

/// Optional filters for timeline queries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimelineQueryOptions {
    /// Tenant namespace for read isolation.
    pub tenant_id: Option<String>,
    /// Project namespace for read isolation.
    pub project_id: Option<String>,
    /// Optional service filter. Empty strings are treated as absent.
    pub service: Option<String>,
    /// Optional limit. Defaults to Phoenix's 50-row page size.
    pub limit: Option<String>,
    /// Optional cursor. Empty strings are treated as absent.
    pub cursor: Option<String>,
    /// Optional comma-separated business event filters.
    pub event_type: Option<String>,
}

pub(crate) fn errors_by_service(
    connection: &Connection,
    service: &str,
    window: &str,
    options: ServiceQueryOptions,
) -> ErrorGroupQueryResult<ErrorsByService> {
    errors_by_service_at(
        connection,
        service,
        window,
        options,
        OffsetDateTime::now_utc(),
    )
}

pub(crate) fn errors_by_service_at(
    connection: &Connection,
    service: &str,
    window: &str,
    options: ServiceQueryOptions,
    now: OffsetDateTime,
) -> ErrorGroupQueryResult<ErrorsByService> {
    let window = QueryWindow::parse(window).ok_or(ErrorGroupQueryError::InvalidWindow)?;
    let groups = list_error_groups(
        connection,
        ErrorGroupFilter::Service {
            service: service.to_owned(),
        },
        window,
        options,
        now,
    )?;

    Ok(errors_by_service_response(
        service.to_owned(),
        window,
        groups,
    ))
}

pub(crate) fn errors_by_error_class(
    connection: &Connection,
    error_class: &str,
    window: &str,
    service: Option<&str>,
    options: ServiceQueryOptions,
) -> ErrorGroupQueryResult<ErrorsByErrorClass> {
    errors_by_error_class_at(
        connection,
        error_class,
        window,
        service,
        options,
        OffsetDateTime::now_utc(),
    )
}

pub(crate) fn errors_by_error_class_at(
    connection: &Connection,
    error_class: &str,
    window: &str,
    service: Option<&str>,
    options: ServiceQueryOptions,
    now: OffsetDateTime,
) -> ErrorGroupQueryResult<ErrorsByErrorClass> {
    let window = QueryWindow::parse(window).ok_or(ErrorGroupQueryError::InvalidWindow)?;
    let groups = list_error_groups(
        connection,
        ErrorGroupFilter::ErrorClass {
            error_class: error_class.to_owned(),
            service: service.map(ToOwned::to_owned),
        },
        window,
        options,
        now,
    )?;

    Ok(errors_by_error_class_response(
        error_class.to_owned(),
        window,
        groups,
    ))
}

pub(crate) fn errors_by_class(connection: &Connection, window: &str) -> QueryResult<ErrorsByClass> {
    errors_by_class_at(connection, window, OffsetDateTime::now_utc())
}

pub(crate) fn errors_by_class_scoped(
    connection: &Connection,
    window: &str,
    tenant_id: &str,
    project_id: &str,
) -> QueryResult<ErrorsByClass> {
    errors_by_class_at_scoped(
        connection,
        window,
        OffsetDateTime::now_utc(),
        Some((tenant_id, project_id)),
    )
}

pub(crate) fn errors_by_class_at(
    connection: &Connection,
    window: &str,
    now: OffsetDateTime,
) -> QueryResult<ErrorsByClass> {
    errors_by_class_at_scoped(connection, window, now, None)
}

fn errors_by_class_at_scoped(
    connection: &Connection,
    window: &str,
    now: OffsetDateTime,
    owner: Option<(&str, &str)>,
) -> QueryResult<ErrorsByClass> {
    let window = QueryWindow::parse(window).ok_or(QueryError::InvalidWindow)?;
    let cutoff = window.cutoff_at(now);
    let groups = error_class_aggregates(connection, &cutoff, owner)?;
    let (total_errors, total_error_classes) = error_class_totals(connection, &cutoff, owner)?;

    Ok(errors_by_class_response(
        window,
        groups,
        total_errors,
        total_error_classes,
    ))
}

pub(crate) fn error_summary(
    connection: &Connection,
    window: &str,
) -> QueryResult<Vec<ErrorSummaryItem>> {
    error_summary_at(connection, window, OffsetDateTime::now_utc())
}

pub(crate) fn error_summary_scoped(
    connection: &Connection,
    window: &str,
    tenant_id: &str,
    project_id: &str,
) -> QueryResult<Vec<ErrorSummaryItem>> {
    error_summary_at_scoped(
        connection,
        window,
        OffsetDateTime::now_utc(),
        Some((tenant_id, project_id)),
    )
}

pub(crate) fn error_summary_at(
    connection: &Connection,
    window: &str,
    now: OffsetDateTime,
) -> QueryResult<Vec<ErrorSummaryItem>> {
    error_summary_at_scoped(connection, window, now, None)
}

fn error_summary_at_scoped(
    connection: &Connection,
    window: &str,
    now: OffsetDateTime,
    owner: Option<(&str, &str)>,
) -> QueryResult<Vec<ErrorSummaryItem>> {
    let window = QueryWindow::parse(window).ok_or(QueryError::InvalidWindow)?;
    let cutoff = window.cutoff_at(now);
    let owner_clause = owner_clause("error_groups", 2);
    let sql = if owner.is_some() {
        format!(
            "SELECT service, SUM(total_count), COUNT(group_hash)
             FROM error_groups
             WHERE last_seen_at >= ?1 AND status = 'active'
             {owner_clause}
             GROUP BY service
             ORDER BY SUM(total_count) DESC"
        )
    } else {
        "SELECT service, SUM(total_count), COUNT(group_hash)
         FROM error_groups
         WHERE last_seen_at >= ?1 AND status = 'active'
         GROUP BY service
         ORDER BY SUM(total_count) DESC"
            .to_owned()
    };
    let mut statement = connection.prepare(&sql)?;
    let rows = if let Some((tenant_id, project_id)) = owner {
        statement.query_map(params![cutoff, tenant_id, project_id], error_summary_row)?
    } else {
        statement.query_map(params![cutoff], error_summary_row)?
    };
    let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn error_summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ErrorSummaryItem> {
    Ok(ErrorSummaryItem {
        service: row.get(0)?,
        total_count: row.get(1)?,
        unique_classes: row.get(2)?,
    })
}

pub(crate) fn report_error_groups(
    connection: &Connection,
    window: &str,
) -> QueryResult<Vec<ErrorGroupSummary>> {
    report_error_groups_at(connection, window, OffsetDateTime::now_utc())
}

pub(crate) fn report_error_groups_scoped(
    connection: &Connection,
    window: &str,
    tenant_id: &str,
    project_id: &str,
) -> QueryResult<Vec<ErrorGroupSummary>> {
    report_error_groups_at_scoped(
        connection,
        window,
        OffsetDateTime::now_utc(),
        Some((tenant_id, project_id)),
    )
}

pub(crate) fn report_error_groups_at(
    connection: &Connection,
    window: &str,
    now: OffsetDateTime,
) -> QueryResult<Vec<ErrorGroupSummary>> {
    report_error_groups_at_scoped(connection, window, now, None)
}

fn report_error_groups_at_scoped(
    connection: &Connection,
    window: &str,
    now: OffsetDateTime,
    owner: Option<(&str, &str)>,
) -> QueryResult<Vec<ErrorGroupSummary>> {
    let window = QueryWindow::parse(window).ok_or(QueryError::InvalidWindow)?;
    let cutoff = window.cutoff_at(now);
    let owner_clause = owner_clause("g", 2);
    let sql = if owner.is_some() {
        format!(
            "SELECT
                g.group_hash,
                g.error_class,
                g.service,
                g.total_count,
                g.first_seen_at,
                g.last_seen_at,
                g.message_template,
                g.severity,
                g.status,
                e.classification_category,
                e.classification_persistence,
                e.classification_component
             FROM error_groups g
             LEFT JOIN errors e ON e.id = g.last_error_id
             WHERE g.last_seen_at >= ?1 AND g.status = 'active'
             {owner_clause}
             ORDER BY g.total_count DESC, g.service ASC, g.error_class ASC
             LIMIT 50"
        )
    } else {
        "SELECT
            g.group_hash,
            g.error_class,
            g.service,
            g.total_count,
            g.first_seen_at,
            g.last_seen_at,
            g.message_template,
            g.severity,
            g.status,
            e.classification_category,
            e.classification_persistence,
            e.classification_component
         FROM error_groups g
         LEFT JOIN errors e ON e.id = g.last_error_id
         WHERE g.last_seen_at >= ?1 AND g.status = 'active'
         ORDER BY g.total_count DESC, g.service ASC, g.error_class ASC
         LIMIT 50"
            .to_owned()
    };
    let mut statement = connection.prepare(&sql)?;
    if let Some((tenant_id, project_id)) = owner {
        groups_from_rows(
            statement.query_map(params![cutoff, tenant_id, project_id], group_from_row)?,
        )
    } else {
        groups_from_rows(statement.query_map(params![cutoff], group_from_row)?)
    }
}

pub(crate) fn recent_transitions(
    connection: &Connection,
    window: &str,
) -> QueryResult<Vec<RecentTransition>> {
    recent_transitions_at(connection, window, OffsetDateTime::now_utc())
}

pub(crate) fn recent_transitions_scoped(
    connection: &Connection,
    window: &str,
    tenant_id: &str,
    project_id: &str,
) -> QueryResult<Vec<RecentTransition>> {
    recent_transitions_at_scoped(
        connection,
        window,
        OffsetDateTime::now_utc(),
        Some((tenant_id, project_id)),
    )
}

pub(crate) fn recent_transitions_at(
    connection: &Connection,
    window: &str,
    now: OffsetDateTime,
) -> QueryResult<Vec<RecentTransition>> {
    recent_transitions_at_scoped(connection, window, now, None)
}

fn recent_transitions_at_scoped(
    connection: &Connection,
    window: &str,
    now: OffsetDateTime,
    owner: Option<(&str, &str)>,
) -> QueryResult<Vec<RecentTransition>> {
    let window = QueryWindow::parse(window).ok_or(QueryError::InvalidWindow)?;
    let cutoff = window.cutoff_at(now);
    let target_owner_clause = owner_clause("t", 2);
    let monitor_owner_clause = owner_clause("m", 2);
    let sql = if owner.is_some() {
        format!(
            "SELECT 'target', t.id, t.name, COALESCE(NULLIF(t.service, ''), t.name), s.state, s.last_transition_at
             FROM targets t
             JOIN target_state s ON s.target_id = t.id
             WHERE s.last_transition_at >= ?1
             {target_owner_clause}
             UNION ALL
             SELECT 'monitor', m.id, m.name, COALESCE(NULLIF(m.service, ''), m.name), s.state, s.last_transition_at
             FROM monitors m
             JOIN monitor_state s ON s.monitor_id = m.id
             WHERE s.last_transition_at >= ?1
             {monitor_owner_clause}
             ORDER BY 6 DESC, 1 DESC, 3 DESC"
        )
    } else {
        "SELECT 'target', t.id, t.name, COALESCE(NULLIF(t.service, ''), t.name), s.state, s.last_transition_at
         FROM targets t
         JOIN target_state s ON s.target_id = t.id
         WHERE s.last_transition_at >= ?1
         UNION ALL
         SELECT 'monitor', m.id, m.name, COALESCE(NULLIF(m.service, ''), m.name), s.state, s.last_transition_at
         FROM monitors m
         JOIN monitor_state s ON s.monitor_id = m.id
         WHERE s.last_transition_at >= ?1
         ORDER BY 6 DESC, 1 DESC, 3 DESC"
            .to_owned()
    };
    let mut statement = connection.prepare(&sql)?;
    let rows = if let Some((tenant_id, project_id)) = owner {
        statement.query_map(
            params![cutoff, tenant_id, project_id],
            recent_transition_row,
        )?
    } else {
        statement.query_map(params![cutoff], recent_transition_row)?
    };
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn recent_transition_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecentTransition> {
    Ok(RecentTransition {
        entity_type: row.get(0)?,
        entity_ref: row.get(1)?,
        name: row.get(2)?,
        service: row.get(3)?,
        state: row.get(4)?,
        transitioned_at: row.get(5)?,
    })
}

pub(crate) fn search_errors(
    connection: &Connection,
    query: &str,
    window: &str,
) -> QueryResult<Vec<SearchResult>> {
    search_errors_at(connection, query, window, OffsetDateTime::now_utc())
}

pub(crate) fn search_errors_scoped(
    connection: &Connection,
    query: &str,
    window: &str,
    tenant_id: &str,
    project_id: &str,
) -> QueryResult<Vec<SearchResult>> {
    search_errors_at_scoped(
        connection,
        query,
        window,
        OffsetDateTime::now_utc(),
        Some((tenant_id, project_id)),
    )
}

pub(crate) fn search_errors_at(
    connection: &Connection,
    query: &str,
    window: &str,
    now: OffsetDateTime,
) -> QueryResult<Vec<SearchResult>> {
    search_errors_at_scoped(connection, query, window, now, None)
}

fn search_errors_at_scoped(
    connection: &Connection,
    query: &str,
    window: &str,
    now: OffsetDateTime,
    owner: Option<(&str, &str)>,
) -> QueryResult<Vec<SearchResult>> {
    let window = QueryWindow::parse(window).ok_or(QueryError::InvalidWindow)?;
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let cutoff = window.cutoff_at(now);
    let quoted = format!("\"{}\"", trimmed.replace('"', "\"\""));
    let owner_clause = owner_clause("e", 3);
    let sql = if owner.is_some() {
        format!(
            "SELECT e.id, e.service, e.error_class, e.message, e.group_hash, e.created_at,
                    -bm25(errors_fts, 1.0, 2.0, 5.0, 1.0) AS score
             FROM errors_fts
             JOIN errors AS e ON e.rowid = errors_fts.rowid
             WHERE errors_fts MATCH ?1 AND e.created_at >= ?2
             {owner_clause}
             ORDER BY score DESC, e.created_at DESC
             LIMIT 20"
        )
    } else {
        "SELECT e.id, e.service, e.error_class, e.message, e.group_hash, e.created_at,
                -bm25(errors_fts, 1.0, 2.0, 5.0, 1.0) AS score
         FROM errors_fts
         JOIN errors AS e ON e.rowid = errors_fts.rowid
         WHERE errors_fts MATCH ?1 AND e.created_at >= ?2
         ORDER BY score DESC, e.created_at DESC
         LIMIT 20"
            .to_owned()
    };
    let mut statement = match connection.prepare(&sql) {
        Ok(statement) => statement,
        Err(rusqlite::Error::SqliteFailure(_, _)) => return Ok(Vec::new()),
        Err(error) => return Err(QueryError::Sqlite(error)),
    };
    let rows = match if let Some((tenant_id, project_id)) = owner {
        statement.query_map(
            params![quoted, cutoff, tenant_id, project_id],
            search_result_row,
        )
    } else {
        statement.query_map(params![quoted, cutoff], search_result_row)
    } {
        Ok(rows) => rows,
        Err(rusqlite::Error::SqliteFailure(_, _)) => return Ok(Vec::new()),
        Err(error) => return Err(QueryError::Sqlite(error)),
    };
    match rows.collect::<rusqlite::Result<Vec<_>>>() {
        Ok(rows) => Ok(rows),
        Err(rusqlite::Error::SqliteFailure(_, _)) => Ok(Vec::new()),
        Err(error) => Err(QueryError::Sqlite(error)),
    }
}

fn search_result_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchResult> {
    Ok(SearchResult {
        id: row.get(0)?,
        service: row.get(1)?,
        error_class: row.get(2)?,
        message: row.get(3)?,
        group_hash: row.get(4)?,
        created_at: row.get(5)?,
        score: row.get(6)?,
    })
}

pub(crate) fn timeline(
    connection: &Connection,
    window: &str,
    options: TimelineQueryOptions,
) -> TimelineQueryResult<TimelineResponse> {
    timeline_at(connection, window, options, OffsetDateTime::now_utc())
}

pub(crate) fn timeline_at(
    connection: &Connection,
    window: &str,
    options: TimelineQueryOptions,
    now: OffsetDateTime,
) -> TimelineQueryResult<TimelineResponse> {
    let window = QueryWindow::parse(window).ok_or(TimelineQueryError::InvalidWindow)?;
    let cutoff = window.cutoff_at(now);
    let limit = parse_timeline_limit(options.limit.as_deref())?;
    let cursor = parse_timeline_cursor(options.cursor.as_deref())?;
    let event_types = parse_timeline_event_types(options.event_type.as_deref())?;
    let service = options.service.filter(|service| !service.is_empty());
    let tenant_id = options.tenant_id.filter(|tenant_id| !tenant_id.is_empty());
    let project_id = options
        .project_id
        .filter(|project_id| !project_id.is_empty());

    let mut sql = String::from(
        "SELECT id, service, event, entity_type, entity_ref, severity, summary, payload, created_at
         FROM service_events
         WHERE created_at >= ?",
    );
    let mut filters = vec![cutoff];

    if let (Some(tenant_id), Some(project_id)) = (tenant_id.as_deref(), project_id.as_deref()) {
        sql.push_str(" AND tenant_id = ? AND project_id = ?");
        filters.push(tenant_id.to_owned());
        filters.push(project_id.to_owned());
    }
    if let Some(service) = service.as_deref() {
        sql.push_str(" AND service = ?");
        filters.push(service.to_owned());
    }
    if let Some(event_types) = event_types.as_ref()
        && !event_types.is_empty()
    {
        let placeholders = std::iter::repeat_n("?", event_types.len())
            .collect::<Vec<_>>()
            .join(", ");
        sql.push_str(" AND event IN (");
        sql.push_str(&placeholders);
        sql.push(')');
        filters.extend(event_types.iter().cloned());
    }
    if let Some(cursor) = cursor.as_ref() {
        sql.push_str(" AND (created_at < ? OR (created_at = ? AND id < ?))");
        filters.push(cursor.created_at.clone());
        filters.push(cursor.created_at.clone());
        filters.push(cursor.id.clone());
    }

    sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
    filters.push((limit + 1).to_string());

    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(filters), |row| {
        let payload_json: String = row.get(7)?;
        Ok(TimelineEvent {
            id: row.get(0)?,
            service: row.get(1)?,
            event: row.get(2)?,
            entity_type: row.get(3)?,
            entity_ref: row.get(4)?,
            severity: row.get(5)?,
            summary: row.get(6)?,
            payload: safe_decode_json(Some(payload_json)).unwrap_or(Value::Null),
            created_at: row.get(8)?,
        })
    })?;
    let mut rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let cursor = if rows.len() > limit {
        rows.truncate(limit);
        rows.last().and_then(|event| {
            encode_timeline_cursor(&TimelineCursor {
                created_at: event.created_at.clone(),
                id: event.id.clone(),
            })
        })
    } else {
        None
    };

    Ok(timeline_response(rows, service, window, cursor))
}

pub(crate) fn error_detail(
    connection: &Connection,
    error_id: &str,
) -> QueryResult<Option<ErrorDetail>> {
    error_detail_for_owner(connection, error_id, None)
}

pub(crate) fn error_detail_scoped(
    connection: &Connection,
    error_id: &str,
    tenant_id: &str,
    project_id: &str,
) -> QueryResult<Option<ErrorDetail>> {
    error_detail_for_owner(connection, error_id, Some((tenant_id, project_id)))
}

fn error_detail_for_owner(
    connection: &Connection,
    error_id: &str,
    owner: Option<(&str, &str)>,
) -> QueryResult<Option<ErrorDetail>> {
    let Some(row) = error_row(connection, error_id, owner)? else {
        return Ok(None);
    };
    let group = group_detail(connection, &row.group_hash, owner)?;
    let incident_ids = incident_ids_for_group(connection, &row.group_hash, owner)?;
    let (count, first_seen, last_seen) = group
        .as_ref()
        .map(|group| {
            (
                group.total_count,
                group.first_seen_at.clone(),
                group.last_seen_at.clone(),
            )
        })
        .unwrap_or((1, row.created_at.clone(), row.created_at.clone()));

    let detail = ErrorDetail {
        summary: String::new(),
        id: row.id,
        service: row.service,
        error_class: row.error_class,
        message: row.message,
        message_template: row.message_template,
        stack_trace: row.stack_trace,
        context: safe_decode_json(row.context),
        severity: row.severity,
        environment: row.environment,
        group_hash: row.group_hash,
        created_at: row.created_at,
        group,
        incident_ids,
    };

    Ok(Some(error_detail_response(
        detail, count, first_seen, last_seen,
    )))
}

pub(crate) fn active_incidents(
    connection: &Connection,
    options: IncidentListOptions,
) -> QueryResult<ActiveIncidents> {
    active_incidents_at(connection, options, OffsetDateTime::now_utc())
}

pub(crate) fn active_incidents_at(
    connection: &Connection,
    options: IncidentListOptions,
    now: OffsetDateTime,
) -> QueryResult<ActiveIncidents> {
    let rows = incident_rows(
        connection,
        options
            .tenant_id
            .as_deref()
            .zip(options.project_id.as_deref()),
    )?;
    let mut incidents = Vec::new();

    for row in rows {
        if !incident_matches_annotation_filters(connection, &row.id, &options)? {
            continue;
        }

        let signals = incident_signals(connection, &row.id)?;
        let signal_owner = row.owner();
        let active_signals = active_signals(connection, signals, signal_owner, now)?;

        if active_signals.is_empty() {
            continue;
        }

        let severity = incident_severity(&active_signals, now);
        let signal_count = active_signals.len();
        incidents.push(ActiveIncident {
            id: row.id,
            service: row.service,
            state: "investigating".to_owned(),
            severity,
            title: row.title,
            opened_at: row.opened_at,
            resolved_at: row.resolved_at,
            signal_count,
            signals: active_signals,
        });
    }

    Ok(active_incidents_response(incidents))
}

pub(crate) fn incident_detail(
    connection: &Connection,
    incident_id: &str,
) -> QueryResult<Option<IncidentDetail>> {
    incident_detail_for_owner(connection, incident_id, None)
}

pub(crate) fn incident_detail_scoped(
    connection: &Connection,
    incident_id: &str,
    tenant_id: &str,
    project_id: &str,
) -> QueryResult<Option<IncidentDetail>> {
    incident_detail_for_owner(connection, incident_id, Some((tenant_id, project_id)))
}

fn incident_detail_for_owner(
    connection: &Connection,
    incident_id: &str,
    owner: Option<(&str, &str)>,
) -> QueryResult<Option<IncidentDetail>> {
    let Some(incident) = incident_detail_incident(connection, incident_id, owner)? else {
        return Ok(None);
    };

    let total_signals = count_incident_signals(connection, incident_id)?;
    let signal_rows = incident_detail_signals(connection, incident_id, MAX_INCIDENT_SIGNALS)?;
    let signals_truncated = total_signals > signal_rows.len();
    let signal_owner = incident.owner();
    let signal_context = load_incident_signal_context(connection, &signal_rows, signal_owner)?;
    let signals = signal_rows
        .iter()
        .map(|signal| format_incident_signal(signal, &signal_context))
        .collect::<Vec<_>>();
    let (annotations, annotations_truncated) =
        incident_annotations(connection, incident_id, MAX_INCIDENT_ANNOTATIONS)?;
    let timeline = incident_timeline_events(connection, incident_id, MAX_INCIDENT_TIMELINE_EVENTS)?;

    Ok(Some(incident_detail_response(
        IncidentDetailIncident {
            id: incident.id,
            service: incident.service,
            state: incident.state,
            severity: incident.severity,
            title: incident.title,
            opened_at: incident.opened_at,
            resolved_at: incident.resolved_at,
            signal_count: total_signals,
        },
        signals,
        signals_truncated,
        annotations,
        annotations_truncated,
        timeline,
    )))
}

fn error_class_aggregates(
    connection: &Connection,
    cutoff: &str,
    owner: Option<(&str, &str)>,
) -> QueryResult<Vec<ErrorClassAggregate>> {
    let owner_clause = owner_clause("error_groups", 2);
    let sql = if owner.is_some() {
        format!(
            "SELECT error_class, COALESCE(SUM(total_count), 0), COUNT(DISTINCT service)
             FROM error_groups
             WHERE last_seen_at >= ?1 AND status = 'active'
             {owner_clause}
             GROUP BY error_class
             ORDER BY SUM(total_count) DESC, error_class ASC
             LIMIT 50"
        )
    } else {
        "SELECT error_class, COALESCE(SUM(total_count), 0), COUNT(DISTINCT service)
         FROM error_groups
         WHERE last_seen_at >= ?1 AND status = 'active'
         GROUP BY error_class
         ORDER BY SUM(total_count) DESC, error_class ASC
         LIMIT 50"
            .to_owned()
    };
    let mut statement = connection.prepare(&sql)?;
    let rows = if let Some((tenant_id, project_id)) = owner {
        statement.query_map(
            params![cutoff, tenant_id, project_id],
            error_class_aggregate_row,
        )?
    } else {
        statement.query_map([cutoff], error_class_aggregate_row)?
    };
    let groups = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(groups)
}

fn error_class_aggregate_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ErrorClassAggregate> {
    Ok(ErrorClassAggregate {
        error_class: row.get(0)?,
        total_count: row.get(1)?,
        service_count: row.get(2)?,
    })
}

fn error_class_totals(
    connection: &Connection,
    cutoff: &str,
    owner: Option<(&str, &str)>,
) -> QueryResult<(u64, u64)> {
    let owner_clause = owner_clause("error_groups", 2);
    let sql = if owner.is_some() {
        format!(
            "SELECT COALESCE(SUM(total_count), 0), COUNT(DISTINCT error_class)
             FROM error_groups
             WHERE last_seen_at >= ?1 AND status = 'active'
             {owner_clause}"
        )
    } else {
        "SELECT COALESCE(SUM(total_count), 0), COUNT(DISTINCT error_class)
         FROM error_groups
         WHERE last_seen_at >= ?1 AND status = 'active'"
            .to_owned()
    };
    let mut statement = connection.prepare(&sql)?;
    let row = if let Some((tenant_id, project_id)) = owner {
        statement.query_row(params![cutoff, tenant_id, project_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
    } else {
        statement.query_row([cutoff], |row| Ok((row.get(0)?, row.get(1)?)))?
    };
    Ok(row)
}

#[derive(Debug)]
struct IncidentRow {
    id: String,
    tenant_id: String,
    project_id: String,
    service: String,
    title: Option<String>,
    opened_at: String,
    resolved_at: Option<String>,
}

impl IncidentRow {
    fn owner(&self) -> Option<(&str, &str)> {
        (!self.tenant_id.is_empty() && !self.project_id.is_empty())
            .then_some((self.tenant_id.as_str(), self.project_id.as_str()))
    }
}

#[derive(Debug)]
struct IncidentSignalRow {
    signal_type: String,
    signal_ref: String,
    attached_at: String,
    resolved_at: Option<String>,
}

#[derive(Debug)]
struct IncidentDetailRow {
    id: String,
    tenant_id: String,
    project_id: String,
    service: String,
    state: String,
    severity: String,
    title: Option<String>,
    opened_at: String,
    resolved_at: Option<String>,
}

impl IncidentDetailRow {
    fn owner(&self) -> Option<(&str, &str)> {
        (!self.tenant_id.is_empty() && !self.project_id.is_empty())
            .then_some((self.tenant_id.as_str(), self.project_id.as_str()))
    }
}

#[derive(Debug)]
struct IncidentDetailSignalRow {
    signal_type: String,
    signal_ref: String,
    attached_at: String,
    resolved_at: Option<String>,
}

#[derive(Debug)]
struct ErrorGroupSignalContext {
    error_class: String,
    total_count: u64,
    first_seen_at: String,
    last_seen_at: String,
    classification: ErrorClassification,
}

#[derive(Debug)]
struct TargetSignalContext {
    name: String,
    current_state: String,
    consecutive_failures: u64,
}

#[derive(Debug)]
struct MonitorSignalContext {
    name: String,
    current_state: String,
}

#[derive(Debug, Default)]
struct IncidentSignalContext {
    error_groups: std::collections::HashMap<String, ErrorGroupSignalContext>,
    targets: std::collections::HashMap<String, TargetSignalContext>,
    monitors: std::collections::HashMap<String, MonitorSignalContext>,
    annotation_counts: std::collections::HashMap<(String, String), u64>,
}

fn incident_rows(
    connection: &Connection,
    owner: Option<(&str, &str)>,
) -> QueryResult<Vec<IncidentRow>> {
    let owner_clause = owner_clause("incidents", 1);
    let sql = if owner.is_some() {
        format!(
            "SELECT id, tenant_id, project_id, service, title, opened_at, resolved_at
             FROM incidents
             WHERE state != 'resolved'
             {owner_clause}
             ORDER BY opened_at DESC"
        )
    } else {
        "SELECT id, service, title, opened_at, resolved_at
         FROM incidents
         WHERE state != 'resolved'
         ORDER BY opened_at DESC"
            .to_owned()
    };
    let mut statement = connection.prepare(&sql)?;
    if let Some((tenant_id, project_id)) = owner {
        let rows = statement.query_map(params![tenant_id, project_id], incident_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    } else {
        let rows = statement.query_map([], legacy_incident_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

fn incident_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IncidentRow> {
    Ok(IncidentRow {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        project_id: row.get(2)?,
        service: row.get(3)?,
        title: row.get(4)?,
        opened_at: row.get(5)?,
        resolved_at: row.get(6)?,
    })
}

fn legacy_incident_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IncidentRow> {
    Ok(IncidentRow {
        id: row.get(0)?,
        tenant_id: String::new(),
        project_id: String::new(),
        service: row.get(1)?,
        title: row.get(2)?,
        opened_at: row.get(3)?,
        resolved_at: row.get(4)?,
    })
}

fn incident_signals(
    connection: &Connection,
    incident_id: &str,
) -> QueryResult<Vec<IncidentSignalRow>> {
    let mut statement = connection.prepare(
        "SELECT signal_type, signal_ref, attached_at, resolved_at
         FROM incident_signals
         WHERE incident_id = ?1
         ORDER BY attached_at ASC, id ASC",
    )?;
    Ok(statement
        .query_map([incident_id], |row| {
            Ok(IncidentSignalRow {
                signal_type: row.get(0)?,
                signal_ref: row.get(1)?,
                attached_at: row.get(2)?,
                resolved_at: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn incident_detail_incident(
    connection: &Connection,
    incident_id: &str,
    owner: Option<(&str, &str)>,
) -> QueryResult<Option<IncidentDetailRow>> {
    let owner_clause = if owner.is_some() {
        " AND tenant_id = ?2 AND project_id = ?3"
    } else {
        ""
    };
    let sql = if owner.is_some() {
        format!(
            "SELECT id, tenant_id, project_id, service, state, severity, title, opened_at, resolved_at
             FROM incidents
             WHERE id = ?1{owner_clause}"
        )
    } else {
        "SELECT id, service, state, severity, title, opened_at, resolved_at
         FROM incidents
         WHERE id = ?1"
            .to_owned()
    };
    let mut statement = connection.prepare(&sql)?;
    let result = if let Some((tenant_id, project_id)) = owner {
        statement.query_row(
            params![incident_id, tenant_id, project_id],
            incident_detail_row,
        )
    } else {
        statement.query_row([incident_id], legacy_incident_detail_row)
    };
    Ok(result.optional()?)
}

fn incident_detail_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IncidentDetailRow> {
    Ok(IncidentDetailRow {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        project_id: row.get(2)?,
        service: row.get(3)?,
        state: row.get(4)?,
        severity: row.get(5)?,
        title: row.get(6)?,
        opened_at: row.get(7)?,
        resolved_at: row.get(8)?,
    })
}

fn legacy_incident_detail_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IncidentDetailRow> {
    Ok(IncidentDetailRow {
        id: row.get(0)?,
        tenant_id: String::new(),
        project_id: String::new(),
        service: row.get(1)?,
        state: row.get(2)?,
        severity: row.get(3)?,
        title: row.get(4)?,
        opened_at: row.get(5)?,
        resolved_at: row.get(6)?,
    })
}

fn count_incident_signals(connection: &Connection, incident_id: &str) -> QueryResult<usize> {
    let count = connection.query_row(
        "SELECT COUNT(*) FROM incident_signals WHERE incident_id = ?1",
        [incident_id],
        |row| row.get::<_, u64>(0),
    )?;
    Ok(count as usize)
}

fn incident_detail_signals(
    connection: &Connection,
    incident_id: &str,
    limit: usize,
) -> QueryResult<Vec<IncidentDetailSignalRow>> {
    let mut statement = connection.prepare(
        "SELECT signal_type, signal_ref, attached_at, resolved_at
         FROM incident_signals
         WHERE incident_id = ?1
         ORDER BY attached_at DESC, id DESC
         LIMIT ?2",
    )?;
    Ok(statement
        .query_map(params![incident_id, limit as u64], |row| {
            Ok(IncidentDetailSignalRow {
                signal_type: row.get(0)?,
                signal_ref: row.get(1)?,
                attached_at: row.get(2)?,
                resolved_at: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn load_incident_signal_context(
    connection: &Connection,
    signals: &[IncidentDetailSignalRow],
    owner: Option<(&str, &str)>,
) -> QueryResult<IncidentSignalContext> {
    let error_refs = signal_refs_for_detail(signals, "error_group");
    let health_refs = signal_refs_for_detail(signals, "health_transition");
    let target_refs = health_refs
        .iter()
        .filter(|reference| reference.starts_with("TGT-"))
        .cloned()
        .collect::<Vec<_>>();
    let monitor_refs = health_refs
        .iter()
        .filter(|reference| reference.starts_with("MON-"))
        .cloned()
        .collect::<Vec<_>>();

    Ok(IncidentSignalContext {
        error_groups: load_error_group_signal_context(connection, &error_refs, owner)?,
        targets: load_target_signal_context(connection, &target_refs)?,
        monitors: load_monitor_signal_context(connection, &monitor_refs)?,
        annotation_counts: load_signal_annotation_counts(connection, signals, owner)?,
    })
}

fn signal_refs_for_detail(signals: &[IncidentDetailSignalRow], signal_type: &str) -> Vec<String> {
    let mut refs = signals
        .iter()
        .filter(|signal| signal.signal_type == signal_type)
        .map(|signal| signal.signal_ref.clone())
        .collect::<Vec<_>>();
    refs.sort();
    refs.dedup();
    refs
}

fn load_error_group_signal_context(
    connection: &Connection,
    refs: &[String],
    owner: Option<(&str, &str)>,
) -> QueryResult<std::collections::HashMap<String, ErrorGroupSignalContext>> {
    if refs.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let owner_clause = if owner.is_some() {
        " AND g.tenant_id = ? AND g.project_id = ?"
    } else {
        ""
    };
    let mut statement = connection.prepare(&format!(
        "SELECT
            g.group_hash,
            g.error_class,
            g.total_count,
            g.first_seen_at,
            g.last_seen_at,
            e.classification_category,
            e.classification_persistence,
            e.classification_component
         FROM error_groups g
         LEFT JOIN errors e ON e.id = g.last_error_id
         WHERE g.group_hash IN ({})
         {owner_clause}",
        placeholders(refs.len()),
    ))?;
    let params = refs.iter().map(String::as_str).chain(
        owner
            .into_iter()
            .flat_map(|(tenant_id, project_id)| [tenant_id, project_id]),
    );
    let rows = statement.query_map(rusqlite::params_from_iter(params), |row| {
        Ok((
            row.get::<_, String>(0)?,
            ErrorGroupSignalContext {
                error_class: row.get(1)?,
                total_count: row.get(2)?,
                first_seen_at: row.get(3)?,
                last_seen_at: row.get(4)?,
                classification: ErrorClassification::new(row.get(5)?, row.get(6)?, row.get(7)?),
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn load_target_signal_context(
    connection: &Connection,
    refs: &[String],
) -> QueryResult<std::collections::HashMap<String, TargetSignalContext>> {
    if refs.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let mut statement = connection.prepare(
        "SELECT
            r.ref,
            COALESCE(t.name, r.ref),
            COALESCE(s.state, 'unknown'),
            COALESCE(s.consecutive_failures, 0)
         FROM (SELECT value AS ref FROM json_each(?1)) r
         LEFT JOIN targets t ON t.id = r.ref
         LEFT JOIN target_state s ON s.target_id = r.ref",
    )?;
    let refs_json = serde_json::to_string(refs).unwrap_or_else(|_| "[]".to_owned());
    let rows = statement.query_map([refs_json], |row| {
        Ok((
            row.get::<_, String>(0)?,
            TargetSignalContext {
                name: row.get(1)?,
                current_state: row.get(2)?,
                consecutive_failures: row.get(3)?,
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn load_monitor_signal_context(
    connection: &Connection,
    refs: &[String],
) -> QueryResult<std::collections::HashMap<String, MonitorSignalContext>> {
    if refs.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let mut statement = connection.prepare(
        "SELECT
            r.ref,
            COALESCE(m.name, r.ref),
            COALESCE(s.state, 'unknown')
         FROM (SELECT value AS ref FROM json_each(?1)) r
         LEFT JOIN monitors m ON m.id = r.ref
         LEFT JOIN monitor_state s ON s.monitor_id = r.ref",
    )?;
    let refs_json = serde_json::to_string(refs).unwrap_or_else(|_| "[]".to_owned());
    let rows = statement.query_map([refs_json], |row| {
        Ok((
            row.get::<_, String>(0)?,
            MonitorSignalContext {
                name: row.get(1)?,
                current_state: row.get(2)?,
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn load_signal_annotation_counts(
    connection: &Connection,
    signals: &[IncidentDetailSignalRow],
    owner: Option<(&str, &str)>,
) -> QueryResult<std::collections::HashMap<(String, String), u64>> {
    let subjects = signal_subjects(signals);
    if subjects.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let owner_clause = if owner.is_some() {
        " AND tenant_id = ?2 AND project_id = ?3"
    } else {
        ""
    };
    let mut statement = connection.prepare(&format!(
        "SELECT subject_type, subject_id, COUNT(*)
         FROM annotations
         WHERE subject_type IS NOT NULL
           AND subject_id IS NOT NULL
           AND (subject_type || char(31) || subject_id) IN (
             SELECT value FROM json_each(?1)
           )
           {owner_clause}
         GROUP BY subject_type, subject_id",
    ))?;
    let subject_keys = subjects
        .iter()
        .map(|(subject_type, subject_id)| format!("{subject_type}\u{1f}{subject_id}"))
        .collect::<Vec<_>>();
    let subject_keys_json = serde_json::to_string(&subject_keys).unwrap_or_else(|_| "[]".into());
    let rows = if let Some((tenant_id, project_id)) = owner {
        statement.query_map(
            params![subject_keys_json, tenant_id, project_id],
            annotation_count_row,
        )?
    } else {
        statement.query_map(params![subject_keys_json], annotation_count_row)?
    };
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn annotation_count_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<((String, String), u64)> {
    Ok((
        (row.get::<_, String>(0)?, row.get::<_, String>(1)?),
        row.get::<_, u64>(2)?,
    ))
}

fn signal_subjects(signals: &[IncidentDetailSignalRow]) -> Vec<(String, String)> {
    let mut subjects = signals
        .iter()
        .filter_map(|signal| signal_subject(&signal.signal_type, &signal.signal_ref))
        .collect::<Vec<_>>();
    subjects.sort();
    subjects.dedup();
    subjects
}

fn signal_subject(signal_type: &str, signal_ref: &str) -> Option<(String, String)> {
    match signal_type {
        "error_group" => Some(("error_group".to_owned(), signal_ref.to_owned())),
        "health_transition" if signal_ref.starts_with("TGT-") => {
            Some(("target".to_owned(), signal_ref.to_owned()))
        }
        "health_transition" if signal_ref.starts_with("MON-") => {
            Some(("monitor".to_owned(), signal_ref.to_owned()))
        }
        _ => None,
    }
}

fn format_incident_signal(
    signal: &IncidentDetailSignalRow,
    context: &IncidentSignalContext,
) -> IncidentDetailSignal {
    match signal.signal_type.as_str() {
        "error_group" => format_error_group_signal(signal, context),
        "health_transition" if signal.signal_ref.starts_with("TGT-") => {
            format_target_signal(signal, context)
        }
        "health_transition" if signal.signal_ref.starts_with("MON-") => {
            format_monitor_signal(signal, context)
        }
        "health_transition" => IncidentDetailSignal {
            signal_type: "health_transition".to_owned(),
            summary: format!(
                "Health transition on {} (detail unavailable).",
                signal.signal_ref
            ),
            group_hash: None,
            error_class: None,
            total_count: None,
            first_seen_at: None,
            last_seen_at: None,
            classification: None,
            target_id: None,
            target_name: None,
            monitor_id: None,
            monitor_name: None,
            current_state: None,
            consecutive_failures: None,
            signal_ref: Some(signal.signal_ref.clone()),
            attached_at: signal.attached_at.clone(),
            resolved_at: signal.resolved_at.clone(),
            annotation_count: 0,
        },
        _ => IncidentDetailSignal {
            signal_type: signal.signal_type.clone(),
            summary: format!(
                "Signal of type {} on {}.",
                signal.signal_type, signal.signal_ref
            ),
            group_hash: None,
            error_class: None,
            total_count: None,
            first_seen_at: None,
            last_seen_at: None,
            classification: None,
            target_id: None,
            target_name: None,
            monitor_id: None,
            monitor_name: None,
            current_state: None,
            consecutive_failures: None,
            signal_ref: Some(signal.signal_ref.clone()),
            attached_at: signal.attached_at.clone(),
            resolved_at: signal.resolved_at.clone(),
            annotation_count: annotation_count(context, &signal.signal_type, &signal.signal_ref),
        },
    }
}

fn format_error_group_signal(
    signal: &IncidentDetailSignalRow,
    context: &IncidentSignalContext,
) -> IncidentDetailSignal {
    let group = context.error_groups.get(&signal.signal_ref);
    IncidentDetailSignal {
        signal_type: "error_group".to_owned(),
        summary: group.map_or_else(
            || {
                format!(
                    "Error group {} (detail unavailable).",
                    truncate_hash(&signal.signal_ref)
                )
            },
            |group| {
                format!(
                    "{} {} of {} (last seen {}).",
                    group.total_count,
                    pluralize(group.total_count, "occurrence", "occurrences"),
                    group.error_class,
                    group.last_seen_at
                )
            },
        ),
        group_hash: Some(signal.signal_ref.clone()),
        error_class: group.map(|group| group.error_class.clone()),
        total_count: group.map(|group| group.total_count),
        first_seen_at: group.map(|group| group.first_seen_at.clone()),
        last_seen_at: group.map(|group| group.last_seen_at.clone()),
        classification: Some(
            group
                .map(|group| group.classification.clone())
                .unwrap_or_else(|| ErrorClassification::new(None, None, None)),
        ),
        target_id: None,
        target_name: None,
        monitor_id: None,
        monitor_name: None,
        current_state: None,
        consecutive_failures: None,
        signal_ref: None,
        attached_at: signal.attached_at.clone(),
        resolved_at: signal.resolved_at.clone(),
        annotation_count: annotation_count(context, &signal.signal_type, &signal.signal_ref),
    }
}

fn format_target_signal(
    signal: &IncidentDetailSignalRow,
    context: &IncidentSignalContext,
) -> IncidentDetailSignal {
    let target = context.targets.get(&signal.signal_ref);
    let name = target
        .map(|target| target.name.clone())
        .unwrap_or_else(|| signal.signal_ref.clone());
    let state = target
        .map(|target| target.current_state.clone())
        .unwrap_or_else(|| "unknown".to_owned());
    let consecutive_failures = target
        .map(|target| target.consecutive_failures)
        .unwrap_or(0);
    let summary = if signal.resolved_at.is_some() {
        format!("Target {name} recovered to {state}.")
    } else {
        format!(
            "Target {name} is {state} ({consecutive_failures} consecutive {}).",
            pluralize(consecutive_failures, "failure", "failures")
        )
    };

    IncidentDetailSignal {
        signal_type: "health_transition".to_owned(),
        summary,
        group_hash: None,
        error_class: None,
        total_count: None,
        first_seen_at: None,
        last_seen_at: None,
        classification: None,
        target_id: Some(signal.signal_ref.clone()),
        target_name: Some(name),
        monitor_id: None,
        monitor_name: None,
        current_state: Some(state),
        consecutive_failures: Some(consecutive_failures),
        signal_ref: None,
        attached_at: signal.attached_at.clone(),
        resolved_at: signal.resolved_at.clone(),
        annotation_count: annotation_count(context, &signal.signal_type, &signal.signal_ref),
    }
}

fn format_monitor_signal(
    signal: &IncidentDetailSignalRow,
    context: &IncidentSignalContext,
) -> IncidentDetailSignal {
    let monitor = context.monitors.get(&signal.signal_ref);
    let name = monitor
        .map(|monitor| monitor.name.clone())
        .unwrap_or_else(|| signal.signal_ref.clone());
    let state = monitor
        .map(|monitor| monitor.current_state.clone())
        .unwrap_or_else(|| "unknown".to_owned());
    let summary = if signal.resolved_at.is_some() {
        format!("Monitor {name} recovered to {state}.")
    } else {
        format!("Monitor {name} is {state}.")
    };

    IncidentDetailSignal {
        signal_type: "health_transition".to_owned(),
        summary,
        group_hash: None,
        error_class: None,
        total_count: None,
        first_seen_at: None,
        last_seen_at: None,
        classification: None,
        target_id: None,
        target_name: None,
        monitor_id: Some(signal.signal_ref.clone()),
        monitor_name: Some(name),
        current_state: Some(state),
        consecutive_failures: None,
        signal_ref: None,
        attached_at: signal.attached_at.clone(),
        resolved_at: signal.resolved_at.clone(),
        annotation_count: annotation_count(context, &signal.signal_type, &signal.signal_ref),
    }
}

fn annotation_count(context: &IncidentSignalContext, signal_type: &str, signal_ref: &str) -> u64 {
    signal_subject(signal_type, signal_ref)
        .and_then(|subject| context.annotation_counts.get(&subject).copied())
        .unwrap_or(0)
}

fn incident_annotations(
    connection: &Connection,
    incident_id: &str,
    limit: usize,
) -> QueryResult<(Vec<IncidentAnnotation>, bool)> {
    let mut statement = connection.prepare(
        "SELECT id, subject_type, subject_id, incident_id, group_hash, agent, action, metadata, created_at
         FROM annotations
         WHERE incident_id = ?1
         ORDER BY created_at DESC, id DESC
         LIMIT ?2",
    )?;
    let rows = statement
        .query_map(params![incident_id, (limit + 1) as u64], |row| {
            Ok(IncidentAnnotation {
                id: row.get(0)?,
                subject_type: row.get(1)?,
                subject_id: row.get(2)?,
                incident_id: row.get(3)?,
                group_hash: row.get(4)?,
                agent: row.get(5)?,
                action: row.get(6)?,
                metadata: safe_decode_json(row.get(7)?),
                created_at: row.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let truncated = rows.len() > limit;
    Ok((rows.into_iter().take(limit).collect(), truncated))
}

fn incident_timeline_events(
    connection: &Connection,
    incident_id: &str,
    limit: usize,
) -> QueryResult<Vec<IncidentTimelineEvent>> {
    let mut statement = connection.prepare(
        "SELECT id, event, severity, summary, created_at
         FROM service_events
         WHERE entity_type = 'incident' AND entity_ref = ?1
         ORDER BY created_at DESC, id DESC
         LIMIT ?2",
    )?;
    Ok(statement
        .query_map(params![incident_id, limit as u64], |row| {
            Ok(IncidentTimelineEvent {
                id: row.get(0)?,
                event: row.get(1)?,
                severity: row.get(2)?,
                summary: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn placeholders(len: usize) -> String {
    std::iter::repeat_n("?", len).collect::<Vec<_>>().join(",")
}

fn truncate_hash(hash: &str) -> String {
    if hash.len() > 12 {
        format!("{}...", &hash[..12])
    } else {
        hash.to_owned()
    }
}

fn pluralize<'a>(count: u64, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

fn active_signals(
    connection: &Connection,
    signals: Vec<IncidentSignalRow>,
    owner: Option<(&str, &str)>,
    now: OffsetDateTime,
) -> QueryResult<Vec<ActiveIncidentSignal>> {
    let mut active = Vec::new();

    for signal in signals {
        if signal.resolved_at.is_some() {
            continue;
        }

        if signal_active_for_report(connection, &signal, owner, now)? {
            active.push(ActiveIncidentSignal {
                signal_type: signal.signal_type,
                signal_ref: signal.signal_ref,
                attached_at: signal.attached_at,
                resolved_at: signal.resolved_at,
            });
        }
    }

    Ok(active)
}

fn signal_active_for_report(
    connection: &Connection,
    signal: &IncidentSignalRow,
    owner: Option<(&str, &str)>,
    now: OffsetDateTime,
) -> QueryResult<bool> {
    match signal.signal_type.as_str() {
        "health_transition" => health_signal_active(connection, &signal.signal_ref),
        "error_group" => error_group_signal_active(connection, &signal.signal_ref, owner, now),
        _ => Ok(false),
    }
}

fn health_signal_active(connection: &Connection, signal_ref: &str) -> QueryResult<bool> {
    let state = if signal_ref.starts_with("TGT-") {
        connection
            .query_row(
                "SELECT state FROM target_state WHERE target_id = ?1",
                [signal_ref],
                |row| row.get::<_, String>(0),
            )
            .optional()?
    } else if signal_ref.starts_with("MON-") {
        connection
            .query_row(
                "SELECT state FROM monitor_state WHERE monitor_id = ?1",
                [signal_ref],
                |row| row.get::<_, String>(0),
            )
            .optional()?
    } else {
        None
    };

    Ok(state.is_some_and(|state| HealthState::persisted_incident_signal_active(&state)))
}

fn error_group_signal_active(
    connection: &Connection,
    signal_ref: &str,
    owner: Option<(&str, &str)>,
    now: OffsetDateTime,
) -> QueryResult<bool> {
    let owner_clause = if owner.is_some() {
        " AND tenant_id = ?2 AND project_id = ?3"
    } else {
        ""
    };
    let sql = format!(
        "SELECT status, last_seen_at FROM error_groups WHERE group_hash = ?1{owner_clause}"
    );
    let mut statement = connection.prepare(&sql)?;
    let row = if let Some((tenant_id, project_id)) = owner {
        statement.query_row(params![signal_ref, tenant_id, project_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
    } else {
        statement.query_row([signal_ref], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
    }
    .optional()?;

    Ok(row.is_some_and(|(status, last_seen_at)| {
        status == "active" && within_incident_window(&last_seen_at, now)
    }))
}

fn incident_matches_annotation_filters(
    connection: &Connection,
    incident_id: &str,
    options: &IncidentListOptions,
) -> QueryResult<bool> {
    if let Some(action) = options.with_annotation.as_deref()
        && !incident_has_annotation(connection, incident_id, action)?
    {
        return Ok(false);
    }

    if let Some(action) = options.without_annotation.as_deref()
        && incident_has_annotation(connection, incident_id, action)?
    {
        return Ok(false);
    }

    Ok(true)
}

fn incident_has_annotation(
    connection: &Connection,
    incident_id: &str,
    action: &str,
) -> QueryResult<bool> {
    let count = connection.query_row(
        "SELECT COUNT(*)
         FROM annotations
         WHERE incident_id = ?1 AND action = ?2",
        params![incident_id, action],
        |row| row.get::<_, u64>(0),
    )?;
    Ok(count > 0)
}

fn incident_severity(signals: &[ActiveIncidentSignal], now: OffsetDateTime) -> String {
    let recent_count = signals
        .iter()
        .filter(|signal| signal_counts_for_severity(signal, now))
        .count();

    if recent_count >= 3 {
        "high".to_owned()
    } else {
        "medium".to_owned()
    }
}

fn signal_counts_for_severity(signal: &ActiveIncidentSignal, now: OffsetDateTime) -> bool {
    // Intentional divergence: health-transition signals are active state, not attached_at recency.
    signal.signal_type == "health_transition" || within_incident_window(&signal.attached_at, now)
}

fn within_incident_window(timestamp: &str, now: OffsetDateTime) -> bool {
    OffsetDateTime::parse(timestamp, &Rfc3339)
        .map(|timestamp| (now - timestamp).whole_seconds() <= 300)
        .unwrap_or(false)
}

fn list_error_groups(
    connection: &Connection,
    filter: ErrorGroupFilter,
    window: QueryWindow,
    options: ServiceQueryOptions,
    now: OffsetDateTime,
) -> ErrorGroupQueryResult<Vec<ErrorGroupSummary>> {
    let cutoff = window.cutoff_at(now);
    let cursor = parse_query_cursor(options.cursor.as_deref())?;
    paged_error_groups(
        connection,
        filter.service(),
        filter.error_class(),
        &cutoff,
        cursor,
        &options,
    )
}

fn parse_query_cursor(cursor: Option<&str>) -> ErrorGroupQueryResult<Option<QueryCursor>> {
    match cursor {
        None | Some("") => Ok(None),
        Some(cursor) => decode_cursor(cursor)
            .map(Some)
            .ok_or(ErrorGroupQueryError::InvalidCursor),
    }
}

fn paged_error_groups(
    connection: &Connection,
    service: Option<&str>,
    error_class: Option<&str>,
    cutoff: &str,
    cursor: Option<QueryCursor>,
    options: &ServiceQueryOptions,
) -> ErrorGroupQueryResult<Vec<ErrorGroupSummary>> {
    let tenant_id = options.tenant_id.as_deref();
    let project_id = options.project_id.as_deref();
    let scoped = tenant_id.is_some() && project_id.is_some();
    match cursor {
        Some(QueryCursor::Structured(cursor)) => {
            let mut statement = connection.prepare(&format!(
                "{} AND (g.total_count < ?9 OR (g.total_count = ?9 AND g.group_hash > ?10))
                 ORDER BY g.total_count DESC, g.group_hash ASC
                 LIMIT 50",
                service_groups_sql(scoped)
            ))?;
            error_query_groups_from_rows(statement.query_map(
                params![
                    service,
                    error_class,
                    cutoff,
                    options.with_annotation.as_deref(),
                    options.with_annotation.as_deref(),
                    options.without_annotation.as_deref(),
                    tenant_id,
                    project_id,
                    cursor.total_count,
                    cursor.group_hash.as_str(),
                ],
                group_from_row,
            )?)
        }
        Some(QueryCursor::LegacyGroupHash(group_hash)) => {
            let mut statement = connection.prepare(&format!(
                "{} AND g.group_hash > ?9
                 ORDER BY g.total_count DESC, g.group_hash ASC
                 LIMIT 50",
                service_groups_sql(scoped)
            ))?;
            error_query_groups_from_rows(statement.query_map(
                params![
                    service,
                    error_class,
                    cutoff,
                    options.with_annotation.as_deref(),
                    options.with_annotation.as_deref(),
                    options.without_annotation.as_deref(),
                    tenant_id,
                    project_id,
                    group_hash.as_str(),
                ],
                group_from_row,
            )?)
        }
        None => {
            let mut statement = connection.prepare(&format!(
                "{}
                 ORDER BY g.total_count DESC, g.group_hash ASC
                 LIMIT 50",
                service_groups_sql(scoped)
            ))?;
            error_query_groups_from_rows(statement.query_map(
                params![
                    service,
                    error_class,
                    cutoff,
                    options.with_annotation.as_deref(),
                    options.with_annotation.as_deref(),
                    options.without_annotation.as_deref(),
                    tenant_id,
                    project_id,
                ],
                group_from_row,
            )?)
        }
    }
}

fn service_groups_sql(scoped: bool) -> String {
    let annotation_owner_clause = if scoped {
        "           AND (?7 IS NULL OR a.tenant_id = ?7)
           AND (?8 IS NULL OR a.project_id = ?8)
"
    } else {
        ""
    };
    let group_owner_clause = if scoped {
        "       AND (?7 IS NULL OR g.tenant_id = ?7)
       AND (?8 IS NULL OR g.project_id = ?8)"
    } else {
        "       AND (?7 IS NULL OR 1 = 1)
       AND (?8 IS NULL OR 1 = 1)"
    };
    format!(
        "SELECT
            g.group_hash,
            g.error_class,
            g.service,
            g.total_count,
            g.first_seen_at,
            g.last_seen_at,
            g.message_template,
            g.severity,
            g.status,
            e.classification_category,
            e.classification_persistence,
            e.classification_component
         FROM error_groups g
         LEFT JOIN errors e ON e.id = g.last_error_id
         WHERE (?1 IS NULL OR g.service = ?1)
           AND (?2 IS NULL OR g.error_class = ?2)
           AND g.last_seen_at >= ?3
           AND g.status = 'active'
           AND (?4 IS NULL OR EXISTS (
             SELECT 1 FROM annotations a
             WHERE a.group_hash = g.group_hash
               AND a.action = ?5
{annotation_owner_clause}       ))
           AND (?6 IS NULL OR NOT EXISTS (
             SELECT 1 FROM annotations a
             WHERE a.group_hash = g.group_hash
               AND a.action = ?6
{annotation_owner_clause}       ))
{group_owner_clause}"
    )
}

enum ErrorGroupFilter {
    Service {
        service: String,
    },
    ErrorClass {
        error_class: String,
        service: Option<String>,
    },
}

impl ErrorGroupFilter {
    fn service(&self) -> Option<&str> {
        match self {
            Self::Service { service } => Some(service),
            Self::ErrorClass { service, .. } => service.as_deref(),
        }
    }

    fn error_class(&self) -> Option<&str> {
        match self {
            Self::Service { .. } => None,
            Self::ErrorClass { error_class, .. } => Some(error_class),
        }
    }
}

fn groups_from_rows(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<ErrorGroupSummary>,
    >,
) -> QueryResult<Vec<ErrorGroupSummary>> {
    let groups = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(groups)
}

fn error_query_groups_from_rows(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<ErrorGroupSummary>,
    >,
) -> ErrorGroupQueryResult<Vec<ErrorGroupSummary>> {
    let groups = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(groups)
}

fn owner_clause(alias: &str, first_parameter: usize) -> String {
    format!(
        " AND {alias}.tenant_id = ?{first_parameter} AND {alias}.project_id = ?{}",
        first_parameter + 1
    )
}

fn group_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ErrorGroupSummary> {
    Ok(ErrorGroupSummary {
        group_hash: row.get(0)?,
        error_class: row.get(1)?,
        service: row.get(2)?,
        total_count: row.get::<_, u64>(3)?,
        first_seen: row.get(4)?,
        last_seen: row.get(5)?,
        sample_message: row.get(6)?,
        severity: row.get(7)?,
        status: row.get(8)?,
        classification: ErrorClassification::new(row.get(9)?, row.get(10)?, row.get(11)?),
    })
}

fn parse_timeline_limit(limit: Option<&str>) -> TimelineQueryResult<usize> {
    match limit {
        None | Some("") => Ok(canary_core::query::DEFAULT_TIMELINE_LIMIT),
        Some(raw) => match raw.parse::<usize>() {
            Ok(value) if (1..=canary_core::query::MAX_TIMELINE_LIMIT).contains(&value) => Ok(value),
            _ => Err(TimelineQueryError::InvalidLimit),
        },
    }
}

fn parse_timeline_cursor(cursor: Option<&str>) -> TimelineQueryResult<Option<TimelineCursor>> {
    match cursor {
        None | Some("") => Ok(None),
        Some(cursor) => decode_timeline_cursor(cursor)
            .map(Some)
            .ok_or(TimelineQueryError::InvalidCursor),
    }
}

fn parse_timeline_event_types(
    event_type: Option<&str>,
) -> TimelineQueryResult<Option<Vec<String>>> {
    let Some(event_type) = event_type else {
        return Ok(None);
    };
    if event_type.is_empty() {
        return Ok(None);
    }

    let types = event_type
        .split(',')
        .map(str::trim)
        .filter(|event| !event.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let invalid = types
        .iter()
        .filter(|event| !webhook_events::business().contains(&event.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if invalid.is_empty() {
        Ok(Some(types))
    } else {
        Err(TimelineQueryError::InvalidEventType(invalid))
    }
}

struct ErrorRow {
    id: String,
    service: String,
    error_class: String,
    message: String,
    message_template: Option<String>,
    stack_trace: Option<String>,
    context: Option<String>,
    severity: String,
    environment: String,
    group_hash: String,
    created_at: String,
}

fn error_row(
    connection: &Connection,
    error_id: &str,
    owner: Option<(&str, &str)>,
) -> QueryResult<Option<ErrorRow>> {
    let owner_clause = if owner.is_some() {
        " AND tenant_id = ?2 AND project_id = ?3"
    } else {
        ""
    };
    let sql = format!(
        "SELECT
                id, service, error_class, message, message_template, stack_trace, context,
                severity, environment, group_hash, created_at
             FROM errors
             WHERE id = ?1{owner_clause}"
    );
    let mut statement = connection.prepare(&sql)?;
    let result = if let Some((tenant_id, project_id)) = owner {
        statement.query_row(params![error_id, tenant_id, project_id], error_row_from_row)
    } else {
        statement.query_row([error_id], error_row_from_row)
    };
    Ok(result.optional()?)
}

fn error_row_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ErrorRow> {
    Ok(ErrorRow {
        id: row.get(0)?,
        service: row.get(1)?,
        error_class: row.get(2)?,
        message: row.get(3)?,
        message_template: row.get(4)?,
        stack_trace: row.get(5)?,
        context: row.get(6)?,
        severity: row.get(7)?,
        environment: row.get(8)?,
        group_hash: row.get(9)?,
        created_at: row.get(10)?,
    })
}

fn group_detail(
    connection: &Connection,
    group_hash: &str,
    owner: Option<(&str, &str)>,
) -> QueryResult<Option<ErrorDetailGroup>> {
    let owner_clause = if owner.is_some() {
        " AND tenant_id = ?2 AND project_id = ?3"
    } else {
        ""
    };
    let sql = format!(
        "SELECT total_count, first_seen_at, last_seen_at, status
             FROM error_groups
             WHERE group_hash = ?1{owner_clause}"
    );
    let mut statement = connection.prepare(&sql)?;
    let result = if let Some((tenant_id, project_id)) = owner {
        statement.query_row(params![group_hash, tenant_id, project_id], |row| {
            Ok(ErrorDetailGroup {
                total_count: row.get(0)?,
                first_seen_at: row.get(1)?,
                last_seen_at: row.get(2)?,
                status: row.get(3)?,
            })
        })
    } else {
        statement.query_row([group_hash], |row| {
            Ok(ErrorDetailGroup {
                total_count: row.get(0)?,
                first_seen_at: row.get(1)?,
                last_seen_at: row.get(2)?,
                status: row.get(3)?,
            })
        })
    };
    Ok(result.optional()?)
}

fn incident_ids_for_group(
    connection: &Connection,
    group_hash: &str,
    owner: Option<(&str, &str)>,
) -> QueryResult<Vec<String>> {
    let owner_clause = if owner.is_some() {
        " AND EXISTS (
            SELECT 1 FROM incidents i
            WHERE i.id = incident_signals.incident_id
              AND i.tenant_id = ?2
              AND i.project_id = ?3
        )"
    } else {
        ""
    };
    let mut statement = connection.prepare(&format!(
        "SELECT DISTINCT incident_id
         FROM incident_signals
         WHERE signal_type = 'error_group' AND signal_ref = ?1
         {owner_clause}
         ORDER BY incident_id",
    ))?;
    let rows = if let Some((tenant_id, project_id)) = owner {
        statement.query_map(
            params![group_hash, tenant_id, project_id],
            string_first_column,
        )?
    } else {
        statement.query_map([group_hash], string_first_column)?
    };
    let ids = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

fn string_first_column(row: &rusqlite::Row<'_>) -> rusqlite::Result<String> {
    row.get::<_, String>(0)
}

fn safe_decode_json(json: Option<String>) -> Option<Value> {
    let json = json?;
    Some(match serde_json::from_str(&json) {
        Ok(decoded) => decoded,
        Err(_) => Value::String(json),
    })
}
