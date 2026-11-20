use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Utc};
use rmcp::{ErrorData, schemars::JsonSchema};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use synapse_core::{
    AudioContext, Event, EventSource, ForbiddenRawDataKind, Observation, RealityAudit,
    RealityBaseline, RealityBaselineStatus, RealityDelta, RealityDriftItem, RealityDriftStatus,
    RealitySourceSurface, RealityTargetKind, RealityTargetRef, RedactionSummary, SourceRef,
    error_codes,
};
use synapse_perception::ObservationAssembler;
use synapse_storage::cf;

use super::{
    Json, ObserveParams, Parameters, SynapseService, observe_include, populate_audio_summary,
    populate_clipboard_summary, populate_fs_recent, tool, tool_router,
};
use crate::{
    m1::{ObserveSlot, mcp_error},
    m3::permissions::{Permission, RequiredPermissions, required},
};

const REALITY_BASELINE_TOOL: &str = "reality_baseline";
const OBSERVE_DELTA_TOOL: &str = "observe_delta";
const REALITY_AUDIT_TOOL: &str = "reality_audit";
const REALITY_EVENT_KIND: &str = "reality_delta";
const DEFAULT_DEPTH: u32 = 2;
const DEFAULT_MAX_ELEMENTS: usize = 60;
const DEFAULT_MAX_DELTAS: u32 = 64;
const MAX_DEPTH: u32 = 6;
const MAX_ELEMENTS: usize = 500;
const MAX_DELTAS: u32 = 256;
/// Prefix of keyed content digests persisted in reality rows (#136).
const KEYED_HASH_PREFIX: &str = "hmac-sha256:";
const UIA_STRUCTURE_COALESCE_THRESHOLD: usize = 8;
const UIA_STRUCTURE_ID_CAP: usize = 32;
const SCHEMA_VERSION: u32 = 1;
const UNPROFILED_PROFILE_KEY: &str = "unprofiled";

