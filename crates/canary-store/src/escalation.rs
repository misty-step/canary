//! Incident escalation: a first-class but orthogonal overlay on top of the
//! deterministic incident state machine owned by `incidents.rs`.
//!
//! Escalation never touches `incidents.state` (that stays a pure function of
//! signal activity, per the module comment on `incidents.rs`). It is modeled
//! the same way remediation claims layer owner + state + timeline events on
//! top of an incident without touching `state`: a nullable `escalated_at`
//! column, set and cleared through this module, with auto-clear on
//! resolution enforced inside `incidents::update_incident_state` so an
//! escalation can never outlive its incident.

use canary_core::query::IncidentEscalation;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::json;

/// Result type returned by incident-escalation read/write models.
pub type EscalationResult<T> = std::result::Result<T, EscalationError>;

/// Escalation validation or storage failure.
#[derive(Debug, thiserror::Error)]
pub enum EscalationError {
    /// A required field was missing or empty.
    #[error("invalid escalation")]
    InvalidEscalation,
    /// Incident row does not exist within the caller's tenant/project/service authority.
    #[error("incident not found")]
    NotFound,
    /// The incident is already resolved; escalation is rejected.
    #[error("incident already resolved")]
    AlreadyResolved,
    /// SQLite rejected the operation.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Command to escalate one incident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscalationInsert {
    /// Incident id.
    pub incident_id: String,
    /// Tenant namespace.
    pub tenant_id: String,
    /// Project namespace.
    pub project_id: String,
    /// Optional service authority boundary.
    pub service: Option<String>,
    /// Agent or human owner requesting the escalation.
    pub owner: String,
    /// Reason for the escalation.
    pub reason: String,
    /// Purpose classification (rides the timeline/webhook payload only; not persisted).
    pub purpose: String,
    /// Idempotency key. Replaying the same key returns the existing escalation.
    pub idempotency_key: String,
    /// Lifecycle event id for the `incident.escalated` timeline entry.
    pub event_id: String,
    /// RFC3339 escalation timestamp.
    pub now: String,
}

/// Command to clear an incident's escalation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeescalationRequest {
    /// Incident id.
    pub incident_id: String,
    /// Tenant namespace.
    pub tenant_id: String,
    /// Project namespace.
    pub project_id: String,
    /// Optional service authority boundary.
    pub service: Option<String>,
    /// Agent or human owner performing the correction.
    pub owner: String,
    /// Optional reason for the correction.
    pub reason: Option<String>,
    /// Lifecycle event id for the `incident.deescalated` timeline entry.
    pub event_id: String,
    /// RFC3339 deescalation timestamp.
    pub now: String,
}

/// Result of an escalate operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EscalateOutcome {
    /// Current escalation overlay state.
    pub escalation: IncidentEscalation,
    /// True when this call set `escalated_at` (as opposed to an idempotent replay).
    pub created: bool,
    /// Incident's resolved service, for callers building a webhook payload.
    pub service: String,
}

/// Result of a deescalate operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeescalateOutcome {
    /// Current escalation overlay state (always cleared).
    pub escalation: IncidentEscalation,
    /// True when this call cleared an active escalation (as opposed to a no-op).
    pub changed: bool,
    /// Incident's resolved service, for callers building a webhook payload.
    pub service: String,
}

struct IncidentEscalationRow {
    id: String,
    service: String,
    resolved_at: Option<String>,
    escalated_at: Option<String>,
    escalated_by: Option<String>,
    escalated_reason: Option<String>,
}

pub(crate) fn escalate(
    connection: &mut Connection,
    insert: EscalationInsert,
) -> EscalationResult<EscalateOutcome> {
    validate_non_empty(&insert.owner)?;
    validate_non_empty(&insert.reason)?;
    validate_non_empty(&insert.purpose)?;
    validate_non_empty(&insert.idempotency_key)?;

    let transaction = connection.transaction()?;
    let incident = incident_row_scoped(
        &transaction,
        &insert.incident_id,
        &insert.tenant_id,
        &insert.project_id,
        insert.service.as_deref(),
    )?
    .ok_or(EscalationError::NotFound)?;

    if incident.resolved_at.is_some() {
        return Err(EscalationError::AlreadyResolved);
    }

    if incident.escalated_at.is_some() {
        // Already escalated. Replaying the same idempotency key — or any
        // repeat escalate call while the escalation is still active — is a
        // safe no-op: escalation is a single boolean overlay, not a
        // competitive-ownership resource like a remediation claim, so a
        // second call never needs to error.
        let escalation = IncidentEscalation {
            incident_id: incident.id,
            escalated_at: incident.escalated_at,
            escalated_by: incident.escalated_by,
            reason: incident.escalated_reason,
        };
        transaction.commit()?;
        return Ok(EscalateOutcome {
            escalation,
            created: false,
            service: incident.service,
        });
    }

    transaction.execute(
        "UPDATE incidents
         SET escalated_at = ?1, escalated_by = ?2, escalated_reason = ?3, escalated_idempotency_key = ?4
         WHERE id = ?5",
        params![
            insert.now,
            insert.owner,
            insert.reason,
            insert.idempotency_key,
            incident.id
        ],
    )?;
    insert_event(
        &transaction,
        &insert.event_id,
        "incident.escalated",
        &incident.id,
        &insert.tenant_id,
        &insert.project_id,
        &incident.service,
        &insert.owner,
        Some(&insert.reason),
        Some(&insert.purpose),
        Some(&insert.now),
        &insert.now,
    )?;
    transaction.commit()?;

    Ok(EscalateOutcome {
        escalation: IncidentEscalation {
            incident_id: incident.id,
            escalated_at: Some(insert.now),
            escalated_by: Some(insert.owner),
            reason: Some(insert.reason),
        },
        created: true,
        service: incident.service,
    })
}

