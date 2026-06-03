//! First-boot seed commands.
//!
//! Seeds are intentionally sparse: Canary creates only the bootstrap API key
//! needed to configure runtime targets, monitors, and webhooks through the API.

use canary_core::{ids::ApiKeyId, secrets};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{ApiKeyInsert, Result, api_keys};

const INITIAL_CONFIG_SEED: &str = "initial_config_v1";

pub(crate) fn apply_initial_seed(
    connection: &mut Connection,
    applied_at: &str,
) -> Result<Option<String>> {
    let transaction = connection.transaction()?;
    let already_applied = transaction
        .query_row(
            "SELECT 1 FROM seed_runs WHERE seed_name = ?1 LIMIT 1",
            [INITIAL_CONFIG_SEED],
            |_| Ok(()),
        )
        .optional()?
        .is_some();

    if already_applied {
        transaction.commit()?;
        return Ok(None);
    }

    let raw_key = secrets::api_key("live");
    api_keys::insert(
        &transaction,
        ApiKeyInsert {
            id: ApiKeyId::generate().into_string(),
            name: "bootstrap".to_owned(),
            key_prefix: api_keys::key_prefix(&raw_key),
            key_hash: bcrypt::hash(&raw_key, bcrypt::DEFAULT_COST)?,
            created_at: applied_at.to_owned(),
            revoked_at: None,
            scope: "admin".to_owned(),
        },
    )?;
    transaction.execute(
        "INSERT INTO seed_runs (seed_name, applied_at) VALUES (?1, ?2)",
        params![INITIAL_CONFIG_SEED, applied_at],
    )?;
    transaction.commit()?;

    Ok(Some(raw_key))
}