static NEXT_REALITY_EPOCH_SEQ: AtomicU64 = AtomicU64::new(1);
static NEXT_REALITY_AUDIT_SEQ: AtomicU64 = AtomicU64::new(1);
static NEXT_REALITY_EVENT_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealityBaselineParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch_id: Option<String>,
    #[serde(default = "default_false")]
    #[schemars(default = "default_false")]
    pub force_new_epoch: bool,
    #[serde(default = "default_include")]
    #[schemars(default = "default_include")]
    pub include: Vec<ObserveSlot>,
    #[serde(default = "default_depth")]
    #[schemars(default = "default_depth")]
    #[schemars(range(min = 1, max = 6))]
    pub depth: u32,
    #[serde(default = "default_max_elements")]
    #[schemars(default = "default_max_elements")]
    #[schemars(range(min = 1, max = 500))]
    pub max_elements: usize,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObserveDeltaParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_epoch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_seq: Option<u64>,
    #[serde(default = "default_include")]
    #[schemars(default = "default_include")]
    pub include: Vec<ObserveSlot>,
    #[serde(default = "default_depth")]
    #[schemars(default = "default_depth")]
    #[schemars(range(min = 1, max = 6))]
    pub depth: u32,
    #[serde(default = "default_max_elements")]
    #[schemars(default = "default_max_elements")]
    #[schemars(range(min = 1, max = 500))]
    pub max_elements: usize,
    #[serde(default = "default_max_deltas")]
    #[schemars(default = "default_max_deltas")]
    #[schemars(range(min = 1, max = 256))]
    pub max_deltas: u32,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealityAuditParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assumption_hash: Option<String>,
    #[serde(default = "default_include")]
    #[schemars(default = "default_include")]
    pub include: Vec<ObserveSlot>,
    #[serde(default = "default_depth")]
    #[schemars(default = "default_depth")]
    #[schemars(range(min = 1, max = 6))]
    pub depth: u32,
    #[serde(default = "default_max_elements")]
    #[schemars(default = "default_max_elements")]
    #[schemars(range(min = 1, max = 500))]
    pub max_elements: usize,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct RealityBaselineResponse {
    pub ok: bool,
    pub created: bool,
    pub profile_key: String,
    pub baseline: RealityBaseline,
    pub baseline_required: bool,
    pub rebase_required: bool,
    pub reason: Option<String>,
    pub head: RealityHeadRow,
    pub readback_rows: Vec<RealityRowReadback>,
    pub size_bytes: u32,
    pub size_estimate_tokens: u32,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObserveDeltaResponse {
    pub ok: bool,
    pub profile_key: Option<String>,
    pub epoch_id: Option<String>,
    pub from_seq: Option<u64>,
    pub to_seq: Option<u64>,
    pub deltas: Vec<RealityDelta>,
    pub cursor: Option<RealityCursor>,
    pub baseline_required: bool,
    pub rebase_required: bool,
    pub reason: Option<String>,
    pub readback_rows: Vec<RealityRowReadback>,
    pub published_sse_events: u32,
    pub size_bytes: u32,
    pub size_estimate_tokens: u32,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealityAuditResponse {
    pub ok: bool,
    pub profile_key: String,
    pub audit: RealityAudit,
    pub baseline_required: bool,
    pub rebase_required: bool,
    pub reason: Option<String>,
    pub row_key: String,
    pub head_key: String,
    pub readback_rows: Vec<RealityRowReadback>,
    pub size_bytes: u32,
    pub size_estimate_tokens: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealityCursor {
    pub epoch_id: String,
    pub since_seq: u64,
    pub head_seq: u64,
    pub has_more: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealityRowReadback {
    pub cf_name: String,
    pub row_key: String,
    pub value_len_bytes: u64,
    pub value_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealityHeadRow {
    pub schema_version: u32,
    pub profile_id: Option<String>,
    pub profile_key: String,
    pub epoch_id: String,
    pub baseline_seq: u64,
    pub head_seq: u64,
    pub compact_state_hash: String,
    pub baseline_row_key: String,
    pub updated_at: DateTime<Utc>,
    pub compact_state: CompactRealityState,
    pub source_refs: Vec<SourceRef>,
    pub size_bytes: u32,
    pub size_estimate_tokens: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactRealityState {
    pub schema_version: u32,
    pub profile_id: Option<String>,
    pub foreground: CompactForeground,
    pub focused: Option<CompactFocused>,
    pub elements: Vec<CompactElement>,
    pub hud: BTreeMap<String, CompactHudReading>,
    pub hud_errors: BTreeMap<String, CompactHudError>,
    pub entities: Vec<CompactEntity>,
    pub audio: CompactAudio,
    pub events: CompactEventCursor,
    pub clipboard: Option<CompactClipboard>,
    pub fs: CompactFsRecent,
    pub diagnostics: CompactDiagnostics,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactForeground {
    pub hwnd: i64,
    pub pid: u32,
    pub process_name: String,
    pub process_path_sha256: Option<String>,
    pub window_title_sha256: Option<String>,
    pub window_bounds: Value,
    pub monitor_index: u32,
    pub profile_id: Option<String>,
    pub is_fullscreen: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactFocused {
    pub element_id: String,
    pub name_sha256: Option<String>,
    pub role: String,
    pub automation_id: Option<String>,
    pub bbox: Value,
    pub enabled: bool,
    pub patterns: Vec<String>,
    pub value_sha256: Option<String>,
    pub selected_text_sha256: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactElement {
    pub element_id: String,
    pub parent: Option<String>,
    pub name_sha256: Option<String>,
    pub role: String,
    pub automation_id: Option<String>,
    pub bbox: Value,
    pub enabled: bool,
    pub focused: bool,
    pub patterns: Vec<String>,
    pub children_count: u32,
    pub depth: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactHudReading {
    pub parsed: Value,
    pub raw_text_sha256: Option<String>,
    pub confidence_milli: u32,
    pub stale_ms: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactHudError {
    pub code: String,
    pub detail_sha256: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactEntity {
    pub entity_id: String,
    pub track_id: u64,
    pub class_label: String,
    pub bbox: Value,
    pub confidence_milli: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactAudio {
    pub rms_db_milli: i32,
    pub vad_speech_recent: bool,
    pub recent_event_count: u32,
    pub latest_event_kind: Option<String>,
    pub latest_event_azimuth_milli: Option<i32>,
    pub latest_event_confidence_milli: Option<u32>,
    pub direction_azimuth_milli: Option<i32>,
    pub direction_confidence_milli: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactEventCursor {
    pub event_count: u32,
    pub latest_non_cursor_kind: Option<String>,
    pub latest_non_cursor_seq: Option<u64>,
    pub latest_non_cursor_source: Option<String>,
    pub latest_non_cursor_data_sha256: Option<String>,
    pub latest_action_seq: Option<u64>,
    pub latest_action_kind: Option<String>,
    pub latest_action_data_sha256: Option<String>,
    pub log_path_sha256: Option<String>,
    pub log_start_offset: Option<u64>,
    pub log_next_offset: Option<u64>,
    pub log_file_len_bytes: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct CompactLogCursorChange {
    path_sha256: Option<String>,
    start_offset: Option<u64>,
    next_offset: Option<u64>,
    file_len_bytes: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct CompactRuntimeEventChange {
    event_count: u32,
    latest_non_cursor_kind: Option<String>,
    latest_non_cursor_seq: Option<u64>,
    latest_non_cursor_source: Option<String>,
    latest_non_cursor_data_sha256: Option<String>,
    latest_action_seq: Option<u64>,
    latest_action_kind: Option<String>,
    latest_action_data_sha256: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactClipboard {
    pub formats: Vec<String>,
    pub text_len: Option<u32>,
    pub text_excerpt_sha256: Option<String>,
    pub redacted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactFsRecent {
    pub event_count: u32,
    pub latest: Option<CompactFsEvent>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactFsEvent {
    pub path_sha256: String,
    pub kind: String,
    pub size_bytes: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompactDiagnostics {
    pub a11y_status: String,
    pub capture_status: String,
    pub detection_status: String,
    pub audio_status: String,
    pub elements_truncated: bool,
    pub entities_truncated: bool,
}

#[derive(Clone, Debug)]
struct CapturedReality {
    observation: Observation,
    compact_state: CompactRealityState,
    compact_state_hash: String,
    source_refs: Vec<SourceRef>,
    size_bytes: u32,
    size_estimate_tokens: u32,
}

#[derive(Clone, Debug)]
struct ProfileSelection {
    profile_id: Option<String>,
    profile_key: String,
}

#[derive(Clone, Debug)]
struct RealityChange {
    kind: &'static str,
    path: String,
    target: RealityTargetRef,
    before: Value,
    after: Value,
}

#[derive(Clone, Debug)]
struct PendingRealityDelta {
    row_key: String,
    delta: RealityDelta,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct UiaStructureAggregate {
    element_count: u32,
    elements_hash: String,
    appeared_count: u32,
    disappeared_count: u32,
    changed_count: u32,
    appeared_ids: Vec<String>,
    disappeared_ids: Vec<String>,
    changed_ids: Vec<String>,
    appeared_ids_truncated: bool,
    disappeared_ids_truncated: bool,
    changed_ids_truncated: bool,
    appeared_elements_hash: Option<String>,
    disappeared_elements_hash: Option<String>,
    before_changed_elements_hash: Option<String>,
    after_changed_elements_hash: Option<String>,
    id_cap: u32,
}

struct UiaElementMaps {
    before_by_id: BTreeMap<String, CompactElement>,
    after_by_id: BTreeMap<String, CompactElement>,
}

struct UiaElementFanout {
    appeared_ids: Vec<String>,
    disappeared_ids: Vec<String>,
    changed_ids: Vec<String>,
    changed_id_set: BTreeSet<String>,
    coalesce: bool,
}

#[derive(Clone, Copy, Debug)]
struct RectTranslation {
    dx: i64,
    dy: i64,
}

#[tool_router(router = reality_tool_router, vis = "pub(super)")]
impl SynapseService {
    #[tool(
        description = "Capture or read a compact delta-first reality baseline and persist CF_KV reality rows"
    )]
    pub async fn reality_baseline(
        &self,
        params: Parameters<RealityBaselineParams>,
    ) -> Result<Json<RealityBaselineResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = REALITY_BASELINE_TOOL,
            force_new_epoch = params.0.force_new_epoch,
            "tool.invocation kind=reality_baseline"
        );
        self.require_m3_permissions(REALITY_BASELINE_TOOL, &required_reality_write_permissions())?;
        let response = self.capture_or_read_reality_baseline(&params.0)?;
        Ok(Json(response))
    }

    #[tool(
        description = "Observe physical reality, persist ordered changes, and return deltas since a cursor"
    )]
    pub async fn observe_delta(
        &self,
        params: Parameters<ObserveDeltaParams>,
    ) -> Result<Json<ObserveDeltaResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = OBSERVE_DELTA_TOOL,
            since_epoch = ?params.0.since_epoch,
            since_seq = ?params.0.since_seq,
            "tool.invocation kind=observe_delta"
        );
        self.require_m3_permissions(OBSERVE_DELTA_TOOL, &required_reality_write_permissions())?;
        let response = self.observe_reality_delta(params.0)?;
        Ok(Json(response))
    }

    #[tool(
        description = "Audit the delta-guided reality assumption against a fresh physical read and persist drift findings"
    )]
    pub async fn reality_audit(
        &self,
        params: Parameters<RealityAuditParams>,
    ) -> Result<Json<RealityAuditResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = REALITY_AUDIT_TOOL,
            epoch_id = ?params.0.epoch_id,
            assumption_hash = ?params.0.assumption_hash,
            "tool.invocation kind=reality_audit"
        );
        self.require_m3_permissions(REALITY_AUDIT_TOOL, &required_reality_write_permissions())?;
        let response = self.audit_reality(&params.0)?;
        Ok(Json(response))
    }
}

impl SynapseService {
    fn capture_or_read_reality_baseline(
        &self,
        params: &RealityBaselineParams,
    ) -> Result<RealityBaselineResponse, ErrorData> {
        let requested_profile_key = params
            .profile_id
            .as_deref()
            .map(validate_key_segment)
            .transpose()?;
        let mut captured_for_new_baseline = None;
        let mut replaces_undecodable_head = false;
        if !params.force_new_epoch && params.epoch_id.is_none() {
            if let Some(profile_key) = requested_profile_key.as_deref() {
                let lookup = self.lookup_reality_head(profile_key)?;
                replaces_undecodable_head = lookup.is_undecodable();
                if let Some(head) = lookup.into_head() {
                    return self.existing_baseline_response(head);
                }
            } else {
                let captured = self.capture_reality_observation(
                    &params.include,
                    params.depth,
                    params.max_elements,
                    "reality_baseline",
                )?;
                let profile = select_profile(None, &captured.observation)?;
                let lookup = self.lookup_reality_head(&profile.profile_key)?;
                replaces_undecodable_head = lookup.is_undecodable();
                if let Some(head) = lookup.into_head() {
                    return self.existing_baseline_response(head);
                }
                captured_for_new_baseline = Some((captured, profile));
            }
        }

        let (captured, profile) = if let Some(captured) = captured_for_new_baseline {
            captured
        } else {
            let captured = self.capture_reality_observation(
                &params.include,
                params.depth,
                params.max_elements,
                "reality_baseline",
            )?;
            let profile = select_profile(params.profile_id.as_deref(), &captured.observation)?;
            (captured, profile)
        };
        let profile_id = profile.profile_id;
        let profile_key = profile.profile_key;
        let epoch_id = params
            .epoch_id
            .as_deref()
            .map(validate_key_segment)
            .transpose()?
            .unwrap_or_else(new_epoch_id);
        let baseline = RealityBaseline {
            epoch_id,
            baseline_seq: 0,
            generated_at: captured.observation.at,
            profile_id: profile_id.clone(),
            source_surfaces: source_surfaces(&captured.source_refs),
            source_refs: captured.source_refs.clone(),
            compact_state_hash: captured.compact_state_hash.clone(),
            redaction: reality_redaction(),
            size_bytes: captured.size_bytes,
            size_estimate_tokens: captured.size_estimate_tokens,
        };
        let baseline_row_key = baseline_row_key(&profile_key, &baseline.epoch_id);
        let head = RealityHeadRow {
            schema_version: SCHEMA_VERSION,
            profile_id,
            profile_key: profile_key.clone(),
            epoch_id: baseline.epoch_id.clone(),
            baseline_seq: baseline.baseline_seq,
            head_seq: baseline.baseline_seq,
            compact_state_hash: captured.compact_state_hash,
            baseline_row_key: baseline_row_key.clone(),
            updated_at: Utc::now(),
            compact_state: captured.compact_state,
            source_refs: captured.source_refs,
            size_bytes: baseline.size_bytes,
            size_estimate_tokens: baseline.size_estimate_tokens,
        };
        let readback_rows = self.write_reality_rows(&[
            (baseline_row_key, reality_row_value(&baseline)?),
            (head_key(&profile_key), reality_row_value(&head)?),
        ])?;
        Ok(RealityBaselineResponse {
            ok: true,
            created: true,
            profile_key,
            baseline,
            baseline_required: false,
            rebase_required: false,
            reason: Some(
                if replaces_undecodable_head {
                    "head_row_undecodable_rebuilt"
                } else {
                    "new_baseline_captured"
                }
                .to_owned(),
            ),
            size_bytes: head.size_bytes,
            size_estimate_tokens: head.size_estimate_tokens,
            head,
            readback_rows,
        })
    }

    fn existing_baseline_response(
        &self,
        head: RealityHeadRow,
    ) -> Result<RealityBaselineResponse, ErrorData> {
        let baseline = self.read_baseline_row(&head.baseline_row_key)?;
        let head_readback = self.readback_kv_row(&head_key(&head.profile_key))?;
        let baseline_readback = self.readback_kv_row(&head.baseline_row_key)?;
        Ok(RealityBaselineResponse {
            ok: true,
            created: false,
            profile_key: head.profile_key.clone(),
            baseline,
            baseline_required: false,
            rebase_required: false,
            reason: Some("existing_baseline_reused".to_owned()),
            size_bytes: head.size_bytes,
            size_estimate_tokens: head.size_estimate_tokens,
            head,
            readback_rows: vec![baseline_readback, head_readback],
        })
    }

    #[allow(clippy::too_many_lines)]
    fn observe_reality_delta(
        &self,
        params: ObserveDeltaParams,
    ) -> Result<ObserveDeltaResponse, ErrorData> {
        let requested_profile_key = params
            .profile_id
            .as_deref()
            .map(validate_key_segment)
            .transpose()?;
        let requested_since_epoch = params
            .since_epoch
            .as_deref()
            .map(validate_key_segment)
            .transpose()?;
        let mut captured_before_head = None;
        let head_profile_key = if let Some(profile_key) = requested_profile_key {
            profile_key
        } else {
            let captured = self.capture_reality_observation(
                &params.include,
                params.depth,
                params.max_elements,
                "observe_delta",
            )?;
            let profile = select_profile(None, &captured.observation)?;
            let profile_key = profile.profile_key.clone();
            captured_before_head = Some((captured, profile));
            profile_key
        };
        let stored_lookup = self.lookup_reality_head(&head_profile_key)?;
        let stored_head_undecodable = stored_lookup.is_undecodable();
        let stored_head = stored_lookup.into_head();
        // A watch whose epoch belongs to another profile's head is a profile
        // switch, not a missing baseline (ref #156).
        if let Some(epoch) = requested_since_epoch.as_deref()
            && stored_head.as_ref().is_none_or(|row| row.epoch_id != epoch)
            && let Some(previous) = self.find_reality_head_by_epoch(epoch, &head_profile_key)?
        {
            let since_seq = params.since_seq.unwrap_or(previous.baseline_seq);
            return Ok(profile_changed_response(
                &previous,
                since_seq,
                &head_profile_key,
            ));
        }
        let Some(mut head) = stored_head else {
            let reason = if stored_head_undecodable {
                format!("missing_baseline: {UNDECODABLE_HEAD_REASON}")
            } else {
                "missing_baseline".to_owned()
            };
            return Ok(missing_baseline_response(
                head_profile_key,
                requested_since_epoch,
                params.since_seq,
                &reason,
            ));
        };
        if let Some(epoch) = requested_since_epoch.as_deref()
            && epoch != head.epoch_id
        {
            return Ok(stale_epoch_response(&head, epoch, params.since_seq));
        }
        let since_seq = params.since_seq.unwrap_or(head.baseline_seq);
        if since_seq > head.head_seq {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "observe_delta since_seq {since_seq} is ahead of head_seq {}",
                    head.head_seq
                ),
            ));
        }

        let (captured, profile) = if let Some(captured) = captured_before_head {
            captured
        } else {
            let captured = self.capture_reality_observation(
                &params.include,
                params.depth,
                params.max_elements,
                "observe_delta",
            )?;
            // The requested profile selected the stored head. Compare the live
            // observation to it so known profile switches become rebase
            // guidance. If the observation cannot resolve a profile, keep the
            // requested head profile instead of inventing an unprofiled switch.
            let observed_profile = select_profile(None, &captured.observation)?;
            let profile = if params.profile_id.is_some() && observed_profile.profile_id.is_none() {
                select_profile(params.profile_id.as_deref(), &captured.observation)?
            } else {
                observed_profile
            };
            (captured, profile)
        };
        if profile.profile_key != head.profile_key {
            return Ok(profile_changed_response(
                &head,
                since_seq,
                &profile.profile_key,
            ));
        }

        let max_deltas = bounded_max_deltas(params.max_deltas)?;
        let mut write_readbacks = Vec::new();
        let mut published_sse_events = 0_u32;
        if head.compact_state_hash != captured.compact_state_hash {
            let changes = reality_changes(&head.compact_state, &captured.compact_state)?;
            if changes.len() > max_deltas {
                return Ok(ObserveDeltaResponse {
                    ok: true,
                    profile_key: Some(head.profile_key.clone()),
                    epoch_id: Some(head.epoch_id.clone()),
                    from_seq: Some(since_seq),
                    to_seq: Some(head.head_seq),
                    deltas: Vec::new(),
                    cursor: Some(cursor(&head, head.head_seq, true)),
                    baseline_required: false,
                    rebase_required: true,
                    reason: Some(format!(
                        "delta_overflow: {} changes exceed max_deltas {max_deltas}",
                        changes.len()
                    )),
                    readback_rows: Vec::new(),
                    published_sse_events: 0,
                    size_bytes: 0,
                    size_estimate_tokens: 0,
                });
            }
            let has_coalesced_uia = changes.iter().any(|change| {
                matches!(
                    change.kind,
                    "uia_elements_changed" | "uia_structure_changed"
                )
            });
            let pending_deltas = pending_reality_deltas(&head, &captured, changes)?;
            if has_coalesced_uia {
                let delta_refs = pending_deltas
                    .iter()
                    .map(|pending| &pending.delta)
                    .collect::<Vec<_>>();
                let (pending_size_bytes, _) = json_size_estimate(&delta_refs)?;
                if pending_size_bytes > captured.size_bytes {
                    return Ok(ObserveDeltaResponse {
                        ok: true,
                        profile_key: Some(head.profile_key.clone()),
                        epoch_id: Some(head.epoch_id.clone()),
                        from_seq: Some(since_seq),
                        to_seq: Some(head.head_seq),
                        deltas: Vec::new(),
                        cursor: Some(cursor(&head, head.head_seq, true)),
                        baseline_required: false,
                        rebase_required: true,
                        reason: Some(format!(
                            "delta_snapshot_budget_exceeded: coalesced delta batch {pending_size_bytes} bytes exceeds compact snapshot {} bytes; capture reality_baseline to rebase",
                            captured.size_bytes
                        )),
                        readback_rows: Vec::new(),
                        published_sse_events: 0,
                        size_bytes: 0,
                        size_estimate_tokens: 0,
                    });
                }
            }
            let mut rows = Vec::with_capacity(pending_deltas.len() + 1);
            for pending in &pending_deltas {
                rows.push((pending.row_key.clone(), reality_row_value(&pending.delta)?));
                head.head_seq = pending.delta.seq;
            }
            head.compact_state_hash = captured.compact_state_hash;
            head.updated_at = Utc::now();
            head.compact_state = captured.compact_state;
            head.source_refs = captured.source_refs;
            head.size_bytes = captured.size_bytes;
            head.size_estimate_tokens = captured.size_estimate_tokens;
            rows.push((head_key(&head.profile_key), reality_row_value(&head)?));
            write_readbacks.extend(self.write_reality_rows(&rows)?);
            for pending in &pending_deltas {
                self.publish_reality_delta(&head.profile_key, &pending.row_key, &pending.delta);
                published_sse_events = published_sse_events.saturating_add(1);
            }
        }

        let (mut deltas, mut read_readbacks, has_more) =
            self.read_delta_rows_after(&head.profile_key, &head.epoch_id, since_seq, max_deltas)?;
        write_readbacks.append(&mut read_readbacks);
        let to_seq = deltas.last().map_or(since_seq, |delta| delta.seq);
        let (size_bytes, size_estimate_tokens) = json_size_estimate(&deltas)?;
        if !has_more && deltas.is_empty() && since_seq == head.head_seq {
            deltas = Vec::new();
        }
        Ok(ObserveDeltaResponse {
            ok: true,
            profile_key: Some(head.profile_key.clone()),
            epoch_id: Some(head.epoch_id.clone()),
            from_seq: Some(since_seq),
            to_seq: Some(to_seq),
            deltas,
            cursor: Some(cursor(&head, to_seq, has_more)),
            baseline_required: false,
            rebase_required: false,
            reason: if to_seq == since_seq {
                Some("no_changes".to_owned())
            } else {
                Some("deltas_returned".to_owned())
            },
            readback_rows: write_readbacks,
            published_sse_events,
            size_bytes,
            size_estimate_tokens,
        })
    }

    fn audit_reality(
        &self,
        params: &RealityAuditParams,
    ) -> Result<RealityAuditResponse, ErrorData> {
        let captured = self.capture_reality_observation(
            &params.include,
            params.depth,
            params.max_elements,
            "reality_audit",
        )?;
        let profile = select_profile(params.profile_id.as_deref(), &captured.observation)?;
        let profile_key = profile.profile_key;
        let head_lookup = self.lookup_reality_head(&profile_key)?;
        let head_undecodable = head_lookup.is_undecodable();
        let head = head_lookup.into_head();
        if let Some(expected_epoch) = params.epoch_id.as_deref() {
            validate_key_segment(expected_epoch)?;
        }
        let compared_epoch = params
            .epoch_id
            .clone()
            .or_else(|| head.as_ref().map(|row| row.epoch_id.clone()))
            .unwrap_or_else(|| "missing-baseline".to_owned());
        let assumption_hash = params
            .assumption_hash
            .as_deref()
            .map(validate_assumption_hash)
            .transpose()?
            .or_else(|| head.as_ref().map(|row| row.compact_state_hash.clone()))
            .unwrap_or_else(|| "missing-baseline".to_owned());
        let baseline_status = if head.is_none() {
            RealityBaselineStatus::SourceUnavailable
        } else if params.epoch_id.as_deref().is_some_and(|expected| {
            Some(expected) != head.as_ref().map(|row| row.epoch_id.as_str())
        }) {
            RealityBaselineStatus::Stale
        } else {
            RealityBaselineStatus::Current
        };
        let (drift_status, drift_items) = audit_drift_status_and_items(
            params,
            head.as_ref(),
            &captured,
            baseline_status,
            &assumption_hash,
        )?;
        let rebase_required = drift_status != RealityDriftStatus::InSync;
        let audit_id = new_audit_id();
        let audit = RealityAudit {
            audit_id: audit_id.clone(),
            epoch_id: compared_epoch,
            baseline_seq: head.as_ref().map_or(0, |row| row.baseline_seq),
            compared_seq_start: head.as_ref().map_or(0, |row| row.baseline_seq),
            compared_seq_end: head.as_ref().map_or(0, |row| row.head_seq),
            ran_at: Utc::now(),
            baseline_status,
            assumption_hash,
            actual_hash: captured.compact_state_hash,
            drift_status,
            drift_items,
            physical_source_refs: captured.source_refs,
            rebase_required,
            rebase_reason: rebase_required.then(|| {
                if head_undecodable {
                    UNDECODABLE_HEAD_REASON.to_owned()
                } else {
                    rebase_reason(baseline_status, drift_status)
                }
            }),
            follow_up_refs: Vec::new(),
        };
        let row_key = audit_row_key(&profile_key, &audit_id);
        let audit_readback = self.write_kv_json_readback(&row_key, &audit, "reality audit row")?;
        let mut readback_rows = vec![audit_readback];
        if let Some(head) = &head {
            readback_rows.push(self.readback_kv_row(&head_key(&head.profile_key))?);
        }
        let (size_bytes, size_estimate_tokens) = json_size_estimate(&audit)?;
        let response_head_key = head
            .as_ref()
            .map_or_else(|| head_key(&profile_key), |row| head_key(&row.profile_key));
        Ok(RealityAuditResponse {
            ok: true,
            profile_key,
            audit,
            baseline_required: head.is_none(),
            rebase_required,
            reason: if rebase_required {
                Some("drift_or_missing_baseline".to_owned())
            } else {
                Some("in_sync".to_owned())
            },
            row_key,
            head_key: response_head_key,
            readback_rows,
            size_bytes,
            size_estimate_tokens,
        })
    }

    fn reality_target_hwnd(&self) -> Result<Option<i64>, ErrorData> {
        let session_id = crate::http::current_mcp_session_id();
        if let Some(session_id) = session_id.as_deref()
            && matches!(
                super::agent_scope::lookup(session_id)?,
                super::agent_scope::TargetLookup::Agent(None)
                    | super::agent_scope::TargetLookup::AgentsActive
            )
        {
            return Err(mcp_error(
                error_codes::TARGET_NOT_SET,
                "reality requires the agent's bound target; refusing foreground fallback",
            ));
        }
        match self.session_target(session_id.as_deref())? {
            Some(super::SessionTarget::Window { hwnd }) => Ok(Some(hwnd)),
            Some(super::SessionTarget::Cdp { cdp_target_id, .. }) => Err(mcp_error(
                error_codes::TARGET_CDP_UNRESOLVED,
                format!(
                    "reality cannot observe CDP target {cdp_target_id:?} through window perception; use target-specific browser readback"
                ),
            )),
            None => Ok(None),
        }
    }

    fn capture_reality_observation(
        &self,
        include: &[ObserveSlot],
        depth: u32,
        max_elements: usize,
        reason: &'static str,
    ) -> Result<CapturedReality, ErrorData> {
        let depth = bounded_depth(depth)?;
        let max_elements = bounded_max_elements(max_elements)?;
        let params = reality_observe_params(include, depth, max_elements);
        let include = observe_include(&params);
        let target_hwnd = self.reality_target_hwnd()?;
        let state = self.m1_state()?;
        let mut input = crate::m1::observe_input(&state, &params, target_hwnd)?;
        if include.fs && input.fs_recent.is_empty() {
            let _ = populate_fs_recent(&mut input, &state.fs_recent_tracker);
        }
        drop(state);

        if include.audio && input.audio == AudioContext::default() {
            populate_audio_summary(&self.m3_state, &mut input);
        }
        if include.clipboard && input.clipboard_summary.is_none() {
            let _ = populate_clipboard_summary(&mut input);
        }
        self.resolve_input_profile_and_hud(&mut input, include.hud);
        if include.events {
            self.populate_everquest_log_events(&mut input);
        }
        let observation = ObservationAssembler::new()
            .assemble(include, input)
            .map_err(|err| mcp_error(err.code(), err.to_string()))?;
        let mut state = self.m1_state()?;
        state.last_observed_foreground = Some(observation.foreground.clone());
        drop(state);
        self.persist_observation(&observation, reason)?;
        let compact_state = compact_state(&observation)?;
        let compact_state_hash = hash_json(&compact_state)?;
        let source_refs = source_refs_for_observation(&observation)?;
        let (size_bytes, size_estimate_tokens) = json_size_estimate(&compact_state)?;
        Ok(CapturedReality {
            observation,
            compact_state,
            compact_state_hash,
            source_refs,
            size_bytes,
            size_estimate_tokens,
        })
    }

    fn lookup_reality_head(&self, profile_key: &str) -> Result<HeadLookup, ErrorData> {
        let key = head_key(profile_key);
        let runtime = self.reflex_runtime()?;
        let runtime = runtime.lock().map_err(|_| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "reflex runtime lock poisoned while reading reality head row",
            )
        })?;
        let Some(bytes) = runtime
            .storage_kv_row(key.as_bytes())
            .map_err(|error| mcp_error(error.code(), error.to_string()))?
        else {
            return Ok(HeadLookup::Absent);
        };
        drop(runtime);
        Ok(
            decode_head_row(&key, &bytes).map_or(HeadLookup::Undecodable, |head| {
                HeadLookup::Present(Box::new(head))
            }),
        )
    }

    /// Finds the stored head (other than `excluding_profile_key`) that owns
    /// `epoch_id`, so a delta poll after a profile switch can be recognised.
    fn find_reality_head_by_epoch(
        &self,
        epoch_id: &str,
        excluding_profile_key: &str,
    ) -> Result<Option<RealityHeadRow>, ErrorData> {
        let rows = {
            let runtime = self.reflex_runtime()?;
            let runtime = runtime.lock().map_err(|_| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "reflex runtime lock poisoned while scanning reality head rows",
                )
            })?;
            runtime
                .storage_cf_prefix_rows(cf::CF_KV, head_key("").as_bytes(), usize::MAX)
                .map_err(|error| mcp_error(error.code(), error.to_string()))?
        };
        for (key, bytes) in rows {
            let Some(head) = decode_head_row(&String::from_utf8_lossy(&key), &bytes) else {
                continue;
            };
            if head.epoch_id == epoch_id && head.profile_key != excluding_profile_key {
                return Ok(Some(head));
            }
        }
        Ok(None)
    }

    fn read_baseline_row(&self, row_key: &str) -> Result<RealityBaseline, ErrorData> {
        let runtime = self.reflex_runtime()?;
        let runtime = runtime.lock().map_err(|_| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "reflex runtime lock poisoned while reading reality baseline row",
            )
        })?;
        let bytes = runtime
            .storage_kv_row(row_key.as_bytes())
            .map_err(|error| mcp_error(error.code(), error.to_string()))?
            .ok_or_else(|| {
                mcp_error(
                    error_codes::STORAGE_READ_FAILED,
                    format!("reality baseline row missing: {row_key}"),
                )
            })?;
        drop(runtime);
        decode_json_row(&bytes, "reality baseline row")
    }

    fn read_delta_rows_after(
        &self,
        profile_key: &str,
        epoch_id: &str,
        since_seq: u64,
        max_deltas: usize,
    ) -> Result<(Vec<RealityDelta>, Vec<RealityRowReadback>, bool), ErrorData> {
        let prefix = delta_prefix(profile_key, epoch_id);
        let start_key = delta_row_key(profile_key, epoch_id, since_seq.saturating_add(1));
        let rows = {
            let runtime = self.reflex_runtime()?;
            let runtime = runtime.lock().map_err(|_| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "reflex runtime lock poisoned while reading reality delta rows",
                )
            })?;
            runtime
                .storage_cf_prefix_rows_from(
                    cf::CF_KV,
                    prefix.as_bytes(),
                    start_key.as_bytes(),
                    max_deltas + 1,
                )
                .map_err(|error| mcp_error(error.code(), error.to_string()))?
        };
        let mut deltas = Vec::new();
        let mut readbacks = Vec::new();
        for (key, bytes) in rows {
            let row_key = String::from_utf8_lossy(&key).into_owned();
            let delta = decode_json_row::<RealityDelta>(&bytes, "reality delta row")?;
            if delta.seq <= since_seq {
                continue;
            }
            if deltas.len() >= max_deltas {
                return Ok((deltas, readbacks, true));
            }
            readbacks.push(RealityRowReadback {
                cf_name: cf::CF_KV.to_owned(),
                row_key,
                value_len_bytes: bytes.len() as u64,
                value_sha256: hash_bytes(&bytes),
            });
            deltas.push(delta);
        }
        Ok((deltas, readbacks, false))
    }

    fn write_reality_rows(
        &self,
        rows: &[(String, Value)],
    ) -> Result<Vec<RealityRowReadback>, ErrorData> {
        let runtime = self.reflex_runtime()?;
        let runtime = runtime.lock().map_err(|_| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "reflex runtime lock poisoned while writing reality transition",
            )
        })?;
        write_reality_rows_with(
            rows,
            |batch| {
                runtime
                    .storage_put_kv_rows(batch)
                    .map_err(|error| mcp_error(error.code(), error.to_string()))
            },
            |key| {
                runtime
                    .storage_kv_row(key)
                    .map_err(|error| mcp_error(error_codes::STORAGE_READ_FAILED, error.to_string()))
            },
        )
    }

    fn write_kv_json_readback<T>(
        &self,
        key: &str,
        row: &T,
        label: &str,
    ) -> Result<RealityRowReadback, ErrorData>
    where
        T: Serialize,
    {
        let encoded = synapse_storage::encode_json(row).map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("encode {label}: {error}"),
            )
        })?;
        let runtime = self.reflex_runtime()?;
        let runtime = runtime.lock().map_err(|_| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("reflex runtime lock poisoned while writing {label}"),
            )
        })?;
        runtime
            .storage_put_kv_rows(vec![(key.as_bytes().to_vec(), encoded)])
            .map_err(|error| mcp_error(error.code(), format!("write {label}: {error}")))?;
        let stored = runtime
            .storage_kv_row(key.as_bytes())
            .map_err(|error| {
                mcp_error(
                    error_codes::STORAGE_READ_FAILED,
                    format!("read {label} after write: {error}"),
                )
            })?
            .ok_or_else(|| {
                mcp_error(
                    error_codes::STORAGE_READ_FAILED,
                    format!("{label} missing after write: {key}"),
                )
            })?;
        drop(runtime);
        Ok(RealityRowReadback {
            cf_name: cf::CF_KV.to_owned(),
            row_key: key.to_owned(),
            value_len_bytes: stored.len() as u64,
            value_sha256: hash_bytes(&stored),
        })
    }

    fn readback_kv_row(&self, key: &str) -> Result<RealityRowReadback, ErrorData> {
        let runtime = self.reflex_runtime()?;
        let runtime = runtime.lock().map_err(|_| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "reflex runtime lock poisoned while reading reality row",
            )
        })?;
        let stored = runtime
            .storage_kv_row(key.as_bytes())
            .map_err(|error| mcp_error(error.code(), error.to_string()))?
            .ok_or_else(|| {
                mcp_error(
                    error_codes::STORAGE_READ_FAILED,
                    format!("reality row missing: {key}"),
                )
            })?;
        drop(runtime);
        Ok(RealityRowReadback {
            cf_name: cf::CF_KV.to_owned(),
            row_key: key.to_owned(),
            value_len_bytes: stored.len() as u64,
            value_sha256: hash_bytes(&stored),
        })
    }

    fn publish_reality_delta(&self, profile_key: &str, row_key: &str, delta: &RealityDelta) {
        let Ok(sse_state) = self.sse_state() else {
            return;
        };
        let event = Event {
            seq: NEXT_REALITY_EVENT_SEQ.fetch_add(1, Ordering::Relaxed),
            at: delta.at,
            source: EventSource::Perception,
            kind: REALITY_EVENT_KIND.to_owned(),
            data: json!({
                "profile_key": profile_key,
                "row_key": row_key,
                "epoch_id": delta.epoch_id,
                "seq": delta.seq,
                "previous_seq": delta.previous_seq,
                "delta_kind": delta.kind,
                "path": delta.path,
                "target": delta.target,
                "before": &delta.before,
                "after": &delta.after,
                "confidence": delta.confidence,
                "redacted": true,
            }),
            correlations: Vec::new(),
        };
        let report = sse_state.event_bus().publish(event);
        tracing::debug!(
            code = "REALITY_DELTA_EVENT_PUBLISHED",
            matched = report.matched,
            queued = report.queued,
            dropped = report.dropped,
            profile_key,
            row_key,
            "reality_delta event published"
        );
    }
}

