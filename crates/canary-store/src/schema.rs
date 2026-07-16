//! Ordered SQLite schema migrations.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::Connection;

/// Current Rust schema version.
pub const SCHEMA_VERSION: u32 = 2026071400;

pub(crate) fn migrate(connection: &mut Connection) -> rusqlite::Result<()> {
    let user_version =
        connection.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))?;
    if user_version == SCHEMA_VERSION {
        // Current databases must stay on the metadata-only path. In
        // particular, do not replay data backfills on every boot.
        validate_schema_columns(connection)?;
        return Ok(());
    }

    let transaction = connection.transaction()?;
    transaction.execute_batch(SCHEMA_SQL)?;
    add_bootstrap_ownership_columns(&transaction)?;
    add_api_key_unbound_grant_column(&transaction)?;
    scope_error_group_identity(&transaction)?;
    backfill_service_scope_columns(&transaction)?;
    add_service_event_telemetry_columns(&transaction)?;
    add_incident_escalation_columns(&transaction)?;
    scope_monitor_name_index(&transaction)?;
    scope_incident_open_service_index(&transaction)?;
    scope_oban_jobs_claim_index(&transaction)?;
    validate_schema_columns(&transaction)?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
    Ok(())
}

const APP_TABLES: &[&str] = &[
    "api_keys",
    "errors",
    "error_groups",
    "targets",
    "target_checks",
    "target_state",
    "webhooks",
    "seed_runs",
    "oban_jobs",
    "incidents",
    "incident_signals",
    "service_events",
    "annotations",
    "remediation_claims",
    "webhook_deliveries",
    "monitors",
    "monitor_state",
    "monitor_check_ins",
    "rate_limit_buckets",
];

