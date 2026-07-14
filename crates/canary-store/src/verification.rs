//! Read-only restored-database verification.

use std::{collections::BTreeMap, path::Path};

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

use crate::{CURRENT_SCHEMA_VERSION, Result};

/// Provider-neutral evidence returned for a restored SQLite database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DataVerification {
    /// Stable schema for automation consumers.
    pub schema: &'static str,
    /// Schema version recorded by SQLite.
    pub schema_version: u32,
    /// Schema version expected by the inspecting Canary build.
    pub expected_schema_version: u32,
    /// Whether the restored database is current for this build.
    pub schema_current: bool,
    /// Result of SQLite's full integrity check.
    pub integrity_check: String,
    /// Number of foreign-key violations.
    pub foreign_key_violations: u64,
    /// Deterministic row counts for every application-owned table present.
    pub table_counts: BTreeMap<String, u64>,
}

impl DataVerification {
    /// Whether the database is structurally valid and current for this build.
    #[must_use]
    pub fn verified(&self) -> bool {
        self.schema_current && self.integrity_check == "ok" && self.foreign_key_violations == 0
    }
}

/// Inspect one SQLite file without migrating or otherwise mutating it.
pub fn verify_database(path: impl AsRef<Path>) -> Result<DataVerification> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    verify_connection(&connection)
}

fn verify_connection(connection: &Connection) -> Result<DataVerification> {
    let schema_version = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let integrity_check = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    let foreign_key_violations =
        connection.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;

    let mut statement = connection.prepare(
        "SELECT name FROM sqlite_schema
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
         ORDER BY name",
    )?;
    let table_names = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut table_counts = BTreeMap::new();
    for table_name in table_names {
        let quoted = table_name.replace('"', "\"\"");
        let count =
            connection.query_row(&format!("SELECT count(*) FROM \"{quoted}\""), [], |row| {
                row.get(0)
            })?;
        table_counts.insert(table_name, count);
    }

    Ok(DataVerification {
        schema: "canary.data-verification.v1",
        schema_version,
        expected_schema_version: CURRENT_SCHEMA_VERSION,
        schema_current: schema_version == CURRENT_SCHEMA_VERSION,
        integrity_check,
        foreign_key_violations,
        table_counts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    #[test]
    fn migrated_database_returns_current_read_only_evidence() -> Result<()> {
        let mut connection = Connection::open_in_memory()?;
        schema::migrate(&mut connection)?;
        connection.execute(
            "INSERT INTO seed_runs (seed_name, applied_at) VALUES ('verification', '2026-07-14T00:00:00Z')",
            [],
        )?;

        let evidence = verify_connection(&connection)?;

        assert!(evidence.verified());
        assert_eq!(evidence.schema, "canary.data-verification.v1");
        assert_eq!(evidence.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(evidence.integrity_check, "ok");
        assert_eq!(evidence.foreign_key_violations, 0);
        assert_eq!(evidence.table_counts.get("seed_runs"), Some(&1));
        Ok(())
    }

    #[test]
    fn older_schema_is_intact_but_not_current() -> Result<()> {
        let connection = Connection::open_in_memory()?;
        connection.execute("CREATE TABLE sample (id INTEGER PRIMARY KEY)", [])?;

        let evidence = verify_connection(&connection)?;

        assert!(!evidence.verified());
        assert_eq!(evidence.integrity_check, "ok");
        assert_eq!(evidence.schema_version, 0);
        assert_eq!(evidence.table_counts.get("sample"), Some(&0));
        Ok(())
    }
}
