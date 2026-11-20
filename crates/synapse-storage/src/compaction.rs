use std::time::{SystemTime, UNIX_EPOCH};

use rocksdb::{CompactionDecision, Options};
use synapse_core::retention::{DEFAULTS, RetentionTtl};

const NANOS_PER_SECOND: u64 = 1_000_000_000;
const SECONDS_PER_HOUR: u64 = 60 * 60;
const HOURS_PER_DAY: u64 = 24;
const TTL_UNPARSEABLE_ROWS_TOTAL: &str = "storage_ttl_unparseable_rows_total";
// Cold SSTs need an expiry pass, but rewriting a CF every day is wasteful when
// its TTL is weeks long. The period scales with the TTL (a quarter of it),
// clamped so short-TTL families still sweep hourly and long ones at least
// monthly.
const PERIODIC_TTL_DIVISOR: u64 = 4;
const MIN_PERIODIC_COMPACTION_SECONDS: u64 = SECONDS_PER_HOUR;
const MAX_PERIODIC_COMPACTION_SECONDS: u64 = 30 * HOURS_PER_DAY * SECONDS_PER_HOUR;
const TS_PREFIX_KEY_LEN: usize = 12;
const REFLEX_AUDIT_TS_DIGITS: usize = 20;

pub(crate) fn install_ttl_filter(options: &mut Options, cf_name: &'static str) {
    let Some(ttl_ns) = ttl_ns_for_cf(cf_name) else {
        return;
    };
    options.set_periodic_compaction_seconds(periodic_compaction_seconds(ttl_ns));
    let filter_name = format!("synapse_ttl_{cf_name}");
    options.set_compaction_filter(&filter_name, move |_level, key, value| {
        ttl_decision_observed(cf_name, ttl_ns, current_time_ns(), key, value)
    });
}

pub(crate) const fn periodic_compaction_seconds(ttl_ns: u64) -> u64 {
    let quarter = ttl_ns / NANOS_PER_SECOND / PERIODIC_TTL_DIVISOR;
    if quarter < MIN_PERIODIC_COMPACTION_SECONDS {
        MIN_PERIODIC_COMPACTION_SECONDS
    } else if quarter > MAX_PERIODIC_COMPACTION_SECONDS {
        MAX_PERIODIC_COMPACTION_SECONDS
    } else {
        quarter
    }
}

/// How a TTL column family embeds the write time in its row key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeyTimestampCodec {
    /// `ts_ns (8 bytes BE) || seq (4 bytes BE)`, exactly 12 bytes.
    BigEndianPrefix,
    /// `{reflex_id}:{ts_ns:020}:{audit_id}` text key.
    ReflexAuditText,
}

impl KeyTimestampCodec {
    pub(crate) fn for_cf(cf_name: &str) -> Option<Self> {
        match cf_name {
            "CF_EVENTS" | "CF_OBSERVATIONS" | "CF_ACTION_LOG" | "CF_TIMELINE" | "CF_EPISODES"
            | "CF_AGENT_EVENTS" => Some(Self::BigEndianPrefix),
            "CF_REFLEX_AUDIT" => Some(Self::ReflexAuditText),
            _ => None,
        }
    }

    pub(crate) fn key_ts_ns(self, key: &[u8]) -> Option<u64> {
        match self {
            Self::BigEndianPrefix => {
                if key.len() != TS_PREFIX_KEY_LEN {
                    return None;
                }
                let bytes: [u8; 8] = key.get(..8)?.try_into().ok()?;
                Some(u64::from_be_bytes(bytes))
            }
            Self::ReflexAuditText => {
                let text = std::str::from_utf8(key).ok()?;
                let field = text.split(':').nth(1)?;
                if field.len() != REFLEX_AUDIT_TS_DIGITS
                    || !field.bytes().all(|b| b.is_ascii_digit())
                {
                    return None;
                }
                field.parse().ok()
            }
        }
    }
}

/// Root-envelope `ts_ns` wins; rows without a usable value timestamp fall back
/// to the timestamp embedded in the key (when the CF's key format has one) so
/// they still expire instead of living forever.
pub(crate) fn resolve_timestamp(cf_name: &str, key: &[u8], value: &[u8]) -> RetentionTimestamp {
    let from_value = extract_ts_ns(value);
    if matches!(from_value, RetentionTimestamp::Valid(_)) {
        return from_value;
    }
    KeyTimestampCodec::for_cf(cf_name)
        .and_then(|codec| codec.key_ts_ns(key))
        .map_or(from_value, RetentionTimestamp::Valid)
}

/// Per-CF contract for the TTL compaction filter (#43). Every CF in
/// `retention::DEFAULTS` with a finite TTL gets the filter; a row expires only
/// if it carries a root `ts_ns` (value) or a key-embedded timestamp
/// ([`KeyTimestampCodec`]). A row with neither is a "legacy" row: it is kept
/// and counted in `storage_ttl_unparseable_rows_total{reason="missing"}`.
///
/// - `CF_PROCESS_HISTORY` (6 h): sole writer `m4::launch_process_history_row`
///   stamps root `ts_ns` = insert time. Rows written before #43 have none and
///   are kept (finite set, tiny, still bounded by the 50/100 MB GC caps).
/// - `CF_AGENT_TRANSCRIPTS` (30 d): both writers (`ambient_agents`,
///   `agent_transcripts`) emit `AgentTranscriptRecord`, whose required root
///   `ts_ns` (ingestion time, `validate()` rejects 0) is the TTL anchor.
///   Nothing to stamp.
/// - `CF_OCR_CACHE` (1 h): every row is a DPAPI-sealed envelope with a
///   plaintext root `ts_ns` (`ocr_cache_seal`, #52); legacy plaintext rows
///   (no `ts_ns`) are deleted at daemon startup by
///   `purge_legacy_plaintext_rows`. Nothing to stamp.
/// - `CF_TELEMETRY` (6 h): reserved. No code writes this CF today; a future
///   writer MUST put a root `ts_ns` (insert time) in every value.
/// - `CF_SESSIONS`: NOT compaction-expired. The CF holds live state keyed by
///   session id: lease rows, session-target rows, CDP target owners, tool
///   profiles and the `session/v1/` audit row. They are removed by explicit
///   reconcile/delete paths and none carry a root `ts_ns`; a timestamp TTL
///   could delete the lease or target of a still-live session. Listed in
///   [`COMPACTION_TTL_EXEMPT_CFS`] so a `ts_ns` added to one row kind later
///   cannot silently start expiring the others. Size is bounded by GC caps.
const COMPACTION_TTL_EXEMPT_CFS: [&str; 1] = ["CF_SESSIONS"];

