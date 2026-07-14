//! Durable read-audit events for responder context fetches.
//!
//! When a non-admin API key reads rich context, Canary records a durable audit
//! event in `service_events` with `retention_class = 'audit'`. Incident context
//! reads also capture the subject and context envelope schema. The audit record
//! captures *who* read *what route* and *when* — never the response payload
//! itself.

use canary_core::ids::EventId;
use canary_store::{Store, TelemetryEventError, TelemetryEventInsert, VerifiedApiKey};
use serde_json::json;

use crate::server_time::current_rfc3339;

pub(crate) struct ReadAuditContext<'a> {
    pub(crate) route: &'a str,
    pub(crate) subject_type: &'a str,
    pub(crate) subject_id: &'a str,
    pub(crate) subject_service: &'a str,
    pub(crate) context_schema: &'a str,
}

/// Record a legacy route-level read-audit event.
///
/// Skips silently for admin keys (admin already has full visibility; the
/// goal is a responder-specific trail). The audit record never contains the
/// response payload — only key id, service binding, route, and timestamp.
pub(crate) fn record_read_audit(store: &mut Store, key: &VerifiedApiKey, route: &str) {
    if key.scope == "admin" {
        return;
    }

    let service = key.service.as_deref().unwrap_or("canary");
    let now = current_rfc3339();
    let attributes = json!({
        "key_id": key.id,
        "key_scope": key.scope,
        "key_service": key.service,
        "route": route,
    });

    let event = TelemetryEventInsert {
        id: EventId::generate(),
        tenant_id: key.tenant_id.clone(),
        project_id: key.project_id.clone(),
        service: service.to_owned(),
        name: "responder.context_read".to_owned(),
        severity: "info".to_owned(),
        summary: format!("{service}: context read by key {} via {route}", key.id),
        attributes_json: attributes.to_string(),
        retention_class: "audit".to_owned(),
        privacy_policy: "redacted".to_owned(),
        sampling_policy: "unsampled".to_owned(),
        created_at: now,
    };

    let _ = store.insert_telemetry_event(event);
}