fn compact_state(observation: &Observation) -> Result<CompactRealityState, ErrorData> {
    let mut hud = BTreeMap::new();
    for (name, reading) in &observation.hud.by_name {
        hud.insert(
            name.clone(),
            CompactHudReading {
                parsed: serde_json::to_value(&reading.parsed)
                    .map_err(|error| encode_value_error(&error))?,
                raw_text_sha256: non_empty_keyed_hash(&reading.raw_text),
                confidence_milli: confidence_milli(reading.confidence),
                stale_ms: reading.stale_ms,
            },
        );
    }
    Ok(CompactRealityState {
        schema_version: SCHEMA_VERSION,
        profile_id: observation.foreground.profile_id.clone(),
        foreground: CompactForeground {
            hwnd: observation.foreground.hwnd,
            pid: observation.foreground.pid,
            process_name: observation.foreground.process_name.clone(),
            process_path_sha256: non_empty_keyed_hash(&observation.foreground.process_path),
            window_title_sha256: non_empty_keyed_hash(&observation.foreground.window_title),
            window_bounds: serde_json::to_value(observation.foreground.window_bounds)
                .map_err(|error| encode_value_error(&error))?,
            monitor_index: observation.foreground.monitor_index,
            profile_id: observation.foreground.profile_id.clone(),
            is_fullscreen: observation.foreground.is_fullscreen,
        },
        focused: observation
            .focused
            .as_ref()
            .map(compact_focused)
            .transpose()?,
        elements: compact_elements(observation)?,
        hud,
        hud_errors: compact_hud_errors(observation),
        entities: compact_entities(observation)?,
        audio: compact_audio(observation),
        events: compact_events(observation),
        clipboard: compact_clipboard(observation),
        fs: compact_fs_recent(observation),
        diagnostics: CompactDiagnostics {
            a11y_status: sensor_status_name(&observation.diagnostics.a11y_status),
            capture_status: sensor_status_name(&observation.diagnostics.capture_status),
            detection_status: sensor_status_name(&observation.diagnostics.detection_status),
            audio_status: sensor_status_name(&observation.diagnostics.audio_status),
            elements_truncated: observation.diagnostics.elements_truncated,
            entities_truncated: observation.diagnostics.entities_truncated,
        },
    })
}

