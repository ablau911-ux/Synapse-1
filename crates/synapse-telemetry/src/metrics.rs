use std::sync::Once;

pub use ::metrics::{
    Unit, counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram,
};

pub const CARDINALITY_LIMIT: u16 = 1_000;

pub use synapse_core::metric_labels::{SUBSCRIBER_SLOT_COUNT, subscriber_slot_label};

pub const EVENTS_DROPPED_FOR_SUBSCRIBER: &str = "events_dropped_for_subscriber";
pub const CACHE_EVICTIONS_TOTAL: &str = "cache_evictions_total";
pub const PROFILES_ACTIVE: &str = "profiles_active";
pub const PROFILE_RELOADS_TOTAL: &str = "profile_reloads_total";
pub const AUDIO_LOOPBACK_UNDERRUNS_TOTAL: &str = "audio_loopback_underruns_total";
pub const AUDIO_STT_INFERENCES_TOTAL: &str = "audio_stt_inferences_total";
pub const AUDIO_STT_LATENCY_MS: &str = "audio_stt_latency_ms";
pub const HTTP_REQUESTS_TOTAL: &str = "http_requests_total";
pub const HTTP_ACTIVE_SESSIONS: &str = "http_active_sessions";
pub const SSE_ACTIVE_SUBSCRIBERS: &str = "sse_active_subscribers";
pub const SSE_BUFFER_OVERFLOWS_TOTAL: &str = "sse_buffer_overflows_total";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetricKind {
    Counter,
    Gauge,
    Histogram,
}

impl MetricKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::Gauge => "gauge",
            Self::Histogram => "histogram",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MetricSpec {
    pub name: &'static str,
    pub kind: MetricKind,
    pub unit: Option<Unit>,
    pub labels: &'static [&'static str],
    pub max_label_combinations: u16,
    pub label_policy: &'static str,
    pub description: &'static str,
}

impl MetricSpec {
    #[must_use]
    pub const fn has_bounded_cardinality(self) -> bool {
        self.max_label_combinations < CARDINALITY_LIMIT
    }

    #[must_use]
    pub fn unit_name(self) -> &'static str {
        self.unit.map_or("none", |unit| unit.as_str())
    }
}

