//! Health observation planning.
//!
//! Runtime code owns scheduling, HTTP transport, SSRF guards, id generation, and
//! SQLite transactions. This module owns the Phoenix-compatible product
//! decisions for already-observed health data. Target snapshots must come from a
//! per-target serialized runtime or a transactionally locked read so the pure
//! state machine never plans from stale counters.

use std::{error::Error, fmt};

use canary_core::{
    health::state_machine::{
        Counters, HealthEffect, HealthEvent, HealthState, HealthWebhookEvent, Thresholds,
        transition,
    },
    ids::{EventId, IncidentId},
};
use canary_store::{
    MonitorCheckInCommit, MonitorCheckInObservation, MonitorOverdueCommit, MonitorTransitionEvent,
    TargetCheckObservation, TargetProbeCommit, TargetTransitionEvent,
};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_MONITOR_OBSERVED_AT_FUTURE_SKEW_SECONDS: i64 = 300;

/// Runtime timestamp and ids for a health observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationContext {
    /// Canonical wall-clock timestamp for persistence.
    pub now: String,
    /// Monotonic milliseconds used for flap detection.
    pub now_millis: i64,
    /// Service-event id used if the observation changes health state.
    pub event_id: EventId,
    /// Incident id used if the observation opens an incident.
    pub incident_id: IncidentId,
    /// Service-event id used if incident correlation emits an event.
    pub incident_event_id: EventId,
}

/// Persisted target configuration and current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetSnapshot {
    /// Target row id.
    pub id: String,
    /// Target display name.
    pub name: String,
    /// Service name resolved with Phoenix's target service fallback.
    pub service: String,
    /// Target URL.
    pub url: String,
    /// Current health state.
    pub state: HealthState,
    /// Current persisted counters and recent transitions.
    pub counters: Counters,
    /// Per-target transition thresholds.
    pub thresholds: Thresholds,
}

/// Already-observed target probe data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetProbeObservation {
    /// HTTP status code returned by the target, when a response was received.
    pub status_code: Option<i64>,
    /// Probe latency in milliseconds.
    pub latency_ms: Option<i64>,
    /// Phoenix-compatible result string. Only `success` is successful.
    pub result: String,
    /// TLS certificate expiration timestamp, when known.
    pub tls_expires_at: Option<String>,
    /// Probe error detail, when the probe failed before a valid response.
    pub error_detail: Option<String>,
    /// Probe region.
    pub region: Option<String>,
}

/// Planned persistence for one target probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedTargetProbe {
    /// Store command to execute in one transaction.
    pub commit: TargetProbeCommit,
    /// Webhook event requested by the pure state machine, if any.
    pub webhook_event: Option<HealthWebhookEvent>,
}

/// Plan the exact store command for one already-observed target probe.
pub fn plan_target_probe(
    target: TargetSnapshot,
    observation: TargetProbeObservation,
    context: ObservationContext,
) -> PlannedTargetProbe {
    let event = if observation.result == "success" {
        HealthEvent::Success
    } else {
        HealthEvent::Failure
    };
    let changed = transition(
        target.state,
        event,
        target.thresholds,
        target.counters,
        context.now_millis,
    );
    let webhook_event = webhook_effect(&changed.effects);
    let transition = webhook_event.map(|_| TargetTransitionEvent {
        name: target.name,
        service: target.service,
        url: target.url,
        previous_state: health_state_as_str(target.state).to_owned(),
        event_id: context.event_id,
        incident_id: context.incident_id,
        incident_event_id: context.incident_event_id,
    });

    PlannedTargetProbe {
        commit: TargetProbeCommit {
            target_id: target.id,
            state: health_state_as_str(changed.state).to_owned(),
            consecutive_failures: changed.counters.consecutive_failures,
            consecutive_successes: changed.counters.consecutive_successes,
            check_succeeded: event == HealthEvent::Success,
            check: TargetCheckObservation {
                status_code: observation.status_code,
                latency_ms: observation.latency_ms,
                result: observation.result,
                tls_expires_at: observation.tls_expires_at,
                error_detail: observation.error_detail,
                region: observation.region,
            },
            now: context.now,
            transition,
        },
        webhook_event,
    }
}

/// Monitor operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorMode {
    /// Fixed interval schedule mode.
    Schedule,
    /// Check-in supplied TTL mode.
    Ttl,
}

impl MonitorMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Schedule => "schedule",
            Self::Ttl => "ttl",
        }
    }
}