fn compact_focused(focused: &synapse_core::FocusedElement) -> Result<CompactFocused, ErrorData> {
    Ok(CompactFocused {
        element_id: focused.element_id.to_string(),
        name_sha256: non_empty_keyed_hash(&focused.name),
        role: focused.role.clone(),
        automation_id: focused.automation_id.clone(),
        bbox: serde_json::to_value(focused.bbox).map_err(|error| encode_value_error(&error))?,
        enabled: focused.enabled,
        patterns: compact_patterns(&focused.patterns),
        value_sha256: focused.value.as_deref().and_then(non_empty_keyed_hash),
        selected_text_sha256: focused
            .selected_text
            .as_deref()
            .and_then(non_empty_keyed_hash),
    })
}

fn compact_elements(observation: &Observation) -> Result<Vec<CompactElement>, ErrorData> {
    let mut elements = observation
        .elements
        .iter()
        .map(|element| {
            Ok(CompactElement {
                element_id: element.element_id.to_string(),
                parent: element.parent.as_ref().map(ToString::to_string),
                name_sha256: non_empty_keyed_hash(&element.name),
                role: element.role.clone(),
                automation_id: element.automation_id.clone(),
                bbox: serde_json::to_value(element.bbox)
                    .map_err(|error| encode_value_error(&error))?,
                enabled: element.enabled,
                focused: element.focused,
                patterns: compact_patterns(&element.patterns),
                children_count: element.children_count,
                depth: element.depth,
            })
        })
        .collect::<Result<Vec<_>, ErrorData>>()?;
    elements.sort_by(|left, right| left.element_id.cmp(&right.element_id));
    Ok(elements)
}

fn compact_hud_errors(observation: &Observation) -> BTreeMap<String, CompactHudError> {
    observation
        .hud
        .errors
        .iter()
        .map(|(name, error)| {
            (
                name.clone(),
                CompactHudError {
                    code: error.code.clone(),
                    detail_sha256: non_empty_keyed_hash(&error.detail),
                },
            )
        })
        .collect()
}

fn compact_entities(observation: &Observation) -> Result<Vec<CompactEntity>, ErrorData> {
    let mut entities = observation
        .entities
        .iter()
        .map(|entity| {
            Ok(CompactEntity {
                entity_id: entity.entity_id.clone(),
                track_id: entity.track_id,
                class_label: entity.class_label.clone(),
                bbox: serde_json::to_value(entity.bbox)
                    .map_err(|error| encode_value_error(&error))?,
                confidence_milli: confidence_milli(entity.confidence),
            })
        })
        .collect::<Result<Vec<_>, ErrorData>>()?;
    entities.sort_by(|left, right| {
        left.entity_id
            .cmp(&right.entity_id)
            .then_with(|| left.track_id.cmp(&right.track_id))
            .then_with(|| left.class_label.cmp(&right.class_label))
    });
    Ok(entities)
}

fn compact_audio(observation: &Observation) -> CompactAudio {
    let latest_event = observation
        .audio
        .recent_events
        .iter()
        .max_by(|left, right| {
            left.at
                .cmp(&right.at)
                .then_with(|| left.kind.cmp(&right.kind))
        });
    CompactAudio {
        rms_db_milli: signed_milli(observation.audio.rms_db),
        vad_speech_recent: observation.audio.vad_speech_recent,
        recent_event_count: len_to_u32(observation.audio.recent_events.len()),
        latest_event_kind: latest_event.map(|event| event.kind.clone()),
        latest_event_azimuth_milli: latest_event
            .and_then(|event| event.azimuth_deg.map(signed_milli)),
        latest_event_confidence_milli: latest_event.map(|event| confidence_milli(event.confidence)),
        direction_azimuth_milli: observation
            .audio
            .direction_estimate
            .map(|direction| signed_milli(direction.azimuth_deg)),
        direction_confidence_milli: observation
            .audio
            .direction_estimate
            .map(|direction| confidence_milli(direction.confidence)),
    }
}

fn compact_clipboard(observation: &Observation) -> Option<CompactClipboard> {
    observation.clipboard_summary.as_ref().map(|summary| {
        let mut formats = summary.formats.clone();
        formats.sort();
        formats.dedup();
        CompactClipboard {
            formats,
            text_len: summary.text_len,
            text_excerpt_sha256: summary
                .text_excerpt
                .as_deref()
                .and_then(non_empty_keyed_hash),
            redacted: summary.redacted,
        }
    })
}

fn compact_fs_recent(observation: &Observation) -> CompactFsRecent {
    let latest = observation.fs_recent.iter().max_by(|left, right| {
        left.at
            .cmp(&right.at)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| fs_event_kind_name(left.kind).cmp(fs_event_kind_name(right.kind)))
    });
    CompactFsRecent {
        event_count: len_to_u32(observation.fs_recent.len()),
        latest: latest.map(|event| CompactFsEvent {
            path_sha256: keyed_hash_bytes(event.path.as_bytes()),
            kind: fs_event_kind_name(event.kind).to_owned(),
            size_bytes: event.size_bytes,
        }),
    }
}

fn compact_patterns(patterns: &[synapse_core::UiaPattern]) -> Vec<String> {
    let mut patterns = patterns
        .iter()
        .map(|pattern| format!("{pattern:?}"))
        .collect::<Vec<_>>();
    patterns.sort();
    patterns
}

fn compact_events(observation: &Observation) -> CompactEventCursor {
    let mut latest_non_cursor_kind = None;
    let mut latest_non_cursor_seq = None;
    let mut latest_non_cursor_source = None;
    let mut latest_non_cursor_data_sha256 = None;
    let mut latest_action_seq = None;
    let mut latest_action_kind = None;
    let mut latest_action_data_sha256 = None;
    let mut log_path_sha256 = None;
    let mut log_start_offset = None;
    let mut log_next_offset = None;
    let mut log_file_len_bytes = None;
    let mut event_count = 0_u32;
    for event in &observation.recent_events {
        if let Some(cursor) = event.data_excerpt.get("cursor") {
            log_start_offset = cursor
                .get("start_offset")
                .and_then(Value::as_u64)
                .or(log_start_offset);
            log_next_offset = cursor
                .get("next_offset")
                .and_then(Value::as_u64)
                .or(log_next_offset);
            log_file_len_bytes = cursor
                .get("file_len_bytes")
                .and_then(Value::as_u64)
                .or(log_file_len_bytes);
        }
        log_path_sha256 = event
            .data_excerpt
            .get("path")
            .and_then(Value::as_str)
            .and_then(non_empty_keyed_hash)
            .or(log_path_sha256);
        if !event.kind.starts_with("everquest.log_cursor") {
            event_count = event_count.saturating_add(1);
            latest_non_cursor_kind = Some(event.kind.clone());
            latest_non_cursor_seq = Some(event.seq);
            latest_non_cursor_source = Some(event_source_name(event.source).to_owned());
            latest_non_cursor_data_sha256 = hash_json(&event.data_excerpt).ok();
        }
        if matches!(
            event.source,
            EventSource::ActionEmitter | EventSource::Reflex | EventSource::System
        ) && (event.kind.contains("action") || event.kind.contains("audit"))
        {
            latest_action_seq = Some(event.seq);
            latest_action_kind = Some(event.kind.clone());
            latest_action_data_sha256 = hash_json(&event.data_excerpt).ok();
        }
    }
    CompactEventCursor {
        event_count,
        latest_non_cursor_kind,
        latest_non_cursor_seq,
        latest_non_cursor_source,
        latest_non_cursor_data_sha256,
        latest_action_seq,
        latest_action_kind,
        latest_action_data_sha256,
        log_path_sha256,
        log_start_offset,
        log_next_offset,
        log_file_len_bytes,
    }
}

fn source_refs_for_observation(observation: &Observation) -> Result<Vec<SourceRef>, ErrorData> {
    let mut refs = vec![
        SourceRef {
            surface: RealitySourceSurface::Window,
            path: format!("hwnd:0x{:x}", observation.foreground.hwnd),
            offset: None,
            hash: Some(hash_json(&json!({
                "hwnd": observation.foreground.hwnd,
                "bounds": observation.foreground.window_bounds,
                "profile_id": observation.foreground.profile_id,
            }))?),
            summary: format!(
                "foreground process={} pid={} title_hash={}",
                observation.foreground.process_name,
                observation.foreground.pid,
                non_empty_keyed_hash(&observation.foreground.window_title)
                    .unwrap_or_else(|| "empty".to_owned())
            ),
        },
        SourceRef {
            surface: RealitySourceSurface::Process,
            path: format!("pid:{}", observation.foreground.pid),
            offset: None,
            hash: non_empty_keyed_hash(&observation.foreground.process_path),
            summary: format!("process {}", observation.foreground.process_name),
        },
    ];
    if let Some(log_ref) = log_source_ref(observation)? {
        refs.push(log_ref);
    }
    if let Some(a11y_ref) = a11y_source_ref(observation)? {
        refs.push(a11y_ref);
    }
    if let Some(entity_ref) = entity_source_ref(observation)? {
        refs.push(entity_ref);
    }
    if let Some(hud_ref) = hud_source_ref(observation)? {
        refs.push(hud_ref);
    }
    if let Some(audio_ref) = audio_source_ref(observation)? {
        refs.push(audio_ref);
    }
    if let Some(clipboard_ref) = clipboard_source_ref(observation)? {
        refs.push(clipboard_ref);
    }
    if let Some(fs_ref) = fs_source_ref(observation)? {
        refs.push(fs_ref);
    }
    if let Some(action_ref) = action_audit_source_ref(observation)? {
        refs.push(action_ref);
    }
    refs.push(SourceRef {
        surface: RealitySourceSurface::Storage,
        path: "CF_OBSERVATIONS/latest".to_owned(),
        offset: None,
        hash: None,
        summary: "full observation also persisted through the observation audit path".to_owned(),
    });
    Ok(refs)
}

fn source_refs_for_change(kind: &str, source_refs: &[SourceRef]) -> Vec<SourceRef> {
    let scoped = source_refs
        .iter()
        .filter(|source| source_ref_matches_change(kind, source.surface))
        .cloned()
        .collect::<Vec<_>>();
    if scoped.is_empty() {
        source_refs.to_vec()
    } else {
        scoped
    }
}

fn source_ref_matches_change(kind: &str, surface: RealitySourceSurface) -> bool {
    match kind {
        "foreground_changed" => matches!(
            surface,
            RealitySourceSurface::Window | RealitySourceSurface::Process
        ),
        "focus_changed"
        | "uia_element_appeared"
        | "uia_element_disappeared"
        | "uia_elements_changed"
        | "uia_structure_changed"
        | "uia_element_name_changed"
        | "uia_element_moved"
        | "uia_element_changed" => matches!(surface, RealitySourceSurface::A11yUia),
        "hud_field_changed" | "hud_error_changed" => {
            matches!(surface, RealitySourceSurface::Hud)
        }
        "entity_appeared"
        | "entity_disappeared"
        | "entity_moved"
        | "entity_class_changed"
        | "entity_confidence_changed"
        | "entity_track_changed" => matches!(surface, RealitySourceSurface::PixelFrame),
        "audio_summary_changed" => matches!(surface, RealitySourceSurface::Audio),
        "log_cursor_changed" => matches!(surface, RealitySourceSurface::GameLog),
        "runtime_event_changed" => matches!(
            surface,
            RealitySourceSurface::ActionAudit | RealitySourceSurface::GameLog
        ),
        "clipboard_summary_changed" => matches!(surface, RealitySourceSurface::Clipboard),
        "filesystem_summary_changed" => matches!(surface, RealitySourceSurface::File),
        _ => true,
    }
}

fn a11y_source_ref(observation: &Observation) -> Result<Option<SourceRef>, ErrorData> {
    if !observation.elements.is_empty() || observation.focused.is_some() {
        return Ok(Some(SourceRef {
            surface: RealitySourceSurface::A11yUia,
            path: "observation/a11y".to_owned(),
            offset: None,
            hash: Some(hash_json(&json!({
                "focused": observation.focused,
                "elements": observation.elements,
                "elements_truncated": observation.diagnostics.elements_truncated,
            }))?),
            summary: format!(
                "a11y elements={} focused={}",
                observation.elements.len(),
                observation.focused.is_some()
            ),
        }));
    }
    Ok(None)
}

fn entity_source_ref(observation: &Observation) -> Result<Option<SourceRef>, ErrorData> {
    if !observation.entities.is_empty() {
        return Ok(Some(SourceRef {
            surface: RealitySourceSurface::PixelFrame,
            path: "observation/entities".to_owned(),
            offset: None,
            hash: Some(hash_json(&json!({
                "entities": observation.entities,
                "entities_truncated": observation.diagnostics.entities_truncated,
            }))?),
            summary: format!("entities={}", observation.entities.len()),
        }));
    }
    Ok(None)
}

fn hud_source_ref(observation: &Observation) -> Result<Option<SourceRef>, ErrorData> {
    if !observation.hud.by_name.is_empty() || !observation.hud.errors.is_empty() {
        return Ok(Some(SourceRef {
            surface: RealitySourceSurface::Hud,
            path: "observation/hud".to_owned(),
            offset: None,
            hash: Some(hash_json(&observation.hud)?),
            summary: format!(
                "hud fields={} errors={}",
                observation.hud.by_name.len(),
                observation.hud.errors.len()
            ),
        }));
    }
    Ok(None)
}

fn audio_source_ref(observation: &Observation) -> Result<Option<SourceRef>, ErrorData> {
    if observation.diagnostics.audio_enabled
        || !observation.audio.recent_events.is_empty()
        || observation.audio.direction_estimate.is_some()
        || observation.audio.vad_speech_recent
    {
        return Ok(Some(SourceRef {
            surface: RealitySourceSurface::Audio,
            path: "observation/audio".to_owned(),
            offset: None,
            hash: Some(hash_json(&observation.audio)?),
            summary: format!(
                "audio events={} vad={}",
                observation.audio.recent_events.len(),
                observation.audio.vad_speech_recent
            ),
        }));
    }
    Ok(None)
}