pub(crate) fn deescalate(
    connection: &mut Connection,
    request: DeescalationRequest,
) -> EscalationResult<DeescalateOutcome> {
    validate_non_empty(&request.owner)?;

    let transaction = connection.transaction()?;
    let incident = incident_row_scoped(
        &transaction,
        &request.incident_id,
        &request.tenant_id,
        &request.project_id,
        request.service.as_deref(),
    )?
    .ok_or(EscalationError::NotFound)?;

    if incident.escalated_at.is_none() {
        // Already clear. Deescalating an incident that is not escalated is a
        // safe no-op so correction calls never need to race a resolution.
        transaction.commit()?;
        return Ok(DeescalateOutcome {
            escalation: IncidentEscalation {
                incident_id: incident.id,
                escalated_at: None,
                escalated_by: None,
                reason: None,
            },
            changed: false,
            service: incident.service,
        });
    }

    transaction.execute(
        "UPDATE incidents
         SET escalated_at = NULL, escalated_by = NULL, escalated_reason = NULL, escalated_idempotency_key = NULL
         WHERE id = ?1",
        params![incident.id],
    )?;
    insert_event(
        &transaction,
        &request.event_id,
        "incident.deescalated",
        &incident.id,
        &request.tenant_id,
        &request.project_id,
        &incident.service,
        &request.owner,
        request.reason.as_deref(),
        None,
        None,
        &request.now,
    )?;
    transaction.commit()?;

    Ok(DeescalateOutcome {
        escalation: IncidentEscalation {
            incident_id: incident.id,
            escalated_at: None,
            escalated_by: None,
            reason: None,
        },
        changed: true,
        service: incident.service,
    })
}

fn incident_row_scoped(
    transaction: &Transaction<'_>,
    incident_id: &str,
    tenant_id: &str,
    project_id: &str,
    service: Option<&str>,
) -> EscalationResult<Option<IncidentEscalationRow>> {
    let incident = transaction
        .query_row(
            "SELECT id, service, resolved_at, escalated_at, escalated_by, escalated_reason
             FROM incidents
             WHERE id = ?1 AND tenant_id = ?2 AND project_id = ?3",
            params![incident_id, tenant_id, project_id],
            |row| {
                Ok(IncidentEscalationRow {
                    id: row.get(0)?,
                    service: row.get(1)?,
                    resolved_at: row.get(2)?,
                    escalated_at: row.get(3)?,
                    escalated_by: row.get(4)?,
                    escalated_reason: row.get(5)?,
                })
            },
        )
        .optional()?;
    Ok(match (incident, service) {
        (Some(incident), Some(service)) if incident.service != service => None,
        (incident, _) => incident,
    })
}

#[allow(clippy::too_many_arguments)]
fn insert_event(
    transaction: &Transaction<'_>,
    event_id: &str,
    event: &str,
    incident_id: &str,
    tenant_id: &str,
    project_id: &str,
    service: &str,
    owner: &str,
    reason: Option<&str>,
    purpose: Option<&str>,
    escalated_at: Option<&str>,
    now: &str,
) -> EscalationResult<()> {
    let payload = json!({
        "event": event,
        "tenant_id": tenant_id,
        "project_id": project_id,
        "service": service,
        "escalation": {
            "incident_id": incident_id,
            "escalated_at": escalated_at,
            "escalated_by": owner,
            "reason": reason,
            "purpose": purpose,
        },
        "timestamp": now,
    });
    let summary = match event {
        "incident.escalated" => format!("{owner} escalated incident {incident_id}."),
        "incident.deescalated" => format!("{owner} deescalated incident {incident_id}."),
        _ => format!("{owner} set incident {incident_id} to {event}."),
    };
    transaction.execute(
        "INSERT INTO service_events (
             id, tenant_id, project_id, service, event, entity_type, entity_ref, severity, summary, payload, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'incident', ?6, NULL, ?7, ?8, ?9)",
        params![
            event_id,
            tenant_id,
            project_id,
            service,
            event,
            incident_id,
            summary,
            payload.to_string(),
            now,
        ],
    )?;
    Ok(())
}

fn validate_non_empty(value: &str) -> EscalationResult<()> {
    if value.trim().is_empty() {
        Err(EscalationError::InvalidEscalation)
    } else {
        Ok(())
    }
}
