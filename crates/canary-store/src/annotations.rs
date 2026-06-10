//! Annotation persistence for agent coordination surfaces.

use canary_core::query::{
    ANNOTATION_SUBJECT_TYPES, Annotation, AnnotationCursor, AnnotationListResponse,
    AnnotationPageResponse, DEFAULT_ANNOTATION_LIMIT, MAX_ANNOTATION_LIMIT,
    annotation_list_response, annotation_page_response, decode_annotation_cursor,
    encode_annotation_cursor,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

/// Result type returned by annotation read/write models.
pub type AnnotationResult<T> = std::result::Result<T, AnnotationError>;

/// Annotation validation or storage failure.
#[derive(Debug, thiserror::Error)]
pub enum AnnotationError {
    /// Subject type is not one of the Phoenix types.
    #[error("invalid annotation subject type")]
    InvalidSubjectType,
    /// Subject row does not exist.
    #[error("annotation subject not found")]
    NotFound,
    /// Limit is not a positive integer up to the Phoenix maximum.
    #[error("invalid annotation limit")]
    InvalidLimit,
    /// Cursor is not a valid Phoenix annotation cursor.
    #[error("invalid annotation cursor")]
    InvalidCursor,
    /// SQLite rejected the operation.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Annotation row to persist.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotationInsert {
    /// Stable annotation id.
    pub id: String,
    /// Subject type.
    pub subject_type: String,
    /// Subject id.
    pub subject_id: String,
    /// Agent name.
    pub agent: String,
    /// Opaque action label.
    pub action: String,
    /// Optional metadata value.
    pub metadata: Option<Value>,
    /// Creation timestamp.
    pub created_at: String,
}

/// Options for `GET /api/v1/annotations`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnnotationPageOptions {
    /// Subject type.
    pub subject_type: String,
    /// Subject id.
    pub subject_id: String,
    /// Optional limit string.
    pub limit: Option<String>,
    /// Optional cursor.
    pub cursor: Option<String>,
}

/// Canonical annotation subject type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationSubjectType {
    /// Incident subject.
    Incident,
    /// Error group subject.
    ErrorGroup,
    /// Target subject.
    Target,
    /// Monitor subject.
    Monitor,
}

impl AnnotationSubjectType {
    /// Parse a Phoenix subject type.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "incident" => Some(Self::Incident),
            "error_group" => Some(Self::ErrorGroup),
            "target" => Some(Self::Target),
            "monitor" => Some(Self::Monitor),
            _ => None,
        }
    }

    /// Return the persisted subject type.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Incident => "incident",
            Self::ErrorGroup => "error_group",
            Self::Target => "target",
            Self::Monitor => "monitor",
        }
    }

    fn exists_sql(self) -> &'static str {
        match self {
            Self::Incident => "SELECT 1 FROM incidents WHERE id = ?1 LIMIT 1",
            Self::ErrorGroup => "SELECT 1 FROM error_groups WHERE group_hash = ?1 LIMIT 1",
            Self::Target => "SELECT 1 FROM targets WHERE id = ?1 LIMIT 1",
            Self::Monitor => "SELECT 1 FROM monitors WHERE id = ?1 LIMIT 1",
        }
    }
}

/// Return Phoenix's accepted annotation subject types in wire order.
pub const fn subject_types() -> &'static [&'static str] {
    &ANNOTATION_SUBJECT_TYPES
}

pub(crate) fn create(
    connection: &Connection,
    insert: AnnotationInsert,
) -> AnnotationResult<Annotation> {
    let subject_type = parse_subject_type(&insert.subject_type)?;
    require_subject(connection, subject_type, &insert.subject_id)?;

    let (incident_id, group_hash) = legacy_keys(subject_type, &insert.subject_id);
    let metadata_json = metadata_to_storage(insert.metadata)?;

    connection.execute(
        "INSERT INTO annotations (
             id, incident_id, group_hash, agent, action, metadata, created_at, subject_type, subject_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            insert.id,
            incident_id,
            group_hash,
            insert.agent,
            insert.action,
            metadata_json,
            insert.created_at,
            subject_type.as_str(),
            insert.subject_id,
        ],
    )?;

    let row = row_by_id(connection, &insert.id)?.ok_or(AnnotationError::NotFound)?;
    Ok(row)
}

