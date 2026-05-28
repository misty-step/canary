use canary_core::query::{
    ErrorClassAggregate, ErrorClassification, ErrorDetail, ErrorDetailGroup, ErrorGroupSummary,
    ErrorsByClass, ErrorsByErrorClass, ErrorsByService, QueryCursor, QueryWindow, decode_cursor,
    error_detail_response, errors_by_class_response, errors_by_error_class_response,
    errors_by_service_response,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use time::OffsetDateTime;

/// Optional filters for service error queries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceQueryOptions {
    /// Optional pagination cursor.
    pub cursor: Option<String>,
    /// Optional annotation action that must exist for the group.
    pub with_annotation: Option<String>,
    /// Optional annotation action that must not exist for the group.
    pub without_annotation: Option<String>,
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

pub(crate) fn errors_by_service(
    connection: &Connection,
    service: &str,
    window: &str,
    options: ServiceQueryOptions,
) -> QueryResult<ErrorsByService> {
    let window = QueryWindow::parse(window).ok_or(QueryError::InvalidWindow)?;
    let groups = list_error_groups(
        connection,
        ErrorGroupFilter::Service {
            service: service.to_owned(),
        },
        window,
        options,
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
) -> QueryResult<ErrorsByErrorClass> {
    let window = QueryWindow::parse(window).ok_or(QueryError::InvalidWindow)?;
    let groups = list_error_groups(
        connection,
        ErrorGroupFilter::ErrorClass {
            error_class: error_class.to_owned(),
            service: service.map(ToOwned::to_owned),
        },
        window,
        options,
    )?;

    Ok(errors_by_error_class_response(
        error_class.to_owned(),
        window,
        groups,
    ))
}

pub(crate) fn errors_by_class(connection: &Connection, window: &str) -> QueryResult<ErrorsByClass> {
    let window = QueryWindow::parse(window).ok_or(QueryError::InvalidWindow)?;
    let cutoff = window.cutoff_at(OffsetDateTime::now_utc());
    let groups = error_class_aggregates(connection, &cutoff)?;
    let (total_errors, total_error_classes) = error_class_totals(connection, &cutoff)?;

    Ok(errors_by_class_response(
        window,
        groups,
        total_errors,
        total_error_classes,
    ))
}

pub(crate) fn error_detail(
    connection: &Connection,
    error_id: &str,
) -> QueryResult<Option<ErrorDetail>> {
    let Some(row) = error_row(connection, error_id)? else {
        return Ok(None);
    };
    let group = group_detail(connection, &row.group_hash)?;
    let incident_ids = incident_ids_for_group(connection, &row.group_hash)?;
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

fn error_class_aggregates(
    connection: &Connection,
    cutoff: &str,
) -> QueryResult<Vec<ErrorClassAggregate>> {
    let mut statement = connection.prepare(
        "SELECT error_class, COALESCE(SUM(total_count), 0), COUNT(DISTINCT service)
         FROM error_groups
         WHERE last_seen_at >= ?1
         GROUP BY error_class
         ORDER BY SUM(total_count) DESC, error_class ASC
         LIMIT 50",
    )?;
    let groups = statement
        .query_map([cutoff], |row| {
            Ok(ErrorClassAggregate {
                error_class: row.get(0)?,
                total_count: row.get(1)?,
                service_count: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(groups)
}

fn error_class_totals(connection: &Connection, cutoff: &str) -> QueryResult<(u64, u64)> {
    Ok(connection.query_row(
        "SELECT COALESCE(SUM(total_count), 0), COUNT(DISTINCT error_class)
         FROM error_groups
         WHERE last_seen_at >= ?1",
        [cutoff],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

fn list_error_groups(
    connection: &Connection,
    filter: ErrorGroupFilter,
    window: QueryWindow,
    options: ServiceQueryOptions,
) -> QueryResult<Vec<ErrorGroupSummary>> {
    let cutoff = window.cutoff_at(OffsetDateTime::now_utc());
    let cursor = options.cursor.as_deref().and_then(decode_cursor);
    paged_error_groups(
        connection,
        filter.service(),
        filter.error_class(),
        &cutoff,
        cursor,
        &options,
    )
}

fn paged_error_groups(
    connection: &Connection,
    service: Option<&str>,
    error_class: Option<&str>,
    cutoff: &str,
    cursor: Option<QueryCursor>,
    options: &ServiceQueryOptions,
) -> QueryResult<Vec<ErrorGroupSummary>> {
    match cursor {
        Some(QueryCursor::Structured(cursor)) => {
            let mut statement = connection.prepare(&format!(
                "{} AND (g.total_count < ?7 OR (g.total_count = ?7 AND g.group_hash > ?8))
                 ORDER BY g.total_count DESC, g.group_hash ASC
                 LIMIT 50",
                service_groups_sql()
            ))?;
            groups_from_rows(statement.query_map(
                params![
                    service,
                    error_class,
                    cutoff,
                    options.with_annotation.as_deref(),
                    options.with_annotation.as_deref(),
                    options.without_annotation.as_deref(),
                    cursor.total_count,
                    cursor.group_hash.as_str(),
                ],
                group_from_row,
            )?)
        }
        Some(QueryCursor::LegacyGroupHash(group_hash)) => {
            let mut statement = connection.prepare(&format!(
                "{} AND g.group_hash > ?7
                 ORDER BY g.total_count DESC, g.group_hash ASC
                 LIMIT 50",
                service_groups_sql()
            ))?;
            groups_from_rows(statement.query_map(
                params![
                    service,
                    error_class,
                    cutoff,
                    options.with_annotation.as_deref(),
                    options.with_annotation.as_deref(),
                    options.without_annotation.as_deref(),
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
                service_groups_sql()
            ))?;
            groups_from_rows(statement.query_map(
                params![
                    service,
                    error_class,
                    cutoff,
                    options.with_annotation.as_deref(),
                    options.with_annotation.as_deref(),
                    options.without_annotation.as_deref(),
                ],
                group_from_row,
            )?)
        }
    }
}

fn service_groups_sql() -> &'static str {
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
       AND (?4 IS NULL OR EXISTS (
         SELECT 1 FROM annotations a
         WHERE a.group_hash = g.group_hash AND a.action = ?5
       ))
       AND (?6 IS NULL OR NOT EXISTS (
         SELECT 1 FROM annotations a
         WHERE a.group_hash = g.group_hash AND a.action = ?6
       ))"
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

fn error_row(connection: &Connection, error_id: &str) -> QueryResult<Option<ErrorRow>> {
    Ok(connection
        .query_row(
            "SELECT
                id, service, error_class, message, message_template, stack_trace, context,
                severity, environment, group_hash, created_at
             FROM errors
             WHERE id = ?1",
            [error_id],
            |row| {
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
            },
        )
        .optional()?)
}

fn group_detail(
    connection: &Connection,
    group_hash: &str,
) -> QueryResult<Option<ErrorDetailGroup>> {
    Ok(connection
        .query_row(
            "SELECT total_count, first_seen_at, last_seen_at, status
             FROM error_groups
             WHERE group_hash = ?1",
            [group_hash],
            |row| {
                Ok(ErrorDetailGroup {
                    total_count: row.get(0)?,
                    first_seen_at: row.get(1)?,
                    last_seen_at: row.get(2)?,
                    status: row.get(3)?,
                })
            },
        )
        .optional()?)
}

fn incident_ids_for_group(connection: &Connection, group_hash: &str) -> QueryResult<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT incident_id
         FROM incident_signals
         WHERE signal_type = 'error_group' AND signal_ref = ?1
         ORDER BY incident_id",
    )?;
    let ids = statement
        .query_map([group_hash], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

fn safe_decode_json(json: Option<String>) -> Option<Value> {
    let json = json?;
    Some(match serde_json::from_str(&json) {
        Ok(decoded) => decoded,
        Err(_) => Value::String(json),
    })
}
