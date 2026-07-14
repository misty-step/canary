//! Compatibility tests for Rust store operations on the frozen legacy SQLite database.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use canary_core::{
    ids::{ErrorId, EventId, IncidentId},
    ingest::classification::{Category, Classification, Component, Persistence},
};
use canary_store::{
    BOOTSTRAP_PROJECT_ID, BOOTSTRAP_TENANT_ID, ErrorIngest, ErrorIngestIds, ErrorIngestPayload,
    IncidentCorrelation, IncidentListOptions, ServiceQueryOptions, Store, WebhookDeliveryInsert,
    WebhookDeliveryListOptions, WebhookDeliveryStatus, fixtures,
};
use rusqlite::{Connection, OpenFlags, params};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const LEGACY_FIXTURE: &str = "tests/fixtures/legacy_schema.db";
const POPULATED_LEGACY_FIXTURE: &str = "tests/fixtures/legacy_read_models.db";
const RUST_SCHEMA_VERSION: u32 = 2026071400;

const LEGACY_MIGRATIONS: &[&str] = &[
    "20260314000001",
    "20260314230000",
    "20260322120000",
    "20260322164500",
    "20260324121000",
    "20260328190000",
    "20260330120000",
    "20260330140000",
    "20260402230500",
    "20260416173500",
    "20260418010000",
    "20260422000000",
];

const RUST_TABLES: &[&str] = &[
    "annotations",
    "api_keys",
    "error_groups",
    "errors",
    "errors_fts",
    "errors_fts_config",
    "errors_fts_data",
    "errors_fts_docsize",
    "errors_fts_idx",
    "incident_signals",
    "incidents",
    "monitor_check_ins",
    "monitor_state",
    "monitors",
    "oban_jobs",
    "seed_runs",
    "service_events",
    "target_checks",
    "target_state",
    "targets",
    "webhook_deliveries",
    "webhooks",
];

const PRODUCT_TABLES: &[&str] = &[
    "annotations",
    "api_keys",
    "error_groups",
    "errors",
    "incident_signals",
    "incidents",
    "monitor_check_ins",
    "monitor_state",
    "monitors",
    "oban_jobs",
    "seed_runs",
    "service_events",
    "target_checks",
    "target_state",
    "targets",
    "webhook_deliveries",
    "webhooks",
];

const FOREIGN_KEY_TABLES: &[&str] = &[
    "annotations",
    "incident_signals",
    "monitor_check_ins",
    "monitor_state",
];

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, PartialEq, Eq)]
struct Column {
    cid: i64,
    name: String,
    data_type: String,
    not_null: bool,
    default_value: Option<String>,
    primary_key_position: i64,
}