fn validate_schema_columns(connection: &Connection) -> rusqlite::Result<()> {
    let reference = Connection::open_in_memory()?;
    reference.execute_batch(SCHEMA_SQL)?;
    scope_monitor_name_index(&reference)?;
    scope_incident_open_service_index(&reference)?;
    for table in APP_TABLES {
        let expected_columns = table_columns(&reference, table)?;
        let actual_columns = table_columns(connection, table)?;
        for (column_name, expected) in expected_columns {
            let Some(actual) = actual_columns.get(&column_name) else {
                return Err(rusqlite::Error::InvalidColumnName(format!(
                    "{table}.{column_name}"
                )));
            };
            if actual != &expected {
                return Err(rusqlite::Error::InvalidColumnName(format!(
                    "{table}.{column_name}"
                )));
            }
        }

        let expected_indexes = table_indexes(&reference, table)?;
        let actual_indexes = table_indexes(connection, table)?;
        for (index_name, expected) in expected_indexes {
            let Some(actual) = actual_indexes.get(&index_name) else {
                return Err(rusqlite::Error::InvalidColumnName(format!(
                    "{table}.{index_name}"
                )));
            };
            if actual != &expected {
                return Err(rusqlite::Error::InvalidColumnName(format!(
                    "{table}.{index_name}"
                )));
            }
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct ColumnInfo {
    data_type: String,
    not_null: bool,
    default_value: Option<String>,
    primary_key_position: i64,
}

fn table_columns(
    connection: &Connection,
    table: &str,
) -> rusqlite::Result<BTreeMap<String, ColumnInfo>> {
    let escaped_table = table.replace('"', "\"\"");
    let mut statement = connection.prepare(&format!("PRAGMA table_info(\"{escaped_table}\")"))?;
    statement
        .query_map([], |row| {
            let name = row.get::<_, String>(1)?;
            Ok((
                name,
                ColumnInfo {
                    data_type: row.get(2)?,
                    not_null: row.get::<_, i64>(3)? == 1,
                    default_value: row.get(4)?,
                    primary_key_position: row.get(5)?,
                },
            ))
        })?
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct IndexInfo {
    unique: bool,
    columns: Vec<String>,
}

fn table_indexes(
    connection: &Connection,
    table: &str,
) -> rusqlite::Result<BTreeMap<String, IndexInfo>> {
    let escaped_table = table.replace('"', "\"\"");
    let mut statement = connection.prepare(&format!("PRAGMA index_list(\"{escaped_table}\")"))?;
    let index_rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)? == 1))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut indexes = BTreeMap::new();
    for (index_name, unique) in index_rows {
        if index_name.starts_with("sqlite_autoindex_") {
            continue;
        }
        indexes.insert(
            index_name.clone(),
            IndexInfo {
                unique,
                columns: index_columns(connection, &index_name)?,
            },
        );
    }
    Ok(indexes)
}

fn index_columns(connection: &Connection, index_name: &str) -> rusqlite::Result<Vec<String>> {
    let escaped_index = index_name.replace('"', "\"\"");
    let mut statement = connection.prepare(&format!("PRAGMA index_info(\"{escaped_index}\")"))?;
    statement
        .query_map([], |row| row.get::<_, String>(2))?
        .collect()
}

fn table_column_names(connection: &Connection, table: &str) -> rusqlite::Result<BTreeSet<String>> {
    Ok(table_columns(connection, table)?.into_keys().collect())
}

fn add_bootstrap_ownership_columns(connection: &Connection) -> rusqlite::Result<()> {
    for table in [
        "api_keys",
        "errors",
        "error_groups",
        "targets",
        "webhooks",
        "incidents",
        "service_events",
        "annotations",
        "webhook_deliveries",
        "monitors",
    ] {
        add_text_column_if_missing(
            connection,
            table,
            "tenant_id",
            "TEXT NOT NULL DEFAULT 'TENANT-bootstrap'",
        )?;
        add_text_column_if_missing(
            connection,
            table,
            "project_id",
            "TEXT NOT NULL DEFAULT 'PROJECT-bootstrap'",
        )?;
    }

    for table in ["api_keys", "webhooks", "annotations", "webhook_deliveries"] {
        add_text_column_if_missing(connection, table, "service", "TEXT")?;
    }

    Ok(())
}

fn add_api_key_unbound_grant_column(connection: &Connection) -> rusqlite::Result<()> {
    if table_column_names(connection, "api_keys")?.contains("allow_unbound") {
        return Ok(());
    }
    connection.execute(
        "ALTER TABLE api_keys ADD COLUMN allow_unbound INTEGER NOT NULL DEFAULT 0",
        [],
    )?;
    Ok(())
}

fn scope_monitor_name_index(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute("DROP INDEX IF EXISTS monitors_name_index", [])?;
    connection.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS monitors_owner_name_index
         ON monitors(tenant_id, project_id, name)",
        [],
    )?;
    Ok(())
}

/// Lead the oban_jobs claim index with `worker` so `claim_due_webhook_delivery_jobs`
/// (which always filters `worker = ?1` first) does not scan every row matching
/// state alone. Existing installs carry the old `state`-led index; drop it so it
/// does not silently shadow the new one.
fn scope_oban_jobs_claim_index(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute("DROP INDEX IF EXISTS oban_jobs_state_queue_index", [])?;
    connection.execute(
        "CREATE INDEX IF NOT EXISTS oban_jobs_worker_state_queue_index
         ON oban_jobs(worker, state, queue, priority, scheduled_at, id)",
        [],
    )?;
    Ok(())
}

