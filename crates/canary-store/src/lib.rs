//! SQLite persistence boundary for the Rust rewrite of Canary.
//!
//! This crate owns the database shape and the single-writer connection. Callers
//! ask it to migrate or persist product operations; they do not assemble SQL
//! from HTTP handlers or worker code.

use std::path::Path;

use rusqlite::Connection;

mod ingest;
mod schema;

pub use ingest::{
    ErrorIngest, ErrorIngestCommit, ErrorIngestIds, ErrorIngestPayload, ErrorServiceEvent,
};

/// Result type returned by the store boundary.
pub type Result<T> = std::result::Result<T, StoreError>;

/// Persistence-layer failure.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite rejected a connection, pragma, migration, or query.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Canary's single writable SQLite connection.
///
/// The Phoenix service deliberately runs `Canary.Repo` with `pool_size: 1`.
/// The Rust rewrite keeps the same operational invariant by making writes go
/// through this owned connection instead of exposing a generic connection pool.
pub struct Store {
    connection: Connection,
}

impl Store {
    /// Open a writable SQLite database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    /// Open a writable in-memory SQLite database.
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    /// Apply Canary's ordered schema migrations.
    pub fn migrate(&mut self) -> Result<()> {
        schema::migrate(&mut self.connection)?;
        Ok(())
    }

