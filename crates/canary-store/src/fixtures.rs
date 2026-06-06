//! Rust-owned SQLite fixtures for migration parity tests.
//!
//! These helpers are operator/test utilities, not runtime behavior. They keep
//! fixture generation behind the store crate so shell scripts do not learn
//! schema details and Phoenix is no longer the only way to materialize the
//! compatibility databases.

use std::{fs, path::Path};

use rusqlite::{Connection, params};

use crate::schema;

/// Create an empty Rust-migrated schema fixture at `path`.
pub fn write_schema_fixture(path: impl AsRef<Path>) -> rusqlite::Result<()> {
    with_fresh_database(path.as_ref(), schema::migrate)
}

/// Create the populated read-model fixture at `path`.
pub fn write_read_model_fixture(path: impl AsRef<Path>) -> rusqlite::Result<()> {
    with_fresh_database(path.as_ref(), |connection| {
        schema::migrate(connection)?;
        insert_read_model_rows(connection)
    })
}

fn with_fresh_database(
    path: &Path,
    write: impl FnOnce(&mut Connection) -> rusqlite::Result<()>,
) -> rusqlite::Result<()> {
    remove_existing_database(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(to_sqlite_error)?;
    }

    let mut connection = Connection::open(path)?;
    write(&mut connection)?;
    validate_database(&connection)?;
    checkpoint(&connection)?;
    Ok(())
}

fn remove_existing_database(path: &Path) -> rusqlite::Result<()> {
    for candidate in [
        path.to_path_buf(),
        path.with_extension(format!(
            "{}-shm",
            path.extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
        )),
        path.with_extension(format!(
            "{}-wal",
            path.extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
        )),
    ] {
        match fs::remove_file(&candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(to_sqlite_error(error)),
        }
    }
    Ok(())
}

fn checkpoint(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "PRAGMA wal_checkpoint(TRUNCATE);
         PRAGMA journal_mode = DELETE;
         VACUUM;",
    )
}

fn validate_database(connection: &Connection) -> rusqlite::Result<()> {
    let integrity =
        connection.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?;
    if integrity != "ok" {
        return Err(message_error(format!(
            "fixture integrity_check failed: {integrity}"
        )));
    }

    let foreign_key_error = connection.query_row("PRAGMA foreign_key_check", [], |_row| Ok(()));
    match foreign_key_error {
        Ok(()) => Err(message_error(
            "fixture foreign_key_check returned violations".to_owned(),
        )),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(()),
        Err(error) => Err(error),
    }
}

