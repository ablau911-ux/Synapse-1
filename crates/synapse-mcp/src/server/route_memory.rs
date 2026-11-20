//! Route memory wiring (#2326 step B): which delivery tier worked per app
//! control, learned from `act` clicks.
//!
//! A delivered click that shows no effect is terminal within one call
//! (at-most-once, #189), so only memory can start the *next* call on a
//! better tier. `ClickRouteMemory` is the one owner, called twice per native
//! element click inside `target_act`: `plan` before dispatch (it may set the
//! coordinate-first hint on `ActClickParams`) and `record` at the reply tail.
//!
//! The decisions live in the pure core (`synapse_core::route_memory`) and in
//! the pure adapters below; this file adds only the `CF_KV` reads/writes
//! under `route-memory/v1/` and the OS reads of the control's identity.
//! A memory read or write error never fails or changes the action: it
//! simply means "no memory" for that call.

use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rmcp::schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use synapse_core::route_memory::{
    ControlIdentity, ControlRouteStats, ControlSignature, ExeStamp, OutcomeKind, PruneCandidate,
    ROUTE_MEMORY_PREFIX, RouteMemoryConfig, RoutePlan, TierOutcome, apply_outcome,
    choose_tier_order, control_signature, decode_row, encode_row, exe_key_prefix,
    is_route_memory_tier, prune_plan, route_memory_key,
};
use synapse_core::{ElementId, UiaPattern, error_codes};
use synapse_storage::{Db, StorageError, cf};

use super::super::{ErrorData, SynapseService};
use super::{
    ActOperation, ActParams, act_facade_error, reject_act_action, reject_act_owner_approved,
    reject_act_reason, reject_act_ttl,
};
use crate::m1::mcp_error;
use crate::m2::ActClickParams;

/// Most rows `route_memory_status` lists in one reply.
pub(super) const ROUTE_MEMORY_STATUS_LIMIT: usize = 100;

const TIER_CDP: &str = "cdp";
const TIER_UIA: &str = "uia";
const TIER_POSTMESSAGE: &str = "postmessage";
const TIER_FOREGROUND: &str = "foreground";
/// Failure reasons that say the tier cannot drive this control.
const CAPABILITY_FAILURE_REASONS: [&str; 4] = [
    "pattern_unsupported",
    "no_observed_delta",
    "backend_unavailable",
    "selection_only",
];
/// The delivery call did not report back: the input may have landed.
const DELIVERY_UNKNOWN_REASON: &str = "delivery_unknown";
const SCOPE_REMEDIATION: &str =
    "omit exe to address every app, or pass the app's process image name such as notepad.exe";

static ROUTE_MEMORY_CONFIG: LazyLock<RouteMemoryConfig> =
    LazyLock::new(RouteMemoryConfig::from_env);
/// One process-wide lock around every read-modify-write and clear, so two
/// sessions clicking the same control never lose each other's counts.
static ROUTE_MEMORY_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Facts about one click that decide today's implicit tier order.
#[derive(Clone, Copy, Debug)]
pub(super) struct ClickRouteFacts<'a> {
    pub web_element: bool,
    pub coordinate_only: bool,
    pub use_invoke_pattern: bool,
    pub coordinate_fallback_on_unsupported: bool,
    pub role: &'a str,
    pub patterns: &'a [UiaPattern],
}

/// Optional `route_memory` field of a `target_act` reply.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
pub struct RouteMemoryReply {
    /// True when memory changed the tier order of this call.
    pub used: bool,
    /// The promoted first tier when `used`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// ok + fail of the promoted tier (0 when not used).
    pub evidence_count: u64,
    /// Wilson lower bound of the promoted tier (0 when not used).
    pub confidence: f64,
    /// `CF_KV <row key>` of this control's route-memory row.
    pub source_of_truth: String,
}

/// Readback of `act operation=route_memory_status|route_memory_clear`.
#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
pub struct RouteMemoryReadback {
    /// `CF_KV <prefix>`: the rows this readback scanned.
    pub source_of_truth: String,
    pub prefix: String,
    /// Whether route memory steers clicks (`SYNAPSE_ROUTE_MEMORY_ENABLED`).
    pub enabled: bool,
    /// Rows under the prefix, corrupt ones included.
    pub total_rows: u64,
    pub corrupt_rows: u64,
    /// True when more valid rows exist than are listed.
    pub truncated: bool,
    pub rows: Vec<RouteMemoryRowReadback>,
    /// `route_memory_clear` only: rows deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_rows: Option<u64>,
    /// `route_memory_clear` only: rows read back under the prefix after the delete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_rows: Option<u64>,
}