fn clipboard_source_ref(observation: &Observation) -> Result<Option<SourceRef>, ErrorData> {
    if let Some(clipboard) = &observation.clipboard_summary {
        return Ok(Some(SourceRef {
            surface: RealitySourceSurface::Clipboard,
            path: "observation/clipboard".to_owned(),
            offset: None,
            hash: Some(hash_json(&json!({
                "formats": clipboard.formats,
                "text_len": clipboard.text_len,
                "text_excerpt_sha256": clipboard
                    .text_excerpt
                    .as_deref()
                    .and_then(non_empty_keyed_hash),
                "redacted": clipboard.redacted,
            }))?),
            summary: format!(
                "clipboard formats={} text_len={:?} redacted={}",
                clipboard.formats.len(),
                clipboard.text_len,
                clipboard.redacted
            ),
        }));
    }
    Ok(None)
}

fn fs_source_ref(observation: &Observation) -> Result<Option<SourceRef>, ErrorData> {
    if !observation.fs_recent.is_empty() {
        return Ok(Some(SourceRef {
            surface: RealitySourceSurface::File,
            path: "observation/fs_recent".to_owned(),
            offset: None,
            hash: Some(hash_json(&compact_fs_recent(observation))?),
            summary: format!("fs_recent events={}", observation.fs_recent.len()),
        }));
    }
    Ok(None)
}

fn action_audit_source_ref(observation: &Observation) -> Result<Option<SourceRef>, ErrorData> {
    let action_events = observation
        .recent_events
        .iter()
        .filter(|event| {
            matches!(
                event.source,
                EventSource::ActionEmitter | EventSource::Reflex | EventSource::System
            ) && (event.kind.contains("action") || event.kind.contains("audit"))
        })
        .collect::<Vec<_>>();
    if !action_events.is_empty() {
        return Ok(Some(SourceRef {
            surface: RealitySourceSurface::ActionAudit,
            path: "observation/events/action_audit".to_owned(),
            offset: action_events.last().map(|event| event.seq),
            hash: Some(hash_json(&action_events)?),
            summary: format!("action/audit events={}", action_events.len()),
        }));
    }
    Ok(None)
}

fn log_source_ref(observation: &Observation) -> Result<Option<SourceRef>, ErrorData> {
    for event in &observation.recent_events {
        let Some(path) = event.data_excerpt.get("path").and_then(Value::as_str) else {
            continue;
        };
        let cursor = event.data_excerpt.get("cursor");
        let next_offset = cursor
            .and_then(|value| value.get("next_offset"))
            .and_then(Value::as_u64);
        let start_offset = cursor
            .and_then(|value| value.get("start_offset"))
            .and_then(Value::as_u64);
        return Ok(Some(SourceRef {
            surface: RealitySourceSurface::GameLog,
            path: path.to_owned(),
            offset: next_offset,
            hash: Some(hash_json(&json!({
                "path": path,
                "start_offset": start_offset,
                "next_offset": next_offset,
            }))?),
            summary: format!("game log cursor next_offset={next_offset:?}"),
        }));
    }
    Ok(None)
}

fn reality_changes(
    before: &CompactRealityState,
    after: &CompactRealityState,
) -> Result<Vec<RealityChange>, ErrorData> {
    let mut changes = Vec::new();
    let foreground_translation = rect_translation(
        &before.foreground.window_bounds,
        &after.foreground.window_bounds,
    );
    push_foreground_changes(&mut changes, &before.foreground, &after.foreground);
    push_focus_changes(
        &mut changes,
        before.focused.as_ref(),
        after.focused.as_ref(),
        foreground_translation,
    );
    push_element_changes(&mut changes, before, after, foreground_translation)?;
    push_hud_changes(&mut changes, before, after);
    push_entity_changes(&mut changes, before, after);
    push_sensor_summary_changes(&mut changes, before, after);
    push_event_changes(&mut changes, before, after);
    push_diagnostics_change(&mut changes, before, after);
    if changes.is_empty() && before != after {
        push_change(
            &mut changes,
            before.clone(),
            after.clone(),
            "compact_state_changed",
            "/".to_owned(),
            RealityTargetRef {
                kind: RealityTargetKind::Other,
                entity_id: None,
                field: Some("compact_state".to_owned()),
            },
        );
    }
    Ok(changes)
}

fn pending_reality_deltas(
    head: &RealityHeadRow,
    captured: &CapturedReality,
    changes: Vec<RealityChange>,
) -> Result<Vec<PendingRealityDelta>, ErrorData> {
    let mut pending = Vec::with_capacity(changes.len());
    let mut previous_seq = head.head_seq;
    let mut previous_hash = head.compact_state_hash.clone();
    for change in changes {
        let seq = previous_seq.saturating_add(1);
        let row_key = delta_row_key(&head.profile_key, &head.epoch_id, seq);
        let source_refs = source_refs_for_change(change.kind, &captured.source_refs);
        let delta = RealityDelta {
            epoch_id: head.epoch_id.clone(),
            seq,
            previous_seq,
            at: captured.observation.at,
            source: EventSource::Perception,
            kind: change.kind.to_owned(),
            path: change.path,
            target: change.target,
            before: change.before,
            after: change.after,
            confidence: 0.95,
            expected_previous_hash: Some(previous_hash.clone()),
            source_refs,
            correlations: Vec::new(),
            conflict: None,
            redaction: reality_redaction(),
        };
        delta.validate_append_order().map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("generated invalid reality delta: {error}"),
            )
        })?;
        previous_seq = seq;
        previous_hash = hash_json(&json!({
            "previous": previous_hash,
            "delta_row_key": row_key,
            "after": &delta.after,
        }))?;
        pending.push(PendingRealityDelta { row_key, delta });
    }
    Ok(pending)
}

fn push_foreground_changes(
    changes: &mut Vec<RealityChange>,
    before: &CompactForeground,
    after: &CompactForeground,
) {
    push_change(
        changes,
        before.hwnd,
        after.hwnd,
        "foreground_changed",
        "/foreground/hwnd".to_owned(),
        foreground_target("hwnd"),
    );
    push_change(
        changes,
        before.pid,
        after.pid,
        "foreground_changed",
        "/foreground/pid".to_owned(),
        foreground_target("pid"),
    );
    push_change(
        changes,
        before.process_name.clone(),
        after.process_name.clone(),
        "foreground_changed",
        "/foreground/process_name".to_owned(),
        foreground_target("process_name"),
    );
    push_change(
        changes,
        before.process_path_sha256.clone(),
        after.process_path_sha256.clone(),
        "foreground_changed",
        "/foreground/process_path_sha256".to_owned(),
        foreground_target("process_path_sha256"),
    );
    push_change(
        changes,
        before.window_title_sha256.clone(),
        after.window_title_sha256.clone(),
        "foreground_changed",
        "/foreground/window_title_sha256".to_owned(),
        foreground_target("window_title_sha256"),
    );
    push_change(
        changes,
        before.window_bounds.clone(),
        after.window_bounds.clone(),
        "foreground_changed",
        "/foreground/window_bounds".to_owned(),
        foreground_target("window_bounds"),
    );
    push_change(
        changes,
        before.monitor_index,
        after.monitor_index,
        "foreground_changed",
        "/foreground/monitor_index".to_owned(),
        foreground_target("monitor_index"),
    );
    push_change(
        changes,
        before.profile_id.clone(),
        after.profile_id.clone(),
        "foreground_changed",
        "/foreground/profile_id".to_owned(),
        foreground_target("profile_id"),
    );
    push_change(
        changes,
        before.is_fullscreen,
        after.is_fullscreen,
        "foreground_changed",
        "/foreground/is_fullscreen".to_owned(),
        foreground_target("is_fullscreen"),
    );
}

fn push_focus_changes(
    changes: &mut Vec<RealityChange>,
    before: Option<&CompactFocused>,
    after: Option<&CompactFocused>,
    foreground_translation: Option<RectTranslation>,
) {
    match (before, after) {
        (None, Some(after)) => push_change(
            changes,
            Option::<CompactFocused>::None,
            Some(after.clone()),
            "focus_changed",
            "/focused".to_owned(),
            focus_target("focused"),
        ),
        (Some(before), None) => push_change(
            changes,
            Some(before.clone()),
            Option::<CompactFocused>::None,
            "focus_changed",
            "/focused".to_owned(),
            focus_target("focused"),
        ),
        (Some(before), Some(after)) => {
            push_change(
                changes,
                before.element_id.clone(),
                after.element_id.clone(),
                "focus_changed",
                "/focused/element_id".to_owned(),
                focus_target("element_id"),
            );
            push_change(
                changes,
                before.name_sha256.clone(),
                after.name_sha256.clone(),
                "focus_changed",
                "/focused/name_sha256".to_owned(),
                focus_target("name_sha256"),
            );
            push_change(
                changes,
                before.role.clone(),
                after.role.clone(),
                "focus_changed",
                "/focused/role".to_owned(),
                focus_target("role"),
            );
            push_change(
                changes,
                before.automation_id.clone(),
                after.automation_id.clone(),
                "focus_changed",
                "/focused/automation_id".to_owned(),
                focus_target("automation_id"),
            );
            if !same_rect_translation(&before.bbox, &after.bbox, foreground_translation) {
                push_change(
                    changes,
                    before.bbox.clone(),
                    after.bbox.clone(),
                    "focus_changed",
                    "/focused/bbox".to_owned(),
                    focus_target("bbox"),
                );
            }
            push_change(
                changes,
                before.enabled,
                after.enabled,
                "focus_changed",
                "/focused/enabled".to_owned(),
                focus_target("enabled"),
            );
            push_change(
                changes,
                before.patterns.clone(),
                after.patterns.clone(),
                "focus_changed",
                "/focused/patterns".to_owned(),
                focus_target("patterns"),
            );
            push_change(
                changes,
                before.value_sha256.clone(),
                after.value_sha256.clone(),
                "focus_changed",
                "/focused/value_sha256".to_owned(),
                focus_target("value_sha256"),
            );
            push_change(
                changes,
                before.selected_text_sha256.clone(),
                after.selected_text_sha256.clone(),
                "focus_changed",
                "/focused/selected_text_sha256".to_owned(),
                focus_target("selected_text_sha256"),
            );
        }
        (None, None) => {}
    }
}