pub(crate) fn list(
    connection: &Connection,
    subject_type: &str,
    subject_id: &str,
) -> AnnotationResult<AnnotationListResponse> {
    let subject_type = parse_subject_type(subject_type)?;
    require_subject(connection, subject_type, subject_id)?;
    let rows = rows_for_subject(connection, subject_type, subject_id, None)?;
    Ok(annotation_list_response(rows))
}

pub(crate) fn page(
    connection: &Connection,
    options: AnnotationPageOptions,
) -> AnnotationResult<AnnotationPageResponse> {
    let subject_type = parse_subject_type(&options.subject_type)?;
    require_subject(connection, subject_type, &options.subject_id)?;
    let limit = parse_limit(options.limit.as_deref())?;
    let cursor = parse_cursor(options.cursor.as_deref())?;
    let rows = rows_for_subject(
        connection,
        subject_type,
        &options.subject_id,
        Some((limit, cursor)),
    )?;
    let total_count = count_for_subject(connection, subject_type, &options.subject_id)?;
    let latest = latest_for_summary(connection, subject_type, &options.subject_id)?;

    let (page, next_cursor) = paginate(rows, limit);
    Ok(annotation_page_response(
        subject_type.as_str(),
        &options.subject_id,
        total_count,
        latest
            .as_ref()
            .map(|latest| (latest.agent.as_str(), latest.created_at.as_str())),
        page,
        next_cursor,
    ))
}

fn parse_subject_type(value: &str) -> AnnotationResult<AnnotationSubjectType> {
    AnnotationSubjectType::parse(value).ok_or(AnnotationError::InvalidSubjectType)
}

fn require_subject(
    connection: &Connection,
    subject_type: AnnotationSubjectType,
    subject_id: &str,
) -> AnnotationResult<()> {
    let found = connection
        .query_row(subject_type.exists_sql(), params![subject_id], |row| {
            row.get::<_, i64>(0)
        })
        .optional()?
        .is_some();

    if found {
        Ok(())
    } else {
        Err(AnnotationError::NotFound)
    }
}

fn legacy_keys(
    subject_type: AnnotationSubjectType,
    subject_id: &str,
) -> (Option<String>, Option<String>) {
    match subject_type {
        AnnotationSubjectType::Incident => (Some(subject_id.to_owned()), None),
        AnnotationSubjectType::ErrorGroup => (None, Some(subject_id.to_owned())),
        AnnotationSubjectType::Target | AnnotationSubjectType::Monitor => (None, None),
    }
}

fn metadata_to_storage(metadata: Option<Value>) -> AnnotationResult<Option<String>> {
    match metadata {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(value @ Value::Object(_)) => serde_json::to_string(&value)
            .map(Some)
            .map_err(|_| rusqlite::Error::InvalidQuery.into()),
        Some(_) => Ok(None),
    }
}

fn parse_limit(limit: Option<&str>) -> AnnotationResult<usize> {
    match limit {
        None | Some("") => Ok(DEFAULT_ANNOTATION_LIMIT),
        Some(value) => match value.parse::<usize>() {
            Ok(value) if (1..=MAX_ANNOTATION_LIMIT).contains(&value) => Ok(value),
            _ => Err(AnnotationError::InvalidLimit),
        },
    }
}

fn parse_cursor(cursor: Option<&str>) -> AnnotationResult<Option<AnnotationCursor>> {
    match cursor {
        None | Some("") => Ok(None),
        Some(value) => decode_annotation_cursor(value)
            .map(Some)
            .ok_or(AnnotationError::InvalidCursor),
    }
}

