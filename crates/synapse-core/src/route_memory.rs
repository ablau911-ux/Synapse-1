//! Route memory (#2326): which delivery tier worked for one app control.
//!
//! A delivered action that shows no effect is terminal within one call
//! (at-most-once, #189), so the only way to start the *next* call on a better
//! tier is to remember per control what worked. This module is the pure core:
//! identity, key, row codec, outcome counting, freshness, tier choice and
//! pruning. It reads no clock, OS or storage; the caller passes `now_ns` and
//! the app's `ExeStamp`, and the thin storage layer in `synapse-mcp` does the
//! `CF_KV` reads and writes under `ROUTE_MEMORY_PREFIX`.
//!
//! Safety by construction: `choose_tier_order` returns a permutation of the
//! static order allowed for the call, never adds a tier, and never moves a
//! foreground tier earlier unless `allow_foreground_promotion` is set.

use std::collections::BTreeMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::routines::wilson_lower_bound;

/// `CF_KV` key prefix of every route-memory row (quota entry in
/// `retention::KV_NAMESPACE_QUOTAS`).
pub const ROUTE_MEMORY_PREFIX: &str = "route-memory/v1/";
/// Version written into every row; other versions are rejected on decode.
pub const ROUTE_MEMORY_RECORD_VERSION: u32 = 1;
/// The closed set of tier names a row may hold; anything else is dropped.
pub const ROUTE_MEMORY_TIERS: [&str; 6] = [
    "cdp",
    "uia",
    "postmessage",
    "win32_message",
    "foreground",
    "foreground_keys",
];
/// Tiers that take the human foreground; never promoted by default.
pub const FOREGROUND_TIERS: [&str; 2] = ["foreground", "foreground_keys"];
/// Largest stored row `decode_row` accepts.
pub const ROUTE_MEMORY_MAX_ROW_BYTES: usize = 8 * 1024;

const EXE_SEGMENT_MAX: usize = 64;
const VERB_SEGMENT_MAX: usize = 32;
const LOSSY_HASH_HEX: usize = 8;
const LOSSY_SUFFIX_LEN: usize = 1 + LOSSY_HASH_HEX;
const SIG_HASH_HEX: usize = 16;
const ID_HASH_HEX: usize = 16;
/// Longest key `route_memory_key` can return.
pub const ROUTE_MEMORY_KEY_MAX_LEN: usize = ROUTE_MEMORY_PREFIX.len()
    + EXE_SEGMENT_MAX
    + LOSSY_SUFFIX_LEN
    + 1
    + VERB_SEGMENT_MAX
    + LOSSY_SUFFIX_LEN
    + 1
    + SIG_HASH_HEX;

const MAX_FIELD_CHARS: usize = 128;
const MAX_EXE_CHARS: usize = 260;
const MAX_REASON_CHARS: usize = 64;
const MAX_COUNT: u64 = 1_000_000_000;
const MAX_STREAK: u32 = 1_000_000;
const NS_PER_DAY: u64 = 86_400_000_000_000;
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
const ENV_PREFIX: &str = "SYNAPSE_ROUTE_MEMORY_";

/// Thresholds and switches. `Default` is the shipped tuning; `from_env`
/// overrides any field with `SYNAPSE_ROUTE_MEMORY_<FIELD>` (invalid values
/// keep the default).
#[derive(Clone, Debug, PartialEq)]
pub struct RouteMemoryConfig {
    /// Master switch: off means the static order and no `route_memory` reply.
    pub enabled: bool,
    /// Attempts (ok + fail) a tier needs before it may be promoted.
    pub min_evidence: u64,
    /// Wilson lower bound a promoted tier must reach.
    pub min_confidence: f64,
    /// How much better (Wilson) the promoted tier must be than the one it
    /// overtakes.
    pub min_margin: f64,
    /// A tier whose current fail streak reaches this is never promoted.
    pub max_fail_streak: u32,
    /// Rows not written for longer than this are ignored and reset.
    pub stale_after_days: u64,
    /// When a tier's ok + fail reaches this, both are halved.
    pub count_cap: u64,
    /// Rows kept per app; the oldest beyond this are pruned on write.
    pub max_rows_per_app: usize,
    /// A row stamped further than this in the future is ignored.
    pub future_skew_ns: u64,
    /// Ignored: a foreground tier is never promoted. Kept so existing configuration still parses.
    pub allow_foreground_promotion: bool,
}