fn backfill_service_scope_columns(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        r#"
        UPDATE annotations
        SET service = (
          CASE subject_type
            WHEN 'incident' THEN (
              SELECT i.service
              FROM incidents i
              WHERE i.id = annotations.subject_id
                AND i.tenant_id = annotations.tenant_id
                AND i.project_id = annotations.project_id
            )
            WHEN 'error_group' THEN (
              SELECT g.service
              FROM error_groups g
              WHERE g.group_hash = annotations.subject_id
                AND g.tenant_id = annotations.tenant_id
                AND g.project_id = annotations.project_id
            )
            WHEN 'target' THEN (
              SELECT COALESCE(NULLIF(t.service, ''), t.name)
              FROM targets t
              WHERE t.id = annotations.subject_id
                AND t.tenant_id = annotations.tenant_id
                AND t.project_id = annotations.project_id
            )
            WHEN 'monitor' THEN (
              SELECT COALESCE(NULLIF(m.service, ''), m.name)
              FROM monitors m
              WHERE m.id = annotations.subject_id
                AND m.tenant_id = annotations.tenant_id
                AND m.project_id = annotations.project_id
            )
          END
        )
        WHERE service IS NULL;

        UPDATE webhook_deliveries
        SET service = (
          SELECT w.service
          FROM webhooks w
          WHERE w.id = webhook_deliveries.webhook_id
            AND w.tenant_id = webhook_deliveries.tenant_id
            AND w.project_id = webhook_deliveries.project_id
            AND w.service IS NOT NULL
          LIMIT 1
        )
        WHERE service IS NULL
          AND EXISTS (
            SELECT 1
            FROM webhooks w
            WHERE w.id = webhook_deliveries.webhook_id
              AND w.tenant_id = webhook_deliveries.tenant_id
              AND w.project_id = webhook_deliveries.project_id
              AND w.service IS NOT NULL
          );

        UPDATE webhook_deliveries
        SET service = (
          SELECT COALESCE(
            json_extract(j.args, '$.payload.error.service'),
            json_extract(j.args, '$.payload.incident.service'),
            json_extract(j.args, '$.payload.target.service'),
            json_extract(j.args, '$.payload.monitor.service'),
            json_extract(j.args, '$.payload.annotation.service'),
            json_extract(j.args, '$.payload.service')
          )
          FROM oban_jobs j
          WHERE json_extract(j.args, '$.delivery_id') = webhook_deliveries.delivery_id
            AND COALESCE(
              json_extract(j.args, '$.payload.error.service'),
              json_extract(j.args, '$.payload.incident.service'),
              json_extract(j.args, '$.payload.target.service'),
              json_extract(j.args, '$.payload.monitor.service'),
              json_extract(j.args, '$.payload.annotation.service'),
              json_extract(j.args, '$.payload.service')
            ) IS NOT NULL
          ORDER BY j.id DESC
          LIMIT 1
        )
        WHERE service IS NULL
          AND EXISTS (
            SELECT 1
            FROM oban_jobs j
            WHERE json_extract(j.args, '$.delivery_id') = webhook_deliveries.delivery_id
              AND COALESCE(
                json_extract(j.args, '$.payload.error.service'),
                json_extract(j.args, '$.payload.incident.service'),
                json_extract(j.args, '$.payload.target.service'),
                json_extract(j.args, '$.payload.monitor.service'),
                json_extract(j.args, '$.payload.annotation.service'),
                json_extract(j.args, '$.payload.service')
              ) IS NOT NULL
          );
        "#,
    )
}

fn scope_error_group_identity(connection: &Connection) -> rusqlite::Result<()> {
    if error_group_identity_is_scoped(connection)? {
        return Ok(());
    }

    connection.execute_batch(
        r#"
        CREATE TABLE error_groups_scoped (
          group_hash TEXT NOT NULL,
          tenant_id TEXT NOT NULL DEFAULT 'TENANT-bootstrap',
          project_id TEXT NOT NULL DEFAULT 'PROJECT-bootstrap',
          service TEXT NOT NULL,
          error_class TEXT NOT NULL,
          message_template TEXT,
          severity TEXT NOT NULL,
          first_seen_at TEXT NOT NULL,
          last_seen_at TEXT NOT NULL,
          total_count INTEGER NOT NULL DEFAULT 1,
          last_error_id TEXT NOT NULL,
          status TEXT DEFAULT 'active',
          PRIMARY KEY (tenant_id, project_id, group_hash)
        );

        INSERT OR IGNORE INTO error_groups_scoped (
          group_hash, tenant_id, project_id, service, error_class, message_template, severity,
          first_seen_at, last_seen_at, total_count, last_error_id, status
        )
        SELECT
          group_hash, tenant_id, project_id, service, error_class, message_template, severity,
          first_seen_at, last_seen_at, total_count, last_error_id, status
        FROM error_groups;

        DROP TABLE error_groups;
        ALTER TABLE error_groups_scoped RENAME TO error_groups;

        CREATE INDEX IF NOT EXISTS error_groups_service_last_seen_at_index
        ON error_groups(service, last_seen_at);
        "#,
    )?;
    Ok(())
}