fn rows_for_subject(
    connection: &Connection,
    subject_type: AnnotationSubjectType,
    subject_id: &str,
    page: Option<(usize, Option<AnnotationCursor>)>,
) -> AnnotationResult<Vec<Annotation>> {
    let (limit, cursor) = page.unwrap_or((usize::MAX - 1, None));
    match cursor {
        Some(cursor) => {
            let mut statement = connection.prepare(
                "SELECT id, subject_type, subject_id, incident_id, group_hash, agent, action,
                        metadata, created_at
                 FROM annotations
                 WHERE subject_type = ?1 AND subject_id = ?2
                   AND (created_at < ?3 OR (created_at = ?3 AND id < ?4))
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?5",
            )?;
            collect_annotations(statement.query_map(
                params![
                    subject_type.as_str(),
                    subject_id,
                    cursor.created_at,
                    cursor.id,
                    (limit + 1) as i64
                ],
                annotation_from_row,
            )?)
        }
        None => {
            let mut statement = connection.prepare(
                "SELECT id, subject_type, subject_id, incident_id, group_hash, agent, action,
                        metadata, created_at
                 FROM annotations
                 WHERE subject_type = ?1 AND subject_id = ?2
                 ORDER BY created_at DESC, id DESC
                 LIMIT ?3",
            )?;
            collect_annotations(statement.query_map(
                params![subject_type.as_str(), subject_id, (limit + 1) as i64],
                annotation_from_row,
            )?)
        }
    }
}

fn row_by_id(connection: &Connection, id: &str) -> AnnotationResult<Option<Annotation>> {
    connection
        .query_row(
            "SELECT id, subject_type, subject_id, incident_id, group_hash, agent, action,
                    metadata, created_at
             FROM annotations
             WHERE id = ?1",
            params![id],
            annotation_from_row,
        )
        .optional()
        .map_err(AnnotationError::from)
}

fn collect_annotations<F>(rows: rusqlite::MappedRows<'_, F>) -> AnnotationResult<Vec<Annotation>>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<Annotation>,
{
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(AnnotationError::from)
}

fn annotation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Annotation> {
    let metadata: Option<String> = row.get(7)?;
    Ok(Annotation {
        id: row.get(0)?,
        subject_type: row.get(1)?,
        subject_id: row.get(2)?,
        incident_id: row.get(3)?,
        group_hash: row.get(4)?,
        agent: row.get(5)?,
        action: row.get(6)?,
        metadata: metadata.map(decode_metadata),
        created_at: row.get(8)?,
    })
}

fn decode_metadata(value: String) -> Value {
    serde_json::from_str(&value).unwrap_or(Value::String(value))
}

fn count_for_subject(
    connection: &Connection,
    subject_type: AnnotationSubjectType,
    subject_id: &str,
) -> AnnotationResult<u64> {
    connection
        .query_row(
            "SELECT count(*) FROM annotations WHERE subject_type = ?1 AND subject_id = ?2",
            params![subject_type.as_str(), subject_id],
            |row| row.get::<_, u64>(0),
        )
        .map_err(AnnotationError::from)
}

struct LatestAnnotationSummary {
    agent: String,
    created_at: String,
}

fn latest_for_summary(
    connection: &Connection,
    subject_type: AnnotationSubjectType,
    subject_id: &str,
) -> AnnotationResult<Option<LatestAnnotationSummary>> {
    connection
        .query_row(
            "SELECT agent, created_at FROM annotations
             WHERE subject_type = ?1 AND subject_id = ?2
             ORDER BY created_at DESC, id DESC
             LIMIT 1",
            params![subject_type.as_str(), subject_id],
            |row| {
                Ok(LatestAnnotationSummary {
                    agent: row.get(0)?,
                    created_at: row.get(1)?,
                })
            },
        )
        .optional()
        .map_err(AnnotationError::from)
}

fn paginate(mut rows: Vec<Annotation>, limit: usize) -> (Vec<Annotation>, Option<String>) {
    let has_more = rows.len() > limit;
    if has_more {
        rows.truncate(limit);
    }
    let cursor = if has_more {
        rows.last().and_then(|last| {
            encode_annotation_cursor(&AnnotationCursor {
                created_at: last.created_at.clone(),
                id: last.id.clone(),
            })
        })
    } else {
        None
    };
    (rows, cursor)
}