fn push_hud_changes(
    changes: &mut Vec<RealityChange>,
    before: &CompactRealityState,
    after: &CompactRealityState,
) {
    let hud_keys = before
        .hud
        .keys()
        .chain(after.hud.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for key in hud_keys {
        push_change(
            changes,
            before.hud.get(&key).cloned(),
            after.hud.get(&key).cloned(),
            "hud_field_changed",
            format!("/hud/{}", json_pointer_segment(&key)),
            RealityTargetRef {
                kind: RealityTargetKind::HudField,
                entity_id: None,
                field: Some(key),
            },
        );
    }
    let hud_error_keys = before
        .hud_errors
        .keys()
        .chain(after.hud_errors.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for key in hud_error_keys {
        push_change(
            changes,
            before.hud_errors.get(&key).cloned(),
            after.hud_errors.get(&key).cloned(),
            "hud_error_changed",
            format!("/hud_errors/{}", json_pointer_segment(&key)),
            RealityTargetRef {
                kind: RealityTargetKind::HudField,
                entity_id: None,
                field: Some(key),
            },
        );
    }
}

fn push_sensor_summary_changes(
    changes: &mut Vec<RealityChange>,
    before: &CompactRealityState,
    after: &CompactRealityState,
) {
    push_change(
        changes,
        before.audio.clone(),
        after.audio.clone(),
        "audio_summary_changed",
        "/audio".to_owned(),
        RealityTargetRef {
            kind: RealityTargetKind::Other,
            entity_id: None,
            field: Some("audio".to_owned()),
        },
    );
    push_change(
        changes,
        before.clipboard.clone(),
        after.clipboard.clone(),
        "clipboard_summary_changed",
        "/clipboard".to_owned(),
        RealityTargetRef {
            kind: RealityTargetKind::Other,
            entity_id: None,
            field: Some("clipboard".to_owned()),
        },
    );
    push_change(
        changes,
        before.fs.clone(),
        after.fs.clone(),
        "filesystem_summary_changed",
        "/fs".to_owned(),
        RealityTargetRef {
            kind: RealityTargetKind::Other,
            entity_id: None,
            field: Some("fs_recent".to_owned()),
        },
    );
}

fn push_event_changes(
    changes: &mut Vec<RealityChange>,
    before: &CompactRealityState,
    after: &CompactRealityState,
) {
    push_change(
        changes,
        log_cursor_change(&before.events),
        log_cursor_change(&after.events),
        "log_cursor_changed",
        "/events/log_cursor".to_owned(),
        RealityTargetRef {
            kind: RealityTargetKind::LogCursor,
            entity_id: None,
            field: None,
        },
    );
    push_change(
        changes,
        runtime_event_change(&before.events),
        runtime_event_change(&after.events),
        "runtime_event_changed",
        "/events/runtime".to_owned(),
        RealityTargetRef {
            kind: RealityTargetKind::Action,
            entity_id: None,
            field: Some("runtime_events".to_owned()),
        },
    );
}

fn push_diagnostics_change(
    changes: &mut Vec<RealityChange>,
    before: &CompactRealityState,
    after: &CompactRealityState,
) {
    push_change(
        changes,
        before.diagnostics.clone(),
        after.diagnostics.clone(),
        "diagnostics_changed",
        "/diagnostics".to_owned(),
        RealityTargetRef {
            kind: RealityTargetKind::Other,
            entity_id: None,
            field: Some("diagnostics".to_owned()),
        },
    );
}

fn push_element_changes(
    changes: &mut Vec<RealityChange>,
    before: &CompactRealityState,
    after: &CompactRealityState,
    foreground_translation: Option<RectTranslation>,
) -> Result<(), ErrorData> {
    let maps = uia_element_maps(before, after);
    let element_ids = maps
        .before_by_id
        .keys()
        .chain(maps.after_by_id.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let fanout = uia_element_fanout(&maps, foreground_translation);
    let mut structure_aggregate_emitted = false;
    for element_id in element_ids {
        let before_element = maps.before_by_id.get(&element_id);
        let after_element = maps.after_by_id.get(&element_id);
        match (before_element, after_element) {
            (None, Some(after_element)) => {
                if fanout.coalesce {
                    maybe_push_uia_structure_change(
                        changes,
                        before,
                        after,
                        &maps,
                        &fanout,
                        &mut structure_aggregate_emitted,
                    )?;
                } else {
                    push_change(
                        changes,
                        Option::<CompactElement>::None,
                        Some(after_element.clone()),
                        "uia_element_appeared",
                        format!("/elements/{}", json_pointer_segment(&element_id)),
                        ui_element_target(&element_id, "element"),
                    );
                }
            }
            (Some(before_element), None) => {
                if fanout.coalesce {
                    maybe_push_uia_structure_change(
                        changes,
                        before,
                        after,
                        &maps,
                        &fanout,
                        &mut structure_aggregate_emitted,
                    )?;
                } else {
                    push_change(
                        changes,
                        Some(before_element.clone()),
                        Option::<CompactElement>::None,
                        "uia_element_disappeared",
                        format!("/elements/{}", json_pointer_segment(&element_id)),
                        ui_element_target(&element_id, "element"),
                    );
                }
            }
            (Some(before_element), Some(after_element)) => {
                if fanout.coalesce && fanout.changed_id_set.contains(&element_id) {
                    maybe_push_uia_structure_change(
                        changes,
                        before,
                        after,
                        &maps,
                        &fanout,
                        &mut structure_aggregate_emitted,
                    )?;
                } else {
                    push_element_field_changes(
                        changes,
                        &element_id,
                        before_element,
                        after_element,
                        foreground_translation,
                    );
                }
            }
            (None, None) => {}
        }
    }
    Ok(())
}

fn uia_element_maps(before: &CompactRealityState, after: &CompactRealityState) -> UiaElementMaps {
    UiaElementMaps {
        before_by_id: before
            .elements
            .iter()
            .map(|element| (element.element_id.clone(), element.clone()))
            .collect(),
        after_by_id: after
            .elements
            .iter()
            .map(|element| (element.element_id.clone(), element.clone()))
            .collect(),
    }
}

fn uia_element_fanout(
    maps: &UiaElementMaps,
    foreground_translation: Option<RectTranslation>,
) -> UiaElementFanout {
    let appeared_ids = maps
        .after_by_id
        .keys()
        .filter(|element_id| !maps.before_by_id.contains_key(*element_id))
        .cloned()
        .collect::<Vec<_>>();
    let disappeared_ids = maps
        .before_by_id
        .keys()
        .filter(|element_id| !maps.after_by_id.contains_key(*element_id))
        .cloned()
        .collect::<Vec<_>>();
    let changed_ids = changed_uia_element_ids(maps, foreground_translation);
    let coalescing_changed_count =
        coalescing_uia_element_change_count(maps, foreground_translation);
    let changed_id_set = changed_ids.iter().cloned().collect::<BTreeSet<_>>();
    let structural_count = appeared_ids.len().saturating_add(disappeared_ids.len());
    let fanout_count = structural_count.saturating_add(coalescing_changed_count);
    UiaElementFanout {
        appeared_ids,
        disappeared_ids,
        changed_ids,
        changed_id_set,
        coalesce: fanout_count >= UIA_STRUCTURE_COALESCE_THRESHOLD,
    }
}

fn changed_uia_element_ids(
    maps: &UiaElementMaps,
    foreground_translation: Option<RectTranslation>,
) -> Vec<String> {
    maps.before_by_id
        .keys()
        .filter(|element_id| {
            match (
                maps.before_by_id.get(*element_id),
                maps.after_by_id.get(*element_id),
            ) {
                (Some(before_element), Some(after_element)) => compact_element_has_field_change(
                    before_element,
                    after_element,
                    foreground_translation,
                ),
                _ => false,
            }
        })
        .cloned()
        .collect()
}

fn coalescing_uia_element_change_count(
    maps: &UiaElementMaps,
    foreground_translation: Option<RectTranslation>,
) -> usize {
    maps.before_by_id
        .keys()
        .filter(|element_id| {
            match (
                maps.before_by_id.get(*element_id),
                maps.after_by_id.get(*element_id),
            ) {
                (Some(before_element), Some(after_element)) => {
                    compact_element_has_coalescing_field_change(
                        before_element,
                        after_element,
                        foreground_translation,
                    )
                }
                _ => false,
            }
        })
        .count()
}

fn maybe_push_uia_structure_change(
    changes: &mut Vec<RealityChange>,
    before: &CompactRealityState,
    after: &CompactRealityState,
    maps: &UiaElementMaps,
    fanout: &UiaElementFanout,
    emitted: &mut bool,
) -> Result<(), ErrorData> {
    if *emitted {
        return Ok(());
    }
    push_uia_structure_change(changes, before, after, maps, fanout)?;
    *emitted = true;
    Ok(())
}

fn push_uia_structure_change(
    changes: &mut Vec<RealityChange>,
    before: &CompactRealityState,
    after: &CompactRealityState,
    maps: &UiaElementMaps,
    fanout: &UiaElementFanout,
) -> Result<(), ErrorData> {
    let before_summary = uia_structure_aggregate(before, maps, fanout, false)?;
    let after_summary = uia_structure_aggregate(after, maps, fanout, true)?;
    let kind = if fanout.appeared_ids.is_empty() && fanout.disappeared_ids.is_empty() {
        "uia_elements_changed"
    } else {
        "uia_structure_changed"
    };
    push_change(
        changes,
        before_summary,
        after_summary,
        kind,
        "/elements".to_owned(),
        RealityTargetRef {
            kind: RealityTargetKind::Other,
            entity_id: None,
            field: Some("elements".to_owned()),
        },
    );
    Ok(())
}

fn uia_structure_aggregate(
    state: &CompactRealityState,
    maps: &UiaElementMaps,
    fanout: &UiaElementFanout,
    include_change: bool,
) -> Result<UiaStructureAggregate, ErrorData> {
    let appeared_elements = fanout
        .appeared_ids
        .iter()
        .filter_map(|element_id| maps.after_by_id.get(element_id))
        .collect::<Vec<_>>();
    let disappeared_elements = fanout
        .disappeared_ids
        .iter()
        .filter_map(|element_id| maps.before_by_id.get(element_id))
        .collect::<Vec<_>>();
    let before_changed_elements = fanout
        .changed_ids
        .iter()
        .filter_map(|element_id| maps.before_by_id.get(element_id))
        .collect::<Vec<_>>();
    let after_changed_elements = fanout
        .changed_ids
        .iter()
        .filter_map(|element_id| maps.after_by_id.get(element_id))
        .collect::<Vec<_>>();
    Ok(UiaStructureAggregate {
        element_count: saturating_u32(state.elements.len()),
        elements_hash: hash_json(&state.elements)?,
        appeared_count: if include_change {
            saturating_u32(fanout.appeared_ids.len())
        } else {
            0
        },
        disappeared_count: if include_change {
            saturating_u32(fanout.disappeared_ids.len())
        } else {
            0
        },
        changed_count: if include_change {
            saturating_u32(fanout.changed_ids.len())
        } else {
            0
        },
        appeared_ids: if include_change {
            capped_ids(&fanout.appeared_ids)
        } else {
            Vec::new()
        },
        disappeared_ids: if include_change {
            capped_ids(&fanout.disappeared_ids)
        } else {
            Vec::new()
        },
        changed_ids: if include_change {
            capped_ids(&fanout.changed_ids)
        } else {
            Vec::new()
        },
        appeared_ids_truncated: include_change && fanout.appeared_ids.len() > UIA_STRUCTURE_ID_CAP,
        disappeared_ids_truncated: include_change
            && fanout.disappeared_ids.len() > UIA_STRUCTURE_ID_CAP,
        changed_ids_truncated: include_change && fanout.changed_ids.len() > UIA_STRUCTURE_ID_CAP,
        appeared_elements_hash: if include_change && !appeared_elements.is_empty() {
            Some(hash_json(&appeared_elements)?)
        } else {
            None
        },
        disappeared_elements_hash: if include_change && !disappeared_elements.is_empty() {
            Some(hash_json(&disappeared_elements)?)
        } else {
            None
        },
        before_changed_elements_hash: if include_change && !before_changed_elements.is_empty() {
            Some(hash_json(&before_changed_elements)?)
        } else {
            None
        },
        after_changed_elements_hash: if include_change && !after_changed_elements.is_empty() {
            Some(hash_json(&after_changed_elements)?)
        } else {
            None
        },
        id_cap: saturating_u32(UIA_STRUCTURE_ID_CAP),
    })
}

fn compact_element_has_field_change(
    before: &CompactElement,
    after: &CompactElement,
    foreground_translation: Option<RectTranslation>,
) -> bool {
    before.name_sha256 != after.name_sha256
        || (!same_rect_translation(&before.bbox, &after.bbox, foreground_translation)
            && before.bbox != after.bbox)
        || before.parent != after.parent
        || before.role != after.role
        || before.automation_id != after.automation_id
        || before.enabled != after.enabled
        || before.focused != after.focused
        || before.patterns != after.patterns
        || before.children_count != after.children_count
        || before.depth != after.depth
}

fn compact_element_has_coalescing_field_change(
    before: &CompactElement,
    after: &CompactElement,
    foreground_translation: Option<RectTranslation>,
) -> bool {
    before.name_sha256 != after.name_sha256
        || (!same_rect_translation(&before.bbox, &after.bbox, foreground_translation)
            && before.bbox != after.bbox)
        || before.parent != after.parent
        || before.role != after.role
        || before.automation_id != after.automation_id
        || before.enabled != after.enabled
        || before.patterns != after.patterns
        || before.depth != after.depth
}

fn capped_ids(ids: &[String]) -> Vec<String> {
    ids.iter().take(UIA_STRUCTURE_ID_CAP).cloned().collect()
}

fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn push_element_field_changes(
    changes: &mut Vec<RealityChange>,
    element_id: &str,
    before: &CompactElement,
    after: &CompactElement,
    foreground_translation: Option<RectTranslation>,
) {
    let base = format!("/elements/{}", json_pointer_segment(element_id));
    push_change(
        changes,
        before.name_sha256.clone(),
        after.name_sha256.clone(),
        "uia_element_name_changed",
        format!("{base}/name_sha256"),
        ui_element_target(element_id, "name_sha256"),
    );
    if !same_rect_translation(&before.bbox, &after.bbox, foreground_translation) {
        push_change(
            changes,
            before.bbox.clone(),
            after.bbox.clone(),
            "uia_element_moved",
            format!("{base}/bbox"),
            ui_element_target(element_id, "bbox"),
        );
    }
    push_change(
        changes,
        before.parent.clone(),
        after.parent.clone(),
        "uia_element_changed",
        format!("{base}/parent"),
        ui_element_target(element_id, "parent"),
    );
    push_change(
        changes,
        before.role.clone(),
        after.role.clone(),
        "uia_element_changed",
        format!("{base}/role"),
        ui_element_target(element_id, "role"),
    );
    push_change(
        changes,
        before.automation_id.clone(),
        after.automation_id.clone(),
        "uia_element_changed",
        format!("{base}/automation_id"),
        ui_element_target(element_id, "automation_id"),
    );
    push_change(
        changes,
        before.enabled,
        after.enabled,
        "uia_element_changed",
        format!("{base}/enabled"),
        ui_element_target(element_id, "enabled"),
    );
    push_change(
        changes,
        before.focused,
        after.focused,
        "uia_element_changed",
        format!("{base}/focused"),
        ui_element_target(element_id, "focused"),
    );
    push_change(
        changes,
        before.patterns.clone(),
        after.patterns.clone(),
        "uia_element_changed",
        format!("{base}/patterns"),
        ui_element_target(element_id, "patterns"),
    );
    push_change(
        changes,
        before.children_count,
        after.children_count,
        "uia_element_changed",
        format!("{base}/children_count"),
        ui_element_target(element_id, "children_count"),
    );
    push_change(
        changes,
        before.depth,
        after.depth,
        "uia_element_changed",
        format!("{base}/depth"),
        ui_element_target(element_id, "depth"),
    );
}

fn push_entity_changes(
    changes: &mut Vec<RealityChange>,
    before: &CompactRealityState,
    after: &CompactRealityState,
) {
    let before_by_id = before
        .entities
        .iter()
        .map(|entity| (entity.entity_id.clone(), entity.clone()))
        .collect::<BTreeMap<_, _>>();
    let after_by_id = after
        .entities
        .iter()
        .map(|entity| (entity.entity_id.clone(), entity.clone()))
        .collect::<BTreeMap<_, _>>();
    let entity_ids = before_by_id
        .keys()
        .chain(after_by_id.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for entity_id in entity_ids {
        match (before_by_id.get(&entity_id), after_by_id.get(&entity_id)) {
            (None, Some(after_entity)) => push_change(
                changes,
                Option::<CompactEntity>::None,
                Some(after_entity.clone()),
                "entity_appeared",
                format!("/entities/{}", json_pointer_segment(&entity_id)),
                entity_target(&entity_id, "entity"),
            ),
            (Some(before_entity), None) => push_change(
                changes,
                Some(before_entity.clone()),
                Option::<CompactEntity>::None,
                "entity_disappeared",
                format!("/entities/{}", json_pointer_segment(&entity_id)),
                entity_target(&entity_id, "entity"),
            ),
            (Some(before_entity), Some(after_entity)) => {
                push_change(
                    changes,
                    before_entity.bbox.clone(),
                    after_entity.bbox.clone(),
                    "entity_moved",
                    format!("/entities/{}/bbox", json_pointer_segment(&entity_id)),
                    entity_target(&entity_id, "bbox"),
                );
                push_change(
                    changes,
                    before_entity.class_label.clone(),
                    after_entity.class_label.clone(),
                    "entity_class_changed",
                    format!("/entities/{}/class_label", json_pointer_segment(&entity_id)),
                    entity_target(&entity_id, "class_label"),
                );
                push_change(
                    changes,
                    before_entity.confidence_milli,
                    after_entity.confidence_milli,
                    "entity_confidence_changed",
                    format!("/entities/{}/confidence", json_pointer_segment(&entity_id)),
                    entity_target(&entity_id, "confidence"),
                );
                push_change(
                    changes,
                    before_entity.track_id,
                    after_entity.track_id,
                    "entity_track_changed",
                    format!("/entities/{}/track_id", json_pointer_segment(&entity_id)),
                    entity_target(&entity_id, "track_id"),
                );
            }
            (None, None) => {}
        }
    }
}

fn entity_target(entity_id: &str, field: &str) -> RealityTargetRef {
    RealityTargetRef {
        kind: RealityTargetKind::Entity,
        entity_id: Some(entity_id.to_owned()),
        field: Some(field.to_owned()),
    }
}

fn foreground_target(field: &str) -> RealityTargetRef {
    RealityTargetRef {
        kind: RealityTargetKind::Foreground,
        entity_id: None,
        field: Some(field.to_owned()),
    }
}

fn focus_target(field: &str) -> RealityTargetRef {
    RealityTargetRef {
        kind: RealityTargetKind::Focus,
        entity_id: None,
        field: Some(field.to_owned()),
    }
}

fn ui_element_target(element_id: &str, field: &str) -> RealityTargetRef {
    RealityTargetRef {
        kind: RealityTargetKind::UiElement,
        entity_id: Some(element_id.to_owned()),
        field: Some(field.to_owned()),
    }
}

fn log_cursor_change(events: &CompactEventCursor) -> CompactLogCursorChange {
    CompactLogCursorChange {
        path_sha256: events.log_path_sha256.clone(),
        start_offset: events.log_start_offset,
        next_offset: events.log_next_offset,
        file_len_bytes: events.log_file_len_bytes,
    }
}

fn runtime_event_change(events: &CompactEventCursor) -> CompactRuntimeEventChange {
    CompactRuntimeEventChange {
        event_count: events.event_count,
        latest_non_cursor_kind: events.latest_non_cursor_kind.clone(),
        latest_non_cursor_seq: events.latest_non_cursor_seq,
        latest_non_cursor_source: events.latest_non_cursor_source.clone(),
        latest_non_cursor_data_sha256: events.latest_non_cursor_data_sha256.clone(),
        latest_action_seq: events.latest_action_seq,
        latest_action_kind: events.latest_action_kind.clone(),
        latest_action_data_sha256: events.latest_action_data_sha256.clone(),
    }
}

fn rect_translation(before: &Value, after: &Value) -> Option<RectTranslation> {
    let before_rect = compact_rect(before)?;
    let after_rect = compact_rect(after)?;
    if before_rect.w != after_rect.w || before_rect.h != after_rect.h {
        return None;
    }
    let dx = after_rect.x.checked_sub(before_rect.x)?;
    let dy = after_rect.y.checked_sub(before_rect.y)?;
    if dx == 0 && dy == 0 {
        return None;
    }
    Some(RectTranslation { dx, dy })
}

fn same_rect_translation(
    before: &Value,
    after: &Value,
    translation: Option<RectTranslation>,
) -> bool {
    let Some(translation) = translation else {
        return false;
    };
    let Some(before_rect) = compact_rect(before) else {
        return false;
    };
    let Some(after_rect) = compact_rect(after) else {
        return false;
    };
    before_rect.w == after_rect.w
        && before_rect.h == after_rect.h
        && before_rect.x.checked_add(translation.dx) == Some(after_rect.x)
        && before_rect.y.checked_add(translation.dy) == Some(after_rect.y)
}

#[derive(Clone, Copy)]
struct CompactRectParts {
    x: i64,
    y: i64,
    w: i64,
    h: i64,
}

fn compact_rect(value: &Value) -> Option<CompactRectParts> {
    Some(CompactRectParts {
        x: value.get("x")?.as_i64()?,
        y: value.get("y")?.as_i64()?,
        w: value.get("w")?.as_i64()?,
        h: value.get("h")?.as_i64()?,
    })
}

fn push_change<T>(
    changes: &mut Vec<RealityChange>,
    before: T,
    after: T,
    kind: &'static str,
    path: String,
    target: RealityTargetRef,
) where
    T: Eq + Serialize,
{
    if before == after {
        return;
    }
    changes.push(RealityChange {
        kind,
        path,
        target,
        before: serde_json::to_value(before).unwrap_or(Value::Null),
        after: serde_json::to_value(after).unwrap_or(Value::Null),
    });
}

fn profile_changed_response(
    head: &RealityHeadRow,
    since_seq: u64,
    observed_profile_key: &str,
) -> ObserveDeltaResponse {
    ObserveDeltaResponse {
        ok: true,
        profile_key: Some(head.profile_key.clone()),
        epoch_id: Some(head.epoch_id.clone()),
        from_seq: Some(since_seq),
        to_seq: Some(head.head_seq),
        deltas: Vec::new(),
        cursor: Some(cursor(head, head.head_seq, false)),
        baseline_required: false,
        rebase_required: true,
        reason: Some(format!(
            "profile_changed: head profile {} but observed {observed_profile_key}",
            head.profile_key
        )),
        readback_rows: Vec::new(),
        published_sse_events: 0,
        size_bytes: 0,
        size_estimate_tokens: 0,
    }
}

fn stale_epoch_response(
    head: &RealityHeadRow,
    requested_epoch: &str,
    since_seq: Option<u64>,
) -> ObserveDeltaResponse {
    ObserveDeltaResponse {
        ok: true,
        profile_key: Some(head.profile_key.clone()),
        epoch_id: Some(head.epoch_id.clone()),
        from_seq: since_seq,
        to_seq: Some(head.head_seq),
        deltas: Vec::new(),
        cursor: Some(cursor(head, head.head_seq, false)),
        baseline_required: false,
        rebase_required: true,
        reason: Some(format!(
            "stale_epoch: requested {requested_epoch}, current {}",
            head.epoch_id
        )),
        readback_rows: Vec::new(),
        published_sse_events: 0,
        size_bytes: 0,
        size_estimate_tokens: 0,
    }
}

fn missing_baseline_response(
    profile_key: String,
    since_epoch: Option<String>,
    since_seq: Option<u64>,
    reason: &str,
) -> ObserveDeltaResponse {
    ObserveDeltaResponse {
        ok: true,
        profile_key: Some(profile_key),
        epoch_id: since_epoch,
        from_seq: since_seq,
        to_seq: since_seq,
        deltas: Vec::new(),
        cursor: None,
        baseline_required: true,
        rebase_required: true,
        reason: Some(reason.to_owned()),
        readback_rows: Vec::new(),
        published_sse_events: 0,
        size_bytes: 0,
        size_estimate_tokens: 0,
    }
}

fn cursor(head: &RealityHeadRow, since_seq: u64, has_more: bool) -> RealityCursor {
    RealityCursor {
        epoch_id: head.epoch_id.clone(),
        since_seq,
        head_seq: head.head_seq,
        has_more,
    }
}

fn select_profile(
    requested: Option<&str>,
    observation: &Observation,
) -> Result<ProfileSelection, ErrorData> {
    let requested = requested.map(validate_key_segment).transpose()?;
    let observed = observation.foreground.profile_id.clone();
    if let (Some(requested), Some(observed)) = (requested.as_deref(), observed.as_deref())
        && requested != observed
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("profile_id {requested:?} does not match observed profile {observed:?}"),
        ));
    }
    let profile_id = requested.or(observed);
    let profile_key = profile_id
        .as_deref()
        .map_or_else(|| UNPROFILED_PROFILE_KEY.to_owned(), ToOwned::to_owned);
    Ok(ProfileSelection {
        profile_id,
        profile_key,
    })
}

fn validate_key_segment(value: &str) -> Result<String, ErrorData> {
    let value = value.trim();
    if value.is_empty() {
        return Err(params_error("reality key segment must not be empty"));
    }
    if value.len() > 128 {
        return Err(params_error("reality key segment must be <= 128 bytes"));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(params_error(
            "reality key segment may contain only ASCII letters, digits, '.', '_', and '-'",
        ));
    }
    Ok(value.to_owned())
}

fn validate_assumption_hash(value: &str) -> Result<String, ErrorData> {
    let value = value.trim();
    if value.is_empty() {
        return Err(params_error(
            "assumption_hash must not be empty when provided",
        ));
    }
    if value.len() > 128 {
        return Err(params_error("assumption_hash must be <= 128 bytes"));
    }
    if value.chars().any(char::is_control) {
        return Err(params_error(
            "assumption_hash must not contain control characters",
        ));
    }
    Ok(value.to_owned())
}

fn bounded_max_deltas(value: u32) -> Result<usize, ErrorData> {
    if value == 0 || value > MAX_DELTAS {
        return Err(params_error(format!(
            "max_deltas must be between 1 and {MAX_DELTAS}"
        )));
    }
    usize::try_from(value).map_err(|_| params_error("max_deltas does not fit usize"))
}

fn bounded_depth(value: u32) -> Result<u32, ErrorData> {
    if value == 0 || value > MAX_DEPTH {
        return Err(params_error(format!(
            "depth must be between 1 and {MAX_DEPTH}"
        )));
    }
    Ok(value)
}

fn bounded_max_elements(value: usize) -> Result<usize, ErrorData> {
    if value == 0 || value > MAX_ELEMENTS {
        return Err(params_error(format!(
            "max_elements must be between 1 and {MAX_ELEMENTS}"
        )));
    }
    Ok(value)
}

fn source_surfaces(source_refs: &[SourceRef]) -> Vec<RealitySourceSurface> {
    let mut surfaces = Vec::new();
    for source in source_refs {
        if !surfaces.contains(&source.surface) {
            surfaces.push(source.surface);
        }
    }
    surfaces
}

fn reality_redaction() -> RedactionSummary {
    let mut redaction = RedactionSummary::default_private();
    redaction.redacted_fields = vec![
        "foreground.process_path".to_owned(),
        "foreground.window_title".to_owned(),
        "focused.name".to_owned(),
        "focused.value".to_owned(),
        "focused.selected_text".to_owned(),
        "elements.name".to_owned(),
        "hud.raw_text".to_owned(),
        "hud.errors.detail".to_owned(),
        "clipboard.text_excerpt".to_owned(),
        "fs_recent.path".to_owned(),
        "event.data_excerpt.raw_body".to_owned(),
    ];
    redaction.forbidden_raw_kinds = vec![
        ForbiddenRawDataKind::RawChatBody,
        ForbiddenRawDataKind::RawLogBody,
        ForbiddenRawDataKind::HighCardinalityPrivateData,
        ForbiddenRawDataKind::Secret,
        ForbiddenRawDataKind::Credential,
        ForbiddenRawDataKind::AccountIdentifier,
    ];
    redaction
}

fn rebase_reason(
    baseline_status: RealityBaselineStatus,
    drift_status: RealityDriftStatus,
) -> String {
    match (baseline_status, drift_status) {
        (RealityBaselineStatus::SourceUnavailable, _) => {
            "baseline row is missing; capture reality_baseline before consuming deltas".to_owned()
        }
        (RealityBaselineStatus::Stale, _) => {
            "requested epoch is stale; capture a fresh baseline".to_owned()
        }
        (_, RealityDriftStatus::RebaseRequired) => {
            "assumption hash differs from physical reality; rebase required".to_owned()
        }
        (_, RealityDriftStatus::MajorDrift) => {
            "major physical drift detected; capture a fresh baseline to rebase".to_owned()
        }
        (_, RealityDriftStatus::MinorDrift) => {
            "minor physical drift detected; capture a fresh baseline to rebase".to_owned()
        }
        (_, RealityDriftStatus::SourceUnavailable) => {
            "physical source unavailable; rebase required after source recovery".to_owned()
        }
        _ => "drift detected; rebase required".to_owned(),
    }
}

fn audit_drift_status_and_items(
    params: &RealityAuditParams,
    head: Option<&RealityHeadRow>,
    captured: &CapturedReality,
    baseline_status: RealityBaselineStatus,
    assumption_hash: &str,
) -> Result<(RealityDriftStatus, Vec<RealityDriftItem>), ErrorData> {
    let Some(head) = head else {
        return Ok((
            RealityDriftStatus::SourceUnavailable,
            vec![RealityDriftItem {
                path: "/baseline".to_owned(),
                assumed: Value::String("reality/head".to_owned()),
                actual: Value::Null,
                severity: RealityDriftStatus::SourceUnavailable,
                source_refs: captured.source_refs.clone(),
            }],
        ));
    };

    if baseline_status == RealityBaselineStatus::Stale {
        return Ok((
            RealityDriftStatus::RebaseRequired,
            vec![RealityDriftItem {
                path: "/epoch_id".to_owned(),
                assumed: params
                    .epoch_id
                    .as_ref()
                    .map_or(Value::Null, |epoch| Value::String(epoch.clone())),
                actual: Value::String(head.epoch_id.clone()),
                severity: RealityDriftStatus::RebaseRequired,
                source_refs: captured.source_refs.clone(),
            }],
        ));
    }

    if audit_source_unavailable(&captured.compact_state.diagnostics, &params.include) {
        return Ok((
            RealityDriftStatus::SourceUnavailable,
            vec![RealityDriftItem {
                path: "/diagnostics".to_owned(),
                assumed: serde_json::to_value(&head.compact_state.diagnostics)
                    .unwrap_or(Value::Null),
                actual: serde_json::to_value(&captured.compact_state.diagnostics)
                    .unwrap_or(Value::Null),
                severity: RealityDriftStatus::SourceUnavailable,
                source_refs: source_refs_for_change("diagnostics_changed", &captured.source_refs),
            }],
        ));
    }

    if assumption_hash != head.compact_state_hash {
        return Ok((
            RealityDriftStatus::RebaseRequired,
            vec![RealityDriftItem {
                path: "/compact_state_hash".to_owned(),
                assumed: Value::String(assumption_hash.to_owned()),
                actual: Value::String(head.compact_state_hash.clone()),
                severity: RealityDriftStatus::RebaseRequired,
                source_refs: captured.source_refs.clone(),
            }],
        ));
    }

    if head.compact_state_hash == captured.compact_state_hash {
        return Ok((RealityDriftStatus::InSync, Vec::new()));
    }

    let changes = reality_changes(&head.compact_state, &captured.compact_state)?;
    let mut drift_status = RealityDriftStatus::MinorDrift;
    let mut drift_items = Vec::with_capacity(changes.len());
    for change in changes {
        let severity = drift_severity_for_change(&change, &captured.compact_state.diagnostics);
        drift_status = max_drift_status(drift_status, severity);
        drift_items.push(RealityDriftItem {
            path: change.path,
            assumed: change.before,
            actual: change.after,
            severity,
            source_refs: source_refs_for_change(change.kind, &captured.source_refs),
        });
    }
    if drift_items.is_empty() {
        drift_items.push(RealityDriftItem {
            path: "/compact_state_hash".to_owned(),
            assumed: Value::String(head.compact_state_hash.clone()),
            actual: Value::String(captured.compact_state_hash.clone()),
            severity: drift_status,
            source_refs: captured.source_refs.clone(),
        });
    }
    Ok((drift_status, drift_items))
}

fn audit_source_unavailable(diagnostics: &CompactDiagnostics, include: &[ObserveSlot]) -> bool {
    let slots = reality_observe_params(include, 0, 0).include;
    let includes_a11y = slots.iter().any(|slot| {
        matches!(
            slot,
            ObserveSlot::Focused
                | ObserveSlot::Elements
                | ObserveSlot::Hud
                | ObserveSlot::Diagnostics
        )
    });
    let includes_capture = slots.iter().any(|slot| {
        matches!(
            slot,
            ObserveSlot::Focused
                | ObserveSlot::Elements
                | ObserveSlot::Entities
                | ObserveSlot::Hud
                | ObserveSlot::Diagnostics
        )
    });
    let includes_detection = slots
        .iter()
        .any(|slot| matches!(slot, ObserveSlot::Entities | ObserveSlot::Diagnostics));
    let includes_audio = slots
        .iter()
        .any(|slot| matches!(slot, ObserveSlot::Audio | ObserveSlot::Diagnostics));

    (includes_a11y && sensor_source_unavailable(&diagnostics.a11y_status))
        || (includes_capture && sensor_source_unavailable(&diagnostics.capture_status))
        || (includes_detection && sensor_source_unavailable(&diagnostics.detection_status))
        || (includes_audio && sensor_source_unavailable(&diagnostics.audio_status))
}

fn sensor_source_unavailable(status: &str) -> bool {
    status == "unavailable" || status.starts_with("degraded_sensor_failed:")
}

fn drift_severity_for_change(
    change: &RealityChange,
    diagnostics: &CompactDiagnostics,
) -> RealityDriftStatus {
    if change.path == "/diagnostics"
        && audit_source_unavailable(diagnostics, &[ObserveSlot::Diagnostics])
    {
        return RealityDriftStatus::SourceUnavailable;
    }
    match change.kind {
        "foreground_changed" => match change.path.as_str() {
            "/foreground/window_bounds"
            | "/foreground/window_title_sha256"
            | "/foreground/monitor_index"
            | "/foreground/is_fullscreen" => RealityDriftStatus::MinorDrift,
            _ => RealityDriftStatus::MajorDrift,
        },
        "uia_structure_changed" | "uia_elements_changed" | "compact_state_changed" => {
            RealityDriftStatus::MajorDrift
        }
        "uia_element_appeared" | "uia_element_disappeared" => RealityDriftStatus::MajorDrift,
        "diagnostics_changed" => RealityDriftStatus::MinorDrift,
        _ => RealityDriftStatus::MinorDrift,
    }
}

fn max_drift_status(
    current: RealityDriftStatus,
    candidate: RealityDriftStatus,
) -> RealityDriftStatus {
    if drift_status_rank(candidate) > drift_status_rank(current) {
        candidate
    } else {
        current
    }
}

const fn drift_status_rank(status: RealityDriftStatus) -> u8 {
    match status {
        RealityDriftStatus::InSync => 0,
        RealityDriftStatus::MinorDrift => 1,
        RealityDriftStatus::MajorDrift => 2,
        RealityDriftStatus::RebaseRequired => 3,
        RealityDriftStatus::SourceUnavailable => 4,
    }
}

fn sensor_status_name(status: &synapse_core::SensorStatus) -> String {
    match status {
        synapse_core::SensorStatus::Healthy => "healthy".to_owned(),
        synapse_core::SensorStatus::DegradedLatency { .. } => "degraded_latency".to_owned(),
        synapse_core::SensorStatus::DegradedSensorFailed { reason_code } => {
            format!("degraded_sensor_failed:{reason_code}")
        }
        synapse_core::SensorStatus::Disabled => "disabled".to_owned(),
        synapse_core::SensorStatus::Unavailable => "unavailable".to_owned(),
    }
}

const fn event_source_name(source: EventSource) -> &'static str {
    match source {
        EventSource::A11yUia => "a11y_uia",
        EventSource::A11yWinEvent => "a11y_win_event",
        EventSource::A11yCdp => "a11y_cdp",
        EventSource::Perception => "perception",
        EventSource::PerceptionDetection => "perception_detection",
        EventSource::PerceptionHud => "perception_hud",
        EventSource::PerceptionAudio => "perception_audio",
        EventSource::Filesystem => "filesystem",
        EventSource::Process => "process",
        EventSource::Clipboard => "clipboard",
        EventSource::ActionEmitter => "action_emitter",
        EventSource::Reflex => "reflex",
        EventSource::System => "system",
    }
}

