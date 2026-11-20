//! #2323: the process-wide metrics recorder and its bounded readback.
//!
//! The recorder is the in-memory Prometheus recorder: no listener, no port,
//! values live only in this process and start from zero after a restart.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

use crate::metrics::{M3_METRICS, MetricKind, MetricSpec, describe_metric};

pub const RECORDER_SOURCE_OF_TRUTH: &str = "process-global metrics recorder (metrics_exporter_prometheus, in-memory, no listener) read by PrometheusHandle::render";
pub const RECORDER_INSTALL_FAILED: &str = "TELEMETRY_METRICS_RECORDER_FAILED";
/// Longest label value the readback shows; longer values are withheld.
pub const MAX_LABEL_VALUE_CHARS: usize = 64;
const HISTOGRAM_SUFFIXES: &[&str] = &["_sum", "_count", "_bucket"];
const HISTOGRAM_LABELS: &[&str] = &["quantile", "le"];

static RECORDER: OnceLock<Result<PrometheusHandle, String>> = OnceLock::new();

/// Installs the recorder once per process, then describes every registered
/// metric on it (descriptions sent before the install went nowhere).
///
/// # Errors
///
/// Returns a `TELEMETRY_METRICS_RECORDER_FAILED` message when another global
/// recorder was installed first; later calls return the same error.
pub fn install_metrics_recorder() -> Result<(), String> {
    RECORDER
        .get_or_init(|| {
            let handle = PrometheusBuilder::new()
                .install_recorder()
                .map_err(|error| format!("{RECORDER_INSTALL_FAILED}: {error}"))?;
            for spec in M3_METRICS {
                describe_metric(*spec);
            }
            Ok(handle)
        })
        .as_ref()
        .map(|_| ())
        .map_err(String::clone)
}

/// What the daemon's recorder holds right now, bounded by the registry.
#[must_use]
pub fn recorder_readback() -> RecorderReadback {
    match RECORDER.get() {
        Some(Ok(handle)) => {
            let rendered = handle.render();
            RecorderReadback::from_render(Some(rendered.as_str()), None, M3_METRICS)
        }
        Some(Err(error)) => RecorderReadback::from_render(None, Some(error.as_str()), M3_METRICS),
        None => RecorderReadback::from_render(None, None, M3_METRICS),
    }
}

/// What the `telemetry` tool may show from the metrics recorder.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecorderReadback {
    pub installed: bool,
    pub install_error: Option<String>,
    /// Registered metrics with at least one exported sample, sorted.
    pub metric_names: Vec<String>,
    /// Exported sample lines (Prometheus text), sorted.
    pub samples: Vec<String>,
    /// Sample lines withheld: unregistered metric, undeclared label key,
    /// over-long label value, over the metric's series cap, or malformed.
    pub rejected_sample_count: usize,
}

impl RecorderReadback {
    /// Keeps only samples of registered metrics with declared label keys,
    /// short label values and at most `max_label_combinations` series each.
    /// `rendered` is `None` when no recorder is installed.
    #[must_use]
    pub fn from_render(
        rendered: Option<&str>,
        install_error: Option<&str>,
        registry: &[MetricSpec],
    ) -> Self {
        let install_error = install_error.map(str::to_owned);
        let Some(rendered) = rendered else {
            return Self {
                install_error,
                ..Self::default()
            };
        };
        let mut rejected_sample_count = 0;
        let mut candidates = Vec::new();
        for line in rendered.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match accepted_sample(line, registry) {
                Some((spec, series)) => candidates.push((spec, series, line)),
                None => rejected_sample_count += 1,
            }
        }
        candidates.sort_by_key(|candidate| candidate.2);

        let mut series_by_metric: BTreeMap<&str, BTreeSet<Vec<(String, String)>>> = BTreeMap::new();
        let mut metric_names = BTreeSet::new();
        let mut samples = Vec::new();
        for (spec, series, line) in candidates {
            let seen = series_by_metric.entry(spec.name).or_default();
            if !seen.contains(&series) && seen.len() >= usize::from(spec.max_label_combinations) {
                rejected_sample_count += 1;
                continue;
            }
            seen.insert(series);
            metric_names.insert(spec.name.to_owned());
            samples.push(line.to_owned());
        }
        Self {
            installed: true,
            install_error,
            metric_names: metric_names.into_iter().collect(),
            samples,
            rejected_sample_count,
        }
    }
}