impl Default for RouteMemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_evidence: 5,
            min_confidence: 0.5,
            min_margin: 0.2,
            max_fail_streak: 3,
            stale_after_days: 30,
            count_cap: 1_000,
            max_rows_per_app: 512,
            future_skew_ns: 300_000_000_000, // 5 min
            allow_foreground_promotion: false,
        }
    }
}

impl RouteMemoryConfig {
    /// Defaults overridden by `SYNAPSE_ROUTE_MEMORY_*` environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Defaults overridden by `lookup("SYNAPSE_ROUTE_MEMORY_<FIELD>")`.
    #[must_use]
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let defaults = Self::default();
        let read = |field: &str| lookup(&format!("{ENV_PREFIX}{field}"));
        Self {
            enabled: read("ENABLED")
                .and_then(|raw| parse_flag(&raw))
                .unwrap_or(defaults.enabled),
            min_evidence: parse_value(read("MIN_EVIDENCE"))
                .filter(|value: &u64| *value >= 1)
                .unwrap_or(defaults.min_evidence),
            min_confidence: parse_unit(read("MIN_CONFIDENCE")).unwrap_or(defaults.min_confidence),
            min_margin: parse_unit(read("MIN_MARGIN")).unwrap_or(defaults.min_margin),
            max_fail_streak: parse_value(read("MAX_FAIL_STREAK"))
                .filter(|value: &u32| *value >= 1)
                .unwrap_or(defaults.max_fail_streak),
            stale_after_days: parse_value(read("STALE_AFTER_DAYS"))
                .filter(|value: &u64| *value >= 1)
                .unwrap_or(defaults.stale_after_days),
            count_cap: parse_value(read("COUNT_CAP"))
                .filter(|value: &u64| *value >= 2)
                .unwrap_or(defaults.count_cap),
            max_rows_per_app: parse_value(read("MAX_ROWS_PER_APP"))
                .filter(|value: &usize| *value >= 1)
                .unwrap_or(defaults.max_rows_per_app),
            future_skew_ns: parse_value(read("FUTURE_SKEW_NS")).unwrap_or(defaults.future_skew_ns),
            allow_foreground_promotion: read("ALLOW_FOREGROUND_PROMOTION")
                .and_then(|raw| parse_flag(&raw))
                .unwrap_or(defaults.allow_foreground_promotion),
        }
    }
}

fn parse_flag(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

fn parse_value<T: FromStr>(raw: Option<String>) -> Option<T> {
    raw.and_then(|raw| raw.trim().parse().ok())
}

fn parse_unit(raw: Option<String>) -> Option<f64> {
    parse_value::<f64>(raw).filter(|value| (0.0..=1.0).contains(value))
}

/// Stable metadata of one control, as read once per action. Window handles
/// and runtime ids are deliberately absent: they change per window/process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ControlIdentity<'a> {
    /// Process image name of the target app (any case).
    pub exe: &'a str,
    /// Class name of the top-level window.
    pub window_class: &'a str,
    /// Control role / control type.
    pub role: &'a str,
    /// UIA automation id (may be empty).
    pub automation_id: &'a str,
    /// Accessible name (may hold user data; never stored raw).
    pub name: &'a str,
}

/// Which identity part the signature id came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdKind {
    /// A safe automation id stored as is.
    AutomationId,
    /// Hash of an unsafe automation id (path, spaces, too long).
    AutomationIdHash,
    /// Hash of the normalised name (no automation id).
    NameHash,
}

impl IdKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AutomationId => "automation_id",
            Self::AutomationIdHash => "automation_id_hash",
            Self::NameHash => "name_hash",
        }
    }
}

/// Stored identity of a control; never holds a raw name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlSignature {
    pub exe: String,
    pub window_class: String,
    pub role: String,
    pub id_kind: IdKind,
    pub id: String,
}