const fn fs_event_kind_name(kind: synapse_core::FsEventKind) -> &'static str {
    match kind {
        synapse_core::FsEventKind::Created => "created",
        synapse_core::FsEventKind::Modified => "modified",
        synapse_core::FsEventKind::Deleted => "deleted",
        synapse_core::FsEventKind::Renamed => "renamed",
    }
}

fn signed_milli(value: f32) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    let scaled = (value * 1000.0).round();
    match format!("{scaled:.0}").parse::<i32>() {
        Ok(value) => value,
        Err(_) if scaled.is_sign_negative() => i32::MIN,
        Err(_) => i32::MAX,
    }
}

fn confidence_milli(value: f32) -> u32 {
    if !value.is_finite() {
        return 0;
    }
    let scaled = (value.clamp(0.0, 1.0) * 1000.0).round();
    if scaled >= 1000.0 {
        1000
    } else if scaled <= 0.0 {
        0
    } else {
        format!("{scaled:.0}").parse::<u32>().unwrap_or(0)
    }
}

fn len_to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Keyed (HMAC-SHA256, per-install key) digest of user-derived text, trimmed.
/// Plain SHA-256 of window titles / field values / element names is
/// brute-forceable (#136); equality comparison is preserved because the key is
/// stable per install.
fn non_empty_keyed_hash(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| keyed_hash_bytes(value.as_bytes()))
}

