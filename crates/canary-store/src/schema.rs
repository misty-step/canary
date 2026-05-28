//! Ordered SQLite schema migrations ported from the Phoenix Ecto migrations.

use rusqlite::Connection;

/// Current Rust schema version.
pub const SCHEMA_VERSION: u32 = 2026042200;

pub(crate) fn migrate(connection: &mut Connection) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(SCHEMA_SQL)?;
    transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    transaction.commit()?;
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
  scope TEXT NOT NULL DEFAULT 'admin'
);

CREATE TABLE IF NOT EXISTS errors (
  id TEXT PRIMARY KEY,
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
  group_hash TEXT PRIMARY KEY,
  service TEXT NOT NULL,
  error_class TEXT NOT NULL,
  message_template TEXT,
  severity TEXT NOT NULL,
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  total_count INTEGER NOT NULL DEFAULT 1,
  last_error_id TEXT NOT NULL,
  status TEXT DEFAULT 'active'
);

CREATE INDEX IF NOT EXISTS error_groups_service_last_seen_at_index
ON error_groups(service, last_seen_at);

CREATE TABLE IF NOT EXISTS targets (
  id TEXT PRIMARY KEY,
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

CREATE INDEX IF NOT EXISTS oban_jobs_state_queue_index
ON oban_jobs(state, queue, priority, scheduled_at, id);

CREATE TABLE IF NOT EXISTS incidents (
  id TEXT PRIMARY KEY,
  service TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'investigating',
  severity TEXT NOT NULL DEFAULT 'medium',
  title TEXT,
  opened_at TEXT NOT NULL,
  resolved_at TEXT
);

CREATE INDEX IF NOT EXISTS incidents_service_state_index
ON incidents(service, state);

CREATE INDEX IF NOT EXISTS incidents_opened_at_index
ON incidents(opened_at);

CREATE UNIQUE INDEX IF NOT EXISTS incidents_open_service_unique_index
ON incidents(service)
WHERE state != 'resolved';

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
  service TEXT NOT NULL,
  event TEXT NOT NULL,
  entity_type TEXT NOT NULL,
  entity_ref TEXT,
  severity TEXT,
  summary TEXT NOT NULL,
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

CREATE TABLE IF NOT EXISTS webhook_deliveries (
  delivery_id TEXT PRIMARY KEY,
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
  name TEXT NOT NULL,
  service TEXT NOT NULL,
  mode TEXT NOT NULL,
  expected_every_ms INTEGER NOT NULL,
  grace_ms INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS monitors_name_index
ON monitors(name);

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
