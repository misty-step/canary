use bcrypt::verify;
use rusqlite::{Connection, params};

use crate::Result;

/// Number of leading characters stored by as `api_keys.key_prefix`.
pub const API_KEY_PREFIX_LEN: usize = 12;
/// Tenant assigned to pre-multitenant rows during the ownership migration.
pub const BOOTSTRAP_TENANT_ID: &str = "TENANT-bootstrap";
/// Project assigned to pre-multitenant rows during the ownership migration.
pub const BOOTSTRAP_PROJECT_ID: &str = "PROJECT-bootstrap";

const DUMMY_BCRYPT_HASH: &str = "$2b$12$C6UzMDM.H6dfI/f/IKcEeO6H9G7Qe0eeDVF2.oTu.2R4z.0/t6j2K";

/// API-key row inserted by callers that already generated and hashed the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyInsert {
    /// Stable API-key identifier.
    pub id: String,
    /// Human-readable key name.
    pub name: String,
    /// First 12 characters of the raw key.
    pub key_prefix: String,
    /// Bcrypt hash of the full raw key.
    pub key_hash: String,
    /// ISO8601 creation timestamp.
    pub created_at: String,
    /// ISO8601 revocation timestamp, when the key is inactive.
    pub revoked_at: Option<String>,
    /// Wire-value scope, such as `admin` or `ingest-only`.
    pub scope: String,
    /// Tenant this key can operate within.
    pub tenant_id: String,
    /// Project this key can operate within.
    pub project_id: String,
    /// Optional service this key is bound to for constrained ingest/read use.
    pub service: Option<String>,
}

/// Active API key whose bcrypt hash matched the supplied raw bearer token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedApiKey {
    /// Stable API-key identifier.
    pub id: String,
    /// Human-readable key name.
    pub name: String,
    /// Wire-value scope, such as `admin` or `ingest-only`.
    pub scope: String,
    /// Tenant this key can operate within.
    pub tenant_id: String,
    /// Project this key can operate within.
    pub project_id: String,
    /// Optional service this key is bound to for constrained ingest/read use.
    pub service: Option<String>,
}

/// Admin-visible API key metadata. The raw key and hash are never exposed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyRecord {
    /// Stable API-key identifier.
    pub id: String,
    /// Human-readable key name.
    pub name: String,
    /// Wire-value scope.
    pub scope: String,
    /// First 12 characters of the raw key.
    pub key_prefix: String,
    /// ISO8601 creation timestamp.
    pub created_at: String,
    /// ISO8601 revocation timestamp, when inactive.
    pub revoked_at: Option<String>,
    /// Tenant this key can operate within.
    pub tenant_id: String,
    /// Project this key can operate within.
    pub project_id: String,
    /// Optional service this key is bound to for constrained ingest/read use.
    pub service: Option<String>,
}

pub(crate) fn insert(connection: &Connection, key: ApiKeyInsert) -> Result<()> {
    connection.execute(
        "INSERT INTO api_keys (
            id, name, key_prefix, key_hash, created_at, revoked_at, scope,
            tenant_id, project_id, service
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            key.id,
            key.name,
            key.key_prefix,
            key.key_hash,
            key.created_at,
            key.revoked_at,
            key.scope,
            key.tenant_id,
            key.project_id,
            key.service
        ],
    )?;
    Ok(())
}