fn add_service_event_telemetry_columns(connection: &Connection) -> rusqlite::Result<()> {
    add_text_column_if_missing(
        connection,
        "service_events",
        "signal_kind",
        "TEXT NOT NULL DEFAULT 'operational' CHECK (signal_kind IN ('operational', 'analytics_event'))",
    )?;
    add_text_column_if_missing(connection, "service_events", "signal_name", "TEXT")?;
    add_text_column_if_missing(
        connection,
        "service_events",
        "attributes",
        "TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(attributes) AND json_type(attributes) = 'object')",
    )?;
    add_text_column_if_missing(
        connection,
        "service_events",
        "retention_class",
        "TEXT NOT NULL DEFAULT 'standard' CHECK (retention_class IN ('ephemeral', 'standard', 'audit'))",
    )?;
    add_text_column_if_missing(
        connection,
        "service_events",
        "privacy_policy",
        "TEXT NOT NULL DEFAULT 'system' CHECK (privacy_policy IN ('system', 'redacted', 'public', 'sensitive'))",
    )?;
    add_text_column_if_missing(
        connection,
        "service_events",
        "sampling_policy",
        "TEXT NOT NULL DEFAULT 'unsampled' CHECK (length(trim(sampling_policy)) > 0)",
    )?;
    connection.execute(
        "CREATE INDEX IF NOT EXISTS service_events_signal_kind_created_at_id_index
         ON service_events(signal_kind, created_at, id)",
        [],
    )?;
    Ok(())
}

fn add_incident_escalation_columns(connection: &Connection) -> rusqlite::Result<()> {
    add_text_column_if_missing(connection, "incidents", "escalated_at", "TEXT")?;
    add_text_column_if_missing(connection, "incidents", "escalated_by", "TEXT")?;
    add_text_column_if_missing(connection, "incidents", "escalated_reason", "TEXT")?;
    add_text_column_if_missing(connection, "incidents", "escalated_idempotency_key", "TEXT")?;
    Ok(())
}

fn error_group_identity_is_scoped(connection: &Connection) -> rusqlite::Result<bool> {
    let mut statement = connection.prepare("PRAGMA table_info(\"error_groups\")")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
    })?;
    let mut tenant_pk = 0;
    let mut project_pk = 0;
    let mut group_pk = 0;
    for row in rows {
        let (name, pk) = row?;
        match name.as_str() {
            "tenant_id" => tenant_pk = pk,
            "project_id" => project_pk = pk,
            "group_hash" => group_pk = pk,
            _ => {}
        }
    }
    Ok(tenant_pk > 0 && project_pk > 0 && group_pk > 0)
}

fn scope_incident_open_service_index(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute(
        "DROP INDEX IF EXISTS incidents_open_service_unique_index",
        [],
    )?;
    connection.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS incidents_open_owner_service_unique_index
         ON incidents(tenant_id, project_id, service)
         WHERE state != 'resolved'",
        [],
    )?;
    Ok(())
}