/// Persisted monitor configuration and current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorSnapshot {
    /// Monitor row id.
    pub id: String,
    /// Monitor display name.
    pub name: String,
    /// Service name resolved with Phoenix's monitor service fallback.
    pub service: String,
    /// Monitor mode.
    pub mode: MonitorMode,
    /// Configured expected interval.
    pub expected_every_ms: i64,
    /// Configured grace period.
    pub grace_ms: i64,
    /// Current health state.
    pub state: HealthState,
}

/// Monitor check-in status accepted by the Phoenix API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorCheckInStatus {
    /// Runtime is alive.
    Alive,
    /// Runtime is still working; not a successful completion.
    InProgress,
    /// Runtime completed successfully.
    Ok,
    /// Runtime reported an error.
    Error,
}

impl MonitorCheckInStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Alive => "alive",
            Self::InProgress => "in_progress",
            Self::Ok => "ok",
            Self::Error => "error",
        }
    }

    fn state(self) -> HealthState {
        match self {
            Self::Error => HealthState::Down,
            Self::Alive | Self::InProgress | Self::Ok => HealthState::Up,
        }
    }
}

/// Already-observed non-HTTP monitor check-in data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorCheckInInput {
    /// Check-in id.
    pub id: String,
    /// Caller supplied idempotency key or external check-in id.
    pub external_id: Option<String>,
    /// Phoenix-compatible status.
    pub status: MonitorCheckInStatus,
    /// Observation timestamp. Deadline math is based on this value, not wall-clock receipt time.
    pub observed_at: String,
    /// Check-in supplied TTL, honored only in TTL mode when positive.
    pub ttl_ms: Option<i64>,
    /// Human-readable check-in summary.
    pub summary: Option<String>,
    /// JSON context string.
    pub context: Option<String>,
}

/// Planned persistence for one monitor check-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedMonitorCheckIn {
    /// Store command to execute in one transaction.
    pub commit: MonitorCheckInCommit,
}

/// Persisted monitor configuration and state needed for overdue evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorOverdueSnapshot {
    /// Monitor row id.
    pub id: String,
    /// Monitor display name.
    pub name: String,
    /// Service name.
    pub service: String,
    /// Monitor mode.
    pub mode: MonitorMode,
    /// Configured expected interval.
    pub expected_every_ms: i64,
    /// Configured grace period.
    pub grace_ms: i64,
    /// Current health state.
    pub state: HealthState,
    /// Last check-in status, when any.
    pub last_check_in_status: Option<String>,
    /// Last check-in timestamp, when any.
    pub last_check_in_at: Option<String>,
    /// Deadline timestamp, when this monitor is eligible for overdue evaluation.
    pub deadline_at: Option<String>,
    /// First missed deadline timestamp, when already degraded from overdue.
    pub first_missed_at: Option<String>,
}

/// Planned persistence for one overdue monitor evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedMonitorOverdue {
    /// Store command to execute in one transaction.
    pub commit: MonitorOverdueCommit,
}

/// Planning error for health observation adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthPlanError {
    /// The observed check-in timestamp was not RFC3339.
    InvalidObservedAt(String),
    /// The observed check-in timestamp is beyond the accepted future clock skew.
    ObservedAtTooFarInFuture,
}

impl fmt::Display for HealthPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidObservedAt(value) => {
                write!(formatter, "invalid monitor observed_at timestamp: {value}")
            }
            Self::ObservedAtTooFarInFuture => write!(
                formatter,
                "monitor observed_at cannot be more than {MAX_MONITOR_OBSERVED_AT_FUTURE_SKEW_SECONDS} seconds after receipt time"
            ),
        }
    }
}

impl Error for HealthPlanError {}

/// Plan the exact store command for one monitor check-in.
pub fn plan_monitor_check_in(
    monitor: MonitorSnapshot,
    input: MonitorCheckInInput,
    context: ObservationContext,
) -> Result<PlannedMonitorCheckIn, HealthPlanError> {
    let state = input.status.state();
    let observed_at = parse_observed_at(&input.observed_at)?;
    reject_observed_at_after_receipt(observed_at, &context.now)?;
    let deadline_at = deadline_at(&monitor, &input, observed_at)?;
    let transitioned = monitor.state != state;
    let transition = transitioned.then(|| MonitorTransitionEvent {
        name: monitor.name,
        service: monitor.service,
        mode: monitor.mode.as_str().to_owned(),
        expected_every_ms: monitor.expected_every_ms,
        grace_ms: monitor.grace_ms,
        previous_state: health_state_as_str(monitor.state).to_owned(),
        event_id: context.event_id,
        incident_id: context.incident_id,
        incident_event_id: context.incident_event_id,
    });

    Ok(PlannedMonitorCheckIn {
        commit: MonitorCheckInCommit {
            monitor_id: monitor.id,
            state: health_state_as_str(state).to_owned(),
            last_check_in_at: Some(input.observed_at.clone()),
            last_check_in_status: Some(input.status.as_str().to_owned()),
            deadline_at: Some(deadline_at),
            check_in: MonitorCheckInObservation {
                id: input.id,
                external_id: input.external_id,
                status: input.status.as_str().to_owned(),
                observed_at: input.observed_at,
                ttl_ms: input.ttl_ms,
                summary: input.summary,
                context: input.context,
            },
            now: context.now,
            transition,
        },
    })
}