fn insert_read_model_rows(connection: &Connection) -> rusqlite::Result<()> {
    let now = "2026-05-28T20:00:00Z";
    let older = "2026-05-28T19:59:00Z";
    let newer = "2026-05-28T20:01:00Z";

    connection.execute(
        "INSERT INTO errors (
            id, service, error_class, message, message_template, stack_trace,
            context, severity, environment, group_hash, fingerprint, region,
            created_at, classification_category, classification_persistence,
            classification_component
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            "ERR-readmodel0001",
            "ramp-api",
            "RuntimeError",
            "agent handoff failed",
            "agent handoff failed",
            "lib/ramp.ex:42: Ramp.run/1",
            r#"{"tenant":"alpha","run_id":"run-123"}"#,
            "error",
            "production",
            "grp-readmodel-runtime",
            r#"["ramp","handoff"]"#,
            "iad",
            now,
            "application",
            "persistent",
            "runtime"
        ],
    )?;

    connection.execute(
        "INSERT INTO error_groups (
            group_hash, service, error_class, message_template, severity,
            first_seen_at, last_seen_at, total_count, last_error_id, status
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            "grp-readmodel-runtime",
            "ramp-api",
            "RuntimeError",
            "agent handoff failed",
            "error",
            older,
            now,
            3_i64,
            "ERR-readmodel0001",
            "active"
        ],
    )?;

    connection.execute(
        "INSERT INTO targets (
            id, name, service, url, method, interval_ms, timeout_ms,
            expected_status, degraded_after, down_after, up_after, active,
            created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            "TGT-readmodel-api",
            "Ramp API",
            "ramp-api",
            "https://ramp.example.com/healthz",
            "GET",
            60_000_i64,
            10_000_i64,
            "200",
            1_i64,
            3_i64,
            1_i64,
            1_i64,
            older
        ],
    )?;

    connection.execute(
        "INSERT INTO target_state (
            target_id, state, consecutive_failures, consecutive_successes,
            last_checked_at, last_failure_at, last_transition_at, sequence
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            "TGT-readmodel-api",
            "down",
            4_i64,
            0_i64,
            now,
            now,
            now,
            7_i64
        ],
    )?;

    connection.execute(
        "INSERT INTO monitors (
            id, name, service, mode, expected_every_ms, grace_ms, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            "MON-readmodel-cron",
            "Ramp nightly import",
            "ramp-api",
            "ttl",
            60_000_i64,
            5_000_i64,
            older
        ],
    )?;

    connection.execute(
        "INSERT INTO monitor_state (
            monitor_id, state, last_check_in_status, last_check_in_at,
            last_success_at, deadline_at, first_missed_at, last_transition_at,
            sequence
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            "MON-readmodel-cron",
            "degraded",
            "alive",
            older,
            older,
            now,
            now,
            now,
            4_i64
        ],
    )?;

    connection.execute(
        "INSERT INTO incidents (
            id, service, state, severity, title, opened_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            "INC-readmodel0001",
            "ramp-api",
            "investigating",
            "medium",
            "ramp-api needs agent attention",
            older
        ],
    )?;

    for (signal_type, signal_ref, attached_at) in [
        ("error_group", "grp-readmodel-runtime", older),
        ("health_transition", "TGT-readmodel-api", now),
        ("health_transition", "MON-readmodel-cron", newer),
    ] {
        connection.execute(
            "INSERT INTO incident_signals (
                incident_id, signal_type, signal_ref, attached_at
             ) VALUES (?1, ?2, ?3, ?4)",
            params!["INC-readmodel0001", signal_type, signal_ref, attached_at],
        )?;
    }

    for (
        id,
        subject_type,
        subject_id,
        incident_id,
        group_hash,
        agent,
        action,
        metadata,
        created_at,
    ) in [
        (
            "ANN-readmodel-incident",
            "incident",
            "INC-readmodel0001",
            Some("INC-readmodel0001"),
            None,
            "bb-sprite",
            "acknowledged",
            r#"{"note":"owner paged"}"#,
            newer,
        ),
        (
            "ANN-readmodel-group",
            "error_group",
            "grp-readmodel-runtime",
            None,
            Some("grp-readmodel-runtime"),
            "triage-agent",
            "triaged",
            r#"{"pr":"https://example.com/pr/1"}"#,
            now,
        ),
        (
            "ANN-readmodel-target",
            "target",
            "TGT-readmodel-api",
            None,
            None,
            "ops-agent",
            "investigating",
            r#"{"runbook":"https://example.com/runbook"}"#,
            now,
        ),
    ] {
        connection.execute(
            "INSERT INTO annotations (
                id, subject_type, subject_id, incident_id, group_hash, agent,
                action, metadata, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                subject_type,
                subject_id,
                incident_id,
                group_hash,
                agent,
                action,
                metadata,
                created_at
            ],
        )?;
    }

    connection.execute(
        "INSERT INTO service_events (
            id, service, event, entity_type, entity_ref, severity, summary,
            payload, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            "EVT-readmodel-incident",
            "ramp-api",
            "incident.opened",
            "incident",
            "INC-readmodel0001",
            "medium",
            "ramp-api: incident opened",
            r#"{"event":"incident.opened","incident":{"id":"INC-readmodel0001"}}"#,
            now
        ],
    )?;

    Ok(())
}

fn to_sqlite_error(error: std::io::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

fn message_error(message: String) -> rusqlite::Error {
    to_sqlite_error(std::io::Error::other(message))
}