    /// Return the Rust schema version stored in `PRAGMA user_version`.
    pub fn schema_version(&self) -> Result<u32> {
        let version = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))?;
        Ok(version)
    }

    /// Commit one validated error ingest transaction.
    pub fn commit_error_ingest(&mut self, ingest: ErrorIngest) -> Result<ErrorIngestCommit> {
        ingest::commit(&mut self.connection, ingest)
    }

    /// Count persisted errors.
    pub fn error_count(&self) -> Result<u64> {
        let count = self
            .connection
            .query_row("SELECT count(*) FROM errors", [], |row| {
                row.get::<_, u64>(0)
            })?;
        Ok(count)
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Self { connection })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::str::FromStr;

    use canary_core::{
        ids::{ErrorId, EventId},
        ingest::classification::{Category, Classification, Component, Persistence},
    };
    use rusqlite::params;
    use serde_json::Value;

    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct Column {
        name: String,
        data_type: String,
        not_null: bool,
        default_value: Option<String>,
        primary_key_position: i64,
    }

    #[test]
    fn migrate_creates_the_current_phoenix_tables() -> Result<()> {
        let mut store = migrated_store()?;

        assert_eq!(
            table_names(&store.connection)?,
            BTreeSet::from([
                "annotations".to_owned(),
                "api_keys".to_owned(),
                "error_groups".to_owned(),
                "errors".to_owned(),
                "errors_fts".to_owned(),
                "errors_fts_config".to_owned(),
                "errors_fts_data".to_owned(),
                "errors_fts_docsize".to_owned(),
                "errors_fts_idx".to_owned(),
                "incident_signals".to_owned(),
                "incidents".to_owned(),
                "monitor_check_ins".to_owned(),
                "monitor_state".to_owned(),
                "monitors".to_owned(),
                "oban_jobs".to_owned(),
                "seed_runs".to_owned(),
                "service_events".to_owned(),
                "target_checks".to_owned(),
                "target_state".to_owned(),
                "targets".to_owned(),
                "webhook_deliveries".to_owned(),
                "webhooks".to_owned(),
            ])
        );
        assert_eq!(store.schema_version()?, schema::SCHEMA_VERSION);

        store.migrate()?;
        assert_eq!(store.schema_version()?, schema::SCHEMA_VERSION);

        Ok(())
    }

    #[test]
    fn errors_table_matches_required_columns_defaults_and_classification() -> Result<()> {
        let store = migrated_store()?;
        let columns = columns(&store.connection, "errors")?;

        assert_column(
            &columns,
            "id",
            ColumnSpec::new("TEXT").primary_key_position(1),
        );
        assert_column(&columns, "service", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "error_class", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "message", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "message_template", ColumnSpec::new("TEXT"));
        assert_column(&columns, "stack_trace", ColumnSpec::new("TEXT"));
        assert_column(&columns, "context", ColumnSpec::new("TEXT"));
        assert_column(
            &columns,
            "severity",
            ColumnSpec::new("TEXT").default_value("'error'"),
        );
        assert_column(
            &columns,
            "environment",
            ColumnSpec::new("TEXT").default_value("'production'"),
        );
        assert_column(&columns, "group_hash", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "fingerprint", ColumnSpec::new("TEXT"));
        assert_column(&columns, "region", ColumnSpec::new("TEXT"));
        assert_column(&columns, "created_at", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "classification_category", ColumnSpec::new("TEXT"));
        assert_column(
            &columns,
            "classification_persistence",
            ColumnSpec::new("TEXT"),
        );
        assert_column(
            &columns,
            "classification_component",
            ColumnSpec::new("TEXT"),
        );
        assert_indexes(
            &store.connection,
            "errors",
            &[
                "errors_service_created_at_index",
                "errors_group_hash_created_at_index",
            ],
        )?;

        Ok(())
    }

    #[test]
    fn fts_table_and_triggers_track_error_rows() -> Result<()> {
        let store = migrated_store()?;

        store.connection.execute(
            "INSERT INTO errors (
                id, service, error_class, message, stack_trace, group_hash, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "ERR-123456789abc",
                "sploot",
                "RuntimeError",
                "boom in worker",
                "stack line",
                "group-a",
                "2026-05-28T20:00:00Z"
            ],
        )?;

        let count = store.connection.query_row(
            "SELECT count(*) FROM errors_fts WHERE errors_fts MATCH 'worker'",
            [],
            |row| row.get::<_, i64>(0),
        )?;

        assert_eq!(count, 1);
        assert_eq!(
            trigger_names(&store.connection, "errors")?,
            BTreeSet::from([
                "errors_fts_delete".to_owned(),
                "errors_fts_insert".to_owned(),
                "errors_fts_update".to_owned(),
            ])
        );

        Ok(())
    }

    #[test]
    fn incidents_keep_one_open_incident_per_service() -> Result<()> {
        let store = migrated_store()?;

        store.connection.execute(
            "INSERT INTO incidents (id, service, state, severity, opened_at)
             VALUES ('INC-123456789abc', 'sploot', 'investigating', 'medium', '2026-05-28T20:00:00Z')",
            [],
        )?;
        let duplicate = store.connection.execute(
            "INSERT INTO incidents (id, service, state, severity, opened_at)
             VALUES ('INC-abcdefghijkl', 'sploot', 'investigating', 'medium', '2026-05-28T20:01:00Z')",
            [],
        );

        assert!(duplicate.is_err());

        Ok(())
    }

    #[test]
    fn webhook_delivery_ledger_preserves_primary_key_and_indexes() -> Result<()> {
        let store = migrated_store()?;
        let columns = columns(&store.connection, "webhook_deliveries")?;

        assert_column(
            &columns,
            "delivery_id",
            ColumnSpec::new("TEXT").primary_key_position(1),
        );
        assert_column(&columns, "webhook_id", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "event", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "status", ColumnSpec::new("TEXT").not_null());
        assert_column(
            &columns,
            "attempt_count",
            ColumnSpec::new("INTEGER").not_null().default_value("0"),
        );
        assert_column(&columns, "created_at", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "updated_at", ColumnSpec::new("TEXT").not_null());
        assert_indexes(
            &store.connection,
            "webhook_deliveries",
            &[
                "webhook_deliveries_created_at_delivery_id_index",
                "webhook_deliveries_webhook_id_created_at_delivery_id_index",
                "webhook_deliveries_event_created_at_delivery_id_index",
                "webhook_deliveries_status_created_at_delivery_id_index",
            ],
        )?;

        Ok(())
    }

    #[test]
    fn monitor_tables_preserve_cascade_and_defaults() -> Result<()> {
        let store = migrated_store()?;
        let monitor_state = columns(&store.connection, "monitor_state")?;

        assert_column(
            &monitor_state,
            "monitor_id",
            ColumnSpec::new("TEXT").primary_key_position(1),
        );
        assert_column(
            &monitor_state,
            "state",
            ColumnSpec::new("TEXT")
                .not_null()
                .default_value("'unknown'"),
        );
        assert_column(
            &monitor_state,
            "sequence",
            ColumnSpec::new("INTEGER").not_null().default_value("0"),
        );
        assert_eq!(
            foreign_keys(&store.connection, "monitor_state")?,
            vec![ForeignKey {
                table: "monitors".to_owned(),
                from: "monitor_id".to_owned(),
                to: "id".to_owned(),
                on_delete: "CASCADE".to_owned(),
            }]
        );

        Ok(())
    }

    #[test]
    fn commit_error_ingest_creates_error_group_and_timeline_event() -> Result<()> {
        let mut store = migrated_store()?;
        let ingest = error_ingest(
            "ERR-123456789abc",
            "EVT-123456789abc",
            "group-new",
            "2026-05-28T20:00:00Z",
        );

        let commit = store.commit_error_ingest(ingest)?;

        assert_eq!(commit.id, "ERR-123456789abc");
        assert_eq!(commit.group_hash, "group-new");
        assert!(commit.is_new_class);
        assert_eq!(
            commit
                .service_event
                .as_ref()
                .map(|event| event.event.as_str()),
            Some("error.new_class")
        );

        assert_eq!(row_count(&store.connection, "errors")?, 1);
        let group_count = store.connection.query_row(
            "SELECT total_count FROM error_groups WHERE group_hash = 'group-new'",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        assert_eq!(group_count, 1);

        let event_payload = store.connection.query_row(
            "SELECT event, entity_type, entity_ref, payload
             FROM service_events
             WHERE entity_ref = 'group-new'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )?;
        assert_eq!(event_payload.0, "error.new_class");
        assert_eq!(event_payload.1, "error_group");
        assert_eq!(event_payload.2, "group-new");

        let payload_json: Value = serde_json::from_str(&event_payload.3).unwrap_or(Value::Null);
        assert_eq!(payload_json["event"], "error.new_class");
        assert_eq!(payload_json["error"]["id"], "ERR-123456789abc");
        assert_eq!(payload_json["error"]["service"], "cadence");
        assert_eq!(payload_json["error"]["group_hash"], "group-new");

        Ok(())
    }

    #[test]
    fn commit_error_ingest_updates_existing_group_without_new_class_event() -> Result<()> {
        let mut store = migrated_store()?;
        store.commit_error_ingest(error_ingest(
            "ERR-123456789abc",
            "EVT-123456789abc",
            "group-dup",
            "2026-05-28T20:00:00Z",
        ))?;

        let commit = store.commit_error_ingest(error_ingest(
            "ERR-abcdefghijkl",
            "EVT-abcdefghijkl",
            "group-dup",
            "2026-05-28T20:05:00Z",
        ))?;

        assert!(!commit.is_new_class);
        assert!(commit.service_event.is_none());
        let group = store.connection.query_row(
            "SELECT total_count, last_error_id, status
             FROM error_groups
             WHERE group_hash = 'group-dup'",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        assert_eq!(
            group,
            (2, "ERR-abcdefghijkl".to_owned(), "active".to_owned())
        );
        assert_eq!(
            service_event_count(&store.connection, "group-dup", "error.new_class")?,
            1
        );

        Ok(())
    }

    #[test]
    fn commit_error_ingest_records_regression_after_twenty_four_hours() -> Result<()> {
        let mut store = migrated_store()?;
        store.commit_error_ingest(error_ingest(
            "ERR-123456789abc",
            "EVT-123456789abc",
            "group-regression",
            "2026-05-27T20:00:00Z",
        ))?;

        let commit = store.commit_error_ingest(error_ingest(
            "ERR-abcdefghijkl",
            "EVT-abcdefghijkl",
            "group-regression",
            "2026-05-28T20:00:00Z",
        ))?;

        assert!(!commit.is_new_class);
        assert_eq!(
            commit
                .service_event
                .as_ref()
                .map(|event| event.event.as_str()),
            Some("error.regression")
        );
        assert_eq!(
            service_event_count(&store.connection, "group-regression", "error.regression")?,
            1
        );

        Ok(())
    }

    fn migrated_store() -> Result<Store> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        Ok(store)
    }

    fn error_ingest(
        error_id: &str,
        event_id: &str,
        group_hash: &str,
        created_at: &str,
    ) -> ErrorIngest {
        ErrorIngest {
            ids: ErrorIngestIds {
                error_id: ErrorId::from_str(error_id).unwrap_or_else(|_| ErrorId::generate()),
                event_id: EventId::from_str(event_id).unwrap_or_else(|_| EventId::generate()),
            },
            payload: ErrorIngestPayload {
                service: "cadence".to_owned(),
                error_class: "DBConnection.ConnectionError".to_owned(),
                message: "pool timed out".to_owned(),
                message_template: "pool timed out".to_owned(),
                stack_trace: Some("stack line".to_owned()),
                context_json: Some(r#"{"tenant":"alpha"}"#.to_owned()),
                severity: "warning".to_owned(),
                environment: "production".to_owned(),
                group_hash: group_hash.to_owned(),
                fingerprint_json: Some(r#"["route","handler"]"#.to_owned()),
                region: Some("iad".to_owned()),
                classification: Classification {
                    category: Category::Infrastructure,
                    persistence: Persistence::Transient,
                    component: Component::Database,
                },
                created_at: created_at.to_owned(),
            },
        }
    }

    fn row_count(connection: &Connection, table: &str) -> Result<i64> {
        let count = connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })?;
        Ok(count)
    }

    fn service_event_count(connection: &Connection, group_hash: &str, event: &str) -> Result<i64> {
        let count = connection.query_row(
            "SELECT count(*) FROM service_events WHERE entity_ref = ?1 AND event = ?2",
            params![group_hash, event],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count)
    }

    fn table_names(connection: &Connection) -> Result<BTreeSet<String>> {
        let mut stmt = connection.prepare(
            "SELECT name
             FROM sqlite_schema
             WHERE type = 'table'
               AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        Ok(names)
    }

    fn columns(connection: &Connection, table: &str) -> Result<BTreeMap<String, Column>> {
        let mut stmt = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = stmt
            .query_map([], |row| {
                Ok(Column {
                    name: row.get(1)?,
                    data_type: row.get(2)?,
                    not_null: row.get::<_, i64>(3)? == 1,
                    default_value: row.get(4)?,
                    primary_key_position: row.get(5)?,
                })
            })?
            .map(|column| column.map(|column| (column.name.clone(), column)))
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
        Ok(columns)
    }

    fn assert_column(columns: &BTreeMap<String, Column>, name: &str, spec: ColumnSpec<'_>) {
        let column = columns.get(name);
        assert!(column.is_some(), "missing column {name}");

        if let Some(column) = column {
            assert_eq!(column.data_type, spec.data_type);
            assert_eq!(column.not_null, spec.not_null);
            assert_eq!(column.default_value.as_deref(), spec.default_value);
            assert_eq!(column.primary_key_position, spec.primary_key_position);
        }
    }

    #[derive(Clone, Copy)]
    struct ColumnSpec<'a> {
        data_type: &'a str,
        not_null: bool,
        default_value: Option<&'a str>,
        primary_key_position: i64,
    }

    impl<'a> ColumnSpec<'a> {
        const fn new(data_type: &'a str) -> Self {
            Self {
                data_type,
                not_null: false,
                default_value: None,
                primary_key_position: 0,
            }
        }

        const fn not_null(mut self) -> Self {
            self.not_null = true;
            self
        }

        const fn default_value(mut self, value: &'a str) -> Self {
            self.default_value = Some(value);
            self
        }

        const fn primary_key_position(mut self, value: i64) -> Self {
            self.primary_key_position = value;
            self
        }
    }

    fn assert_indexes(connection: &Connection, table: &str, expected: &[&str]) -> Result<()> {
        let actual = index_names(connection, table)?;
        for name in expected {
            assert!(actual.contains(*name), "missing index {name}");
        }
        Ok(())
    }

    fn index_names(connection: &Connection, table: &str) -> Result<BTreeSet<String>> {
        let mut stmt = connection.prepare(&format!("PRAGMA index_list({table})"))?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        Ok(names)
    }

    fn trigger_names(connection: &Connection, table: &str) -> Result<BTreeSet<String>> {
        let mut stmt = connection.prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'trigger' AND tbl_name = ?1
             ORDER BY name",
        )?;
        let names = stmt
            .query_map([table], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        Ok(names)
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ForeignKey {
        table: String,
        from: String,
        to: String,
        on_delete: String,
    }

    fn foreign_keys(connection: &Connection, table: &str) -> Result<Vec<ForeignKey>> {
        let mut stmt = connection.prepare(&format!("PRAGMA foreign_key_list({table})"))?;
        let keys = stmt
            .query_map([], |row| {
                Ok(ForeignKey {
                    table: row.get(2)?,
                    from: row.get(3)?,
                    to: row.get(4)?,
                    on_delete: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(keys)
    }
}