#[derive(Debug, PartialEq, Eq)]
struct ColumnShape {
    name: String,
    data_type: String,
    not_null: bool,
    default_value: Option<String>,
    primary_key_position: i64,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct IndexSql {
    table: String,
    name: String,
    sql: String,
}

#[derive(Debug, PartialEq, Eq)]
struct ForeignKey {
    table: String,
    from: String,
    to: String,
    on_delete: String,
}

#[test]
fn legacy_fixture_has_the_tables_rust_expects() -> Result<(), Box<dyn Error>> {
    let fixture = open_fixture_read_only()?;
    let mut names = table_names(&fixture)?;
    assert!(names.remove("schema_migrations"));

    assert_eq!(
        names,
        RUST_TABLES.iter().map(|name| (*name).to_owned()).collect()
    );
    assert_eq!(user_version(&fixture)?, 0);
    assert_eq!(
        migration_versions(&fixture)?,
        LEGACY_MIGRATIONS
            .iter()
            .map(|version| (*version).to_owned())
            .collect::<Vec<_>>()
    );

    Ok(())
}

#[test]
fn legacy_fixture_columns_match_rust_schema() -> Result<(), Box<dyn Error>> {
    let (dir, path) = copy_fixture("columns")?;
    let mut store = Store::open(&path)?;
    store.migrate()?;
    drop(store);
    let fixture = Connection::open(&path)?;
    let (rust_dir, rust) = rust_schema_connection()?;

    for table in PRODUCT_TABLES {
        assert_eq!(
            column_shapes(&fixture, table)?,
            column_shapes(&rust, table)?,
            "column drift in {table}; regenerate Rust fixtures with bin/regenerate-rust-fixtures and audit the frozen legacy fixture"
        );
    }

    fs::remove_dir_all(rust_dir)?;
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn legacy_fixture_indexes_and_foreign_keys_match_rust_schema() -> Result<(), Box<dyn Error>> {
    let (dir, path) = copy_fixture("indexes")?;
    let mut store = Store::open(&path)?;
    store.migrate()?;
    drop(store);
    let fixture = Connection::open(&path)?;
    let (rust_dir, rust) = rust_schema_connection()?;

    assert_eq!(index_sql(&fixture)?, index_sql(&rust)?);
    assert!(index_sql(&fixture)?.iter().any(|index| {
        index.name == "incidents_open_owner_service_unique_index"
            && index.sql.contains("tenant_id, project_id, service")
            && index.sql.contains("WHERE state != 'resolved'")
    }));
    assert!(index_sql(&fixture)?.iter().any(|index| {
        index.name == "monitors_owner_name_index"
            && index.sql.contains("tenant_id, project_id, name")
    }));

    for table in FOREIGN_KEY_TABLES {
        assert_eq!(
            foreign_keys(&fixture, table)?,
            foreign_keys(&rust, table)?,
            "foreign-key drift in {table}"
        );
    }

    fs::remove_dir_all(rust_dir)?;
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn rust_migrate_restamps_a_legacy_fixture_without_schema_drift() -> Result<(), Box<dyn Error>> {
    let (dir, path) = copy_fixture("restamp")?;
    let before = Connection::open(&path)?;
    let mut before_tables = table_names(&before)?;
    assert!(before_tables.remove("schema_migrations"));
    let (rust_dir, rust) = rust_schema_connection()?;
    let mut rust_tables = table_names(&rust)?;
    assert!(rust_tables.remove("rate_limit_buckets"));
    assert!(rust_tables.remove("remediation_claims"));
    assert_eq!(before_tables, rust_tables);
    fs::remove_dir_all(rust_dir)?;
    assert_eq!(user_version(&before)?, 0);
    drop(before);

    let mut store = Store::open(&path)?;
    store.migrate()?;
    assert_eq!(store.schema_version()?, RUST_SCHEMA_VERSION);
    drop(store);

    let after = Connection::open(&path)?;
    assert_eq!(
        migration_versions(&after)?,
        LEGACY_MIGRATIONS
            .iter()
            .map(|version| (*version).to_owned())
            .collect::<Vec<_>>()
    );
    let mut after_tables = table_names(&after)?;
    assert!(after_tables.remove("schema_migrations"));
    let (rust_dir, rust) = rust_schema_connection()?;
    assert_eq!(after_tables, table_names(&rust)?);
    fs::remove_dir_all(rust_dir)?;

    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn rust_store_writes_work_against_a_legacy_fixture() -> Result<(), Box<dyn Error>> {
    let (dir, path) = copy_fixture("write")?;
    let mut store = Store::open(&path)?;
    store.migrate()?;

    let ingest = error_ingest(
        "ERR-123456789abc",
        "EVT-123456789abc",
        "group-legacy-fixture",
        "2026-05-28T20:00:00Z",
    )?;
    let commit = store.commit_error_ingest(ingest)?;
    assert!(commit.is_new_class);
    assert_eq!(
        commit
            .service_event
            .as_ref()
            .map(|event| event.event.as_str()),
        Some("error.new_class")
    );

    let incident = store.correlate_incident(IncidentCorrelation {
        tenant_id: BOOTSTRAP_TENANT_ID.to_owned(),
        project_id: BOOTSTRAP_PROJECT_ID.to_owned(),
        signal_type: "error_group".to_owned(),
        signal_ref: "group-legacy-fixture".to_owned(),
        service: "cadence".to_owned(),
        incident_id: IncidentId::from_str("INC-123456789abc")?,
        event_id: EventId::from_str("EVT-abcdefghijkl")?,
        now: "2026-05-28T20:00:01Z".to_owned(),
    })?;
    assert_eq!(
        incident.as_ref().map(|event| event.event.as_str()),
        Some("incident.opened")
    );
    assert_eq!(store.error_count()?, 1);
    drop(store);

    let connection = Connection::open(&path)?;
    let fts_count = connection.query_row(
        "SELECT count(*) FROM errors_fts WHERE errors_fts MATCH 'worker'",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    assert_eq!(fts_count, 1);
    assert_eq!(
        trigger_names(&connection, "errors")?,
        BTreeSet::from([
            "errors_fts_delete".to_owned(),
            "errors_fts_insert".to_owned(),
            "errors_fts_update".to_owned(),
        ])
    );
    assert_eq!(
        service_event_count(&connection, "INC-123456789abc", "incident.opened")?,
        1
    );

    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn rust_fixture_generator_writes_schema_artifact() -> Result<(), Box<dyn Error>> {
    let (dir, path) = generated_fixture_path("rust-schema")?;
    fixtures::write_schema_fixture(&path)?;
    assert_no_sqlite_sidecars(&path);

    let generated = Connection::open(&path)?;
    assert_eq!(table_names(&generated)?, rust_table_names()?);
    assert_eq!(user_version(&generated)?, RUST_SCHEMA_VERSION);
    assert_eq!(
        migration_versions_or_empty(&generated)?,
        Vec::<String>::new()
    );

    let (rust_dir, rust) = rust_schema_connection()?;
    for table in PRODUCT_TABLES {
        assert_eq!(
            columns(&generated, table)?,
            columns(&rust, table)?,
            "column drift in Rust-generated fixture table {table}"
        );
    }
    assert_eq!(index_sql(&generated)?, index_sql(&rust)?);
    for table in FOREIGN_KEY_TABLES {
        assert_eq!(
            foreign_keys(&generated, table)?,
            foreign_keys(&rust, table)?,
            "foreign-key drift in Rust-generated fixture table {table}"
        );
    }

    fs::remove_dir_all(rust_dir)?;
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn rust_fixture_generator_writes_read_model_artifact() -> Result<(), Box<dyn Error>> {
    let (dir, path) = generated_fixture_path("rust-read-model")?;
    fixtures::write_read_model_fixture(&path)?;
    assert_no_sqlite_sidecars(&path);

    let store = Store::open(&path)?;
    assert_populated_read_models(&store)?;

    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn webhook_delivery_queries_keep_order_on_a_legacy_fixture() -> Result<(), Box<dyn Error>> {
    let (dir, path) = copy_fixture("webhook-order")?;
    let mut store = Store::open(&path)?;
    store.migrate()?;

    for (delivery_id, now) in [
        ("DLV-222222222222", "2026-05-28T20:00:02Z"),
        ("DLV-111111111111", "2026-05-28T20:00:01Z"),
        ("DLV-333333333333", "2026-05-28T20:00:02Z"),
    ] {
        store.create_pending_webhook_delivery(WebhookDeliveryInsert {
            delivery_id: delivery_id.to_owned(),
            webhook_id: "WHK-123456789abc".to_owned(),
            tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
            project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
            service: None,
            event: "error.new_class".to_owned(),
            now: now.to_owned(),
        })?;
    }

    let rows = store.webhook_deliveries(WebhookDeliveryListOptions {
        status: Some(WebhookDeliveryStatus::Pending),
        ..WebhookDeliveryListOptions::default()
    })?;
    let ids = rows
        .iter()
        .map(|row| row.delivery_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec!["DLV-333333333333", "DLV-222222222222", "DLV-111111111111"]
    );

    drop(store);
    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn rust_query_read_models_read_populated_legacy_rows() -> Result<(), Box<dyn Error>> {
    let (dir, path) = copy_populated_fixture("read-models")?;
    let store = Store::open(&path)?;

    assert_populated_read_models(&store)?;

    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn rust_now_relative_queries_read_populated_legacy_rows_at_fixed_time() -> Result<(), Box<dyn Error>>
{
    let (dir, path) = copy_populated_fixture("read-models-at")?;
    let mut store = Store::open(&path)?;
    let as_of = OffsetDateTime::parse("2026-05-28T20:02:00Z", &Rfc3339)?;

    let service =
        store.errors_by_service_at("ramp-api", "24h", ServiceQueryOptions::default(), as_of)?;
    assert_eq!(service.total_errors, 3);
    assert_eq!(service.groups.len(), 1);
    assert_eq!(service.groups[0].group_hash, "grp-readmodel-runtime");
    assert_eq!(service.groups[0].classification.persistence, "persistent");
    assert_eq!(
        service.summary,
        "3 errors in ramp-api in the last 24h. 1 unique classes. Most frequent: RuntimeError (3 occurrences)."
    );

    let class = store.errors_by_error_class_at(
        "RuntimeError",
        "24h",
        Some("ramp-api"),
        ServiceQueryOptions::default(),
        as_of,
    )?;
    assert_eq!(class.total_errors, 3);
    assert_eq!(class.groups[0].service, "ramp-api");

    let classes = store.errors_by_class_at("24h", as_of)?;
    assert_eq!(classes.total_errors, 3);
    assert_eq!(classes.total_error_classes, 1);
    assert_eq!(classes.groups[0].error_class, "RuntimeError");

    let after_window = OffsetDateTime::parse("2026-05-30T20:02:00Z", &Rfc3339)?;
    let expired = store.errors_by_service_at(
        "ramp-api",
        "24h",
        ServiceQueryOptions::default(),
        after_window,
    )?;
    assert_eq!(expired.total_errors, 0);
    assert_eq!(expired.groups, Vec::new());
    assert_eq!(
        expired.summary,
        "0 errors in ramp-api in the last 24h. 0 unique classes."
    );

    let incidents = store.active_incidents_at(IncidentListOptions::default(), as_of)?;
    assert_eq!(incidents.incidents.len(), 1);
    assert_eq!(incidents.incidents[0].id, "INC-readmodel0001");
    assert_eq!(incidents.incidents[0].severity, "high");
    assert_eq!(incidents.incidents[0].signal_count, 3);
    assert_eq!(
        incidents.summary,
        "1 open incident across 1 service. 1 high-severity incident. Newest: ramp-api at 2026-05-28T19:59:00Z."
    );

    let with_ack = store.active_incidents_at(
        IncidentListOptions {
            with_annotation: Some("acknowledged".to_owned()),
            without_annotation: None,
            ..IncidentListOptions::default()
        },
        as_of,
    )?;
    assert_eq!(with_ack.incidents.len(), 1);

    let without_ack = store.active_incidents_at(
        IncidentListOptions {
            with_annotation: None,
            without_annotation: Some("acknowledged".to_owned()),
            ..IncidentListOptions::default()
        },
        as_of,
    )?;
    assert_eq!(without_ack.incidents, Vec::new());

    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn rust_fixed_clock_queries_keep_legacy_pagination_and_incident_boundary()
-> Result<(), Box<dyn Error>> {
    let (dir, path) = copy_populated_fixture("read-model-boundaries")?;
    insert_paged_error_groups(&path, "2026-05-28T20:00:00Z")?;
    let mut store = Store::open(&path)?;
    let as_of = OffsetDateTime::parse("2026-05-28T20:02:00Z", &Rfc3339)?;

    let first_page =
        store.errors_by_service_at("ramp-api", "24h", ServiceQueryOptions::default(), as_of)?;
    assert_eq!(first_page.groups.len(), 50);
    assert!(first_page.cursor.is_some());
    assert_eq!(first_page.groups[0].group_hash, "grp-page-001");
    assert_eq!(first_page.groups[49].group_hash, "grp-page-050");

    let second_page = store.errors_by_service_at(
        "ramp-api",
        "24h",
        ServiceQueryOptions {
            cursor: first_page.cursor,
            ..ServiceQueryOptions::default()
        },
        as_of,
    )?;
    assert_eq!(
        second_page
            .groups
            .iter()
            .map(|group| group.group_hash.as_str())
            .collect::<Vec<_>>(),
        vec!["grp-page-051", "grp-readmodel-runtime"]
    );
    assert_eq!(second_page.cursor, None);

    let inclusive_boundary = OffsetDateTime::parse("2026-05-28T20:04:00Z", &Rfc3339)?;
    let inclusive =
        store.active_incidents_at(IncidentListOptions::default(), inclusive_boundary)?;
    assert_eq!(inclusive.incidents.len(), 1);
    assert_eq!(inclusive.incidents[0].signal_count, 3);
    assert_eq!(inclusive.incidents[0].severity, "high");

    let after_boundary = OffsetDateTime::parse("2026-05-28T20:04:01Z", &Rfc3339)?;
    let aged = store.active_incidents_at(IncidentListOptions::default(), after_boundary)?;
    assert_eq!(aged.incidents.len(), 1);
    assert_eq!(aged.incidents[0].signal_count, 3);
    assert_eq!(aged.incidents[0].severity, "medium");

    let error_group_expired = OffsetDateTime::parse("2026-05-28T20:05:01Z", &Rfc3339)?;
    let health_only =
        store.active_incidents_at(IncidentListOptions::default(), error_group_expired)?;
    assert_eq!(health_only.incidents.len(), 1);
    assert_eq!(health_only.incidents[0].signal_count, 2);
    assert_eq!(health_only.incidents[0].severity, "medium");
    assert_eq!(
        health_only.incidents[0]
            .signals
            .iter()
            .map(|signal| signal.signal_ref.as_str())
            .collect::<Vec<_>>(),
        vec!["TGT-readmodel-api", "MON-readmodel-cron"]
    );

    insert_stale_active_health_signal(&path)?;
    let stale_health_only =
        store.active_incidents_at(IncidentListOptions::default(), error_group_expired)?;
    assert_eq!(stale_health_only.incidents.len(), 1);
    assert_eq!(stale_health_only.incidents[0].signal_count, 3);
    assert_eq!(
        stale_health_only.incidents[0].severity, "high",
        "active health-transition signals are stateful in Rust severity even after attached_at ages out"
    );
    assert_eq!(
        stale_health_only.incidents[0]
            .signals
            .iter()
            .map(|signal| signal.signal_ref.as_str())
            .collect::<Vec<_>>(),
        vec![
            "TGT-readmodel-api",
            "TGT-readmodel-worker",
            "MON-readmodel-cron",
        ]
    );

    fs::remove_dir_all(dir)?;
    Ok(())
}

#[test]
fn rust_health_read_models_read_populated_legacy_rows() -> Result<(), Box<dyn Error>> {
    let (dir, path) = copy_populated_fixture("health-read-models")?;
    let mut store = Store::open(&path)?;

    let targets = store.list_targets()?;
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].id, "TGT-readmodel-api");
    assert_eq!(targets[0].name, "Ramp API");
    assert_eq!(targets[0].service, "ramp-api");
    assert_eq!(targets[0].method, "GET");
    assert_eq!(targets[0].interval_ms, 60_000);
    assert_eq!(targets[0].timeout_ms, 10_000);
    assert_eq!(targets[0].expected_status, "200");
    assert!(targets[0].active);

    let schedules = store.active_target_probe_schedules()?;
    assert_eq!(schedules.len(), 1);
    assert_eq!(schedules[0].target_id, "TGT-readmodel-api");
    assert_eq!(schedules[0].interval_ms, 60_000);

    let target_snapshot = store
        .target_probe_snapshot_by_id("TGT-readmodel-api")?
        .ok_or("missing target probe snapshot")?;
    assert_eq!(target_snapshot.name, "Ramp API");
    assert_eq!(target_snapshot.service, "ramp-api");
    assert_eq!(target_snapshot.state, "down");
    assert_eq!(target_snapshot.consecutive_failures, 4);
    assert_eq!(target_snapshot.expected_status, "200");

    let monitor_snapshot = store
        .monitor_check_in_snapshot_by_name("Ramp nightly import")?
        .ok_or("missing monitor check-in snapshot")?;
    assert_eq!(monitor_snapshot.id, "MON-readmodel-cron");
    assert_eq!(monitor_snapshot.service, "ramp-api");
    assert_eq!(monitor_snapshot.mode, "ttl");
    assert_eq!(monitor_snapshot.expected_every_ms, 60_000);
    assert_eq!(monitor_snapshot.grace_ms, 5_000);
    assert_eq!(monitor_snapshot.state, "degraded");

    let overdue = store.monitor_overdue_candidates("2026-06-01T00:00:00Z")?;
    assert_eq!(overdue.len(), 1);
    assert_eq!(overdue[0].id, "MON-readmodel-cron");
    assert_eq!(overdue[0].last_check_in_status.as_deref(), Some("alive"));
    assert_eq!(
        overdue[0].deadline_at.as_deref(),
        Some("2026-05-28T20:00:00Z")
    );

    fs::remove_dir_all(dir)?;
    Ok(())
}

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(LEGACY_FIXTURE)
}

fn populated_fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(POPULATED_LEGACY_FIXTURE)
}

fn open_fixture_read_only() -> Result<Connection, Box<dyn Error>> {
    Ok(Connection::open_with_flags(
        fixture_path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?)
}

fn copy_fixture(name: &str) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    copy_fixture_from(name, fixture_path())
}

fn copy_populated_fixture(name: &str) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    copy_fixture_from(name, populated_fixture_path())
}

fn copy_fixture_from(name: &str, source: PathBuf) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!("canary-legacy-fixture-{name}-{}", temp_suffix()?));
    fs::create_dir_all(&dir)?;
    let path = dir.join("legacy_fixture.db");
    fs::copy(source, &path)?;
    Ok((dir, path))
}

fn generated_fixture_path(name: &str) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!("canary-rust-fixture-{name}-{}", temp_suffix()?));
    fs::create_dir_all(&dir)?;
    let path = dir.join("rust_fixture.db");
    Ok((dir, path))
}

fn temp_suffix() -> Result<String, Box<dyn Error>> {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(format!("{}-{nonce}-{counter}", std::process::id()))
}

fn assert_no_sqlite_sidecars(path: &Path) {
    assert!(!path.with_extension("db-shm").exists());
    assert!(!path.with_extension("db-wal").exists());
}

fn rust_schema_connection() -> Result<(PathBuf, Connection), Box<dyn Error>> {
    let (dir, path) = copy_fixture("rust-schema-empty")?;
    fs::remove_file(&path)?;
    let mut store = Store::open(&path)?;
    store.migrate()?;
    drop(store);
    let connection = Connection::open(&path)?;
    Ok((dir, connection))
}

fn table_names(connection: &Connection) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let mut statement = connection.prepare(
        "SELECT name
         FROM sqlite_schema
         WHERE type = 'table'
           AND name NOT LIKE 'sqlite_%'
         ORDER BY name",
    )?;
    Ok(statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?)
}

fn rust_table_names() -> Result<BTreeSet<String>, Box<dyn Error>> {
    let (dir, connection) = rust_schema_connection()?;
    let names = table_names(&connection)?;
    fs::remove_dir_all(dir)?;
    Ok(names)
}

fn columns(
    connection: &Connection,
    table: &str,
) -> Result<BTreeMap<String, Column>, Box<dyn Error>> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    Ok(statement
        .query_map([], |row| {
            Ok(Column {
                cid: row.get(0)?,
                name: row.get(1)?,
                data_type: row.get(2)?,
                not_null: row.get::<_, i64>(3)? == 1,
                default_value: row.get(4)?,
                primary_key_position: row.get(5)?,
            })
        })?
        .map(|column| column.map(|column| (column.name.clone(), column)))
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?)
}