/// Plan the exact store command for one overdue monitor evaluation.
pub fn plan_monitor_overdue(
    monitor: MonitorOverdueSnapshot,
    context: ObservationContext,
) -> Result<Option<PlannedMonitorOverdue>, HealthPlanError> {
    let Some(deadline_at) = monitor.deadline_at.as_deref() else {
        return Ok(None);
    };
    if !timestamp_after(&context.now, deadline_at, "deadline_at")? {
        return Ok(None);
    }
    if monitor.last_check_in_status.as_deref() == Some("error")
        || monitor.state == HealthState::Down
    {
        return Ok(None);
    }

    let next_state = match monitor.state {
        HealthState::Unknown | HealthState::Up => Some(HealthState::Degraded),
        HealthState::Degraded
            if monitor_overdue_window_elapsed(
                monitor.first_missed_at.as_deref(),
                &context.now,
                monitor.expected_every_ms,
            )? =>
        {
            Some(HealthState::Down)
        }
        HealthState::Degraded | HealthState::Paused | HealthState::Flapping | HealthState::Down => {
            None
        }
    };

    let Some(next_state) = next_state else {
        return Ok(None);
    };

    Ok(Some(PlannedMonitorOverdue {
        commit: MonitorOverdueCommit {
            monitor_id: monitor.id,
            state: health_state_as_str(next_state).to_owned(),
            first_missed_at: (next_state == HealthState::Degraded).then_some(context.now.clone()),
            last_check_in_at: monitor.last_check_in_at,
            last_check_in_status: monitor.last_check_in_status,
            deadline_at: monitor.deadline_at,
            now: context.now,
            transition: MonitorTransitionEvent {
                name: monitor.name,
                service: monitor.service,
                mode: monitor.mode.as_str().to_owned(),
                expected_every_ms: monitor.expected_every_ms,
                grace_ms: monitor.grace_ms,
                previous_state: health_state_as_str(monitor.state).to_owned(),
                event_id: context.event_id,
                incident_id: context.incident_id,
                incident_event_id: context.incident_event_id,
            },
        },
    }))
}

fn deadline_at(
    monitor: &MonitorSnapshot,
    input: &MonitorCheckInInput,
    observed: OffsetDateTime,
) -> Result<String, HealthPlanError> {
    let deadline = observed
        + Duration::milliseconds(effective_interval_ms(monitor, input))
        + Duration::milliseconds(monitor.grace_ms);
    deadline
        .format(&Rfc3339)
        .map_err(|_| HealthPlanError::InvalidObservedAt(input.observed_at.clone()))
}

fn parse_observed_at(value: &str) -> Result<OffsetDateTime, HealthPlanError> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| HealthPlanError::InvalidObservedAt(value.to_owned()))
}

fn reject_observed_at_after_receipt(
    observed_at: OffsetDateTime,
    receipt_time: &str,
) -> Result<(), HealthPlanError> {
    let Some(receipt_time) = parse_monitor_timestamp("now", receipt_time) else {
        return Ok(());
    };
    if observed_at - receipt_time > Duration::seconds(MAX_MONITOR_OBSERVED_AT_FUTURE_SKEW_SECONDS) {
        return Err(HealthPlanError::ObservedAtTooFarInFuture);
    }
    Ok(())
}

fn effective_interval_ms(monitor: &MonitorSnapshot, input: &MonitorCheckInInput) -> i64 {
    match (monitor.mode, input.ttl_ms) {
        (MonitorMode::Ttl, Some(ttl_ms)) if ttl_ms > 0 => ttl_ms,
        _ => monitor.expected_every_ms,
    }
}

