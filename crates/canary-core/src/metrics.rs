//! Prometheus exposition formatting for Canary runtime snapshots.
//!
//! The metrics endpoint is an operational adapter, but the line format and
//! metric names are still a public contract. Keeping the renderer here gives
//! agents one typed place to extend instead of scattering string formatting
//! across HTTP handlers and store queries.

/// Point-in-time runtime metrics gathered by the store/runtime boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetricsSnapshot {
    /// Persisted error rows.
    pub errors_total: u64,
    /// Pending or retrying webhook deliveries.
    pub webhook_queue_depth: u64,
    /// Webhook delivery ledger counts by status.
    pub webhook_delivery_totals: Vec<LabeledCount>,
    /// Oban-compatible job queue depths.
    pub oban_queue_depths: Vec<LabeledCount>,
    /// Target health-state gauges.
    pub target_states: Vec<HealthStateMetric>,
    /// Monitor health-state gauges.
    pub monitor_states: Vec<HealthStateMetric>,
}

/// Count associated with one label value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabeledCount {
    /// Label value.
    pub label: String,
    /// Count value.
    pub count: u64,
}

/// One health-state gauge sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthStateMetric {
    /// Target or monitor id.
    pub id: String,
    /// Service name.
    pub service: String,
    /// Current state label.
    pub state: String,
    /// Gauge value.
    pub value: u8,
}

/// Render a snapshot in Prometheus text exposition format.
pub fn render_prometheus(snapshot: &MetricsSnapshot) -> String {
    let mut output = String::new();

    metric_header(
        &mut output,
        "canary_errors_total",
        "Persisted Canary error rows.",
        "counter",
    );
    sample(
        &mut output,
        "canary_errors_total",
        &[],
        snapshot.errors_total,
    );

    metric_header(
        &mut output,
        "canary_webhook_queue_depth",
        "Pending or retrying webhook deliveries.",
        "gauge",
    );
    sample(
        &mut output,
        "canary_webhook_queue_depth",
        &[],
        snapshot.webhook_queue_depth,
    );

    metric_header(
        &mut output,
        "canary_webhook_delivery_total",
        "Webhook deliveries by terminal or retry status.",
        "counter",
    );
    for status in &snapshot.webhook_delivery_totals {
        sample(
            &mut output,
            "canary_webhook_delivery_total",
            &[("status", &status.label)],
            status.count,
        );
    }

    metric_header(
        &mut output,
        "canary_oban_queue_depth",
        "Oban-compatible jobs waiting or executing by queue.",
        "gauge",
    );
    for queue in &snapshot.oban_queue_depths {
        sample(
            &mut output,
            "canary_oban_queue_depth",
            &[("queue", &queue.label)],
            queue.count,
        );
    }

    metric_header(
        &mut output,
        "canary_probe_state",
        "Current target probe state gauges.",
        "gauge",
    );
    for target in &snapshot.target_states {
        sample(
            &mut output,
            "canary_probe_state",
            &[
                ("target_id", &target.id),
                ("service", &target.service),
                ("state", &target.state),
            ],
            u64::from(target.value),
        );
    }

    metric_header(
        &mut output,
        "canary_monitor_state",
        "Current non-HTTP monitor state gauges.",
        "gauge",
    );
    for monitor in &snapshot.monitor_states {
        sample(
            &mut output,
            "canary_monitor_state",
            &[
                ("monitor_id", &monitor.id),
                ("service", &monitor.service),
                ("state", &monitor.state),
            ],
            u64::from(monitor.value),
        );
    }

    output
}

fn metric_header(output: &mut String, name: &str, help: &str, metric_type: &str) {
    output.push_str("# HELP ");
    output.push_str(name);
    output.push(' ');
    output.push_str(help);
    output.push('\n');
    output.push_str("# TYPE ");
    output.push_str(name);
    output.push(' ');
    output.push_str(metric_type);
    output.push('\n');
}

fn sample(output: &mut String, name: &str, labels: &[(&str, &str)], value: u64) {
    output.push_str(name);
    if !labels.is_empty() {
        output.push('{');
        for (index, (key, raw_value)) in labels.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            output.push_str(key);
            output.push_str("=\"");
            escape_label_value(output, raw_value);
            output.push('"');
        }
        output.push('}');
    }
    output.push(' ');
    output.push_str(&value.to_string());
    output.push('\n');
}

fn escape_label_value(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            _ => output.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_prometheus_escapes_labels_and_emits_help_type_headers() {
        let body = render_prometheus(&MetricsSnapshot {
            errors_total: 2,
            webhook_queue_depth: 1,
            webhook_delivery_totals: vec![LabeledCount {
                label: "delivered".to_owned(),
                count: 3,
            }],
            oban_queue_depths: vec![LabeledCount {
                label: "webhooks".to_owned(),
                count: 4,
            }],
            target_states: vec![HealthStateMetric {
                id: "TGT-\"quoted\"".to_owned(),
                service: "svc\\name".to_owned(),
                state: "degraded".to_owned(),
                value: 1,
            }],
            monitor_states: Vec::new(),
        });

        assert!(body.contains("# HELP canary_webhook_queue_depth"));
        assert!(body.contains("# TYPE canary_oban_queue_depth gauge"));
        assert!(body.contains("canary_errors_total 2"));
        assert!(body.contains("canary_probe_state{target_id=\"TGT-\\\"quoted\\\"\",service=\"svc\\\\name\",state=\"degraded\"} 1"));
    }
}