fn add_text_column_if_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> rusqlite::Result<()> {
    if table_column_names(connection, table)?.contains(column) {
        return Ok(());
    }

    let escaped_table = table.replace('"', "\"\"");
    let escaped_column = column.replace('"', "\"\"");
    connection.execute(
        &format!("ALTER TABLE \"{escaped_table}\" ADD COLUMN \"{escaped_column}\" {definition}"),
        [],
    )?;
    Ok(())
}

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS api_keys (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  key_prefix TEXT NOT NULL,
  key_hash TEXT NOT NULL,
  created_at TEXT NOT NULL,
  revoked_at TEXT,
  scope TEXT NOT NULL DEFAULT 'admin',
  tenant_id TEXT NOT NULL DEFAULT 'TENANT-bootstrap',
  project_id TEXT NOT NULL DEFAULT 'PROJECT-bootstrap',
  service TEXT,
  allow_unbound INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS rate_limit_buckets (
  kind TEXT NOT NULL,
  identity TEXT NOT NULL,
  window_start_ms INTEGER NOT NULL,
  window_ms INTEGER NOT NULL,
  count INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  PRIMARY KEY (kind, identity)
);

CREATE TABLE IF NOT EXISTS errors (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL DEFAULT 'TENANT-bootstrap',
  project_id TEXT NOT NULL DEFAULT 'PROJECT-bootstrap',
  service TEXT NOT NULL,
  error_class TEXT NOT NULL,
  message TEXT NOT NULL,
  message_template TEXT,
  stack_trace TEXT,
  context TEXT,
  severity TEXT DEFAULT 'error',
  environment TEXT DEFAULT 'production',
  group_hash TEXT NOT NULL,
  fingerprint TEXT,
  region TEXT,
  created_at TEXT NOT NULL,
  classification_category TEXT,
  classification_persistence TEXT,
  classification_component TEXT
);

CREATE INDEX IF NOT EXISTS errors_service_created_at_index
ON errors(service, created_at);

CREATE INDEX IF NOT EXISTS errors_group_hash_created_at_index
ON errors(group_hash, created_at);

CREATE TABLE IF NOT EXISTS error_groups (
  group_hash TEXT NOT NULL,
  tenant_id TEXT NOT NULL DEFAULT 'TENANT-bootstrap',
  project_id TEXT NOT NULL DEFAULT 'PROJECT-bootstrap',
  service TEXT NOT NULL,
  error_class TEXT NOT NULL,
  message_template TEXT,
  severity TEXT NOT NULL,
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  total_count INTEGER NOT NULL DEFAULT 1,
  last_error_id TEXT NOT NULL,
  status TEXT DEFAULT 'active',
  PRIMARY KEY (tenant_id, project_id, group_hash)
);

CREATE INDEX IF NOT EXISTS error_groups_service_last_seen_at_index
ON error_groups(service, last_seen_at);

CREATE TABLE IF NOT EXISTS targets (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL DEFAULT 'TENANT-bootstrap',
  project_id TEXT NOT NULL DEFAULT 'PROJECT-bootstrap',
  url TEXT NOT NULL,
  name TEXT NOT NULL,
  method TEXT DEFAULT 'GET',
  headers TEXT,
  interval_ms INTEGER DEFAULT 60000,
  timeout_ms INTEGER DEFAULT 10000,
  expected_status TEXT DEFAULT '200',
  body_contains TEXT,
  degraded_after INTEGER DEFAULT 1,
  down_after INTEGER DEFAULT 3,
  up_after INTEGER DEFAULT 1,
  active INTEGER DEFAULT 1,
  created_at TEXT NOT NULL,
  service TEXT
);

CREATE INDEX IF NOT EXISTS targets_service_index
ON targets(service);

CREATE TABLE IF NOT EXISTS target_checks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  target_id TEXT NOT NULL,
  checked_at TEXT NOT NULL,
  status_code INTEGER,
  latency_ms INTEGER,
  result TEXT NOT NULL,
  tls_expires_at TEXT,
  error_detail TEXT,
  region TEXT
);

CREATE INDEX IF NOT EXISTS target_checks_target_id_checked_at_index
ON target_checks(target_id, checked_at);

