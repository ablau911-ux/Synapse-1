use serde_json::Value;

use crate::{DataPredicate, Event, EventFilter};

#[must_use]
pub fn matches_event_filter(filter: &EventFilter, event: &Event) -> bool {
    match filter {
        EventFilter::All => true,
        EventFilter::None => false,
        EventFilter::Kind { kind } => event.kind == *kind,
        EventFilter::Source { source } => event.source == *source,
        EventFilter::And { args } => args.iter().all(|item| matches_event_filter(item, event)),
        EventFilter::Or { args } => args.iter().any(|item| matches_event_filter(item, event)),
        EventFilter::Not { arg } => !matches_event_filter(arg, event),
        EventFilter::Data { path, predicate } => {
            matches_data_predicate(predicate, event.data.pointer(path))
        }
    }
}

#[must_use]
pub fn matches_data_predicate(predicate: &DataPredicate, value: Option<&Value>) -> bool {
    match predicate {
        DataPredicate::Exists => value.is_some(),
        DataPredicate::Eq { value: expected } => value == Some(expected),
        DataPredicate::Ne { value: expected } => value.is_some_and(|actual| actual != expected),
        DataPredicate::Lt { value: expected } => {
            compare_values(value, expected).is_some_and(std::cmp::Ordering::is_lt)
        }
        DataPredicate::Le { value: expected } => {
            compare_values(value, expected).is_some_and(std::cmp::Ordering::is_le)
        }
        DataPredicate::Gt { value: expected } => {
            compare_values(value, expected).is_some_and(std::cmp::Ordering::is_gt)
        }
        DataPredicate::Ge { value: expected } => {
            compare_values(value, expected).is_some_and(std::cmp::Ordering::is_ge)
        }
        DataPredicate::Regex { pattern } => value
            .and_then(Value::as_str)
            .is_some_and(|actual| compiled_regex(pattern).is_some_and(|re| re.is_match(actual))),
        DataPredicate::InSet { values } => {
            value.is_some_and(|actual| values.iter().any(|item| item == actual))
        }
    }
}

/// Upper bound on distinct compiled patterns kept; the cache is cleared when
/// exceeded so unbounded caller-supplied patterns cannot grow it forever.
const REGEX_CACHE_CAPACITY: usize = 256;

static REGEX_CACHE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, Option<std::sync::Arc<regex::Regex>>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Compiles `pattern` once and reuses it: `Regex` predicates are evaluated on
/// the event-bus hot path for every event. An invalid pattern is cached as
/// `None` (it never matches) so it is not re-parsed per event either.
fn compiled_regex(pattern: &str) -> Option<std::sync::Arc<regex::Regex>> {
    let mut cache = REGEX_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entry) = cache.get(pattern) {
        return entry.clone();
    }
    if cache.len() >= REGEX_CACHE_CAPACITY {
        cache.clear();
    }
    let compiled = regex::Regex::new(pattern).ok().map(std::sync::Arc::new);
    cache.insert(pattern.to_owned(), compiled.clone());
    compiled
}

fn compare_values(value: Option<&Value>, expected: &Value) -> Option<std::cmp::Ordering> {
    let actual = value?;

    match (actual, expected) {
        (Value::Number(actual), Value::Number(expected)) => {
            actual.as_f64()?.partial_cmp(&expected.as_f64()?)
        }
        (Value::String(actual), Value::String(expected)) => Some(actual.cmp(expected)),
        _ => None,
    }
}
