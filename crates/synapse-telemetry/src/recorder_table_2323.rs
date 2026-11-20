//! #2323 frozen behaviour table (manager-owned, work method v3). Codex and Opus
//! never edit this file. One case per L row of the #2323 table: what the
//! `telemetry` tool may read back from the metrics recorder's render.

use super::RecorderReadback;
use crate::metrics::{M3_METRICS, MetricKind, MetricSpec, Unit};

const EVICTIONS: MetricSpec = MetricSpec {
    name: "cache_evictions_total",
    kind: MetricKind::Counter,
    unit: Some(Unit::Count),
    labels: &["cf", "reason"],
    max_label_combinations: 2,
    label_policy: "table: two series at most",
    description: "table counter",
};
const SESSIONS: MetricSpec = MetricSpec {
    name: "http_active_sessions",
    kind: MetricKind::Gauge,
    unit: Some(Unit::Count),
    labels: &[],
    max_label_combinations: 1,
    label_policy: "table: unlabeled",
    description: "table gauge",
};
const LATENCY: MetricSpec = MetricSpec {
    name: "audio_stt_latency_ms",
    kind: MetricKind::Histogram,
    unit: Some(Unit::Milliseconds),
    labels: &[],
    max_label_combinations: 1,
    label_policy: "table: unlabeled",
    description: "table histogram",
};
const TABLE_REGISTRY: &[MetricSpec] = &[EVICTIONS, SESSIONS, LATENCY];

const EVICTED_CF_EVENTS: &str = r#"cache_evictions_total{cf="CF_EVENTS",reason="soft_cap"} 6"#;

fn installed(names: &[&str], samples: &[&str], rejected: usize) -> RecorderReadback {
    RecorderReadback {
        installed: true,
        install_error: None,
        metric_names: names.iter().map(|name| (*name).to_owned()).collect(),
        samples: samples.iter().map(|sample| (*sample).to_owned()).collect(),
        rejected_sample_count: rejected,
    }
}