fn timestamp_after(now: &str, then: &str, field: &'static str) -> Result<bool, HealthPlanError> {
    let Some(now) = parse_monitor_timestamp("now", now) else {
        return Ok(false);
    };
    let Some(then) = parse_monitor_timestamp(field, then) else {
        return Ok(false);
    };
    Ok(now > then)
}

fn monitor_overdue_window_elapsed(
    first_missed_at: Option<&str>,
    now: &str,
    expected_every_ms: i64,
) -> Result<bool, HealthPlanError> {
    let Some(first_missed_at) = first_missed_at else {
        return Ok(false);
    };
    let Some(first_missed) = parse_monitor_timestamp("first_missed_at", first_missed_at) else {
        return Ok(false);
    };
    let Some(now) = parse_monitor_timestamp("now", now) else {
        return Ok(false);
    };
    Ok(now - first_missed >= Duration::milliseconds(expected_every_ms))
}

fn parse_monitor_timestamp(_field: &'static str, value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339).ok()
}

fn webhook_effect(effects: &[HealthEffect]) -> Option<HealthWebhookEvent> {
    effects.iter().find_map(|effect| match effect {
        HealthEffect::Webhook(event) => Some(*event),
        HealthEffect::Transition { .. } => None,
    })
}

fn health_state_as_str(state: HealthState) -> &'static str {
    state.as_str()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use canary_core::ids::{EventId, IncidentId};

    use super::*;

    const NOW: &str = "2026-05-28T20:00:00Z";

    fn context() -> Result<ObservationContext, Box<dyn Error>> {
        Ok(ObservationContext {
            now: NOW.to_owned(),
            now_millis: 1_000_000,
            event_id: EventId::from_str("EVT-healthplan00")?,
            incident_id: IncidentId::from_str("INC-healthplan00")?,
            incident_event_id: EventId::from_str("EVT-healthincid0")?,
        })
    }

    fn target(state: HealthState) -> TargetSnapshot {
        TargetSnapshot {
            id: "TGT-api".to_owned(),
            name: "API".to_owned(),
            service: "api".to_owned(),
            url: "https://api.example.test/health".to_owned(),
            state,
            counters: Counters::initial(),
            thresholds: Thresholds::default(),
        }
    }

    fn success_probe() -> TargetProbeObservation {
        TargetProbeObservation {
            status_code: Some(200),
            latency_ms: Some(22),
            result: "success".to_owned(),
            tls_expires_at: None,
            error_detail: None,
            region: Some("iad".to_owned()),
        }
    }

    #[test]
    fn target_probe_success_uses_core_state_machine_and_plans_recovered_transition()
    -> Result<(), Box<dyn Error>> {
        let plan = plan_target_probe(target(HealthState::Unknown), success_probe(), context()?);

        assert_eq!(plan.commit.state, "up");
        assert_eq!(plan.commit.consecutive_successes, 1);
        assert!(plan.commit.check_succeeded);
        assert_eq!(plan.webhook_event, Some(HealthWebhookEvent::Recovered));
        let transition = plan.commit.transition.ok_or("missing transition")?;
        assert_eq!(transition.previous_state, "unknown");
        assert_eq!(transition.service, "api");

        Ok(())
    }

    #[test]
    fn target_probe_below_threshold_persists_check_without_transition() -> Result<(), Box<dyn Error>>
    {
        let mut snapshot = target(HealthState::Up);
        snapshot.thresholds = Thresholds {
            degraded_after: 3,
            ..Thresholds::default()
        };
        let plan = plan_target_probe(
            snapshot,
            TargetProbeObservation {
                result: "timeout".to_owned(),
                error_detail: Some("timed out".to_owned()),
                ..success_probe()
            },
            context()?,
        );

        assert_eq!(plan.commit.state, "up");
        assert_eq!(plan.commit.consecutive_failures, 1);
        assert!(!plan.commit.check_succeeded);
        assert!(plan.commit.transition.is_none());
        assert!(plan.webhook_event.is_none());

        Ok(())
    }

    #[test]
    fn target_probe_flapping_state_does_not_plan_webhook_transition() -> Result<(), Box<dyn Error>>
    {
        let mut snapshot = target(HealthState::Down);
        snapshot.counters = Counters {
            consecutive_failures: 0,
            consecutive_successes: 0,
            transitions: vec![
                (HealthState::Up, 999_000),
                (HealthState::Down, 998_000),
                (HealthState::Up, 997_000),
            ],
        };
        let plan = plan_target_probe(snapshot, success_probe(), context()?);

        assert_eq!(plan.commit.state, "flapping");
        assert!(plan.commit.transition.is_none());
        assert!(plan.webhook_event.is_none());

        Ok(())
    }

    fn monitor(state: HealthState, mode: MonitorMode) -> MonitorSnapshot {
        MonitorSnapshot {
            id: "MON-worker".to_owned(),
            name: "Worker".to_owned(),
            service: "worker".to_owned(),
            mode,
            expected_every_ms: 60_000,
            grace_ms: 5_000,
            state,
        }
    }

    fn check_in(status: MonitorCheckInStatus) -> MonitorCheckInInput {
        MonitorCheckInInput {
            id: "CHK-worker0001".to_owned(),
            external_id: Some("deploy-42".to_owned()),
            status,
            observed_at: NOW.to_owned(),
            ttl_ms: Some(120_000),
            summary: None,
            context: None,
        }
    }

    #[test]
    fn monitor_ttl_check_in_uses_observed_at_and_positive_ttl_for_deadline()
    -> Result<(), Box<dyn Error>> {
        let plan = plan_monitor_check_in(
            monitor(HealthState::Unknown, MonitorMode::Ttl),
            check_in(MonitorCheckInStatus::Alive),
            context()?,
        )?;

        assert_eq!(plan.commit.state, "up");
        assert_eq!(plan.commit.last_check_in_status.as_deref(), Some("alive"));
        assert_eq!(
            plan.commit.deadline_at.as_deref(),
            Some("2026-05-28T20:02:05Z")
        );
        assert!(plan.commit.transition.is_some());

        Ok(())
    }

    #[test]
    fn monitor_check_in_rejects_observed_at_beyond_future_skew() -> Result<(), Box<dyn Error>> {
        let mut input = check_in(MonitorCheckInStatus::Alive);
        input.observed_at = "2026-05-28T20:05:01Z".to_owned();

        let Err(error) = plan_monitor_check_in(
            monitor(HealthState::Unknown, MonitorMode::Ttl),
            input,
            context()?,
        ) else {
            return Err("future observed_at beyond skew should be rejected".into());
        };

        assert_eq!(
            error.to_string(),
            "monitor observed_at cannot be more than 300 seconds after receipt time"
        );

        Ok(())
    }

    #[test]
    fn monitor_check_in_accepts_observed_at_at_future_skew_limit() -> Result<(), Box<dyn Error>> {
        let mut input = check_in(MonitorCheckInStatus::Alive);
        input.observed_at = "2026-05-28T20:05:00Z".to_owned();

        let plan = plan_monitor_check_in(
            monitor(HealthState::Unknown, MonitorMode::Ttl),
            input,
            context()?,
        )?;

        assert_eq!(
            plan.commit.deadline_at.as_deref(),
            Some("2026-05-28T20:07:05Z")
        );

        Ok(())
    }

    #[test]
    fn monitor_in_progress_maps_to_up_without_error_or_success_semantics()
    -> Result<(), Box<dyn Error>> {
        let plan = plan_monitor_check_in(
            monitor(HealthState::Up, MonitorMode::Schedule),
            check_in(MonitorCheckInStatus::InProgress),
            context()?,
        )?;

        assert_eq!(plan.commit.state, "up");
        assert_eq!(
            plan.commit.last_check_in_status.as_deref(),
            Some("in_progress")
        );
        assert_eq!(
            plan.commit.deadline_at.as_deref(),
            Some("2026-05-28T20:01:05Z")
        );
        assert!(plan.commit.transition.is_none());

        Ok(())
    }

    #[test]
    fn monitor_error_check_in_plans_down_transition() -> Result<(), Box<dyn Error>> {
        let plan = plan_monitor_check_in(
            monitor(HealthState::Up, MonitorMode::Schedule),
            check_in(MonitorCheckInStatus::Error),
            context()?,
        )?;

        assert_eq!(plan.commit.state, "down");
        assert_eq!(plan.commit.last_check_in_status.as_deref(), Some("error"));
        let transition = plan.commit.transition.ok_or("missing transition")?;
        assert_eq!(transition.previous_state, "up");
        assert_eq!(transition.mode, "schedule");

        Ok(())
    }

    fn overdue_monitor(
        state: HealthState,
        deadline_at: Option<&str>,
        first_missed_at: Option<&str>,
    ) -> MonitorOverdueSnapshot {
        MonitorOverdueSnapshot {
            id: "MON-worker".to_owned(),
            name: "Worker".to_owned(),
            service: "worker".to_owned(),
            mode: MonitorMode::Schedule,
            expected_every_ms: 60_000,
            grace_ms: 5_000,
            state,
            last_check_in_status: Some("alive".to_owned()),
            last_check_in_at: Some("2026-05-28T19:59:00Z".to_owned()),
            deadline_at: deadline_at.map(str::to_owned),
            first_missed_at: first_missed_at.map(str::to_owned),
        }
    }

    #[test]
    fn monitor_overdue_degrades_unknown_or_up_after_deadline() -> Result<(), Box<dyn Error>> {
        let plan = plan_monitor_overdue(
            overdue_monitor(HealthState::Up, Some("2026-05-28T19:59:59Z"), None),
            context()?,
        )?
        .ok_or("missing overdue plan")?;

        assert_eq!(plan.commit.state, "degraded");
        assert_eq!(
            plan.commit.first_missed_at.as_deref(),
            Some("2026-05-28T20:00:00Z")
        );
        assert_eq!(plan.commit.last_check_in_status.as_deref(), Some("alive"));
        assert_eq!(plan.commit.transition.previous_state, "up");

        Ok(())
    }

    #[test]
    fn monitor_overdue_escalates_degraded_after_expected_window() -> Result<(), Box<dyn Error>> {
        let plan = plan_monitor_overdue(
            overdue_monitor(
                HealthState::Degraded,
                Some("2026-05-28T19:59:00Z"),
                Some("2026-05-28T19:58:59Z"),
            ),
            context()?,
        )?
        .ok_or("missing overdue plan")?;

        assert_eq!(plan.commit.state, "down");
        assert!(plan.commit.first_missed_at.is_none());
        assert_eq!(plan.commit.transition.previous_state, "degraded");

        Ok(())
    }

    #[test]
    fn monitor_overdue_ttl_mode_waits_for_expected_window_before_down() -> Result<(), Box<dyn Error>>
    {
        let mut monitor = overdue_monitor(
            HealthState::Degraded,
            Some("2026-05-28T19:59:00Z"),
            Some("2026-05-28T19:59:30Z"),
        );
        monitor.mode = MonitorMode::Ttl;
        monitor.expected_every_ms = 60_000;

        assert!(plan_monitor_overdue(monitor.clone(), context()?)?.is_none());

        monitor.first_missed_at = Some("2026-05-28T19:58:59Z".to_owned());
        let plan = plan_monitor_overdue(monitor, context()?)?.ok_or("missing overdue plan")?;

        assert_eq!(plan.commit.state, "down");
        assert_eq!(plan.commit.transition.mode, "ttl");
        assert_eq!(plan.commit.transition.previous_state, "degraded");
        Ok(())
    }

    #[test]
    fn monitor_overdue_skips_error_status_down_state_and_unexpired_deadlines()
    -> Result<(), Box<dyn Error>> {
        let mut error_status = overdue_monitor(HealthState::Up, Some("2026-05-28T19:59:59Z"), None);
        error_status.last_check_in_status = Some("error".to_owned());
        assert!(plan_monitor_overdue(error_status, context()?)?.is_none());
        assert!(
            plan_monitor_overdue(
                overdue_monitor(HealthState::Down, Some("2026-05-28T19:59:59Z"), None),
                context()?,
            )?
            .is_none()
        );
        assert!(
            plan_monitor_overdue(
                overdue_monitor(HealthState::Up, Some("2026-05-28T20:00:00Z"), None),
                context()?,
            )?
            .is_none()
        );

        Ok(())
    }

    #[test]
    fn monitor_overdue_waits_for_first_missed_window_before_down() -> Result<(), Box<dyn Error>> {
        let plan = plan_monitor_overdue(
            overdue_monitor(
                HealthState::Degraded,
                Some("2026-05-28T19:59:00Z"),
                Some("2026-05-28T19:59:01Z"),
            ),
            context()?,
        )?;

        assert!(plan.is_none());
        Ok(())
    }

    #[test]
    fn monitor_overdue_ignores_malformed_persisted_timestamps() -> Result<(), Box<dyn Error>> {
        assert!(
            plan_monitor_overdue(
                overdue_monitor(HealthState::Up, Some("not-a-timestamp"), None),
                context()?,
            )?
            .is_none()
        );
        assert!(
            plan_monitor_overdue(
                overdue_monitor(
                    HealthState::Degraded,
                    Some("2026-05-28T19:59:00Z"),
                    Some("not-a-timestamp"),
                ),
                context()?,
            )?
            .is_none()
        );

        Ok(())
    }
}