/// Image file size and mtime of the app binary; a different stamp means the
/// app changed and its stats no longer apply. Pass the same value (e.g.
/// `ExeStamp::default()`) whenever the stamp cannot be read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExeStamp {
    pub size: u64,
    pub mtime_ns: u64,
}

/// Counters for one tier of one control.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierStats {
    /// Delivered and verified.
    pub ok: u64,
    /// Failed on this tier (including delivered with no observed delta).
    pub fail: u64,
    /// Refused before delivery.
    pub refused: u64,
    /// Delivered without verification.
    pub unverified: u64,
    /// Consecutive failures since the last verified delivery.
    pub fail_streak: u32,
    pub last_ts_ns: u64,
    /// Last failure reason code (sanitised, bounded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reason: Option<String>,
}

impl TierStats {
    fn record(&mut self, kind: &OutcomeKind, now_ns: u64, count_cap: u64) {
        match kind {
            OutcomeKind::Verified => {
                self.ok = self.ok.saturating_add(1);
                self.fail_streak = 0;
            }
            OutcomeKind::Failed { reason } => {
                self.fail = self.fail.saturating_add(1);
                self.fail_streak = self.fail_streak.saturating_add(1).min(MAX_STREAK);
                self.last_reason = Some(sanitize_reason(reason));
            }
            OutcomeKind::Refused => self.refused = self.refused.saturating_add(1),
            OutcomeKind::Unverified => self.unverified = self.unverified.saturating_add(1),
        }
        self.last_ts_ns = now_ns;
        let cap = count_cap.max(2);
        if self.ok.saturating_add(self.fail) >= cap {
            self.ok /= 2;
            self.fail /= 2;
        }
        if self.refused >= cap {
            self.refused /= 2;
        }
        if self.unverified >= cap {
            self.unverified /= 2;
        }
    }

    fn clamp(&mut self) {
        self.ok = self.ok.min(MAX_COUNT);
        self.fail = self.fail.min(MAX_COUNT);
        self.refused = self.refused.min(MAX_COUNT);
        self.unverified = self.unverified.min(MAX_COUNT);
        self.fail_streak = self.fail_streak.min(MAX_STREAK);
        self.last_reason = self.last_reason.as_deref().map(sanitize_reason);
    }

    const fn evidence(&self) -> u64 {
        self.ok.saturating_add(self.fail)
    }

    fn confidence(&self) -> f64 {
        wilson_lower_bound(self.ok, self.evidence())
    }
}

/// One route-memory row (JSON in `CF_KV`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlRouteStats {
    pub record_version: u32,
    /// Last write.
    pub ts_ns: u64,
    /// First write of this row (since the last reset).
    pub first_ts_ns: u64,
    pub sig: ControlSignature,
    pub exe_stamp: ExeStamp,
    pub tiers: BTreeMap<String, TierStats>,
}

impl ControlRouteStats {
    fn empty(sig: &ControlSignature, now_ns: u64, exe_stamp: ExeStamp) -> Self {
        Self {
            record_version: ROUTE_MEMORY_RECORD_VERSION,
            ts_ns: now_ns,
            first_ts_ns: now_ns,
            sig: sig.clone(),
            exe_stamp,
            tiers: BTreeMap::new(),
        }
    }
}

/// Why a stored row was not used (the caller then treats it as absent).
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum RowRejected {
    #[error("route memory row is too large")]
    TooLarge,
    #[error("route memory row is not a valid record")]
    Malformed,
    #[error("route memory row version {0} is not supported")]
    UnsupportedVersion(u32),
}

/// Result of one tier attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutcomeKind {
    /// Delivered and the postcondition was verified.
    Verified,
    /// Failed on this tier; `reason` is the reason code.
    Failed { reason: String },
    /// Refused before delivery (no ok/fail change).
    Refused,
    /// Delivered, no verification asked (no ok/fail change).
    Unverified,
}

/// One tier attempt of one action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierOutcome {
    pub tier: String,
    pub kind: OutcomeKind,
}

/// Whether a row may steer the next call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    /// Not written for longer than `stale_after_days`.
    StaleAge,
    /// The app binary changed.
    StaleBinary,
    /// Stamped beyond `future_skew_ns` in the future.
    FutureClock,
}