#[test]
fn recorder_readback_table_2323() {
    let value_64 = "x".repeat(64);
    let value_65 = "x".repeat(65);
    let at_64 = format!(r#"cache_evictions_total{{cf="CF_EVENTS",reason="{value_64}"}} 1"#);
    let over_64 = format!(r#"cache_evictions_total{{cf="CF_EVENTS",reason="{value_65}"}} 1"#);
    let help_type =
        "# HELP cache_evictions_total Rows evicted.\n# TYPE cache_evictions_total counter\n\n";
    let three_series = concat!(
        r#"cache_evictions_total{cf="CF_C",reason="soft_cap"} 3"#,
        "\n",
        r#"cache_evictions_total{cf="CF_A",reason="soft_cap"} 1"#,
        "\n",
        r#"cache_evictions_total{cf="CF_B",reason="soft_cap"} 2"#,
        "\n",
    );
    let two_series = concat!(
        r#"cache_evictions_total{cf="CF_B",reason="soft_cap"} 2"#,
        "\n",
        r#"cache_evictions_total{cf="CF_A",reason="soft_cap"} 1"#,
        "\n",
    );
    let summary = concat!(
        r#"audio_stt_latency_ms{quantile="0.5"} 12"#,
        "\n",
        "audio_stt_latency_ms_sum 30\n",
        "audio_stt_latency_ms_count 3\n",
    );
    let unordered = format!("http_active_sessions 2\n{EVICTED_CF_EVENTS}\n");

    // (row, rendered by the recorder (None = no recorder), install error, registry, expected)
    let rows: [(
        &str,
        Option<&str>,
        Option<&str>,
        &[MetricSpec],
        RecorderReadback,
    ); 18] = [
        (
            "2323a_normal_registered_counter_exported_unchanged",
            Some(EVICTED_CF_EVENTS),
            None,
            TABLE_REGISTRY,
            installed(&["cache_evictions_total"], &[EVICTED_CF_EVENTS], 0),
        ),
        (
            "2323a_normal_daemon_registry_accepts_storage_eviction_sample",
            Some(EVICTED_CF_EVENTS),
            None,
            M3_METRICS,
            installed(&["cache_evictions_total"], &[EVICTED_CF_EVENTS], 0),
        ),
        (
            "2323b_empty_no_recorder_reports_not_installed_and_no_samples",
            None,
            None,
            TABLE_REGISTRY,
            RecorderReadback::default(),
        ),
        (
            "2323b_empty_install_failed_reports_error_and_no_samples",
            None,
            Some("TELEMETRY_METRICS_RECORDER_FAILED: recorder already set"),
            TABLE_REGISTRY,
            RecorderReadback {
                install_error: Some(
                    "TELEMETRY_METRICS_RECORDER_FAILED: recorder already set".to_owned(),
                ),
                ..RecorderReadback::default()
            },
        ),
        (
            "2323c_empty_installed_nothing_emitted_is_empty_not_fabricated",
            Some(""),
            None,
            TABLE_REGISTRY,
            installed(&[], &[], 0),
        ),
        (
            "2323c_empty_help_type_and_blank_lines_are_not_samples",
            Some(help_type),
            None,
            TABLE_REGISTRY,
            installed(&[], &[], 0),
        ),
        (
            "2323d_boundary_series_at_cap_all_exported_sorted",
            Some(two_series),
            None,
            TABLE_REGISTRY,
            installed(
                &["cache_evictions_total"],
                &[
                    r#"cache_evictions_total{cf="CF_A",reason="soft_cap"} 1"#,
                    r#"cache_evictions_total{cf="CF_B",reason="soft_cap"} 2"#,
                ],
                0,
            ),
        ),
        (
            "2323d_boundary_one_series_over_cap_is_rejected",
            Some(three_series),
            None,
            TABLE_REGISTRY,
            installed(
                &["cache_evictions_total"],
                &[
                    r#"cache_evictions_total{cf="CF_A",reason="soft_cap"} 1"#,
                    r#"cache_evictions_total{cf="CF_B",reason="soft_cap"} 2"#,
                ],
                1,
            ),
        ),
        (
            "2323d_boundary_label_value_of_64_chars_is_exported",
            Some(at_64.as_str()),
            None,
            TABLE_REGISTRY,
            installed(&["cache_evictions_total"], &[at_64.as_str()], 0),
        ),
        (
            "2323e_hostile_label_value_of_65_chars_is_rejected",
            Some(over_64.as_str()),
            None,
            TABLE_REGISTRY,
            installed(&[], &[], 1),
        ),
        (
            "2323e_hostile_unregistered_metric_with_private_path_label_is_rejected",
            Some(r#"audio_events_total{kind="C:\\Users\\owner\\secret.txt"} 1"#),
            None,
            TABLE_REGISTRY,
            installed(&[], &[], 1),
        ),
        (
            "2323e_hostile_undeclared_session_id_label_is_rejected",
            Some(
                r#"cache_evictions_total{cf="CF_EVENTS",reason="soft_cap",session_id="3f2a9c1e"} 1"#,
            ),
            None,
            TABLE_REGISTRY,
            installed(&[], &[], 1),
        ),
        (
            "2323f_invalid_unterminated_label_set_is_rejected",
            Some(r#"cache_evictions_total{cf="CF_EVENTS 1"#),
            None,
            TABLE_REGISTRY,
            installed(&[], &[], 1),
        ),
        (
            "2323f_invalid_missing_value_is_rejected",
            Some(r#"cache_evictions_total{cf="CF_EVENTS",reason="soft_cap"}"#),
            None,
            TABLE_REGISTRY,
            installed(&[], &[], 1),
        ),
        (
            "2323f_invalid_non_numeric_value_is_rejected",
            Some(r#"cache_evictions_total{cf="CF_EVENTS",reason="soft_cap"} six"#),
            None,
            TABLE_REGISTRY,
            installed(&[], &[], 1),
        ),
        (
            "2323g_histogram_summary_lines_exported_under_base_name",
            Some(summary),
            None,
            TABLE_REGISTRY,
            installed(
                &["audio_stt_latency_ms"],
                &[
                    "audio_stt_latency_ms_count 3",
                    "audio_stt_latency_ms_sum 30",
                    r#"audio_stt_latency_ms{quantile="0.5"} 12"#,
                ],
                0,
            ),
        ),
        (
            "2323h_neighbour_output_sorted_whatever_the_render_order",
            Some(unordered.as_str()),
            None,
            TABLE_REGISTRY,
            installed(
                &["cache_evictions_total", "http_active_sessions"],
                &[EVICTED_CF_EVENTS, "http_active_sessions 2"],
                0,
            ),
        ),
        (
            "2323h_neighbour_escaped_quote_and_comma_in_label_value_parsed",
            Some(r#"cache_evictions_total{cf="a\"b,c",reason="soft_cap"} 1"#),
            None,
            TABLE_REGISTRY,
            installed(
                &["cache_evictions_total"],
                &[r#"cache_evictions_total{cf="a\"b,c",reason="soft_cap"} 1"#],
                0,
            ),
        ),
    ];

    let mut failures = Vec::new();
    for (row, rendered, install_error, registry, expected) in rows {
        let got = RecorderReadback::from_render(rendered, install_error, registry);
        if got != expected {
            failures.push(format!("row {row}: got {got:?}, expected {expected:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "#2323 table rows failed:\n{}",
        failures.join("\n")
    );
}
