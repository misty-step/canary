//! Durable remediation-claim coordination state for agents.

use canary_core::ids::EventId;
use canary_core::query::{
    ClaimCursor, DEFAULT_CLAIM_LIMIT, MAX_CLAIM_LIMIT, REMEDIATION_CLAIM_SUBJECT_TYPES,
    RemediationClaim, RemediationClaimSummary, RemediationClaimsResponse, claim_state_is_valid,
    decode_claim_cursor, encode_claim_cursor, remediation_claims_response,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params, types::Type};
use serde_json::json;

/// Result type returned by remediation-claim read/write models.
pub type ClaimResult<T> = std::result::Result<T, ClaimError>;

/// Claim validation, conflict, or storage failure.
#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    /// Subject type is not one of the accepted claim types.
    #[error("invalid claim subject type")]
    InvalidSubjectType,
    /// Claim state is outside the bounded state set.
    #[error("invalid claim state")]
    InvalidState,
    /// Required claim field is missing or empty.
    #[error("invalid claim")]
    InvalidClaim,
    /// Limit is not a positive integer up to the claim maximum.
    #[error("invalid claim limit")]
    InvalidLimit,
    /// Cursor is not a valid remediation-claim cursor.
    #[error("invalid claim cursor")]
    InvalidCursor,
    /// Subject or claim row does not exist.
    #[error("claim subject not found")]
    NotFound,
    /// A different active owner already holds the subject.
    #[error("claim conflict")]
    Conflict(Box<RemediationClaimSummary>),
    /// A terminal claim cannot be reopened.
    #[error("claim transition not allowed")]
    InvalidTransition,
    /// SQLite rejected the operation.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Claim row to persist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimInsert {
    /// Claim id.
    pub id: String,
    /// Lifecycle event id.
    pub event_id: String,
    /// Tenant namespace.
    pub tenant_id: String,
    /// Project namespace.
    pub project_id: String,
    /// Optional service authority boundary.
    pub service: Option<String>,
    /// Subject type.
    pub subject_type: String,
    /// Subject id.
    pub subject_id: String,
    /// Agent or automation owner.
    pub owner: String,
    /// Purpose for the claim.
    pub purpose: String,
    /// Idempotency key.
    pub idempotency_key: String,
    /// Evidence links.
    pub evidence_links: Vec<String>,
    /// Creation/update timestamp.
    pub now: String,
    /// Expiration timestamp.
    pub expires_at: String,
}

/// Result of a claim create operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimCreateOutcome {
    /// Persisted or replayed claim.
    pub claim: RemediationClaim,
    /// Whether this call inserted the claim.
    pub created: bool,
}

/// Claim transition command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimTransition {
    /// Lifecycle event id.
    pub event_id: String,
    /// Claim id.
    pub claim_id: String,
    /// Tenant namespace.
    pub tenant_id: String,
    /// Project namespace.
    pub project_id: String,
    /// Optional service authority boundary.
    pub service: Option<String>,
    /// Agent or automation owner making the transition.
    pub owner: String,
    /// Next bounded state.
    pub state: String,
    /// Evidence links appended to the claim.
    pub evidence_links: Vec<String>,
    /// Update timestamp.
    pub now: String,
}

/// Claim list options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaimListOptions {
    /// Tenant namespace.
    pub tenant_id: Option<String>,
    /// Project namespace.
    pub project_id: Option<String>,
    /// Optional service authority boundary.
    pub service: Option<String>,
    /// Subject type.
    pub subject_type: String,
    /// Subject id.
    pub subject_id: String,
    /// Optional limit string.
    pub limit: Option<String>,
    /// Optional pagination cursor.
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaimSubjectType {
    Incident,
    ErrorGroup,
    Target,
    Monitor,
}

impl ClaimSubjectType {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "incident" => Some(Self::Incident),
            "error_group" => Some(Self::ErrorGroup),
            "target" => Some(Self::Target),
            "monitor" => Some(Self::Monitor),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Incident => "incident",
            Self::ErrorGroup => "error_group",
            Self::Target => "target",
            Self::Monitor => "monitor",
        }
    }
}