/// Tier order for one call.
#[derive(Clone, Debug, PartialEq)]
pub struct RoutePlan<'a> {
    /// A permutation of the static order.
    pub order: Vec<&'a str>,
    /// True when memory changed the order.
    pub used: bool,
    /// The promoted first tier when `used`.
    pub tier: Option<&'a str>,
    /// ok + fail of the promoted tier (0 when not used).
    pub evidence_count: u64,
    /// Wilson lower bound of the promoted tier (0 when not used).
    pub confidence: f64,
}

impl<'a> RoutePlan<'a> {
    const fn unchanged(order: Vec<&'a str>) -> Self {
        Self {
            order,
            used: false,
            tier: None,
            evidence_count: 0,
            confidence: 0.0,
        }
    }
}

/// One stored row of an app, for pruning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PruneCandidate {
    pub key: String,
    pub ts_ns: u64,
}

/// True for a tier name in the closed set `ROUTE_MEMORY_TIERS`.
#[must_use]
pub fn is_route_memory_tier(tier: &str) -> bool {
    ROUTE_MEMORY_TIERS.contains(&tier)
}

fn is_foreground_tier(tier: &str) -> bool {
    FOREGROUND_TIERS.contains(&tier)
}

/// Stable signature of a control.
///
/// Exe + window class + role + an id part (safe automation id as is, else a
/// hash of the automation id, else a hash of the name). `None` when there is
/// no exe or no id and no name, so that control gets no memory.
#[must_use]
pub fn control_signature(identity: &ControlIdentity<'_>) -> Option<ControlSignature> {
    let exe = normalise_exe(identity.exe);
    if exe.is_empty() {
        return None;
    }
    let automation_id = identity.automation_id.trim();
    let name = identity.name.trim();
    let (id_kind, id) = if is_safe_automation_id(automation_id) {
        (IdKind::AutomationId, automation_id.to_owned())
    } else if !automation_id.is_empty() {
        (
            IdKind::AutomationIdHash,
            hash_hex(&[normalise_label(automation_id).as_str()], ID_HASH_HEX),
        )
    } else if !name.is_empty() {
        (
            IdKind::NameHash,
            hash_hex(&[normalise_label(name).as_str()], ID_HASH_HEX),
        )
    } else {
        return None;
    };
    Some(ControlSignature {
        exe,
        window_class: bounded(identity.window_class),
        role: bounded(identity.role),
        id_kind,
        id,
    })
}

/// `route-memory/v1/<exe>/<verb>/<sig16>`: sanitised segments, at most
/// `ROUTE_MEMORY_KEY_MAX_LEN` bytes, always inside `exe_key_prefix(exe)`.
#[must_use]
pub fn route_memory_key(sig: &ControlSignature, verb: &str) -> String {
    let digest = hash_hex(
        &[
            sig.exe.as_str(),
            sig.window_class.as_str(),
            sig.role.as_str(),
            sig.id_kind.as_str(),
            sig.id.as_str(),
        ],
        SIG_HASH_HEX,
    );
    let verb = key_segment(&verb.trim().to_lowercase(), VERB_SEGMENT_MAX);
    format!("{}{verb}/{digest}", exe_key_prefix(&sig.exe))
}

/// `route-memory/v1/<exe>/`: the prefix that lists or clears one app.
#[must_use]
pub fn exe_key_prefix(exe: &str) -> String {
    let segment = key_segment(&normalise_exe(exe), EXE_SEGMENT_MAX);
    format!("{ROUTE_MEMORY_PREFIX}{segment}/")
}

/// Decodes a stored row; drops unknown tiers and clamps counters.
///
/// # Errors
/// `TooLarge`, `Malformed` (not JSON or not this record) or
/// `UnsupportedVersion`; the caller then treats the row as absent.
pub fn decode_row(bytes: &[u8]) -> Result<ControlRouteStats, RowRejected> {
    if bytes.len() > ROUTE_MEMORY_MAX_ROW_BYTES {
        return Err(RowRejected::TooLarge);
    }
    let mut stats: ControlRouteStats =
        serde_json::from_slice(bytes).map_err(|_| RowRejected::Malformed)?;
    if stats.record_version != ROUTE_MEMORY_RECORD_VERSION {
        return Err(RowRejected::UnsupportedVersion(stats.record_version));
    }
    stats.tiers.retain(|tier, _| is_route_memory_tier(tier));
    for tier in stats.tiers.values_mut() {
        tier.clamp();
    }
    Ok(stats)
}