pub(crate) fn ttl_ns_for_cf(cf_name: &str) -> Option<u64> {
    if COMPACTION_TTL_EXEMPT_CFS.contains(&cf_name) {
        return None;
    }
    DEFAULTS
        .iter()
        .find(|default| default.cf == cf_name)
        .and_then(|default| ttl_to_ns(default.ttl))
}

#[cfg(test)]
pub(crate) fn ttl_decision(ttl_ns: u64, now_ns: u64, value: &[u8]) -> CompactionDecision {
    extract_ts_ns(value).decision(ttl_ns, now_ns)
}

fn ttl_decision_observed(
    cf_name: &'static str,
    ttl_ns: u64,
    now_ns: u64,
    key: &[u8],
    value: &[u8],
) -> CompactionDecision {
    let timestamp = resolve_timestamp(cf_name, key, value);
    if let Some(reason) = timestamp.invalid_reason() {
        // Count compaction encounters, not unique rows: fail-closed records can
        // be revisited by later compactions. Labels remain bounded to the known
        // retention CF set and these two fixed reasons.
        synapse_telemetry::metrics::counter!(
            TTL_UNPARSEABLE_ROWS_TOTAL,
            "cf" => cf_name,
            "reason" => reason
        )
        .increment(1);
    }
    timestamp.decision(ttl_ns, now_ns)
}

#[cfg(test)]
#[path = "compaction_key_tests.rs"]
mod key_tests;

#[cfg(test)]
#[path = "compaction_live_state_tests.rs"]
mod live_state_tests;

fn ttl_to_ns(ttl: RetentionTtl) -> Option<u64> {
    match ttl {
        RetentionTtl::None | RetentionTtl::LruOnly => None,
        RetentionTtl::Hours(hours) => hours
            .checked_mul(SECONDS_PER_HOUR)?
            .checked_mul(NANOS_PER_SECOND),
        RetentionTtl::Days(days) => days
            .checked_mul(HOURS_PER_DAY)?
            .checked_mul(SECONDS_PER_HOUR)?
            .checked_mul(NANOS_PER_SECOND),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RetentionTimestamp {
    Valid(u64),
    Missing,
    Invalid,
}

impl RetentionTimestamp {
    pub(crate) const fn invalid_reason(self) -> Option<&'static str> {
        match self {
            Self::Valid(_) => None,
            Self::Missing => Some("missing"),
            Self::Invalid => Some("invalid"),
        }
    }

    const fn decision(self, ttl_ns: u64, now_ns: u64) -> CompactionDecision {
        match self {
            Self::Valid(ts_ns) if now_ns.saturating_sub(ts_ns) > ttl_ns => {
                CompactionDecision::Remove
            }
            Self::Valid(_) | Self::Missing | Self::Invalid => CompactionDecision::Keep,
        }
    }
}

// Only the record envelope owns retention time. Serde ignores nested payloads
// without allocating them and rejects malformed or duplicate root fields.
pub(crate) fn extract_ts_ns(value: &[u8]) -> RetentionTimestamp {
    #[derive(serde::Deserialize)]
    struct TimestampEnvelope {
        ts_ns: Option<u64>,
    }

    serde_json::from_slice::<TimestampEnvelope>(value).map_or(
        RetentionTimestamp::Invalid,
        |record| {
            record
                .ts_ns
                .map_or(RetentionTimestamp::Missing, RetentionTimestamp::Valid)
        },
    )
}

fn current_time_ns() -> u64 {
    #[cfg(test)]
    {
        let fixed = TEST_NOW_NS.load(std::sync::atomic::Ordering::SeqCst);
        if fixed != 0 {
            return fixed;
        }
    }

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
static TEST_NOW_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// The fixed test clock is process-global because RocksDB runs the TTL compaction
// filter on background threads that read `current_time_ns()`. Without serialization,
// parallel `cargo test` threads race on it (one guard's drop resets the clock to 0
// mid-compaction in another test), producing flaky TTL eviction. This lock is held
// for the lifetime of each guard so only one test drives the clock at a time.
#[cfg(test)]
static TEST_CLOCK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) struct TestClockGuard {
    previous: u64,
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for TestClockGuard {
    fn drop(&mut self) {
        TEST_NOW_NS.store(self.previous, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
pub(crate) fn set_test_now_ns(now_ns: u64) -> TestClockGuard {
    let lock = match TEST_CLOCK_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let previous = TEST_NOW_NS.swap(now_ns, std::sync::atomic::Ordering::SeqCst);
    TestClockGuard {
        previous,
        _lock: lock,
    }
}