/// Return accepted claim subject types in wire order.
pub const fn subject_types() -> &'static [&'static str] {
    &REMEDIATION_CLAIM_SUBJECT_TYPES
}

pub(crate) fn create(
    connection: &mut Connection,
    insert: ClaimInsert,
) -> ClaimResult<ClaimCreateOutcome> {
    validate_non_empty(&insert.owner)?;
    validate_non_empty(&insert.purpose)?;
    validate_non_empty(&insert.idempotency_key)?;
    validate_non_empty(&insert.expires_at)?;
    validate_rfc3339_like(&insert.expires_at)?;
    validate_evidence_links(&insert.evidence_links)?;
    let subject_type = parse_subject_type(&insert.subject_type)?;

    let transaction = connection.transaction()?;
    let service = require_subject(
        &transaction,
        subject_type,
        &insert.subject_id,
        Some((&insert.tenant_id, &insert.project_id)),
        insert.service.as_deref(),
    )?;
    expire_due_claims(
        &transaction,
        &insert.tenant_id,
        &insert.project_id,
        subject_type,
        &insert.subject_id,
        &insert.now,
    )?;

    if let Some(existing) = claim_by_idempotency(
        &transaction,
        &insert.tenant_id,
        &insert.project_id,
        subject_type,
        &insert.subject_id,
        &insert.idempotency_key,
    )? {
        transaction.commit()?;
        return Ok(ClaimCreateOutcome {
            claim: existing,
            created: false,
        });
    }

    if let Some(current) = current_claim_for_subject_tx(
        &transaction,
        &insert.tenant_id,
        &insert.project_id,
        subject_type,
        &insert.subject_id,
        &insert.now,
    )? {
        return Err(ClaimError::Conflict(Box::new(current.summary())));
    }

    let evidence_json = evidence_to_storage(&insert.evidence_links)?;
    transaction.execute(
        "INSERT INTO remediation_claims (
             id, tenant_id, project_id, service, subject_type, subject_id, owner, purpose,
             state, idempotency_key, evidence_links, created_at, updated_at, expires_at,
             released_at, completed_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'claimed', ?9, ?10, ?11, ?11, ?12, NULL, NULL)",
        params![
            insert.id,
            insert.tenant_id,
            insert.project_id,
            service,
            subject_type.as_str(),
            insert.subject_id,
            insert.owner,
            insert.purpose,
            insert.idempotency_key,
            evidence_json,
            insert.now,
            insert.expires_at,
        ],
    )?;
    let claim = row_by_id_tx(&transaction, &insert.id)?.ok_or(ClaimError::NotFound)?;
    insert_event(
        &transaction,
        &insert.event_id,
        "remediation_claim.created",
        &claim,
        &insert.now,
    )?;
    transaction.commit()?;
    Ok(ClaimCreateOutcome {
        claim,
        created: true,
    })
}

pub(crate) fn list(
    connection: &mut Connection,
    options: ClaimListOptions,
) -> ClaimResult<RemediationClaimsResponse> {
    let subject_type = parse_subject_type(&options.subject_type)?;
    let limit = parse_limit(options.limit.as_deref())?;
    let cursor = parse_cursor(options.cursor.as_deref())?;
    let owner = options
        .tenant_id
        .as_deref()
        .zip(options.project_id.as_deref());
    let transaction = connection.transaction()?;
    require_subject(
        &transaction,
        subject_type,
        &options.subject_id,
        owner,
        options.service.as_deref(),
    )?;
    let now = current_time_string();
    let current_claim = if let Some((tenant_id, project_id)) = owner {
        expire_due_claims(
            &transaction,
            tenant_id,
            project_id,
            subject_type,
            &options.subject_id,
            &now,
        )?;
        current_claim_for_subject_tx(
            &transaction,
            tenant_id,
            project_id,
            subject_type,
            &options.subject_id,
            &now,
        )?
        .map(|claim| claim.summary())
    } else {
        expire_due_claims_for_subject(&transaction, subject_type, &options.subject_id, &now)?;
        current_claim_for_subject_any_owner_tx(
            &transaction,
            subject_type,
            &options.subject_id,
            &now,
        )?
        .map(|claim| claim.summary())
    };
    let mut claims = claims_for_subject(
        &transaction,
        subject_type,
        &options.subject_id,
        owner,
        limit,
        cursor,
    )?;
    let cursor = paginate(&mut claims, limit);
    transaction.commit()?;
    Ok(remediation_claims_response(
        subject_type.as_str(),
        &options.subject_id,
        claims,
        limit,
        current_claim,
        cursor,
    ))
}