/// Encodes a row as JSON.
///
/// # Errors
/// `Malformed` if serialisation fails (not expected for this type).
pub fn encode_row(stats: &ControlRouteStats) -> Result<Vec<u8>, RowRejected> {
    serde_json::to_vec(stats).map_err(|_| RowRejected::Malformed)
}

/// Whether `stats` may steer a call made now against this app binary.
#[must_use]
pub fn stats_freshness(
    stats: &ControlRouteStats,
    now_ns: u64,
    exe_stamp: ExeStamp,
    cfg: &RouteMemoryConfig,
) -> Freshness {
    if stats.ts_ns > now_ns.saturating_add(cfg.future_skew_ns) {
        return Freshness::FutureClock;
    }
    if stats.exe_stamp != exe_stamp {
        return Freshness::StaleBinary;
    }
    if now_ns.saturating_sub(stats.ts_ns) > cfg.stale_after_days.saturating_mul(NS_PER_DAY) {
        return Freshness::StaleAge;
    }
    Freshness::Fresh
}

/// Folds one action's tier outcomes into the row. A missing, stale or
/// foreign (other signature) prior row is replaced by a fresh one; unknown
/// tiers are dropped.
#[must_use]
pub fn apply_outcome(
    prior: Option<ControlRouteStats>,
    sig: &ControlSignature,
    outcomes: &[TierOutcome],
    now_ns: u64,
    exe_stamp: ExeStamp,
    cfg: &RouteMemoryConfig,
) -> ControlRouteStats {
    let mut stats = match prior {
        Some(prior)
            if prior.sig == *sig
                && stats_freshness(&prior, now_ns, exe_stamp, cfg) == Freshness::Fresh =>
        {
            prior
        }
        _ => ControlRouteStats::empty(sig, now_ns, exe_stamp),
    };
    stats.tiers.retain(|tier, _| is_route_memory_tier(tier));
    for outcome in outcomes
        .iter()
        .filter(|outcome| is_route_memory_tier(&outcome.tier))
    {
        stats.tiers.entry(outcome.tier.clone()).or_default().record(
            &outcome.kind,
            now_ns,
            cfg.count_cap,
        );
    }
    stats.record_version = ROUTE_MEMORY_RECORD_VERSION;
    stats.ts_ns = now_ns;
    stats.exe_stamp = exe_stamp;
    stats
}

/// Orders the call's static tiers using the row.
///
/// A later tier overtakes an earlier one only when the later tier is good
/// (evidence, confidence, streak) and the earlier one has failure evidence and
/// is worse by `min_margin`. Disabled, absent or non-fresh rows keep the
/// static order.
#[must_use]
pub fn choose_tier_order<'a>(
    static_order: &[&'a str],
    stats: Option<&ControlRouteStats>,
    now_ns: u64,
    exe_stamp: ExeStamp,
    cfg: &RouteMemoryConfig,
) -> RoutePlan<'a> {
    let mut order = static_order.to_vec();
    let Some(stats) = stats.filter(|stats| {
        cfg.enabled && stats_freshness(stats, now_ns, exe_stamp, cfg) == Freshness::Fresh
    }) else {
        return RoutePlan::unchanged(order);
    };
    for start in 1..order.len() {
        let mut at = start;
        while at > 0 && overtakes(stats, order[at], order[at - 1], cfg) {
            order.swap(at, at - 1);
            at -= 1;
        }
    }
    if order.as_slice() == static_order {
        return RoutePlan::unchanged(order);
    }
    let lead = order.first().copied();
    let lead_stats = lead.and_then(|tier| stats.tiers.get(tier));
    RoutePlan {
        used: true,
        tier: lead,
        evidence_count: lead_stats.map_or(0, TierStats::evidence),
        confidence: lead_stats.map_or(0.0, TierStats::confidence),
        order,
    }
}