/// The registry entry a sample line belongs to and its series key (the
/// declared labels, sorted), or `None` when the line must be withheld.
fn accepted_sample<'r>(
    line: &str,
    registry: &'r [MetricSpec],
) -> Option<(&'r MetricSpec, Vec<(String, String)>)> {
    let (name, labels) = parse_sample(line)?;
    let spec = registry.iter().find(|spec| sample_belongs_to(spec, name))?;
    let mut series = Vec::new();
    for (key, value) in labels {
        if value.chars().count() > MAX_LABEL_VALUE_CHARS {
            return None;
        }
        if spec.labels.contains(&key.as_str()) {
            series.push((key, value));
        } else if !(spec.kind == MetricKind::Histogram && HISTOGRAM_LABELS.contains(&key.as_str()))
        {
            return None;
        }
    }
    series.sort();
    Some((spec, series))
}

fn sample_belongs_to(spec: &MetricSpec, name: &str) -> bool {
    name == spec.name
        || (spec.kind == MetricKind::Histogram
            && name
                .strip_prefix(spec.name)
                .is_some_and(|suffix| HISTOGRAM_SUFFIXES.contains(&suffix)))
}

/// Parses `name{key="value",...} value [timestamp]` (Prometheus text format).
fn parse_sample(line: &str) -> Option<(&str, Vec<(String, String)>)> {
    let name_end = line
        .find(|c: char| c == '{' || c.is_whitespace())
        .unwrap_or(line.len());
    let (name, mut rest) = line.split_at(name_end);
    if !is_name(name, true) {
        return None;
    }
    let mut labels = Vec::new();
    if let Some(after_brace) = rest.strip_prefix('{') {
        let (parsed, after_labels) = parse_labels(after_brace)?;
        labels = parsed;
        rest = after_labels;
    }
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let mut fields = rest.split_whitespace();
    fields.next()?.parse::<f64>().ok()?;
    if let Some(timestamp) = fields.next() {
        timestamp.parse::<i64>().ok()?;
    }
    if fields.next().is_some() {
        return None;
    }
    Some((name, labels))
}

/// Parses `key="value",...}` and returns the labels and the text after `}`.
fn parse_labels(mut text: &str) -> Option<(Vec<(String, String)>, &str)> {
    let mut labels = Vec::new();
    loop {
        if let Some(after) = text.strip_prefix('}') {
            return Some((labels, after));
        }
        let (key, after_key) = text.split_at(text.find('=')?);
        if !is_name(key, false) {
            return None;
        }
        let (value, after_value) = parse_quoted(after_key.strip_prefix("=\"")?)?;
        labels.push((key.to_owned(), value));
        text = if let Some(after_comma) = after_value.strip_prefix(',') {
            after_comma
        } else if after_value.starts_with('}') {
            after_value
        } else {
            return None;
        };
    }
}

/// Reads a label value up to its closing quote, undoing `\\`, `\"` and `\n`.
fn parse_quoted(text: &str) -> Option<(String, &str)> {
    let mut value = String::new();
    let mut chars = text.char_indices();
    while let Some((index, c)) = chars.next() {
        match c {
            '"' => return Some((value, &text[index + 1..])),
            '\\' => match chars.next()?.1 {
                '\\' => value.push('\\'),
                '"' => value.push('"'),
                'n' => value.push('\n'),
                _ => return None,
            },
            _ => value.push(c),
        }
    }
    None
}

/// Prometheus metric names may contain `:`; label names may not.
fn is_name(text: &str, allow_colon: bool) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || (allow_colon && c == ':'))
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || (allow_colon && c == ':'))
}

#[cfg(test)]
#[path = "recorder_table_2323.rs"]
// The frozen table's row tuples are its own; it is never edited.
#[allow(clippy::type_complexity)]
mod recorder_table_2323;