pub(crate) fn read_scoped(
    connection: &mut Connection,
    claim_id: &str,
    tenant_id: &str,
    project_id: &str,
    service: Option<&str>,
) -> ClaimResult<Option<RemediationClaim>> {
    let transaction = connection.transaction()?;
    if let Some(claim) = row_by_id_tx(&transaction, claim_id)?
        && claim.tenant_id == tenant_id
        && claim.project_id == project_id
    {
        expire_due_claims(
            &transaction,
            tenant_id,
            project_id,
            parse_subject_type(&claim.subject_type)?,
            &claim.subject_id,
            &current_time_string(),
        )?;
    }
    let claim = row_by_id_tx(&transaction, claim_id)?
        .filter(|claim| claim.tenant_id == tenant_id && claim.project_id == project_id);
    if let (Some(claim), Some(service)) = (claim.as_ref(), service)
        && claim.service.as_deref() != Some(service)
    {
        return Ok(None);
    }
    transaction.commit()?;
    Ok(claim)
}

pub(crate) fn transition(
    connection: &mut Connection,
    transition: ClaimTransition,
) -> ClaimResult<RemediationClaim> {
    if !claim_state_is_valid(&transition.state) {
        return Err(ClaimError::InvalidState);
    }
    let transaction = connection.transaction()?;
    let mut claim = row_by_id_tx(&transaction, &transition.claim_id)?
        .filter(|claim| {
            claim.tenant_id == transition.tenant_id && claim.project_id == transition.project_id
        })
        .ok_or(ClaimError::NotFound)?;
    if let Some(service) = transition.service.as_deref()
        && claim.service.as_deref() != Some(service)
    {
        return Err(ClaimError::NotFound);
    }
    if claim.owner != transition.owner {
        return Err(ClaimError::InvalidTransition);
    }
    expire_due_claims(
        &transaction,
        &transition.tenant_id,
        &transition.project_id,
        parse_subject_type(&claim.subject_type)?,
        &claim.subject_id,
        &transition.now,
    )?;
    claim = row_by_id_tx(&transaction, &transition.claim_id)?.ok_or(ClaimError::NotFound)?;
    if is_terminal(&claim.state) && claim.state != transition.state {
        return Err(ClaimError::InvalidTransition);
    }
    let evidence_links = merged_evidence(&claim.evidence_links, &transition.evidence_links);
    let evidence_json = evidence_to_storage(&evidence_links)?;
    let completed_at = if is_terminal(&transition.state) {
        Some(transition.now.clone())
    } else {
        claim.completed_at.clone()
    };
    let released_at = if transition.state == "released" {
        Some(transition.now.clone())
    } else {
        claim.released_at.clone()
    };
    transaction.execute(
        "UPDATE remediation_claims
         SET state = ?1, evidence_links = ?2, updated_at = ?3, completed_at = ?4, released_at = ?5
         WHERE id = ?6",
        params![
            transition.state,
            evidence_json,
            transition.now,
            completed_at,
            released_at,
            transition.claim_id,
        ],
    )?;
    claim = row_by_id_tx(&transaction, &transition.claim_id)?.ok_or(ClaimError::NotFound)?;
    let event = if claim.state == "released" {
        "remediation_claim.released"
    } else {
        "remediation_claim.updated"
    };
    insert_event(
        &transaction,
        &transition.event_id,
        event,
        &claim,
        &transition.now,
    )?;
    transaction.commit()?;
    Ok(claim)
}