pub(crate) fn record_context_read_audit(
    store: &mut Store,
    key: &VerifiedApiKey,
    context: ReadAuditContext<'_>,
) -> Result<Option<String>, TelemetryEventError> {
    if key.scope == "admin" {
        return Ok(None);
    }

    let now = current_rfc3339();
    let event_id = EventId::generate();
    let event_id_string = event_id.as_str().to_owned();
    let attributes = json!({
        "reader": {
            "key_id": key.id,
            "key_name": key.name,
            "scope": key.scope,
            "service": key.service,
        },
        "route": context.route,
        "subject": {
            "type": context.subject_type,
            "id": context.subject_id,
            "service": context.subject_service,
        },
        "context_envelope": {
            "schema": context.context_schema,
            "privacy_policy": "redacted",
            "retention_class": "audit",
        },
        "read_at": now,
    });

    let event = TelemetryEventInsert {
        id: event_id,
        tenant_id: key.tenant_id.clone(),
        project_id: key.project_id.clone(),
        service: context.subject_service.to_owned(),
        name: "responder.context_read".to_owned(),
        severity: "info".to_owned(),
        summary: format!(
            "{}: {} {} read by key {} via {}",
            context.subject_service,
            context.subject_type,
            context.subject_id,
            key.id,
            context.route
        ),
        attributes_json: attributes.to_string(),
        retention_class: "audit".to_owned(),
        privacy_policy: "redacted".to_owned(),
        sampling_policy: "unsampled".to_owned(),
        created_at: now,
    };

    store.insert_telemetry_event(event)?;
    Ok(Some(event_id_string))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use canary_store::{BOOTSTRAP_PROJECT_ID, BOOTSTRAP_TENANT_ID, TimelineQueryOptions};

    fn make_migrated_store() -> Store {
        let mut store = Store::open_in_memory().unwrap();
        store.migrate().unwrap();
        store
    }

    fn test_key(scope: &str, service: Option<&str>) -> VerifiedApiKey {
        VerifiedApiKey {
            id: "KEY-test".to_owned(),
            name: "test key".to_owned(),
            scope: scope.to_owned(),
            tenant_id: BOOTSTRAP_TENANT_ID.to_owned(),
            project_id: BOOTSTRAP_PROJECT_ID.to_owned(),
            service: service.map(|s| s.to_owned()),
            allow_unbound: scope == "read-only" && service.is_none(),
        }
    }

    #[test]
    fn responder_read_produces_audit_event() {
        let mut store = make_migrated_store();
        let key = test_key("responder-write", Some("test-svc"));

        record_read_audit(&mut store, &key, "GET /api/v1/report");

        let response = store
            .timeline(
                "1h",
                TimelineQueryOptions {
                    tenant_id: Some(BOOTSTRAP_TENANT_ID.to_owned()),
                    project_id: Some(BOOTSTRAP_PROJECT_ID.to_owned()),
                    service: Some("test-svc".to_owned()),
                    limit: Some("10".to_owned()),
                    cursor: None,
                    event_type: Some("telemetry.event".to_owned()),
                },
            )
            .unwrap();

        let audit_events: Vec<_> = response
            .events
            .iter()
            .filter(|e| e.event == "telemetry.event")
            .collect();
        assert_eq!(audit_events.len(), 1);
        assert_eq!(audit_events[0].service, "test-svc");
    }

    #[test]
    fn admin_read_produces_no_audit_event() {
        let mut store = make_migrated_store();
        let key = test_key("admin", None);

        record_read_audit(&mut store, &key, "GET /api/v1/report");

        let response = store
            .timeline(
                "1h",
                TimelineQueryOptions {
                    tenant_id: Some(BOOTSTRAP_TENANT_ID.to_owned()),
                    project_id: Some(BOOTSTRAP_PROJECT_ID.to_owned()),
                    service: None,
                    limit: Some("10".to_owned()),
                    cursor: None,
                    event_type: Some("telemetry.event".to_owned()),
                },
            )
            .unwrap();

        assert!(response.events.is_empty());
    }

    #[test]
    fn read_only_key_produces_audit_event() {
        let mut store = make_migrated_store();
        let key = test_key("read-only", None);

        record_read_audit(&mut store, &key, "GET /api/v1/incidents/{id}");

        let response = store
            .timeline(
                "1h",
                TimelineQueryOptions {
                    tenant_id: Some(BOOTSTRAP_TENANT_ID.to_owned()),
                    project_id: Some(BOOTSTRAP_PROJECT_ID.to_owned()),
                    service: Some("canary".to_owned()),
                    limit: Some("10".to_owned()),
                    cursor: None,
                    event_type: Some("telemetry.event".to_owned()),
                },
            )
            .unwrap();

        assert_eq!(response.events.len(), 1);
    }

    #[test]
    fn audit_event_does_not_contain_payload_body() {
        let mut store = make_migrated_store();
        let key = test_key("responder-write", Some("svc-a"));

        record_read_audit(&mut store, &key, "GET /api/v1/errors/{id}");

        let response = store
            .timeline(
                "1h",
                TimelineQueryOptions {
                    tenant_id: Some(BOOTSTRAP_TENANT_ID.to_owned()),
                    project_id: Some(BOOTSTRAP_PROJECT_ID.to_owned()),
                    service: Some("svc-a".to_owned()),
                    limit: Some("10".to_owned()),
                    cursor: None,
                    event_type: Some("telemetry.event".to_owned()),
                },
            )
            .unwrap();

        let event = &response.events[0];
        let attrs = &event.attributes;
        assert!(attrs.get("key_id").is_some());
        assert!(attrs.get("route").is_some());
        assert!(
            attrs.get("response_body").is_none(),
            "audit event must not contain the response payload"
        );
    }

    #[test]
    fn context_read_audit_records_reader_subject_and_envelope() {
        let mut store = make_migrated_store();
        let key = test_key("responder-write", Some("svc-a"));

        let audit_id = record_context_read_audit(
            &mut store,
            &key,
            ReadAuditContext {
                route: "GET /api/v1/incidents/{id}",
                subject_type: "incident",
                subject_id: "INC-test",
                subject_service: "svc-a",
                context_schema: "canary.responder_context.incident.v1",
            },
        )
        .unwrap();

        let response = store
            .timeline(
                "1h",
                TimelineQueryOptions {
                    tenant_id: Some(BOOTSTRAP_TENANT_ID.to_owned()),
                    project_id: Some(BOOTSTRAP_PROJECT_ID.to_owned()),
                    service: Some("svc-a".to_owned()),
                    limit: Some("10".to_owned()),
                    cursor: None,
                    event_type: Some("telemetry.event".to_owned()),
                },
            )
            .unwrap();

        assert!(audit_id.is_some());
        assert_eq!(response.events.len(), 1);
        let attrs = &response.events[0].attributes;
        assert_eq!(attrs["reader"]["key_id"], "KEY-test");
        assert_eq!(attrs["subject"]["type"], "incident");
        assert_eq!(attrs["subject"]["id"], "INC-test");
        assert_eq!(
            attrs["context_envelope"]["schema"],
            "canary.responder_context.incident.v1"
        );
        assert!(attrs.get("response_body").is_none());
    }
}