CREATE TABLE IF NOT EXISTS target_state (
  target_id TEXT PRIMARY KEY,
  state TEXT DEFAULT 'unknown',
  consecutive_failures INTEGER DEFAULT 0,
  consecutive_successes INTEGER DEFAULT 0,
  last_checked_at TEXT,
  last_success_at TEXT,
  last_failure_at TEXT,
  last_transition_at TEXT,
  sequence INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS webhooks (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL DEFAULT 'TENANT-bootstrap',
  project_id TEXT NOT NULL DEFAULT 'PROJECT-bootstrap',
  service TEXT,
  url TEXT NOT NULL,
  events TEXT NOT NULL,
  secret TEXT NOT NULL,
  active INTEGER DEFAULT 1,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS seed_runs (
  seed_name TEXT PRIMARY KEY,
  applied_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS oban_jobs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  state TEXT NOT NULL DEFAULT 'available',
  queue TEXT NOT NULL DEFAULT 'default',
  worker TEXT NOT NULL,
  args TEXT NOT NULL DEFAULT '{}',
  meta TEXT NOT NULL DEFAULT '{}',
  tags TEXT NOT NULL DEFAULT '[]',
  errors TEXT NOT NULL DEFAULT '[]',
  attempt INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL DEFAULT 20,
  priority INTEGER NOT NULL DEFAULT 0,
  inserted_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  scheduled_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  attempted_at TEXT,
  attempted_by TEXT NOT NULL DEFAULT '[]',
  cancelled_at TEXT,
  completed_at TEXT,
  discarded_at TEXT
);

CREATE INDEX IF NOT EXISTS oban_jobs_worker_state_queue_index
ON oban_jobs(worker, state, queue, priority, scheduled_at, id);

CREATE TABLE IF NOT EXISTS incidents (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL DEFAULT 'TENANT-bootstrap',
  project_id TEXT NOT NULL DEFAULT 'PROJECT-bootstrap',
  service TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'investigating',
  severity TEXT NOT NULL DEFAULT 'medium',
  title TEXT,
  opened_at TEXT NOT NULL,
  resolved_at TEXT,
  escalated_at TEXT,
  escalated_by TEXT,
  escalated_reason TEXT,
  escalated_idempotency_key TEXT
);

CREATE INDEX IF NOT EXISTS incidents_service_state_index
ON incidents(service, state);

CREATE INDEX IF NOT EXISTS incidents_opened_at_index
ON incidents(opened_at);

CREATE TABLE IF NOT EXISTS incident_signals (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  incident_id TEXT NOT NULL REFERENCES incidents(id) ON DELETE CASCADE,
  signal_type TEXT NOT NULL,
  signal_ref TEXT NOT NULL,
  attached_at TEXT NOT NULL,
  resolved_at TEXT
);

CREATE INDEX IF NOT EXISTS incident_signals_incident_id_index
ON incident_signals(incident_id);

CREATE UNIQUE INDEX IF NOT EXISTS incident_signals_incident_id_signal_type_signal_ref_index
ON incident_signals(incident_id, signal_type, signal_ref);

CREATE VIRTUAL TABLE IF NOT EXISTS errors_fts USING fts5(
  service,
  error_class,
  message,
  stack_trace,
  content='errors',
  content_rowid='rowid'
);

CREATE TRIGGER IF NOT EXISTS errors_fts_insert
AFTER INSERT ON errors
BEGIN
  INSERT INTO errors_fts(rowid, service, error_class, message, stack_trace)
  VALUES (new.rowid, new.service, new.error_class, new.message, new.stack_trace);
END;

CREATE TRIGGER IF NOT EXISTS errors_fts_delete
AFTER DELETE ON errors
BEGIN
  INSERT INTO errors_fts(errors_fts, rowid, service, error_class, message, stack_trace)
  VALUES ('delete', old.rowid, old.service, old.error_class, old.message, old.stack_trace);
END;

CREATE TRIGGER IF NOT EXISTS errors_fts_update
AFTER UPDATE ON errors
BEGIN
  INSERT INTO errors_fts(errors_fts, rowid, service, error_class, message, stack_trace)
  VALUES ('delete', old.rowid, old.service, old.error_class, old.message, old.stack_trace);

  INSERT INTO errors_fts(rowid, service, error_class, message, stack_trace)
  VALUES (new.rowid, new.service, new.error_class, new.message, new.stack_trace);
END;

CREATE TABLE IF NOT EXISTS service_events (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL DEFAULT 'TENANT-bootstrap',
  project_id TEXT NOT NULL DEFAULT 'PROJECT-bootstrap',
  service TEXT NOT NULL,
  event TEXT NOT NULL,
  signal_kind TEXT NOT NULL DEFAULT 'operational' CHECK (signal_kind IN ('operational', 'analytics_event')),
  signal_name TEXT,
  entity_type TEXT NOT NULL,
  entity_ref TEXT,
  severity TEXT,
  summary TEXT NOT NULL,
  attributes TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(attributes) AND json_type(attributes) = 'object'),
  retention_class TEXT NOT NULL DEFAULT 'standard' CHECK (retention_class IN ('ephemeral', 'standard', 'audit')),
  privacy_policy TEXT NOT NULL DEFAULT 'system' CHECK (privacy_policy IN ('system', 'redacted', 'public', 'sensitive')),
  sampling_policy TEXT NOT NULL DEFAULT 'unsampled' CHECK (length(trim(sampling_policy)) > 0),
  payload TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS service_events_service_created_at_id_index
ON service_events(service, created_at, id);

CREATE INDEX IF NOT EXISTS service_events_created_at_id_index
ON service_events(created_at, id);

CREATE INDEX IF NOT EXISTS service_events_event_created_at_id_index
ON service_events(event, created_at, id);

CREATE TABLE IF NOT EXISTS annotations (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL DEFAULT 'TENANT-bootstrap',
  project_id TEXT NOT NULL DEFAULT 'PROJECT-bootstrap',
  service TEXT,
  incident_id TEXT REFERENCES incidents(id) ON DELETE CASCADE,
  group_hash TEXT,
  agent TEXT NOT NULL,
  action TEXT NOT NULL,
  metadata TEXT,
  created_at TEXT NOT NULL,
  subject_type TEXT,
  subject_id TEXT
);

CREATE INDEX IF NOT EXISTS annotations_incident_id_action_index
ON annotations(incident_id, action);

CREATE INDEX IF NOT EXISTS annotations_group_hash_action_index
ON annotations(group_hash, action);

CREATE INDEX IF NOT EXISTS annotations_action_index
ON annotations(action);

CREATE INDEX IF NOT EXISTS annotations_subject_type_subject_id_created_at_index
ON annotations(subject_type, subject_id, created_at);

CREATE UNIQUE INDEX IF NOT EXISTS annotations_subject_type_subject_id_id_index
ON annotations(subject_type, subject_id, id);

CREATE TABLE IF NOT EXISTS remediation_claims (
  id TEXT PRIMARY KEY CHECK (id LIKE 'CLM-%'),
  tenant_id TEXT NOT NULL DEFAULT 'TENANT-bootstrap',
  project_id TEXT NOT NULL DEFAULT 'PROJECT-bootstrap',
  service TEXT,
  subject_type TEXT NOT NULL CHECK (subject_type IN ('incident', 'error_group', 'target', 'monitor')),
  subject_id TEXT NOT NULL CHECK (length(trim(subject_id)) > 0),
  owner TEXT NOT NULL CHECK (length(trim(owner)) > 0),
  purpose TEXT NOT NULL CHECK (length(trim(purpose)) > 0),
  state TEXT NOT NULL CHECK (state IN ('claimed', 'investigating', 'fix_proposed', 'verified', 'dismissed', 'expired', 'released')),
  idempotency_key TEXT NOT NULL CHECK (length(trim(idempotency_key)) > 0),
  evidence_links TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(evidence_links) AND json_type(evidence_links) = 'array'),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  expires_at TEXT NOT NULL CHECK (strftime('%Y-%m-%dT%H:%M:%SZ', expires_at) IS NOT NULL),
  released_at TEXT,
  completed_at TEXT
);

CREATE TRIGGER IF NOT EXISTS remediation_claims_evidence_links_strings_insert
BEFORE INSERT ON remediation_claims
WHEN EXISTS (SELECT 1 FROM json_each(NEW.evidence_links) WHERE type != 'text')
BEGIN
  SELECT RAISE(ABORT, 'remediation_claims.evidence_links must be an array of strings');
END;

CREATE TRIGGER IF NOT EXISTS remediation_claims_evidence_links_strings_update
BEFORE UPDATE OF evidence_links ON remediation_claims
WHEN EXISTS (SELECT 1 FROM json_each(NEW.evidence_links) WHERE type != 'text')
BEGIN
  SELECT RAISE(ABORT, 'remediation_claims.evidence_links must be an array of strings');
END;

CREATE INDEX IF NOT EXISTS remediation_claims_subject_state_expires_at_index
ON remediation_claims(tenant_id, project_id, subject_type, subject_id, state, expires_at);

CREATE UNIQUE INDEX IF NOT EXISTS remediation_claims_idempotency_index
ON remediation_claims(tenant_id, project_id, subject_type, subject_id, idempotency_key);

CREATE UNIQUE INDEX IF NOT EXISTS remediation_claims_one_active_subject_index
ON remediation_claims(tenant_id, project_id, subject_type, subject_id)
WHERE state IN ('claimed', 'investigating', 'fix_proposed');

CREATE INDEX IF NOT EXISTS remediation_claims_service_updated_at_index
ON remediation_claims(service, updated_at);

-- Serves the fleet-wide active-claims list (ORDER BY updated_at DESC,
-- id DESC with LIMIT) without a temp b-tree sort. SQLite only uses a
-- partial index when the query's WHERE contains this state IN (...)
-- literal byte-for-byte, same term order — keep the list in sync with
-- the active-state literals in claims.rs and query.rs.
CREATE INDEX IF NOT EXISTS remediation_claims_active_updated_at_index
ON remediation_claims(tenant_id, project_id, updated_at DESC, id DESC)
WHERE state IN ('claimed', 'investigating', 'fix_proposed');

CREATE TABLE IF NOT EXISTS webhook_deliveries (
  delivery_id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL DEFAULT 'TENANT-bootstrap',
  project_id TEXT NOT NULL DEFAULT 'PROJECT-bootstrap',
  service TEXT,
  webhook_id TEXT NOT NULL,
  event TEXT NOT NULL,
  status TEXT NOT NULL,
  attempt_count INTEGER NOT NULL DEFAULT 0,
  reason TEXT,
  first_attempt_at TEXT,
  last_attempt_at TEXT,
  delivered_at TEXT,
  discarded_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS webhook_deliveries_created_at_delivery_id_index
ON webhook_deliveries(created_at, delivery_id);

CREATE INDEX IF NOT EXISTS webhook_deliveries_webhook_id_created_at_delivery_id_index
ON webhook_deliveries(webhook_id, created_at, delivery_id);

CREATE INDEX IF NOT EXISTS webhook_deliveries_event_created_at_delivery_id_index
ON webhook_deliveries(event, created_at, delivery_id);

CREATE INDEX IF NOT EXISTS webhook_deliveries_status_created_at_delivery_id_index
ON webhook_deliveries(status, created_at, delivery_id);

CREATE TABLE IF NOT EXISTS monitors (
  id TEXT PRIMARY KEY,
  tenant_id TEXT NOT NULL DEFAULT 'TENANT-bootstrap',
  project_id TEXT NOT NULL DEFAULT 'PROJECT-bootstrap',
  name TEXT NOT NULL,
  service TEXT NOT NULL,
  mode TEXT NOT NULL,
  expected_every_ms INTEGER NOT NULL,
  grace_ms INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS monitors_service_index
ON monitors(service);

CREATE TABLE IF NOT EXISTS monitor_state (
  monitor_id TEXT PRIMARY KEY REFERENCES monitors(id) ON DELETE CASCADE,
  state TEXT NOT NULL DEFAULT 'unknown',
  last_check_in_status TEXT,
  last_check_in_at TEXT,
  last_success_at TEXT,
  last_failure_at TEXT,
  deadline_at TEXT,
  first_missed_at TEXT,
  last_transition_at TEXT,
  sequence INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS monitor_state_deadline_at_index
ON monitor_state(deadline_at);

CREATE TABLE IF NOT EXISTS monitor_check_ins (
  id TEXT PRIMARY KEY,
  monitor_id TEXT NOT NULL REFERENCES monitors(id) ON DELETE CASCADE,
  external_id TEXT,
  status TEXT NOT NULL,
  observed_at TEXT NOT NULL,
  ttl_ms INTEGER,
  summary TEXT,
  context TEXT
);

CREATE INDEX IF NOT EXISTS monitor_check_ins_monitor_id_observed_at_index
ON monitor_check_ins(monitor_id, observed_at);
"#;
