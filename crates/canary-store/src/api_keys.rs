use bcrypt::verify;
use rusqlite::{Connection, params};

use crate::Result;

/// Number of leading characters stored by Phoenix as `api_keys.key_prefix`.
pub const API_KEY_PREFIX_LEN: usize = 12;

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
    /// Phoenix wire-value scope, such as `admin` or `ingest-only`.
    pub scope: String,
}

/// Active API key whose bcrypt hash matched the supplied raw bearer token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedApiKey {
    /// Stable API-key identifier.
    pub id: String,
    /// Human-readable key name.
    pub name: String,
    /// Phoenix wire-value scope, such as `admin` or `ingest-only`.
    pub scope: String,
}

pub(crate) fn insert(connection: &Connection, key: ApiKeyInsert) -> Result<()> {
    connection.execute(
        "INSERT INTO api_keys (
            id, name, key_prefix, key_hash, created_at, revoked_at, scope
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            key.id,
            key.name,
            key.key_prefix,
            key.key_hash,
            key.created_at,
            key.revoked_at,
            key.scope
        ],
    )?;
    Ok(())
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
            }));
        }
    }

    Ok(None)
}

pub(crate) fn key_prefix(raw_key: &str) -> String {
    raw_key.chars().take(API_KEY_PREFIX_LEN).collect()
}

fn active_candidates(connection: &Connection, key_prefix: &str) -> Result<Vec<ApiKeyCandidate>> {
    let mut statement = connection.prepare(
        "SELECT id, name, scope, key_hash
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
}