fn column_shapes(
    connection: &Connection,
    table: &str,
) -> Result<BTreeMap<String, ColumnShape>, Box<dyn Error>> {
    Ok(columns(connection, table)?
        .into_iter()
        .map(|(name, column)| {
            (
                name,
                ColumnShape {
                    name: column.name,
                    data_type: column.data_type,
                    not_null: column.not_null,
                    default_value: column.default_value,
                    primary_key_position: column.primary_key_position,
                },
            )
        })
        .collect())
}

fn index_sql(connection: &Connection) -> Result<BTreeSet<IndexSql>, Box<dyn Error>> {
    let mut statement = connection.prepare(
        "SELECT tbl_name, name, sql
         FROM sqlite_schema
         WHERE type = 'index'
           AND name NOT LIKE 'sqlite_autoindex%'
         ORDER BY tbl_name, name",
    )?;
    Ok(statement
        .query_map([], |row| {
            Ok(IndexSql {
                table: row.get(0)?,
                name: row.get(1)?,
                sql: normalize_sql(&row.get::<_, String>(2)?),
            })
        })?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?)
}

fn foreign_keys(connection: &Connection, table: &str) -> Result<Vec<ForeignKey>, Box<dyn Error>> {
    let mut statement = connection.prepare(&format!("PRAGMA foreign_key_list({table})"))?;
    Ok(statement
        .query_map([], |row| {
            Ok(ForeignKey {
                table: row.get(2)?,
                from: row.get(3)?,
                to: row.get(4)?,
                on_delete: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn trigger_names(connection: &Connection, table: &str) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let mut statement = connection.prepare(
        "SELECT name FROM sqlite_schema
         WHERE type = 'trigger' AND tbl_name = ?1
         ORDER BY name",
    )?;
    Ok(statement
        .query_map([table], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?)
}

fn service_event_count(
    connection: &Connection,
    entity_ref: &str,
    event: &str,
) -> Result<i64, Box<dyn Error>> {
    Ok(connection.query_row(
        "SELECT count(*) FROM service_events WHERE entity_ref = ?1 AND event = ?2",
        params![entity_ref, event],
        |row| row.get(0),
    )?)
}

fn user_version(connection: &Connection) -> Result<u32, Box<dyn Error>> {
    Ok(connection.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

fn migration_versions(connection: &Connection) -> Result<Vec<String>, Box<dyn Error>> {
    let mut statement =
        connection.prepare("SELECT version FROM schema_migrations ORDER BY version")?;
    Ok(statement
        .query_map([], |row| {
            row.get::<_, i64>(0).map(|version| version.to_string())
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn migration_versions_or_empty(connection: &Connection) -> Result<Vec<String>, Box<dyn Error>> {
    let exists = connection.query_row(
        "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'schema_migrations'",
        [],
        |_row| Ok(()),
    );
    match exists {
        Ok(()) => migration_versions(connection),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Vec::new()),
        Err(error) => Err(Box::new(error)),
    }
}

fn assert_populated_read_models(store: &Store) -> Result<(), Box<dyn Error>> {
    let error = store
        .error_detail("ERR-readmodel0001")?
        .ok_or("missing fixture error detail")?;
    assert_eq!(error.service, "ramp-api");
    assert_eq!(error.group_hash, "grp-readmodel-runtime");
    assert_eq!(
        error.context.as_ref().and_then(|ctx| ctx.get("tenant")),
        Some(&serde_json::json!("alpha"))
    );
    assert_eq!(error.group.as_ref().map(|group| group.total_count), Some(3));
    assert_eq!(error.incident_ids, vec!["INC-readmodel0001"]);

    let detail = store
        .incident_detail("INC-readmodel0001")?
        .ok_or("missing fixture incident detail")?;
    assert_eq!(detail.incident.service, "ramp-api");
    assert_eq!(detail.annotations.len(), 1);
    assert_eq!(detail.annotations[0].action, "acknowledged");
    assert_eq!(detail.recent_timeline_events.len(), 1);
    assert_eq!(detail.recent_timeline_events[0].event, "incident.opened");

    let target = detail
        .signals
        .iter()
        .find(|signal| signal.target_id.as_deref() == Some("TGT-readmodel-api"))
        .ok_or("missing target health signal")?;
    assert_eq!(target.target_name.as_deref(), Some("Ramp API"));
    assert_eq!(target.current_state.as_deref(), Some("down"));
    assert_eq!(target.consecutive_failures, Some(4));
    assert_eq!(target.annotation_count, 1);

    let monitor = detail
        .signals
        .iter()
        .find(|signal| signal.monitor_id.as_deref() == Some("MON-readmodel-cron"))
        .ok_or("missing monitor health signal")?;
    assert_eq!(monitor.monitor_name.as_deref(), Some("Ramp nightly import"));
    assert_eq!(monitor.current_state.as_deref(), Some("degraded"));

    let group = detail
        .signals
        .iter()
        .find(|signal| signal.group_hash.as_deref() == Some("grp-readmodel-runtime"))
        .ok_or("missing error-group signal")?;
    assert_eq!(group.error_class.as_deref(), Some("RuntimeError"));
    assert_eq!(group.annotation_count, 1);

    Ok(())
}

fn normalize_sql(sql: &str) -> String {
    sql.replace('"', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" (", "(")
}

fn error_ingest(
    error_id: &str,
    event_id: &str,
    group_hash: &str,
    created_at: &str,
) -> Result<ErrorIngest, Box<dyn Error>> {
    Ok(ErrorIngest {
        ids: ErrorIngestIds {
            error_id: ErrorId::from_str(error_id)?,
            event_id: EventId::from_str(event_id)?,
        },
        payload: ErrorIngestPayload {
            tenant_id: canary_store::BOOTSTRAP_TENANT_ID.to_owned(),
            project_id: canary_store::BOOTSTRAP_PROJECT_ID.to_owned(),
            service: "cadence".to_owned(),
            error_class: "DBConnection.ConnectionError".to_owned(),
            message: "worker pool timed out".to_owned(),
            message_template: "worker pool timed out".to_owned(),
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
    })
}

fn insert_paged_error_groups(path: &Path, last_seen_at: &str) -> Result<(), Box<dyn Error>> {
    let connection = Connection::open(path)?;

    for index in 1..=51 {
        connection.execute(
            "INSERT INTO error_groups (
                group_hash, service, error_class, severity, first_seen_at, last_seen_at,
                message_template, last_error_id, total_count, status
             ) VALUES (?1, 'ramp-api', ?2, 'error', ?3, ?3, ?4, ?5, ?6, 'active')",
            params![
                format!("grp-page-{index:03}"),
                format!("PageError{index:03}"),
                last_seen_at,
                format!("paged sample {index}"),
                format!("ERR-page-{index:03}"),
                200 - index,
            ],
        )?;
    }

    Ok(())
}

fn insert_stale_active_health_signal(path: &Path) -> Result<(), Box<dyn Error>> {
    let connection = Connection::open(path)?;
    connection.execute(
        "INSERT INTO targets (id, url, name, service, created_at)
         VALUES (
            'TGT-readmodel-worker',
            'https://worker.example.test/health',
            'Worker',
            'ramp-api',
            '2026-05-28T19:00:00Z'
         )",
        [],
    )?;
    connection.execute(
        "INSERT INTO target_state (target_id, state)
         VALUES ('TGT-readmodel-worker', 'down')",
        [],
    )?;
    connection.execute(
        "INSERT INTO incident_signals (incident_id, signal_type, signal_ref, attached_at)
         VALUES (
            'INC-readmodel0001',
            'health_transition',
            'TGT-readmodel-worker',
            '2026-05-28T20:00:00Z'
         )",
        [],
    )?;
    Ok(())
}