pub(crate) fn list(connection: &Connection) -> Result<Vec<ApiKeyRecord>> {
    let mut statement = connection.prepare(
        "SELECT id, name, scope, key_prefix, created_at, revoked_at,
                tenant_id, project_id, service
         FROM api_keys
         ORDER BY created_at DESC",
    )?;
    let keys = statement
        .query_map([], |row| {
            Ok(ApiKeyRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                scope: row.get(2)?,
                key_prefix: row.get(3)?,
                created_at: row.get(4)?,
                revoked_at: row.get(5)?,
                tenant_id: row.get(6)?,
                project_id: row.get(7)?,
                service: row.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(keys)
}

pub(crate) fn list_scoped(
    connection: &Connection,
    tenant_id: &str,
    project_id: &str,
) -> Result<Vec<ApiKeyRecord>> {
    let mut statement = connection.prepare(
        "SELECT id, name, scope, key_prefix, created_at, revoked_at,
                tenant_id, project_id, service
         FROM api_keys
         WHERE tenant_id = ?1 AND project_id = ?2
         ORDER BY created_at DESC",
    )?;
    let keys = statement
        .query_map(params![tenant_id, project_id], |row| {
            Ok(ApiKeyRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                scope: row.get(2)?,
                key_prefix: row.get(3)?,
                created_at: row.get(4)?,
                revoked_at: row.get(5)?,
                tenant_id: row.get(6)?,
                project_id: row.get(7)?,
                service: row.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(keys)
}

pub(crate) fn revoke(connection: &Connection, key_id: &str, revoked_at: &str) -> Result<bool> {
    let changed = connection.execute(
        "UPDATE api_keys
         SET revoked_at = ?2
         WHERE id = ?1",
        params![key_id, revoked_at],
    )?;
    Ok(changed > 0)
}

pub(crate) fn revoke_scoped(
    connection: &Connection,
    key_id: &str,
    revoked_at: &str,
    tenant_id: &str,
    project_id: &str,
) -> Result<bool> {
    let changed = connection.execute(
        "UPDATE api_keys
         SET revoked_at = ?2
         WHERE id = ?1 AND tenant_id = ?3 AND project_id = ?4 AND revoked_at IS NULL",
        params![key_id, revoked_at, tenant_id, project_id],
    )?;
    Ok(changed > 0)
}

pub(crate) fn verify_key(connection: &Connection, raw_key: &str) -> Result<Option<VerifiedApiKey>> {
    let prefix = key_prefix(raw_key);
    let candidates = active_candidates(connection, &prefix)?;

    if candidates.is_empty() {
        let _ = verify(raw_key, DUMMY_BCRYPT_HASH);
        return Ok(None);
    }

    for candidate in candidates {
        if matches!(verify(raw_key, &candidate.key_hash), Ok(true)) {
            return Ok(Some(VerifiedApiKey {
                id: candidate.id,
                name: candidate.name,
                scope: candidate.scope,
                tenant_id: candidate.tenant_id,
                project_id: candidate.project_id,
                service: candidate.service,
            }));
        }
    }

    Ok(None)
}

pub(crate) fn active_key_prefix_exists(connection: &Connection, raw_key: &str) -> Result<bool> {
    let prefix = key_prefix(raw_key);
    let exists = connection.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM api_keys
            WHERE key_prefix = ?1 AND revoked_at IS NULL
         )",
        [prefix],
        |row| row.get::<_, bool>(0),
    )?;
    Ok(exists)
}

pub(crate) fn key_prefix(raw_key: &str) -> String {
    raw_key.chars().take(API_KEY_PREFIX_LEN).collect()
}

fn active_candidates(connection: &Connection, key_prefix: &str) -> Result<Vec<ApiKeyCandidate>> {
    let mut statement = connection.prepare(
        "SELECT id, name, scope, key_hash, tenant_id, project_id, service
         FROM api_keys
         WHERE key_prefix = ?1 AND revoked_at IS NULL
         ORDER BY created_at ASC, id ASC",
    )?;
    let candidates = statement
        .query_map([key_prefix], |row| {
            Ok(ApiKeyCandidate {
                id: row.get(0)?,
                name: row.get(1)?,
                scope: row.get(2)?,
                key_hash: row.get(3)?,
                tenant_id: row.get(4)?,
                project_id: row.get(5)?,
                service: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(candidates)
}

struct ApiKeyCandidate {
    id: String,
    name: String,
    scope: String,
    key_hash: String,
    tenant_id: String,
    project_id: String,
    service: Option<String>,
}