fn overtakes(
    stats: &ControlRouteStats,
    later: &str,
    earlier: &str,
    cfg: &RouteMemoryConfig,
) -> bool {
    if !is_route_memory_tier(later) || is_foreground_tier(later) {
        return false;
    }
    let Some(later_stats) = stats.tiers.get(later) else {
        return false;
    };
    if later_stats.evidence() < cfg.min_evidence || later_stats.fail_streak >= cfg.max_fail_streak {
        return false;
    }
    let later_conf = later_stats.confidence();
    if later_conf < cfg.min_confidence {
        return false;
    }
    let Some(earlier_stats) = stats.tiers.get(earlier) else {
        return false;
    };
    if earlier_stats.fail == 0 {
        return false;
    }
    later_conf - earlier_stats.confidence() >= cfg.min_margin
}

/// Keys to delete so `exe` keeps at most `max_rows_per_app` rows: its oldest
/// by `ts_ns` (ties by key). Rows of other apps are never selected.
#[must_use]
pub fn prune_plan(exe: &str, rows: &[PruneCandidate], cfg: &RouteMemoryConfig) -> Vec<String> {
    let prefix = exe_key_prefix(exe);
    let mut owned: Vec<&PruneCandidate> = rows
        .iter()
        .filter(|row| row.key.starts_with(&prefix))
        .collect();
    let cap = cfg.max_rows_per_app.max(1);
    if owned.len() <= cap {
        return Vec::new();
    }
    owned.sort_by(|a, b| a.ts_ns.cmp(&b.ts_ns).then_with(|| a.key.cmp(&b.key)));
    let excess = owned.len() - cap;
    owned
        .into_iter()
        .take(excess)
        .map(|row| row.key.clone())
        .collect()
}

/// Trimmed, lowercased, bounded; idempotent so `exe_key_prefix(sig.exe)`
/// and `exe_key_prefix(raw exe)` agree.
fn normalise_exe(exe: &str) -> String {
    let lowered: String = exe
        .trim()
        .to_lowercase()
        .chars()
        .take(MAX_EXE_CHARS)
        .collect();
    lowered.trim_end().to_owned()
}

fn bounded(text: &str) -> String {
    text.trim().chars().take(MAX_FIELD_CHARS).collect()
}

fn is_safe_automation_id(id: &str) -> bool {
    (1..=MAX_FIELD_CHARS).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-'))
}

/// Trimmed, lowercased, every digit run replaced by `#` ("Item 12" and
/// "Item 97" are the same control class).
fn normalise_label(raw: &str) -> String {
    let mut label = String::with_capacity(raw.len());
    let mut in_digits = false;
    for c in raw.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_digit() {
            if !in_digits {
                label.push('#');
            }
            in_digits = true;
        } else {
            in_digits = false;
            label.push(c);
        }
    }
    label
}

fn sanitize_reason(raw: &str) -> String {
    raw.trim()
        .chars()
        .take(MAX_REASON_CHARS)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// One key segment from already-lowercased text: `[a-z0-9._-]`, no leading
/// dots, at most `max` bytes. Any lossy change appends `~<hash8>` of the
/// input so distinct inputs keep distinct segments.
fn key_segment(normalised: &str, max: usize) -> String {
    let mut segment: String = normalised
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut lossy = segment != normalised;
    let without_dots = segment.trim_start_matches('.');
    if without_dots.len() != segment.len() {
        lossy = true;
        segment = without_dots.to_owned();
    }
    if segment.len() > max {
        segment.truncate(max);
        lossy = true;
    }
    if lossy {
        segment.push('~');
        segment.push_str(&hash_hex(&[normalised], LOSSY_HASH_HEX));
    }
    if segment.is_empty() {
        segment.push('_');
    }
    segment
}

/// First `hex_len` lowercase hex chars of SHA-256 over `parts` joined by 0x1F.
fn hash_hex(parts: &[&str], hex_len: usize) -> String {
    let mut hasher = Sha256::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            hasher.update([0x1F]);
        }
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(hex_len + 1);
    for byte in digest.iter().take(hex_len.div_ceil(2)) {
        hex.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        hex.push(char::from(HEX_DIGITS[usize::from(byte & 0x0F)]));
    }
    hex.truncate(hex_len);
    hex
}