pub const M3_METRICS: &[MetricSpec] = &[
    MetricSpec {
        name: EVENTS_DROPPED_FOR_SUBSCRIBER,
        kind: MetricKind::Counter,
        unit: Some(Unit::Count),
        labels: &["subscriber_slot"],
        max_label_combinations: 64,
        label_policy: "subscriber_slot is a stable hash bucket 0..63 of the subscription id (see subscriber_slot_label), never the raw UUID",
        description: "Events dropped by a bounded per-subscriber event queue.",
    },
    MetricSpec {
        name: CACHE_EVICTIONS_TOTAL,
        kind: MetricKind::Counter,
        unit: Some(Unit::Count),
        labels: &["cf", "reason"],
        max_label_combinations: 64,
        label_policy: "column-family closed set by eviction reason closed set.",
        description: "Rows evicted from storage caches or column-family retention.",
    },
    MetricSpec {
        name: PROFILES_ACTIVE,
        kind: MetricKind::Gauge,
        unit: Some(Unit::Count),
        labels: &["profile_id"],
        max_label_combinations: 128,
        label_policy: "bundled/operator profile ID set capped by loaded profile count.",
        description: "Active profile marker, 1 for active and 0 for inactive.",
    },
    MetricSpec {
        name: PROFILE_RELOADS_TOTAL,
        kind: MetricKind::Counter,
        unit: Some(Unit::Count),
        labels: &["profile_id", "outcome"],
        max_label_combinations: 256,
        label_policy: "loaded profile ID cap by reload outcome closed set.",
        description: "Profile reload attempts by profile and outcome.",
    },
    MetricSpec {
        name: AUDIO_LOOPBACK_UNDERRUNS_TOTAL,
        kind: MetricKind::Counter,
        unit: Some(Unit::Count),
        labels: &[],
        max_label_combinations: 1,
        label_policy: "unlabeled loopback underrun counter.",
        description: "Audio loopback underruns observed while reading the ring.",
    },
    MetricSpec {
        name: AUDIO_STT_INFERENCES_TOTAL,
        kind: MetricKind::Counter,
        unit: Some(Unit::Count),
        labels: &["outcome"],
        max_label_combinations: 8,
        label_policy: "STT outcome closed set.",
        description: "Speech-to-text inference attempts by outcome.",
    },
    MetricSpec {
        name: AUDIO_STT_LATENCY_MS,
        kind: MetricKind::Histogram,
        unit: Some(Unit::Milliseconds),
        labels: &[],
        max_label_combinations: 1,
        label_policy: "unlabeled STT latency distribution.",
        description: "Speech-to-text inference latency in milliseconds.",
    },
    MetricSpec {
        name: HTTP_REQUESTS_TOTAL,
        kind: MetricKind::Counter,
        unit: Some(Unit::Count),
        labels: &["path", "status"],
        max_label_combinations: 64,
        label_policy: "normalized route path by status-code class/closed status set.",
        description: "HTTP transport requests by normalized path and status.",
    },
    MetricSpec {
        name: HTTP_ACTIVE_SESSIONS,
        kind: MetricKind::Gauge,
        unit: Some(Unit::Count),
        labels: &[],
        max_label_combinations: 1,
        label_policy: "unlabeled active HTTP session gauge.",
        description: "Currently active streamable HTTP MCP sessions.",
    },
    MetricSpec {
        name: SSE_ACTIVE_SUBSCRIBERS,
        kind: MetricKind::Gauge,
        unit: Some(Unit::Count),
        labels: &[],
        max_label_combinations: 1,
        label_policy: "unlabeled active SSE subscriber gauge.",
        description: "Currently active SSE event subscribers.",
    },
    MetricSpec {
        name: SSE_BUFFER_OVERFLOWS_TOTAL,
        kind: MetricKind::Counter,
        unit: Some(Unit::Count),
        labels: &[],
        max_label_combinations: 1,
        label_policy: "unlabeled SSE ring overflow counter.",
        description: "SSE ring buffer overflows.",
    },
];

static REGISTER_M3_METRICS: Once = Once::new();

pub fn register_m3_metrics() {
    REGISTER_M3_METRICS.call_once(|| {
        for spec in M3_METRICS {
            describe_metric(*spec);
            tracing::info!(
                code = "M3_METRIC_REGISTERED",
                metric_name = spec.name,
                metric_kind = spec.kind.as_str(),
                unit = spec.unit_name(),
                labels = ?spec.labels,
                max_label_combinations = spec.max_label_combinations,
                label_policy = spec.label_policy,
                "M3 metric registered"
            );
        }
        tracing::info!(
            code = "M3_METRICS_REGISTERED",
            metric_count = M3_METRICS.len(),
            cardinality_limit = CARDINALITY_LIMIT,
            "M3 metric registry ready"
        );
    });
}

#[must_use]
pub const fn m3_metric_specs() -> &'static [MetricSpec] {
    M3_METRICS
}

pub(crate) fn describe_metric(spec: MetricSpec) {
    match (spec.kind, spec.unit) {
        (MetricKind::Counter, Some(unit)) => describe_counter!(spec.name, unit, spec.description),
        (MetricKind::Counter, None) => describe_counter!(spec.name, spec.description),
        (MetricKind::Gauge, Some(unit)) => describe_gauge!(spec.name, unit, spec.description),
        (MetricKind::Gauge, None) => describe_gauge!(spec.name, spec.description),
        (MetricKind::Histogram, Some(unit)) => {
            describe_histogram!(spec.name, unit, spec.description);
        }
        (MetricKind::Histogram, None) => describe_histogram!(spec.name, spec.description),
    }
}

#[cfg(test)]
#[path = "subscriber_slot_tests.rs"]
mod subscriber_slot_tests;
