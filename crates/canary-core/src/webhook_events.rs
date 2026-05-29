//! Webhook event names accepted by Canary's subscription API.

const BUSINESS_EVENTS: [&str; 10] = [
    "health_check.degraded",
    "health_check.down",
    "health_check.recovered",
    "health_check.tls_expiring",
    "error.new_class",
    "error.regression",
    "incident.opened",
    "incident.updated",
    "incident.resolved",
    "annotation.added",
];
const DIAGNOSTIC_EVENTS: [&str; 1] = ["canary.ping"];

/// Return true when an event name is accepted by the webhook subscription API.
pub fn valid(event: &str) -> bool {
    BUSINESS_EVENTS.contains(&event) || DIAGNOSTIC_EVENTS.contains(&event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_events_match_phoenix_business_and_diagnostic_names() {
        for event in BUSINESS_EVENTS.into_iter().chain(DIAGNOSTIC_EVENTS) {
            assert!(valid(event));
        }

        assert!(!valid("bogus.event"));
    }
}