/// One stored control row; never holds a raw accessible name.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct RouteMemoryRowReadback {
    pub key: String,
    pub exe: String,
    pub role: String,
    pub id_kind: String,
    pub id: String,
    pub ts_ns: u64,
    pub tiers: Vec<RouteMemoryTierReadback>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct RouteMemoryTierReadback {
    pub tier: String,
    pub ok: u64,
    pub fail: u64,
    pub refused: u64,
    pub unverified: u64,
    pub fail_streak: u32,
}

/// Route memory of one native element click: planned before dispatch,
/// recorded at the reply tail.
pub(super) struct ClickRouteMemory {
    db: Arc<Db>,
    sig: ControlSignature,
    key: String,
    exe_stamp: ExeStamp,
    row_present: bool,
    plan: RoutePlan<'static>,
    coordinate_first: bool,
}

impl ClickRouteMemory {
    /// Reads the control's identity and row and plans the tier order.
    /// `None` (no memory, unchanged click) when memory is off, the element
    /// is a web or coordinate-only element, or any read fails.
    pub(super) fn plan(
        service: &SynapseService,
        verb: &str,
        element_id: &ElementId,
        params: &ActClickParams,
    ) -> Option<Self> {
        let cfg = &*ROUTE_MEMORY_CONFIG;
        if !cfg.enabled {
            return None;
        }
        // Web elements have one tier (cdp) and coordinate-only elements have
        // no stable identity: part 1 gives neither memory.
        if synapse_a11y::cdp_backend_from_element_id(element_id).is_some()
            || crate::m1::is_browser_ocr_element_id(element_id)
        {
            return None;
        }
        let db = service.m3_storage().ok()?;
        let metadata = synapse_a11y::element_metadata(element_id).ok()?;
        let app = TargetApp::of(element_id)?;
        let sig = control_signature(&ControlIdentity {
            exe: &app.exe,
            // No window-class reader is reachable from here yet (step B
            // decision); the signature is exe + role + id.
            window_class: "",
            role: &metadata.role,
            automation_id: metadata.automation_id.as_deref().unwrap_or(""),
            name: &metadata.name,
        })?;
        let static_order = click_static_tier_order(&ClickRouteFacts {
            web_element: false,
            coordinate_only: false,
            use_invoke_pattern: params.use_invoke_pattern,
            coordinate_fallback_on_unsupported: params.coordinate_fallback_on_unsupported,
            role: &metadata.role,
            patterns: &metadata.patterns,
        });
        let key = route_memory_key(&sig, verb);
        let prior = read_row(&db, &key);
        let plan = choose_tier_order(&static_order, prior.as_ref(), now_ns(), app.stamp, cfg);
        let coordinate_first = click_coordinate_first(&static_order, &plan.order);
        Some(Self {
            db,
            sig,
            key,
            exe_stamp: app.stamp,
            row_present: prior.is_some(),
            plan,
            coordinate_first,
        })
    }

    /// True when the click should enter on postmessage instead of UIA.
    pub(super) const fn coordinate_first(&self) -> bool {
        self.coordinate_first
    }

    /// Folds this click's tier outcomes into the row and returns the reply
    /// field. A write failure is logged and never changes the action reply.
    pub(super) fn record(self, delivery_state: &str, result: &Value) -> Option<RouteMemoryReply> {
        let cfg = &*ROUTE_MEMORY_CONFIG;
        let outcomes = outcomes_from_click_result(delivery_state, result);
        if !outcomes.is_empty()
            && let Err(detail) = self.write(&outcomes, cfg)
        {
            tracing::warn!(
                code = "ROUTE_MEMORY_WRITE_SKIPPED",
                key = %self.key,
                detail = %detail,
                "route memory write skipped; the action result stands"
            );
        }
        route_memory_reply(cfg.enabled, self.row_present, &self.plan, &self.key)
    }

