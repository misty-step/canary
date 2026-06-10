//! Pure health target state machine.
//!
//! This is a direct Rust equivalent of `Canary.Health.StateMachine.transition/4`.
//! It remains side-effect free: webhook delivery and persistence are returned as
//! typed effects for outer modules to interpret.

use std::time::Duration;

use serde::{Deserialize, Serialize};

const FLAP_WINDOW: Duration = Duration::from_secs(10 * 60);
const FLAP_THRESHOLD: usize = 4;

/// Target health state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// No check result has been observed.
    Unknown,
    /// Target is healthy.
    Up,
    /// Target has started failing but has not crossed the down threshold.
    Degraded,
    /// Target is down.
    Down,
    /// Target is administratively paused.
    Paused,
    /// Target is changing state too often in the flap window.
    Flapping,
}

impl HealthState {
    /// Parse the health-state value persisted by Phoenix and Rust stores.
    pub fn parse_persisted(value: &str) -> Option<Self> {
        match value {
            "unknown" => Some(Self::Unknown),
            "up" => Some(Self::Up),
            "degraded" => Some(Self::Degraded),
            "down" => Some(Self::Down),
            "paused" => Some(Self::Paused),
            "flapping" => Some(Self::Flapping),
            _ => None,
        }
    }

    /// Persisted wire/database representation for this state.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Up => "up",
            Self::Degraded => "degraded",
            Self::Down => "down",
            Self::Paused => "paused",
            Self::Flapping => "flapping",
        }
    }

    /// Whether this state keeps a health-transition incident signal active.
    pub const fn incident_signal_active(self) -> bool {
        !matches!(self, Self::Up)
    }

    /// Phoenix-compatible incident activity for a persisted health-state string.
    ///
    /// Unknown persisted values are treated as active, matching Phoenix's
    /// `%{} -> true` branch for any loaded non-`up` state row.
    pub fn persisted_incident_signal_active(value: &str) -> bool {
        Self::parse_persisted(value)
            .map_or(value != Self::Up.as_str(), Self::incident_signal_active)
    }
}

/// Probe event fed into the state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthEvent {
    /// A probe succeeded.
    Success,
    /// A probe failed.
    Failure,
}

/// Transition thresholds configured per target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    /// Failures before `up` becomes `degraded`.
    pub degraded_after: u32,
    /// Failures before `degraded` becomes `down`.
    pub down_after: u32,
    /// Successes before a failing state becomes `up`.
    pub up_after: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            degraded_after: 1,
            down_after: 3,
            up_after: 1,
        }
    }
}

/// Mutable counters persisted outside the pure transition function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counters {
    /// Consecutive failed probes.
    pub consecutive_failures: u32,
    /// Consecutive successful probes.
    pub consecutive_successes: u32,
    /// Recent state transitions as `(new_state, monotonic_millis)`.
    pub transitions: Vec<(HealthState, i64)>,
}

impl Counters {
    /// Initial counters for a new target.
    pub fn initial() -> Self {
        Self {
            consecutive_failures: 0,
            consecutive_successes: 0,
            transitions: Vec::new(),
        }
    }
}

/// Webhook event emitted by a health transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthWebhookEvent {
    /// `health_check.recovered`.
    Recovered,
    /// `health_check.degraded`.
    Degraded,
    /// `health_check.down`.
    Down,
}

impl HealthWebhookEvent {
    /// Event name used on the webhook wire contract.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recovered => "health_check.recovered",
            Self::Degraded => "health_check.degraded",
            Self::Down => "health_check.down",
        }
    }
}

/// Side effects returned as data for the caller to interpret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthEffect {
    /// State changed from one value to another.
    Transition {
        /// Previous state.
        from: HealthState,
        /// New state.
        to: HealthState,
    },
    /// Webhook should be enqueued by the caller.
    Webhook(HealthWebhookEvent),
}

/// Output of a health transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    /// New state.
    pub state: HealthState,
    /// Updated counters.
    pub counters: Counters,
    /// Side effects represented as data.
    pub effects: Vec<HealthEffect>,
}