pub(crate) fn current_claim_for_subject(
    connection: &Connection,
    tenant_id: &str,
    project_id: &str,
    subject_type: &str,
    subject_id: &str,
    now: &str,
) -> ClaimResult<Option<RemediationClaimSummary>> {
    let subject_type = parse_subject_type(subject_type)?;
    Ok(current_claim_for_subject_conn(
        connection,
        tenant_id,
        project_id,
        subject_type,
        subject_id,
        now,
    )?
    .map(|claim| claim.summary()))
}

pub(crate) fn expire_due_claims_for_owner(
    connection: &mut Connection,
    tenant_id: &str,
    project_id: &str,
    now: &str,
) -> ClaimResult<()> {
    let transaction = connection.transaction()?;
    let mut statement = transaction.prepare(
        "SELECT id, tenant_id, project_id, service, subject_type, subject_id, owner, purpose,
                state, idempotency_key, evidence_links, created_at, updated_at, expires_at,
                released_at, completed_at
         FROM remediation_claims
         WHERE tenant_id = ?1 AND project_id = ?2
           AND state IN ('claimed', 'investigating', 'fix_proposed')
           AND expires_at <= ?3
         ORDER BY updated_at ASC, id ASC",
    )?;
    let due_claims = statement
        .query_map(params![tenant_id, project_id, now], claim_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    expire_claim_rows(&transaction, due_claims, now)?;
    transaction.commit()?;
    Ok(())
}

fn expire_due_claims(
    transaction: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    subject_type: ClaimSubjectType,
    subject_id: &str,
    now: &str,
) -> ClaimResult<()> {
    let mut statement = transaction.prepare(
        "SELECT id, tenant_id, project_id, service, subject_type, subject_id, owner, purpose,
                state, idempotency_key, evidence_links, created_at, updated_at, expires_at,
                released_at, completed_at
         FROM remediation_claims
         WHERE tenant_id = ?1 AND project_id = ?2 AND subject_type = ?3 AND subject_id = ?4
           AND state IN ('claimed', 'investigating', 'fix_proposed')
           AND expires_at <= ?5
         ORDER BY updated_at ASC, id ASC",
    )?;
    let due_claims = statement
        .query_map(
            params![
                tenant_id,
                project_id,
                subject_type.as_str(),
                subject_id,
                now
            ],
            claim_from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    expire_claim_rows(transaction, due_claims, now)
}

fn expire_due_claims_for_subject(
    transaction: &Transaction<'_>,
    subject_type: ClaimSubjectType,
    subject_id: &str,
    now: &str,
) -> ClaimResult<()> {
    let mut statement = transaction.prepare(
        "SELECT id, tenant_id, project_id, service, subject_type, subject_id, owner, purpose,
                state, idempotency_key, evidence_links, created_at, updated_at, expires_at,
                released_at, completed_at
         FROM remediation_claims
         WHERE subject_type = ?1 AND subject_id = ?2
           AND state IN ('claimed', 'investigating', 'fix_proposed')
           AND expires_at <= ?3
         ORDER BY updated_at ASC, id ASC",
    )?;
    let due_claims = statement
        .query_map(
            params![subject_type.as_str(), subject_id, now],
            claim_from_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    expire_claim_rows(transaction, due_claims, now)
}

fn expire_claim_rows(
    transaction: &Transaction<'_>,
    due_claims: Vec<RemediationClaim>,
    now: &str,
) -> ClaimResult<()> {
    for mut claim in due_claims {
        claim.state = "expired".to_owned();
        claim.updated_at = now.to_owned();
        claim.completed_at = Some(claim.completed_at.unwrap_or_else(|| now.to_owned()));
        transaction.execute(
            "UPDATE remediation_claims
             SET state = 'expired', updated_at = ?1, completed_at = COALESCE(completed_at, ?1)
             WHERE id = ?2",
            params![now, claim.id],
        )?;
        insert_event(
            transaction,
            &EventId::generate().into_string(),
            "remediation_claim.expired",
            &claim,
            now,
        )?;
    }
    Ok(())
}

fn current_time_string() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

fn current_claim_for_subject_tx(
    transaction: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    subject_type: ClaimSubjectType,
    subject_id: &str,
    now: &str,
) -> ClaimResult<Option<RemediationClaim>> {
    current_claim_for_subject_on_connection(
        transaction,
        tenant_id,
        project_id,
        subject_type,
        subject_id,
        now,
    )
}

fn current_claim_for_subject_conn(
    connection: &Connection,
    tenant_id: &str,
    project_id: &str,
    subject_type: ClaimSubjectType,
    subject_id: &str,
    now: &str,
) -> ClaimResult<Option<RemediationClaim>> {
    current_claim_for_subject_on_connection(
        connection,
        tenant_id,
        project_id,
        subject_type,
        subject_id,
        now,
    )
}

fn current_claim_for_subject_any_owner_tx(
    transaction: &Transaction<'_>,
    subject_type: ClaimSubjectType,
    subject_id: &str,
    now: &str,
) -> ClaimResult<Option<RemediationClaim>> {
    transaction
        .query_row(
            "SELECT id, tenant_id, project_id, service, subject_type, subject_id, owner, purpose,
                    state, idempotency_key, evidence_links, created_at, updated_at, expires_at,
                    released_at, completed_at
             FROM remediation_claims
             WHERE subject_type = ?1 AND subject_id = ?2
               AND state IN ('claimed', 'investigating', 'fix_proposed')
               AND expires_at > ?3
             ORDER BY updated_at DESC, id DESC
             LIMIT 1",
            params![subject_type.as_str(), subject_id, now],
            claim_from_row,
        )
        .optional()
        .map_err(ClaimError::from)
}

fn current_claim_for_subject_on_connection(
    connection: &Connection,
    tenant_id: &str,
    project_id: &str,
    subject_type: ClaimSubjectType,
    subject_id: &str,
    now: &str,
) -> ClaimResult<Option<RemediationClaim>> {
    connection
        .query_row(
            "SELECT id, tenant_id, project_id, service, subject_type, subject_id, owner, purpose,
                    state, idempotency_key, evidence_links, created_at, updated_at, expires_at,
                    released_at, completed_at
             FROM remediation_claims
             WHERE tenant_id = ?1 AND project_id = ?2 AND subject_type = ?3 AND subject_id = ?4
               AND state IN ('claimed', 'investigating', 'fix_proposed')
               AND expires_at > ?5
             ORDER BY updated_at DESC, id DESC
             LIMIT 1",
            params![
                tenant_id,
                project_id,
                subject_type.as_str(),
                subject_id,
                now
            ],
            claim_from_row,
        )
        .optional()
        .map_err(ClaimError::from)
}

fn claim_by_idempotency(
    transaction: &Transaction<'_>,
    tenant_id: &str,
    project_id: &str,
    subject_type: ClaimSubjectType,
    subject_id: &str,
    idempotency_key: &str,
) -> ClaimResult<Option<RemediationClaim>> {
    transaction
        .query_row(
            "SELECT id, tenant_id, project_id, service, subject_type, subject_id, owner, purpose,
                    state, idempotency_key, evidence_links, created_at, updated_at, expires_at,
                    released_at, completed_at
             FROM remediation_claims
             WHERE tenant_id = ?1 AND project_id = ?2 AND subject_type = ?3 AND subject_id = ?4
               AND idempotency_key = ?5
             ORDER BY created_at DESC, id DESC
             LIMIT 1",
            params![
                tenant_id,
                project_id,
                subject_type.as_str(),
                subject_id,
                idempotency_key
            ],
            claim_from_row,
        )
        .optional()
        .map_err(ClaimError::from)
}

fn claims_for_subject(
    connection: &Connection,
    subject_type: ClaimSubjectType,
    subject_id: &str,
    owner: Option<(&str, &str)>,
    limit: usize,
    cursor: Option<ClaimCursor>,
) -> ClaimResult<Vec<RemediationClaim>> {
    let owner_clause = if owner.is_some() {
        " AND tenant_id = ?3 AND project_id = ?4"
    } else {
        ""
    };
    if let Some(cursor) = cursor {
        let mut statement = connection.prepare(&format!(
            "SELECT id, tenant_id, project_id, service, subject_type, subject_id, owner, purpose,
                    state, idempotency_key, evidence_links, created_at, updated_at, expires_at,
                    released_at, completed_at
             FROM remediation_claims
             WHERE subject_type = ?1 AND subject_id = ?2
             {owner_clause}
               AND (created_at < ?5 OR (created_at = ?5 AND id < ?6))
             ORDER BY created_at DESC, id DESC
             LIMIT ?7",
        ))?;
        if let Some((tenant_id, project_id)) = owner {
            return statement
                .query_map(
                    params![
                        subject_type.as_str(),
                        subject_id,
                        tenant_id,
                        project_id,
                        cursor.created_at,
                        cursor.id,
                        (limit + 1) as i64
                    ],
                    claim_from_row,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(ClaimError::from);
        }
        let mut statement = connection.prepare(
            "SELECT id, tenant_id, project_id, service, subject_type, subject_id, owner, purpose,
                    state, idempotency_key, evidence_links, created_at, updated_at, expires_at,
                    released_at, completed_at
             FROM remediation_claims
             WHERE subject_type = ?1 AND subject_id = ?2
               AND (created_at < ?3 OR (created_at = ?3 AND id < ?4))
             ORDER BY created_at DESC, id DESC
             LIMIT ?5",
        )?;
        return statement
            .query_map(
                params![
                    subject_type.as_str(),
                    subject_id,
                    cursor.created_at,
                    cursor.id,
                    (limit + 1) as i64
                ],
                claim_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(ClaimError::from);
    }

    let mut statement = connection.prepare(&format!(
        "SELECT id, tenant_id, project_id, service, subject_type, subject_id, owner, purpose,
                state, idempotency_key, evidence_links, created_at, updated_at, expires_at,
                released_at, completed_at
         FROM remediation_claims
         WHERE subject_type = ?1 AND subject_id = ?2
         {owner_clause}
         ORDER BY created_at DESC, id DESC
         LIMIT ?5",
    ))?;
    if let Some((tenant_id, project_id)) = owner {
        statement
            .query_map(
                params![
                    subject_type.as_str(),
                    subject_id,
                    tenant_id,
                    project_id,
                    (limit + 1) as i64
                ],
                claim_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(ClaimError::from)
    } else {
        let mut statement = connection.prepare(
            "SELECT id, tenant_id, project_id, service, subject_type, subject_id, owner, purpose,
                    state, idempotency_key, evidence_links, created_at, updated_at, expires_at,
                    released_at, completed_at
             FROM remediation_claims
             WHERE subject_type = ?1 AND subject_id = ?2
             ORDER BY created_at DESC, id DESC
             LIMIT ?3",
        )?;
        statement
            .query_map(
                params![subject_type.as_str(), subject_id, (limit + 1) as i64],
                claim_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(ClaimError::from)
    }
}

fn row_by_id_tx(
    transaction: &Transaction<'_>,
    claim_id: &str,
) -> ClaimResult<Option<RemediationClaim>> {
    transaction
        .query_row(
            "SELECT id, tenant_id, project_id, service, subject_type, subject_id, owner, purpose,
                    state, idempotency_key, evidence_links, created_at, updated_at, expires_at,
                    released_at, completed_at
             FROM remediation_claims
             WHERE id = ?1",
            params![claim_id],
            claim_from_row,
        )
        .optional()
        .map_err(ClaimError::from)
}

fn insert_event(
    transaction: &Transaction<'_>,
    event_id: &str,
    event: &str,
    claim: &RemediationClaim,
    now: &str,
) -> ClaimResult<()> {
    let payload = json!({
        "event": event,
        "tenant_id": claim.tenant_id,
        "project_id": claim.project_id,
        "service": claim.service,
        "claim": claim,
        "timestamp": now,
    });
    transaction.execute(
        "INSERT INTO service_events (
             id, tenant_id, project_id, service, event, entity_type, entity_ref, severity, summary, payload, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9, ?10)",
        params![
            event_id,
            claim.tenant_id,
            claim.project_id,
            claim.service.as_deref().unwrap_or("unknown"),
            event,
            claim.subject_type,
            claim.subject_id,
            claim_summary(event, claim),
            payload.to_string(),
            now,
        ],
    )?;
    Ok(())
}

fn claim_summary(event: &str, claim: &RemediationClaim) -> String {
    match event {
        "remediation_claim.created" => {
            format!(
                "{} claimed {} {}.",
                claim.owner, claim.subject_type, claim.subject_id
            )
        }
        "remediation_claim.released" => {
            format!(
                "{} released {} {}.",
                claim.owner, claim.subject_type, claim.subject_id
            )
        }
        _ => format!(
            "{} set {} {} to {}.",
            claim.owner, claim.subject_type, claim.subject_id, claim.state
        ),
    }
}

fn parse_subject_type(value: &str) -> ClaimResult<ClaimSubjectType> {
    ClaimSubjectType::parse(value).ok_or(ClaimError::InvalidSubjectType)
}

fn validate_non_empty(value: &str) -> ClaimResult<()> {
    if value.trim().is_empty() {
        Err(ClaimError::InvalidClaim)
    } else {
        Ok(())
    }
}

fn validate_evidence_links(evidence_links: &[String]) -> ClaimResult<()> {
    if evidence_links.iter().any(|value| value.trim().is_empty()) {
        Err(ClaimError::InvalidClaim)
    } else {
        Ok(())
    }
}

fn validate_rfc3339_like(value: &str) -> ClaimResult<()> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map(|_| ())
        .map_err(|_| ClaimError::InvalidClaim)
}

fn parse_limit(limit: Option<&str>) -> ClaimResult<usize> {
    match limit {
        None | Some("") => Ok(DEFAULT_CLAIM_LIMIT),
        Some(value) => match value.parse::<usize>() {
            Ok(value) if (1..=MAX_CLAIM_LIMIT).contains(&value) => Ok(value),
            _ => Err(ClaimError::InvalidLimit),
        },
    }
}

fn parse_cursor(cursor: Option<&str>) -> ClaimResult<Option<ClaimCursor>> {
    match cursor {
        None | Some("") => Ok(None),
        Some(value) => decode_claim_cursor(value)
            .map(Some)
            .ok_or(ClaimError::InvalidCursor),
    }
}

fn paginate(rows: &mut Vec<RemediationClaim>, limit: usize) -> Option<String> {
    if rows.len() <= limit {
        return None;
    }
    rows.truncate(limit);
    rows.last().and_then(|last| {
        encode_claim_cursor(&ClaimCursor {
            created_at: last.created_at.clone(),
            id: last.id.clone(),
        })
    })
}

fn is_terminal(state: &str) -> bool {
    matches!(state, "verified" | "dismissed" | "expired" | "released")
}

fn merged_evidence(existing: &[String], additions: &[String]) -> Vec<String> {
    let mut merged = existing.to_vec();
    for link in additions {
        if !merged.contains(link) {
            merged.push(link.clone());
        }
    }
    merged
}

fn evidence_to_storage(evidence_links: &[String]) -> ClaimResult<String> {
    serde_json::to_string(evidence_links).map_err(|_| ClaimError::InvalidClaim)
}

fn decode_evidence(value: Option<String>) -> rusqlite::Result<Vec<String>> {
    match value {
        Some(value) => serde_json::from_str::<Vec<String>>(&value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(10, Type::Text, Box::new(error))
        }),
        None => Ok(Vec::new()),
    }
}

fn decode_rfc3339(value: String, column: usize) -> rusqlite::Result<String> {
    time::OffsetDateTime::parse(&value, &time::format_description::well_known::Rfc3339)
        .map(|_| value)
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(error))
        })
}

fn claim_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RemediationClaim> {
    Ok(RemediationClaim {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        project_id: row.get(2)?,
        service: row.get(3)?,
        subject_type: row.get(4)?,
        subject_id: row.get(5)?,
        owner: row.get(6)?,
        purpose: row.get(7)?,
        state: row.get(8)?,
        idempotency_key: row.get(9)?,
        evidence_links: decode_evidence(row.get(10)?)?,
        created_at: decode_rfc3339(row.get(11)?, 11)?,
        updated_at: decode_rfc3339(row.get(12)?, 12)?,
        expires_at: decode_rfc3339(row.get(13)?, 13)?,
        released_at: row
            .get::<_, Option<String>>(14)?
            .map(|value| decode_rfc3339(value, 14))
            .transpose()?,
        completed_at: row
            .get::<_, Option<String>>(15)?
            .map(|value| decode_rfc3339(value, 15))
            .transpose()?,
    })
}

fn require_subject(
    connection: &Connection,
    subject_type: ClaimSubjectType,
    subject_id: &str,
    owner: Option<(&str, &str)>,
    service: Option<&str>,
) -> ClaimResult<Option<String>> {
    let mut where_clause = match subject_type {
        ClaimSubjectType::ErrorGroup => " WHERE group_hash = ?".to_owned(),
        ClaimSubjectType::Incident | ClaimSubjectType::Target | ClaimSubjectType::Monitor => {
            " WHERE id = ?".to_owned()
        }
    };
    let mut filters = vec![subject_id.to_owned()];
    if let Some((tenant_id, project_id)) = owner {
        where_clause.push_str(" AND tenant_id = ? AND project_id = ?");
        filters.push(tenant_id.to_owned());
        filters.push(project_id.to_owned());
    }
    if let Some(service) = service {
        match subject_type {
            ClaimSubjectType::Incident | ClaimSubjectType::ErrorGroup => {
                where_clause.push_str(" AND service = ?");
            }
            ClaimSubjectType::Target | ClaimSubjectType::Monitor => {
                where_clause.push_str(" AND COALESCE(NULLIF(service, ''), name) = ?");
            }
        }
        filters.push(service.to_owned());
    }

    let service_expr = match subject_type {
        ClaimSubjectType::Incident | ClaimSubjectType::ErrorGroup => "service",
        ClaimSubjectType::Target | ClaimSubjectType::Monitor => {
            "COALESCE(NULLIF(service, ''), name)"
        }
    };
    let sql = match subject_type {
        ClaimSubjectType::Incident => {
            format!("SELECT {service_expr} FROM incidents{where_clause} LIMIT 1")
        }
        ClaimSubjectType::ErrorGroup => {
            format!("SELECT {service_expr} FROM error_groups{where_clause} LIMIT 1")
        }
        ClaimSubjectType::Target => {
            format!("SELECT {service_expr} FROM targets{where_clause} LIMIT 1")
        }
        ClaimSubjectType::Monitor => {
            format!("SELECT {service_expr} FROM monitors{where_clause} LIMIT 1")
        }
    };
    let mut statement = connection.prepare(&sql)?;
    statement
        .query_row(rusqlite::params_from_iter(filters), |row| {
            row.get::<_, Option<String>>(0)
        })
        .optional()?
        .ok_or(ClaimError::NotFound)
}