    fn write(&self, outcomes: &[TierOutcome], cfg: &RouteMemoryConfig) -> Result<(), String> {
        let _guard = ROUTE_MEMORY_WRITE_LOCK
            .lock()
            .map_err(|_poisoned| "route memory write lock poisoned".to_owned())?;
        // Re-read under the lock: another session's write since `plan` counts.
        let prior = read_row(&self.db, &self.key);
        let new_row = prior.is_none();
        let stats = apply_outcome(prior, &self.sig, outcomes, now_ns(), self.exe_stamp, cfg);
        let encoded = encode_row(&stats).map_err(|error| error.to_string())?;
        match self
            .db
            .put_batch(cf::CF_KV, [(self.key.clone().into_bytes(), encoded)])
        {
            Ok(()) if new_row => prune_app(&self.db, &self.sig.exe, cfg.max_rows_per_app, cfg),
            Ok(()) => Ok(()),
            Err(StorageError::NamespaceQuotaReached { .. }) => {
                prune_app(&self.db, &self.sig.exe, cfg.max_rows_per_app / 2, cfg)?;
                Err("route memory quota reached; pruned this app and skipped the write".to_owned())
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

/// The target app of an element: process image name and binary stamp.
struct TargetApp {
    exe: String,
    stamp: ExeStamp,
}

impl TargetApp {
    /// `None` when the image path cannot be read: a `pid-N` fallback name
    /// would key memory by process instance, not by app.
    fn of(element_id: &ElementId) -> Option<Self> {
        let hwnd = element_id.parts().ok()?.hwnd;
        let root = synapse_a11y::top_level_root_hwnd(hwnd).ok()?;
        let context = synapse_a11y::foreground_context(root).ok()?;
        let path = Path::new(&context.process_path);
        let exe = path.file_name()?.to_string_lossy().into_owned();
        Some(Self {
            exe,
            stamp: exe_stamp(path),
        })
    }
}

/// Image file size + mtime; `ExeStamp::default()` when unreadable.
fn exe_stamp(path: &Path) -> ExeStamp {
    std::fs::metadata(path).map_or_else(
        |_error| ExeStamp::default(),
        |meta| ExeStamp {
            size: meta.len(),
            mtime_ns: meta
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .and_then(|since| u64::try_from(since.as_nanos()).ok())
                .unwrap_or(0),
        },
    )
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| u64::try_from(since.as_nanos()).ok())
        .unwrap_or(0)
}

/// Exactly one `CF_KV` row (one iterator step); corrupt or absent = `None`.
fn read_row(db: &Db, key: &str) -> Option<ControlRouteStats> {
    let (rows, _more) = db.scan_cf_from(cf::CF_KV, key.as_bytes(), 1).ok()?;
    rows.into_iter()
        .find(|(stored, _value)| stored.as_slice() == key.as_bytes())
        .and_then(|(_key, value)| decode_row(&value).ok())
}

/// Deletes the oldest rows of one app beyond `cap`.
fn prune_app(db: &Db, exe: &str, cap: usize, cfg: &RouteMemoryConfig) -> Result<(), String> {
    let prefix = exe_key_prefix(exe);
    let rows = db
        .scan_cf_prefix(cf::CF_KV, prefix.as_bytes())
        .map_err(|error| error.to_string())?;
    let candidates: Vec<PruneCandidate> = rows
        .into_iter()
        .map(|(key, value)| PruneCandidate {
            key: String::from_utf8_lossy(&key).into_owned(),
            ts_ns: decode_row(&value).map_or(0, |row| row.ts_ns),
        })
        .collect();
    let cfg = RouteMemoryConfig {
        max_rows_per_app: cap.max(1),
        ..cfg.clone()
    };
    let doomed = prune_plan(exe, &candidates, &cfg);
    if doomed.is_empty() {
        return Ok(());
    }
    db.delete_batch(cf::CF_KV, doomed.into_iter().map(String::into_bytes))
        .map_err(|error| error.to_string())
}

pub(super) const fn is_route_memory_operation(operation: ActOperation) -> bool {
    matches!(
        operation,
        ActOperation::RouteMemoryStatus | ActOperation::RouteMemoryClear
    )
}

/// The route-memory operations take only `exe`.
pub(super) fn validate_route_memory_params(
    params: &ActParams,
    operation: ActOperation,
) -> Result<(), ErrorData> {
    reject_act_action(params, operation)?;
    reject_act_reason(params, operation)?;
    reject_act_ttl(params, operation)?;
    reject_act_owner_approved(params, operation)
}

/// `act operation=route_memory_status`: bounded listing for one app or all.
pub(super) fn route_memory_status(
    service: &SynapseService,
    exe: Option<&str>,
) -> Result<RouteMemoryReadback, ErrorData> {
    let prefix = scope_or_error(ActOperation::RouteMemoryStatus, exe)?;
    let db = service.m3_storage()?;
    let rows = db
        .scan_cf_prefix(cf::CF_KV, prefix.as_bytes())
        .map_err(|error| mcp_error(error.code(), error.to_string()))?;
    Ok(status_readback(&prefix, rows, ROUTE_MEMORY_CONFIG.enabled))
}

/// `act operation=route_memory_clear`: deletes one app's rows or all, then
/// reads the prefix back.
pub(super) fn route_memory_clear(
    service: &SynapseService,
    exe: Option<&str>,
) -> Result<RouteMemoryReadback, ErrorData> {
    let prefix = scope_or_error(ActOperation::RouteMemoryClear, exe)?;
    let db = service.m3_storage()?;
    let storage_error = |error: StorageError| mcp_error(error.code(), error.to_string());
    let _guard = ROUTE_MEMORY_WRITE_LOCK.lock().map_err(|_poisoned| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            "route memory write lock poisoned",
        )
    })?;
    let keys: Vec<Vec<u8>> = db
        .scan_cf_prefix(cf::CF_KV, prefix.as_bytes())
        .map_err(storage_error)?
        .into_iter()
        .map(|(key, _value)| key)
        .collect();
    let deleted = u64::try_from(keys.len()).unwrap_or(u64::MAX);
    if !keys.is_empty() {
        db.delete_batch(cf::CF_KV, keys).map_err(storage_error)?;
    }
    let remaining = db
        .scan_cf_prefix(cf::CF_KV, prefix.as_bytes())
        .map_err(storage_error)?;
    let mut readback = status_readback(&prefix, remaining, ROUTE_MEMORY_CONFIG.enabled);
    readback.deleted_rows = Some(deleted);
    readback.remaining_rows = Some(readback.total_rows);
    Ok(readback)
}

fn scope_or_error(operation: ActOperation, exe: Option<&str>) -> Result<String, ErrorData> {
    route_memory_scope(exe)
        .map_err(|detail| act_facade_error(operation, detail, SCOPE_REMEDIATION, "exe"))
}

/// Per-tier outcomes of one click from its `target_act` result (the click
/// reply's `tier_attempts`, or the error's `data.tier_attempts`). One
/// outcome per known tier, in first-attempt order; a capability failure
/// outranks a delivery, a delivery outranks a refusal. A refused or
/// deferred call changes no ok/fail count.
pub(super) fn outcomes_from_click_result(delivery_state: &str, result: &Value) -> Vec<TierOutcome> {
    let Some(attempts) = result
        .get("tier_attempts")
        .or_else(|| result.pointer("/error/data/tier_attempts"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let refused_call = matches!(
        delivery_state,
        "refused_before_delivery" | "deferred_without_delivery"
    );
    let verified_call = matches!(
        delivery_state,
        "delivered_and_verified" | "delivered_and_pixel_verified"
    );
    let mut outcomes: Vec<TierOutcome> = Vec::new();
    for attempt in attempts {
        let tier = attempt.get("tier").and_then(Value::as_str).unwrap_or("");
        let status = attempt.get("status").and_then(Value::as_str).unwrap_or("");
        if !is_route_memory_tier(tier) || !matches!(status, "delivered" | "failed") {
            continue;
        }
        let reason = attempt
            .get("reason_code")
            .and_then(Value::as_str)
            .unwrap_or("");
        let kind = if refused_call {
            OutcomeKind::Refused
        } else if status == "delivered" {
            if verified_call {
                OutcomeKind::Verified
            } else {
                OutcomeKind::Unverified
            }
        } else if CAPABILITY_FAILURE_REASONS.contains(&reason) {
            OutcomeKind::Failed {
                reason: reason.to_owned(),
            }
        } else if reason == DELIVERY_UNKNOWN_REASON {
            OutcomeKind::Unverified
        } else {
            OutcomeKind::Refused
        };
        match outcomes.iter_mut().find(|outcome| outcome.tier == tier) {
            Some(existing) if outcome_rank(&kind) >= outcome_rank(&existing.kind) => {
                existing.kind = kind;
            }
            Some(_) => {}
            None => outcomes.push(TierOutcome {
                tier: tier.to_owned(),
                kind,
            }),
        }
    }
    outcomes
}

const fn outcome_rank(kind: &OutcomeKind) -> u8 {
    match kind {
        OutcomeKind::Refused => 0,
        OutcomeKind::Unverified => 1,
        OutcomeKind::Verified => 2,
        OutcomeKind::Failed { .. } => 3,
    }
}

/// Today's implicit element-click tier order as data (behaviour unchanged):
/// web = cdp; coordinate-only, invoke-pattern off and list items go
/// coordinate-direct; otherwise UIA first, then the coordinate route unless
/// coordinate fallback is off. A control that may need the foreground caret
/// never gets postmessage, so memory can never move foreground earlier.
pub(super) fn click_static_tier_order(facts: &ClickRouteFacts<'_>) -> Vec<&'static str> {
    if facts.web_element {
        return vec![TIER_CDP];
    }
    if facts.coordinate_only {
        return vec![TIER_POSTMESSAGE, TIER_FOREGROUND];
    }
    let coordinate = if click_may_need_foreground_caret(facts.role, facts.patterns) {
        vec![TIER_FOREGROUND]
    } else {
        vec![TIER_POSTMESSAGE, TIER_FOREGROUND]
    };
    if !facts.use_invoke_pattern || facts.role.to_lowercase().contains("list item") {
        return coordinate;
    }
    if !facts.coordinate_fallback_on_unsupported {
        return vec![TIER_UIA];
    }
    let mut order = vec![TIER_UIA];
    order.extend(coordinate);
    order
}

/// Conservative superset of the click path's foreground-caret rule (that
/// rule also needs enabled + keyboard-focusable + a non-empty bbox): a wider
/// match only removes postmessage, i.e. only removes promotions.
fn click_may_need_foreground_caret(role: &str, patterns: &[UiaPattern]) -> bool {
    let role = role.to_lowercase();
    ["edit", "document", "text"]
        .iter()
        .any(|part| role.contains(part))
        || patterns
            .iter()
            .any(|pattern| matches!(pattern, UiaPattern::Value | UiaPattern::Text))
}

/// The click enters on postmessage instead of UIA only when memory put
/// postmessage first over a UIA-first static order.
pub(super) fn click_coordinate_first(static_order: &[&str], plan_order: &[&str]) -> bool {
    static_order.first() == Some(&TIER_UIA) && plan_order.first() == Some(&TIER_POSTMESSAGE)
}

/// The reply field: absent when memory is off or the control has no row.
pub(super) fn route_memory_reply(
    enabled: bool,
    row_present: bool,
    plan: &RoutePlan<'_>,
    key: &str,
) -> Option<RouteMemoryReply> {
    if !enabled || !row_present {
        return None;
    }
    Some(RouteMemoryReply {
        used: plan.used,
        tier: plan.tier.map(str::to_owned),
        evidence_count: plan.evidence_count,
        confidence: plan.confidence,
        source_of_truth: format!("CF_KV {key}"),
    })
}

/// Key prefix addressed by status/clear: one app, or every app when `exe` is
/// omitted. A blank `exe` is rejected, never widened to every app.
pub(super) fn route_memory_scope(exe: Option<&str>) -> Result<String, &'static str> {
    match exe {
        None => Ok(ROUTE_MEMORY_PREFIX.to_owned()),
        Some(exe) if exe.trim().is_empty() => Err(
            "act route memory exe must be a non-empty process image name; omit exe to address every app",
        ),
        Some(exe) => Ok(exe_key_prefix(exe)),
    }
}

/// Bounded listing of scanned rows; corrupt rows are counted, not listed.
pub(super) fn status_readback(
    prefix: &str,
    rows: Vec<(Vec<u8>, Vec<u8>)>,
    enabled: bool,
) -> RouteMemoryReadback {
    let total_rows = u64::try_from(rows.len()).unwrap_or(u64::MAX);
    let mut corrupt_rows = 0_u64;
    let mut valid_rows = 0_u64;
    let mut listed = Vec::new();
    for (key, value) in rows {
        let Ok(stats) = decode_row(&value) else {
            corrupt_rows = corrupt_rows.saturating_add(1);
            continue;
        };
        valid_rows = valid_rows.saturating_add(1);
        if listed.len() < ROUTE_MEMORY_STATUS_LIMIT {
            listed.push(row_readback(&key, stats));
        }
    }
    RouteMemoryReadback {
        source_of_truth: format!("CF_KV {prefix}"),
        prefix: prefix.to_owned(),
        enabled,
        total_rows,
        corrupt_rows,
        truncated: valid_rows > u64::try_from(listed.len()).unwrap_or(u64::MAX),
        rows: listed,
        deleted_rows: None,
        remaining_rows: None,
    }
}

fn row_readback(key: &[u8], stats: ControlRouteStats) -> RouteMemoryRowReadback {
    RouteMemoryRowReadback {
        key: String::from_utf8_lossy(key).into_owned(),
        exe: stats.sig.exe,
        role: stats.sig.role,
        id_kind: stats.sig.id_kind.as_str().to_owned(),
        id: stats.sig.id,
        ts_ns: stats.ts_ns,
        tiers: stats
            .tiers
            .into_iter()
            .map(|(tier, counts)| RouteMemoryTierReadback {
                tier,
                ok: counts.ok,
                fail: counts.fail,
                refused: counts.refused,
                unverified: counts.unverified,
                fail_streak: counts.fail_streak,
            })
            .collect(),
    }
}

#[cfg(test)]
#[path = "route_memory_table_2326.rs"]
mod route_memory_table_2326;