/// Compute the next state without persistence, network calls, or logging.
pub fn transition(
    current_state: HealthState,
    event: HealthEvent,
    thresholds: Thresholds,
    counters: Counters,
    now_millis: i64,
) -> Transition {
    let mut counters = update_counters(counters, event);
    let mut new_state = compute_next_state(current_state, event, thresholds, &counters);

    if current_state != new_state {
        let (flap_state, flap_counters) =
            detect_flapping(current_state, new_state, counters, now_millis);
        new_state = flap_state;
        counters = flap_counters;
    }

    let effects = if new_state == current_state {
        Vec::new()
    } else {
        let mut effects = vec![HealthEffect::Transition {
            from: current_state,
            to: new_state,
        }];
        if let Some(webhook) = webhook_effect(new_state) {
            effects.push(HealthEffect::Webhook(webhook));
        }
        effects
    };

    Transition {
        state: new_state,
        counters,
        effects,
    }
}

fn update_counters(mut counters: Counters, event: HealthEvent) -> Counters {
    match event {
        HealthEvent::Success => {
            counters.consecutive_successes += 1;
            counters.consecutive_failures = 0;
        }
        HealthEvent::Failure => {
            counters.consecutive_failures += 1;
            counters.consecutive_successes = 0;
        }
    }
    counters
}

fn compute_next_state(
    current_state: HealthState,
    event: HealthEvent,
    thresholds: Thresholds,
    counters: &Counters,
) -> HealthState {
    match (current_state, event) {
        (HealthState::Unknown, HealthEvent::Success) => HealthState::Up,
        (HealthState::Unknown, HealthEvent::Failure) => HealthState::Degraded,
        (HealthState::Up, HealthEvent::Failure)
            if counters.consecutive_failures >= thresholds.degraded_after =>
        {
            HealthState::Degraded
        }
        (HealthState::Up, _) => HealthState::Up,
        (HealthState::Degraded, HealthEvent::Success)
            if counters.consecutive_successes >= thresholds.up_after =>
        {
            HealthState::Up
        }
        (HealthState::Degraded, HealthEvent::Failure)
            if counters.consecutive_failures >= thresholds.down_after =>
        {
            HealthState::Down
        }
        (HealthState::Degraded, _) => HealthState::Degraded,
        (HealthState::Down, HealthEvent::Success)
            if counters.consecutive_successes >= thresholds.up_after =>
        {
            HealthState::Up
        }
        (HealthState::Down, _) => HealthState::Down,
        (HealthState::Paused, _) => HealthState::Paused,
        (HealthState::Flapping, HealthEvent::Success)
            if counters.consecutive_successes >= thresholds.up_after =>
        {
            HealthState::Up
        }
        (HealthState::Flapping, HealthEvent::Failure)
            if counters.consecutive_failures >= thresholds.down_after =>
        {
            HealthState::Down
        }
        (HealthState::Flapping, _) => HealthState::Flapping,
    }
}

fn detect_flapping(
    _old_state: HealthState,
    new_state: HealthState,
    mut counters: Counters,
    now_millis: i64,
) -> (HealthState, Counters) {
    counters.transitions.insert(0, (new_state, now_millis));
    let window_millis = i64::try_from(FLAP_WINDOW.as_millis()).unwrap_or(i64::MAX);
    counters
        .transitions
        .retain(|(_, timestamp)| now_millis - *timestamp < window_millis);

    if counters.transitions.len() >= FLAP_THRESHOLD
        && !matches!(new_state, HealthState::Paused | HealthState::Flapping)
    {
        (HealthState::Flapping, counters)
    } else {
        (new_state, counters)
    }
}