/// Keyed digest of a serialized structure. Used for every hash that covers
/// observation content (focused values, element names, HUD text, event data).
fn hash_json<T>(value: &T) -> Result<String, ErrorData>
where
    T: Serialize,
{
    let bytes = serde_json::to_vec(value).map_err(|error| encode_value_error(&error))?;
    Ok(keyed_hash_bytes(&bytes))
}

fn keyed_hash_bytes(bytes: &[u8]) -> String {
    format!(
        "{KEYED_HASH_PREFIX}{}",
        crate::content_digest::keyed_digest_hex(bytes)
    )
}

fn reality_row_value(row: &impl Serialize) -> Result<Value, ErrorData> {
    serde_json::to_value(row).map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("encode reality row: {error}"),
        )
    })
}

fn write_reality_rows_with(
    rows: &[(String, Value)],
    commit: impl FnOnce(Vec<(Vec<u8>, Vec<u8>)>) -> Result<(), ErrorData>,
    mut read: impl FnMut(&[u8]) -> Result<Option<Vec<u8>>, ErrorData>,
) -> Result<Vec<RealityRowReadback>, ErrorData> {
    let encoded = rows
        .iter()
        .map(|(key, value)| {
            synapse_storage::encode_json(value)
                .map(|bytes| (key.as_bytes().to_vec(), bytes))
                .map_err(|error| {
                    mcp_error(
                        error_codes::TOOL_INTERNAL_ERROR,
                        format!("encode reality row {key}: {error}"),
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    commit(encoded)?;
    rows.iter()
        .map(|(key, _)| {
            let stored = read(key.as_bytes())?.ok_or_else(|| {
                mcp_error(
                    error_codes::STORAGE_READ_FAILED,
                    format!("reality row missing after commit: {key}"),
                )
            })?;
            Ok(RealityRowReadback {
                cf_name: cf::CF_KV.to_owned(),
                row_key: key.clone(),
                value_len_bytes: stored.len() as u64,
                value_sha256: hash_bytes(&stored),
            })
        })
        .collect()
}

// Plain SHA-256 on purpose (#174): only the physical-row readbacks
// (`value_sha256` of reality baseline/delta rows) use it. Those rows already
// hold keyed digests instead of raw user content, so the row digest is
// structural integrity evidence. Every path or text digest uses the keyed form.
fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn json_size_estimate<T>(value: &T) -> Result<(u32, u32), ErrorData>
where
    T: Serialize,
{
    let len = serde_json::to_vec(value)
        .map_err(|error| encode_value_error(&error))?
        .len();
    let size_bytes = u32::try_from(len).unwrap_or(u32::MAX);
    Ok((size_bytes, size_bytes.div_ceil(4)))
}

fn decode_json_row<T>(bytes: &[u8], label: &str) -> Result<T, ErrorData>
where
    T: DeserializeOwned,
{
    serde_json::from_slice(bytes).map_err(|error| {
        mcp_error(
            error_codes::STORAGE_CORRUPTED,
            format!("decode {label}: {error}"),
        )
    })
}

fn encode_value_error(error: &serde_json::Error) -> ErrorData {
    mcp_error(
        error_codes::TOOL_INTERNAL_ERROR,
        format!("encode reality compact state: {error}"),
    )
}

fn params_error(message: impl Into<String>) -> ErrorData {
    mcp_error(error_codes::TOOL_PARAMS_INVALID, message)
}

fn baseline_row_key(profile_key: &str, epoch_id: &str) -> String {
    format!("reality/baseline/v1/{profile_key}/{epoch_id}")
}

fn delta_row_key(profile_key: &str, epoch_id: &str, seq: u64) -> String {
    format!("reality/delta/v1/{profile_key}/{epoch_id}/{seq:020}")
}

fn delta_prefix(profile_key: &str, epoch_id: &str) -> String {
    format!("reality/delta/v1/{profile_key}/{epoch_id}/")
}

fn audit_row_key(profile_key: &str, audit_id: &str) -> String {
    format!("reality/audit/v1/{profile_key}/{audit_id}")
}

fn head_key(profile_key: &str) -> String {
    format!("reality/head/v1/{profile_key}")
}

/// Why a profile must rebase when its stored head row cannot be decoded (#234).
const UNDECODABLE_HEAD_REASON: &str =
    "stored head row is undecodable; call reality baseline to rebuild it";

/// A profile's stored head row as read by this build (#234).
enum HeadLookup {
    Present(Box<RealityHeadRow>),
    Absent,
    /// A row exists but this build cannot decode it; the profile must rebase.
    Undecodable,
}

impl HeadLookup {
    fn into_head(self) -> Option<RealityHeadRow> {
        match self {
            Self::Present(head) => Some(*head),
            Self::Absent | Self::Undecodable => None,
        }
    }

    const fn is_undecodable(&self) -> bool {
        matches!(self, Self::Undecodable)
    }
}

/// Decodes a stored head row. A row this build cannot decode (for example one
/// written by another build with extra fields) is derived state: it is treated
/// as absent, so its profile rebases and scans of other profiles keep working
/// (#234).
pub(super) fn decode_head_row(key: &str, bytes: &[u8]) -> Option<RealityHeadRow> {
    const MAX_WARNED_REALITY_HEAD_KEYS: usize = 1024;
    static WARNED: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeSet<String>>> =
        std::sync::OnceLock::new();
    let warned = WARNED.get_or_init(Default::default);
    match serde_json::from_slice(bytes) {
        Ok(head) => {
            if let Ok(mut keys) = warned.lock() {
                keys.remove(key);
            }
            Some(head)
        }
        Err(error) => {
            let first = warned.lock().is_ok_and(|mut keys| {
                keys.len() < MAX_WARNED_REALITY_HEAD_KEYS && keys.insert(key.to_owned())
            });
            if first {
                tracing::warn!(
                    code = "REALITY_HEAD_ROW_UNDECODABLE",
                    key,
                    %error,
                    "reality head row is undecodable; treating it as absent so the profile rebases"
                );
            }
            None
        }
    }
}

fn new_epoch_id() -> String {
    format!(
        "epoch-{:020}-{:010}",
        now_ts_ns(),
        NEXT_REALITY_EPOCH_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

fn new_audit_id() -> String {
    format!(
        "audit-{:020}-{:010}",
        now_ts_ns(),
        NEXT_REALITY_AUDIT_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

fn now_ts_ns() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

fn json_pointer_segment(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn required_reality_write_permissions() -> RequiredPermissions {
    required([
        Permission::ReadStorage,
        Permission::WriteStorage,
        Permission::ReadEvents,
    ])
}

const fn default_false() -> bool {
    false
}

const fn default_depth() -> u32 {
    DEFAULT_DEPTH
}

const fn default_max_elements() -> usize {
    DEFAULT_MAX_ELEMENTS
}

const fn default_max_deltas() -> u32 {
    DEFAULT_MAX_DELTAS
}

fn default_include() -> Vec<ObserveSlot> {
    ObserveParams::default().include
}

fn reality_observe_params(
    include: &[ObserveSlot],
    depth: u32,
    max_elements: usize,
) -> ObserveParams {
    ObserveParams {
        // Serde supplies default slots only when include is omitted.
        include: include.to_vec(),
        depth: Some(depth),
        max_elements: Some(max_elements),
        ..ObserveParams::default()
    }
}

#[cfg(test)]
#[path = "tests/reality_profile_switch_156.rs"]
mod reality_profile_switch_156_tests;

#[cfg(test)]
#[path = "tests/reality_keyed_digests_136.rs"]
mod reality_keyed_digests_136_tests;

#[cfg(test)]
#[path = "tests/reality_undecodable_head_234.rs"]
mod reality_undecodable_head_234_tests;

#[cfg(test)]
#[path = "tests/reality_test_support.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/reality_corrupt_warning_216.rs"]
mod reality_corrupt_warning_216;

#[cfg(test)]
#[path = "tests/reality_atomic_rows_253.rs"]
mod reality_atomic_rows_253;

#[cfg(test)]
#[path = "tests/reality_agent_scope_299.rs"]
mod reality_agent_scope_299_tests;
