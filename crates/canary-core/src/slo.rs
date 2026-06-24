//! Deterministic service SLO class defaults.
//!
//! Canary does not store per-service SLO overrides yet. These defaults give
//! agents stable objectives to reason against without adding a configuration
//! table before the burn-rate surface needs one.

/// Service-level objective metadata attached to service SLI summaries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServiceSloObjective {
    /// Coarse SLO class name.
    pub class: &'static str,
    /// Why Canary assigned this class.
    pub source: &'static str,
    /// Availability target as a ratio from 0.0 to 1.0.
    pub availability_target: f64,
    /// Average latency target in milliseconds.
    pub latency_ms_average_target: u64,
    /// Allowed error events per hour before the service spends its error budget.
    pub error_budget_events_per_hour: u64,
}

/// Default for services with an HTTP target or non-HTTP monitor.
pub const STANDARD_HEALTH_SURFACE: ServiceSloObjective = ServiceSloObjective {
    class: "standard",
    source: "default_health_surface",
    availability_target: 0.995,
    latency_ms_average_target: 1_000,
    error_budget_events_per_hour: 5,
};

/// Default for services that only appear through errors, incidents, or timeline
/// signals and do not yet have a configured health surface.
pub const BEST_EFFORT_SIGNAL_ONLY: ServiceSloObjective = ServiceSloObjective {
    class: "best_effort",
    source: "default_signal_only",
    availability_target: 0.99,
    latency_ms_average_target: 2_500,
    error_budget_events_per_hour: 20,
};

/// Choose the default SLO objective for a service row.
pub fn default_service_slo(has_health_surface: bool) -> ServiceSloObjective {
    if has_health_surface {
        STANDARD_HEALTH_SURFACE
    } else {
        BEST_EFFORT_SIGNAL_ONLY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_service_slo_uses_health_surface_as_the_standard_class_boundary() {
        assert_eq!(default_service_slo(true), STANDARD_HEALTH_SURFACE);
        assert_eq!(default_service_slo(false), BEST_EFFORT_SIGNAL_ONLY);
    }
}