fn webhook_effect(new_state: HealthState) -> Option<HealthWebhookEvent> {
    match new_state {
        HealthState::Up => Some(HealthWebhookEvent::Recovered),
        HealthState::Degraded => Some(HealthWebhookEvent::Degraded),
        HealthState::Down => Some(HealthWebhookEvent::Down),
        HealthState::Unknown | HealthState::Paused | HealthState::Flapping => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    fn run(state: HealthState, event: HealthEvent, counters: Counters) -> Transition {
        transition(state, event, Thresholds::default(), counters, NOW)
    }

    #[test]
    fn unknown_transitions_to_up_on_first_success() {
        let result = run(
            HealthState::Unknown,
            HealthEvent::Success,
            Counters::initial(),
        );
        assert_eq!(result.state, HealthState::Up);
        assert!(result.effects.contains(&HealthEffect::Transition {
            from: HealthState::Unknown,
            to: HealthState::Up,
        }));
    }

    #[test]
    fn unknown_transitions_to_degraded_on_first_failure() {
        let result = run(
            HealthState::Unknown,
            HealthEvent::Failure,
            Counters::initial(),
        );
        assert_eq!(result.state, HealthState::Degraded);
        assert!(result.effects.contains(&HealthEffect::Transition {
            from: HealthState::Unknown,
            to: HealthState::Degraded,
        }));
    }

    #[test]
    fn up_respects_degraded_after_threshold() {
        let thresholds = Thresholds {
            degraded_after: 3,
            ..Thresholds::default()
        };
        let counters = Counters {
            consecutive_failures: 0,
            ..Counters::initial()
        };

        let result = transition(
            HealthState::Up,
            HealthEvent::Failure,
            thresholds,
            counters,
            NOW,
        );

        assert_eq!(result.state, HealthState::Up);
        assert!(result.effects.is_empty());
    }

    #[test]
    fn degraded_transitions_to_down_after_down_threshold() {
        let counters = Counters {
            consecutive_failures: 2,
            ..Counters::initial()
        };
        let result = run(HealthState::Degraded, HealthEvent::Failure, counters);
        assert_eq!(result.state, HealthState::Down);
        assert!(
            result
                .effects
                .contains(&HealthEffect::Webhook(HealthWebhookEvent::Down))
        );
    }

    #[test]
    fn success_resets_failure_counter() {
        let counters = Counters {
            consecutive_failures: 5,
            ..Counters::initial()
        };
        let result = run(HealthState::Down, HealthEvent::Success, counters);
        assert_eq!(result.counters.consecutive_failures, 0);
        assert_eq!(result.counters.consecutive_successes, 1);
    }

    #[test]
    fn failure_resets_success_counter() {
        let counters = Counters {
            consecutive_successes: 3,
            ..Counters::initial()
        };
        let result = run(HealthState::Up, HealthEvent::Failure, counters);
        assert_eq!(result.counters.consecutive_successes, 0);
        assert_eq!(result.counters.consecutive_failures, 1);
    }

    #[test]
    fn detects_flapping_after_four_recent_transitions() {
        let counters = Counters {
            consecutive_failures: 0,
            consecutive_successes: 0,
            transitions: vec![
                (HealthState::Up, NOW - 1_000),
                (HealthState::Down, NOW - 2_000),
                (HealthState::Up, NOW - 3_000),
            ],
        };

        let result = transition(
            HealthState::Down,
            HealthEvent::Success,
            Thresholds::default(),
            counters,
            NOW,
        );

        assert_eq!(result.state, HealthState::Flapping);
        assert!(result.effects.contains(&HealthEffect::Transition {
            from: HealthState::Down,
            to: HealthState::Flapping,
        }));
    }

    #[test]
    fn incident_signal_activity_matches_phoenix_non_up_contract() {
        assert!(!HealthState::Up.incident_signal_active());

        for state in [
            HealthState::Unknown,
            HealthState::Degraded,
            HealthState::Down,
            HealthState::Paused,
            HealthState::Flapping,
        ] {
            assert!(
                state.incident_signal_active(),
                "{state:?} should stay active"
            );
            assert!(HealthState::persisted_incident_signal_active(
                state.as_str()
            ));
        }

        assert!(!HealthState::persisted_incident_signal_active("up"));
        assert!(HealthState::persisted_incident_signal_active(
            "vendor-new-state"
        ));
    }
}
