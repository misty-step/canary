//! Operator key minting.
//!
//! `canary-server mint-key` is the no-data-loss recovery path for issuing a new
//! scoped API key directly against the production SQLite store when the
//! one-time bootstrap key has been lost. It reuses the exact wire shape of the
//! `POST /api/v1/keys` admin route (raw `sk_<env>_<nanoid>`, 12-char prefix,
//! bcrypt hash) so minted keys are indistinguishable from API-issued ones.

use std::path::Path;

use canary_store::{API_KEY_PREFIX_LEN, ApiKeyInsert, Store};

use crate::server_time::current_rfc3339;

/// Scopes accepted by the minting path, mirroring the router's permission model.
const VALID_SCOPES: [&str; 3] = ["admin", "read-only", "ingest-only"];

/// Failure modes when minting an operator API key.
#[derive(Debug)]
pub enum MintKeyError {
    /// The requested scope is not one of `admin`, `read-only`, `ingest-only`.
    InvalidScope(String),
    /// Opening, migrating, or writing to the store failed.
    Store(canary_store::StoreError),
    /// Bcrypt hashing of the raw key failed.
    Hash(bcrypt::BcryptError),
}

impl std::fmt::Display for MintKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MintKeyError::InvalidScope(scope) => write!(
                f,
                "invalid scope {scope:?}; expected one of admin, read-only, ingest-only"
            ),
            MintKeyError::Store(error) => write!(f, "store error: {error}"),
            MintKeyError::Hash(error) => write!(f, "hash error: {error}"),
        }
    }
}

impl std::error::Error for MintKeyError {}

/// Mint a scoped API key against the store at `db_path` and return the raw key.
///
/// The raw key is shown only here; the store persists the bcrypt hash. Opening
/// a second connection while the server runs is safe for this one-shot insert:
/// SQLite WAL serializes the brief write transaction behind the live writer.
pub fn mint_key(db_path: &Path, scope: &str, name: &str) -> Result<String, MintKeyError> {
    if !VALID_SCOPES.contains(&scope) {
        return Err(MintKeyError::InvalidScope(scope.to_owned()));
    }

    let mut store = Store::open(db_path).map_err(MintKeyError::Store)?;
    store.migrate().map_err(MintKeyError::Store)?;

    let raw_key = canary_core::secrets::api_key("live");
    let key_hash = bcrypt::hash(&raw_key, bcrypt::DEFAULT_COST).map_err(MintKeyError::Hash)?;

    store
        .insert_api_key(ApiKeyInsert {
            id: canary_core::ids::ApiKeyId::generate().into_string(),
            name: name.to_owned(),
            key_prefix: raw_key.chars().take(API_KEY_PREFIX_LEN).collect(),
            key_hash,
            created_at: current_rfc3339(),
            revoked_at: None,
            scope: scope.to_owned(),
        })
        .map_err(MintKeyError::Store)?;

    Ok(raw_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Unique temp DB path per test without pulling in a temp-file dependency.
    fn unique_db_path(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "canary-keygen-test-{tag}-{}-{nanos}-{seq}.db",
            std::process::id()
        ))
    }

    struct DbGuard(PathBuf);
    impl Drop for DbGuard {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
            }
        }
    }

    #[test]
    fn mint_key_round_trips_through_verification() -> Result<(), Box<dyn std::error::Error>> {
        let db_path = unique_db_path("roundtrip");
        let _guard = DbGuard(db_path.clone());

        let raw_key = mint_key(&db_path, "admin", "recovery")?;

        let store = Store::open(&db_path)?;
        let verified = store
            .verify_api_key(&raw_key)?
            .ok_or("minted key should verify as active")?;
        assert_eq!(verified.scope, "admin");
        Ok(())
    }

    #[test]
    fn mint_key_rejects_unknown_scope() {
        let db_path = unique_db_path("badscope");
        let _guard = DbGuard(db_path.clone());

        let result = mint_key(&db_path, "superuser", "x");
        assert!(
            matches!(&result, Err(MintKeyError::InvalidScope(scope)) if scope == "superuser"),
            "expected InvalidScope(\"superuser\"), got {result:?}"
        );
    }
}
