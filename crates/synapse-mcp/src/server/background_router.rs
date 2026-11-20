//! `target_act` (#1005): a compact, high-level capability-preserving computer-use
//! router.
//!
//! The raw tool surface is large, and model priors make low-level primitive
//! selection brittle and foreground-prone. `target_act` gives agents one
//! intent-named verb that routes to the correct session-targeted primitive: a
//! background/target path when that satisfies the task, an agent logical
//! foreground/foreground-lane path when foreground-equivalent semantics are
//! required, and never an implicit fallback to the human OS foreground. It is a
//! thin dispatcher: each verb delegates to the existing tool method, inheriting
//! that tool's target resolution, action audit (#1006), and lease/foreground
//! guards (#999/#1004) - so a normal (leaseless) session can drive its owned
//! target but cannot seize the human foreground.

use super::browser_field::BrowserSetValueParams;
use super::delivery_claim::DeliveryClaim;
use super::m1_tools::validate_cdp_target_id;
use super::m2_tools::{
    BackgroundTargetForegroundGuard, background_click_foreground_lost_error,
    merge_background_foreground_error,
};
use super::{
    CdpTargetOwner, ErrorData, Json, Parameters, SessionTarget, SynapseService, tool, tool_router,
};
use crate::m1::{
    CaptureScreenshotParams, CdpActivateTabParams, CdpNavigateAction, CdpNavigateTabParams,
    CdpTargetInfoParams, ObserveParams, mcp_error,
};
use crate::m2::{
    ActClickParams, ActFocusWindowParams, ActPressParams, ActScrollParams, ActScrollPoint,
    ActSetFieldTextLocator, ActSetFieldTextParams, ActTypeParams, act_press_normalized_labels,
    default_auto_wait_timeout_ms, default_verify_timeout_ms,
};
use crate::m4::{ActRunShellExecutionMode, ActRunShellParams};
use crate::server::element_root::ElementRoot;
use rmcp::schemars::{JsonSchema, schema_for};
use rmcp::{RoleServer, model::ErrorCode, service::RequestContext};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use synapse_core::{AccessibleNode, ElementId, Point, Rect, UiaPattern, error_codes};

#[path = "route_memory.rs"]
mod route_memory;

#[path = "act_route.rs"]
mod act_route;

use act_route::{ActBackend, ActLocator, ActRoute, ActTarget, ActVerb};

#[path = "element_scope.rs"]
mod element_scope;

use element_scope::{ElementScope, ScopeElement, ScopeTarget, ScopeVerdict};

#[path = "drag_route.rs"]
// #2350: `target_act_drag` reads its facts with Windows-only readers.
#[cfg_attr(not(windows), allow(dead_code))]
mod drag_route;

#[cfg(windows)]
use drag_route::{DragArea, DragBackend, DragEnd, DragPath, DragRoute};

const DEFAULT_TARGET_ACT_SHELL_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_TARGET_ACT_FOCUS_STABLE_MS: u32 = 75;
const DEFAULT_TARGET_ACT_SAVE_TIMEOUT_MS: u32 = 2_000;
const DEFAULT_TARGET_ACT_CLEANUP_NOTEPAD_TABS_TIMEOUT_MS: u32 = 5_000;
const TARGET_ACT_SAVE_POLL_INTERVAL_MS: u64 = 50;
const TARGET_ACT_SAVE_SOURCE_OF_TRUTH: &str = "file.bytes";
const TARGET_ACT_CLEANUP_NOTEPAD_TABS_SOURCE_OF_TRUTH: &str = "hidden_desktop.notepad_tabs.uia";
pub(super) const TARGET_ACT_STATUS_OK: &str = "ok";
pub(super) const TARGET_ACT_STATUS_VERIFY_NEEDED: &str = "verify_needed";
pub(super) const TARGET_ACT_STATUS_REFUSED: &str = "refused";
const TARGET_ACT_STATUS_ERROR: &str = "error";
/// #9: transient foreground refusal (lock screen / foreground lock); nothing was
/// delivered, the target and claim are preserved, the caller may retry.
pub(super) const TARGET_ACT_STATUS_DEFERRED: &str = "deferred";
const TARGET_ACT_KNOWN_VERBS: &str = "read, screenshot, navigate, set_field, insert_text, append_text, set_selection, click, dblclick, hover, tap, drag, scroll, dispatch_event, clear, focus, blur, select_text, check, uncheck, type, key, press, select, submit, save, cleanup_notepad_tabs, run_shell, focus_window, set_window_bounds";
const ACT_FACADE_SOURCE_OF_TRUTH: &str = "act facade CF_ACTION_LOG command audit row + target/action audit row + post-action target readback + synapse_action input lease + daemon-tool-events.jsonl";
const TARGET_ACT_SECRET_SAFE_REDACTION_POLICY: &str = "target_act_secret_safe_v1";

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ActOperation {
    #[default]
    Invoke,
    Foreground,
    LeaseAcquire,
    LeaseStatus,
    LeaseRelease,
    // #2326: read / delete the learned per-control delivery-tier memory.
    RouteMemoryStatus,
    RouteMemoryClear,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ActParams {
    #[serde(default)]
    operation: ActOperation,
    #[serde(default)]
    action: Option<TargetActParams>,
    #[serde(default)]
    reason: Option<String>,
    /// Foreground input lease lifetime for operation=foreground or
    /// operation=lease_acquire. Must be in [100, 30000].
    #[serde(default)]
    #[schemars(range(min = 100, max = 30000))]
    ttl_ms: Option<u64>,
    /// #258, operation=foreground only: `true` with a non-empty `reason`
    /// records that the owner explicitly asked for this foreground action
    /// while using the PC; it bypasses the user-activity gate and is audited.
    #[serde(default)]
    owner_approved: Option<bool>,
    /// #2326, operation=route_memory_status / route_memory_clear only: the
    /// target app's process image name (e.g. `notepad.exe`); omit to address
    /// every app. Rejected by every other operation.
    #[serde(default)]
    exe: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ActResponse {
    operation: ActOperation,
    source_of_truth: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action: Option<TargetActResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    foreground: Option<ActForegroundEscalation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease: Option<super::lease_tools::ControlLeaseResponse>,
    /// #2326: route-memory readback of route_memory_status / route_memory_clear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    route_memory: Option<route_memory::RouteMemoryReadback>,
}

#[derive(Clone, Debug, JsonSchema)]
#[schemars(transparent)]
pub struct TargetActVerb(String);

impl<'de> Deserialize<'de> for TargetActVerb {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Self(raw.trim().to_ascii_lowercase()))
    }
}

impl TargetActVerb {
    fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetActParams {
    /// The high-level operation to perform on the session target.
    pub verb: TargetActVerb,
    /// Internal marker set only by the audited `act operation=foreground`
    /// wrapper for an explicit keyboard chord. It is deliberately absent from
    /// the public JSON contract: foreground delivery must be an explicit facade
    /// operation with its own lease, reason, target-claim, and focus checks.
    #[serde(skip)]
    #[schemars(skip)]
    force_foreground_key_delivery: bool,
    /// `navigate`: destination URL.
    #[serde(default)]
    pub url: Option<String>,
    /// `screenshot`: output file path. `save`: existing document file path
    /// used as the physical Source of Truth after the target-scoped save.
    #[serde(default)]
    pub path: Option<String>,
    /// Observed element id (from observe/find) for set_field, click, check, or uncheck.
    /// Browser DOM actions use observed CDP ids when possible and otherwise
    /// treat this as a page element id.
    #[serde(default)]
    pub element_id: Option<String>,
    /// `set_field` / browser DOM action: strict CSS selector routed to the safe
    /// normal-Chrome bridge (background, no foreground, no DOM/action debugger attach).
    #[serde(default)]
    pub selector: Option<String>,
    /// `set_field`: full replacement text (empty clears the field). `save`:
    /// optional expected file contents for the post-save file-byte readback.
    /// `insert_text` / `append_text`: text to type through the target-scoped
    /// keyboard path.
    #[serde(default)]
    pub text: Option<String>,
    /// `key` / `press`: raw key or chord, e.g. `Ctrl+End`, `Tab`, `Ctrl+Z`.
    /// Prefer this over `keys` when a single chord string is easier for the
    /// caller to produce.
    #[serde(default)]
    pub key: Option<String>,
    /// `key` / `press`: explicit raw key list, e.g. `["ctrl", "end"]`.
    #[serde(default)]
    pub keys: Vec<String>,
    /// `set_selection`: zero-based selection start offset.
    #[serde(default, alias = "start")]
    pub selection_start: Option<u32>,
    /// `set_selection`: zero-based selection end offset.
    #[serde(default, alias = "end")]
    pub selection_end: Option<u32>,
    /// `set_field`: native/UIA role to resolve at action time. Browser DOM
    /// action: accessible/ARIA role to resolve.
    #[serde(default)]
    pub role: Option<String>,
    /// `set_field`: native/UIA accessible name to resolve at action time.
    /// Browser DOM action: accessible name to resolve.
    #[serde(default)]
    pub name: Option<String>,
    /// `set_field`: native/UIA automation id to resolve at action time.
    #[serde(default)]
    pub automation_id: Option<String>,
    /// Browser DOM action: value match. For `select`, this is the option value
    /// when `option` is omitted. `cleanup_notepad_tabs`: modified-tab policy
    /// (`discard_modified` default, or `refuse_modified`).
    #[serde(default)]
    pub value: Option<String>,
    /// `select`: option text or option value.
    #[serde(default)]
    pub option: Option<String>,
    /// `select`: option label/text. Use when the select element itself is
    /// located separately and the option must be matched by label.
    #[serde(default, alias = "optionLabel", alias = "label")]
    pub option_label: Option<String>,
    /// `select`: zero-based option index.
    #[serde(default, alias = "optionIndex", alias = "index")]
    pub option_index: Option<i32>,
    /// `select`: one or more explicit option specs for single or multi-select.
    /// Each entry must contain exactly one of `value`, `label`, or `index`.
    #[serde(default)]
    pub options: Vec<TargetActSelectOption>,
    /// `dispatch_event`: DOM event type to dispatch on the matched element.
    #[serde(default, alias = "eventType")]
    pub event_type: Option<String>,
    /// `dispatch_event`: EventInit/CustomEventInit-style JSON object. `detail`
    /// creates a CustomEvent; common mouse/keyboard/input event types use their
    /// specialized constructors when available.
    #[serde(default, alias = "eventInit")]
    pub event_init: Option<Value>,
    /// `click`: click count for target element clicks. Defaults to 1; valid range is 1..=3.
    /// `dblclick` defaults to 2 and rejects any other count.
    #[serde(default, alias = "clickCount")]
    pub clicks: Option<u8>,
    /// `click` / `dblclick`: mouse button for browser/native click delivery.
    #[serde(default)]
    pub button: Option<TargetActMouseButton>,
    /// `click` / `dblclick`: modifier keys held while dispatching the click.
    #[serde(default)]
    pub modifiers: Vec<TargetActClickModifier>,
    /// `click` / `dblclick`: element-relative click position X in CSS pixels
    /// for browser DOM locator clicks. Use nested `position` for Playwright-style
    /// input, or `position_x`/`position_y` aliases for flat callers.
    #[serde(default, alias = "positionX", alias = "offset_x", alias = "offsetX")]
    pub position_x: Option<i32>,
    /// `click` / `dblclick`: element-relative click position Y in CSS pixels.
    #[serde(default, alias = "positionY", alias = "offset_y", alias = "offsetY")]
    pub position_y: Option<i32>,
    /// `click` / `dblclick`: Playwright-style element-relative position.
    /// With `coordinate_space` and no top-level `x`/`y`, this is accepted as the
    /// coordinate pair for target-owned coordinate clicks.
    #[serde(default)]
    pub position: Option<TargetActClickPosition>,
    /// `click` / `type`: coordinate X for target-owned coordinate fallback.
    /// Defaults to screen coordinates; set coordinate_space for viewport/window-relative input.
    #[serde(default)]
    pub x: Option<i32>,
    /// `click` / `type`: coordinate Y for target-owned coordinate fallback.
    /// Defaults to screen coordinates; set coordinate_space for viewport/window-relative input.
    #[serde(default)]
    pub y: Option<i32>,
    /// `set_window_bounds`: requested outer-window width in screen pixels. Omit to
    /// preserve the current width (move-only). Must be > 0 when supplied.
    #[serde(default)]
    pub width: Option<i32>,
    /// `set_window_bounds`: requested outer-window height in screen pixels. Omit to
    /// preserve the current height (move-only). Must be > 0 when supplied.
    #[serde(default)]
    pub height: Option<i32>,
    /// `click` / `type`: coordinate space for x/y. `screen` uses desktop pixels,
    /// `window` uses the target outer-window origin, and `viewport` uses page client pixels.
    #[serde(default)]
    pub coordinate_space: Option<TargetActCoordinateSpace>,
    /// Browser DOM action readback wait budget (ms). Defaults to the browser
    /// bridge command budget and is capped by the daemon command timeout.
    #[serde(default)]
    pub wait_timeout_ms: Option<u64>,
    /// Browser DOM action / delegated CDP field action: opt in to polling
    /// actionability before dispatch. Default false preserves existing behavior.
    #[serde(default)]
    pub auto_wait: bool,
    /// Browser action output mode for secret-producing pages. When true,
    /// Synapse suppresses page-text collection in the Chrome bridge and returns
    /// only structural state plus hashes/lengths for any redacted scalar fields.
    #[serde(
        default,
        alias = "secretSafe",
        alias = "suppress_page_text",
        alias = "suppressPageText"
    )]
    pub secret_safe: bool,
    /// Auto-wait timeout in milliseconds when auto_wait=true.
    #[serde(default = "default_auto_wait_timeout_ms")]
    #[schemars(default = "default_auto_wait_timeout_ms", range(min = 50, max = 30000))]
    pub auto_wait_timeout_ms: u32,
    /// `run_shell`: executable/program name (arguments go in `args`).
    #[serde(default)]
    pub command: Option<String>,
    /// `run_shell`: literal arguments. `scroll`: [dx, dy] wheel notches; `["0", "-1"]` is one notch down.
    #[serde(default)]
    pub args: Vec<String>,
    /// `run_shell`: working directory.
    #[serde(default)]
    pub working_dir: Option<String>,
    /// `run_shell`: inline wait budget (ms). Defaults to 30000.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// `drag`: element id from find of the end (the start is `element_id`).
    #[serde(default)]
    pub to_element_id: Option<String>,
    /// `drag` on a Chrome bridge tab: CSS selector of the end (the start is
    /// `selector`).
    #[serde(default)]
    pub to_selector: Option<String>,
    /// `drag`: end point X in the target's own coordinates (the start is
    /// `x`/`y`): a window counts from its top-left corner, a raw-CDP page in
    /// viewport CSS pixels.
    #[serde(default)]
    pub to_x: Option<i32>,
    /// `drag`: end point Y, as `to_x`.
    #[serde(default)]
    pub to_y: Option<i32>,
    /// `drag`: duration in milliseconds, 1..=10000; default 350.
    #[serde(default)]
    pub duration_ms: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetActSelectOption {
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub index: Option<i32>,
}

#[derive(Copy, Clone, Debug, Deserialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TargetActMouseButton {
    Left,
    Right,
    Middle,
}

impl TargetActMouseButton {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetActClickModifier {
    Ctrl,
    Shift,
    Alt,
    Meta,
}

impl<'de> Deserialize<'de> for TargetActClickModifier {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de;

        let raw = String::deserialize(deserializer)?;
        match raw.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Ok(Self::Ctrl),
            "shift" => Ok(Self::Shift),
            "alt" | "option" => Ok(Self::Alt),
            "meta" | "super" | "cmd" | "command" => Ok(Self::Meta),
            other => Err(de::Error::custom(format!(
                "unsupported click modifier {other:?}; expected ctrl, shift, alt, or meta"
            ))),
        }
    }
}

impl TargetActClickModifier {
    const fn as_bridge_str(self) -> &'static str {
        match self {
            Self::Ctrl => "ctrl",
            Self::Shift => "shift",
            Self::Alt => "alt",
            Self::Meta => "meta",
        }
    }

    const fn as_act_click_str(self) -> &'static str {
        match self {
            Self::Ctrl => "ctrl",
            Self::Shift => "shift",
            Self::Alt => "alt",
            Self::Meta => "super",
        }
    }
}

#[derive(Copy, Clone, Debug, Deserialize, JsonSchema, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TargetActClickPosition {
    pub x: i32,
    pub y: i32,
}

#[derive(Copy, Clone, Debug, Deserialize, JsonSchema, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TargetActCoordinateSpace {
    Screen,
    Window,
    Viewport,
}

impl TargetActCoordinateSpace {
    const fn as_bridge_str(self) -> &'static str {
        match self {
            Self::Screen => "screen",
            Self::Window => "window",
            Self::Viewport => "viewport",
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct TargetActCoordinate {
    x: i32,
    y: i32,
    space: TargetActCoordinateSpace,
}

fn target_act_params_schema_defs() -> Map<String, Value> {
    let schema = serde_json::to_value(schema_for!(TargetActParams))
        .unwrap_or_else(|error| panic!("target_act params schema should serialize: {error}"));
    let mut schema = match schema {
        Value::Object(object) => object,
        _ => panic!("target_act params schema should be an object"),
    };
    schema.remove("$schema");
    let mut defs = match schema.remove("$defs") {
        Some(Value::Object(defs)) => defs,
        Some(other) => panic!("target_act params $defs should be an object, got {other}"),
        None => Map::new(),
    };
    defs.insert("TargetActParams".to_owned(), Value::Object(schema));
    defs
}

fn act_input_schema() -> Arc<Map<String, Value>> {
    let target_act_defs = target_act_params_schema_defs();
    let min_ttl_ms = synapse_action::MIN_LEASE_TTL_MS;
    let max_ttl_ms = synapse_action::MAX_LEASE_TTL_MS;
    let default_ttl_ms = synapse_action::DEFAULT_LEASE_TTL_MS;
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "$defs": target_act_defs,
        "properties": {
            "operation": {
                "type": "string",
                "enum": ["invoke", "foreground", "lease_acquire", "lease_status", "lease_release", "route_memory_status", "route_memory_clear"],
                "default": "invoke",
                "description": "Act facade operation. Omit only for the default invoke operation."
            },
            "action": {
                "anyOf": [
                    { "$ref": "#/$defs/TargetActParams" },
                    { "type": "null" }
                ],
                "description": "Target action payload. Accepted by operation=invoke and operation=foreground only."
            },
            "reason": {
                "type": ["string", "null"],
                "minLength": 1,
                "description": "Foreground escalation reason. Required by operation=foreground; rejected by invoke and lease operations."
            },
            "ttl_ms": {
                "type": ["integer", "null"],
                "minimum": min_ttl_ms,
                "maximum": max_ttl_ms,
                "default": default_ttl_ms,
                "description": "Foreground input lease lifetime in milliseconds. Accepted by operation=foreground and operation=lease_acquire."
            },
            "owner_approved": {
                "type": ["boolean", "null"],
                "description": "operation=foreground only. true (with a non-empty reason) records that the owner explicitly asked for this foreground action while using the PC: it bypasses the USER_ACTIVE / USER_ACTIVITY_UNKNOWN user-activity gate (never USER_WORKSTATION_LOCKED) and is audited with the reason. Rejected by invoke and lease operations."
            },
            "exe": {
                "type": ["string", "null"],
                "minLength": 1,
                "description": "operation=route_memory_status / route_memory_clear only: the target app's process image name (e.g. notepad.exe). Omit to address every app's route memory. Rejected by every other operation."
            }
        },
        "oneOf": [
            {
                "title": "act operation=invoke",
                "type": "object",
                "additionalProperties": false,
                "required": ["action"],
                "properties": {
                    "operation": {
                        "type": "string",
                        "const": "invoke",
                        "default": "invoke",
                        "description": "Default target-scoped action operation."
                    },
                    "action": { "$ref": "#/$defs/TargetActParams" }
                }
            },
            {
                "title": "act operation=foreground",
                "type": "object",
                "additionalProperties": false,
                "required": ["operation", "action", "reason"],
                "properties": {
                    "operation": {
                        "type": "string",
                        "const": "foreground",
                        "description": "Run one action through the audited foreground escalation path."
                    },
                    "action": { "$ref": "#/$defs/TargetActParams" },
                    "reason": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Required non-empty foreground escalation reason."
                    },
                    "ttl_ms": { "$ref": "#/properties/ttl_ms" },
                    "owner_approved": { "$ref": "#/properties/owner_approved" }
                }
            },
            {
                "title": "act operation=lease_acquire",
                "type": "object",
                "additionalProperties": false,
                "required": ["operation"],
                "properties": {
                    "operation": {
                        "type": "string",
                        "const": "lease_acquire",
                        "description": "Acquire the foreground input lease for this MCP session."
                    },
                    "ttl_ms": { "$ref": "#/properties/ttl_ms" }
                }
            },
            {
                "title": "act operation=lease_status",
                "type": "object",
                "additionalProperties": false,
                "required": ["operation"],
                "properties": {
                    "operation": {
                        "type": "string",
                        "const": "lease_status",
                        "description": "Read the foreground input lease status."
                    }
                }
            },
            {
                "title": "act operation=lease_release",
                "type": "object",
                "additionalProperties": false,
                "required": ["operation"],
                "properties": {
                    "operation": {
                        "type": "string",
                        "const": "lease_release",
                        "description": "Release this session's foreground input lease."
                    }
                }
            },
            {
                "title": "act operation=route_memory_status",
                "type": "object",
                "additionalProperties": false,
                "required": ["operation"],
                "properties": {
                    "operation": {
                        "type": "string",
                        "const": "route_memory_status",
                        "description": "Read the learned per-control delivery-tier memory (CF_KV route-memory/v1/) for one app (exe) or every app; bounded listing with per-tier counts."
                    },
                    "exe": { "$ref": "#/properties/exe" }
                }
            },
            {
                "title": "act operation=route_memory_clear",
                "type": "object",
                "additionalProperties": false,
                "required": ["operation"],
                "properties": {
                    "operation": {
                        "type": "string",
                        "const": "route_memory_clear",
                        "description": "Delete the learned delivery-tier memory of one app (exe) or every app; the result names the CF_KV prefix, the rows deleted and the row count read back."
                    },
                    "exe": { "$ref": "#/properties/exe" }
                }
            }
        ]
    });
    match schema {
        Value::Object(object) => Arc::new(object),
        _ => Arc::new(Map::new()),
    }
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetActResponse {
    pub verb: String,
    /// True when the delegated primitive completed without error; delivery_state
    /// separately reports whether a postcondition readback verified the effect.
    pub ok: bool,
    /// `ok`, `verify_needed`, `refused`, `deferred` (retryable, see `result.retry_after_ms`), or `error`.
    pub status: String,
    /// `delivered_and_verified`, `delivered_and_pixel_verified`,
    /// `delivered_unverified`, `refused_before_delivery`, `deferred_without_delivery`,
    /// or `failed_before_delivery`.
    pub delivery_state: String,
    /// The background primitive this verb routed to.
    pub delegated_tool: String,
    pub routing: String,
    /// The delegated tool's full response.
    pub result: Value,
    /// #2326: the learned route of this control (native element clicks);
    /// absent when route memory is off or holds no row for the control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_memory: Option<route_memory::RouteMemoryReply>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActForegroundParams {
    /// Why the real OS-foreground route is needed. Required, non-empty; audited.
    pub reason: String,
    /// The target_act action to run under the temporary escalation.
    pub action: TargetActParams,
    /// Foreground input lease TTL in ms while the action runs (default 30000).
    #[serde(default)]
    pub ttl_ms: Option<u64>,
    /// #258: `true` records that the owner explicitly asked for this action
    /// while using the PC; with the required non-empty `reason` it bypasses
    /// the user-activity gate for this escalation. Audited.
    #[serde(default)]
    pub owner_approved: Option<bool>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActForegroundEscalation {
    pub reason: String,
    /// True when this call newly acquired the lease (and therefore released it).
    pub acquired_lease: bool,
    pub released_lease: bool,
    pub prior_profile: String,
    pub escalated_to: String,
    pub restored_profile: String,
    pub profile_restored: bool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActForegroundResponse {
    pub ok: bool,
    pub action: TargetActResponse,
    pub escalation: ActForegroundEscalation,
}

#[tool_router(router = background_router_tool_router, vis = "pub(super)")]
impl SynapseService {
    #[tool(
        description = "Public action facade. operation=invoke routes one target-scoped action through target_act. operation=foreground runs the action through the audited foreground escalation path with a required non-empty reason; it is refused before any lease or input with USER_ACTIVE while the human used a physical keyboard/mouse within the quiet window (SYNAPSE_USER_ACTIVE_QUIET_MS, default 60000 ms), or USER_ACTIVITY_UNKNOWN when input monitoring is off, unless owner_approved=true records that the owner asked for this action; USER_WORKSTATION_LOCKED while the workstation is locked, even with owner_approved (refusals carry owner_presence, retry_after_ms and retry_when). With operation=foreground, verb=focus_window intentionally persists the new foreground; other focused actions restore prior foreground unless the human moved it. operation=lease_acquire/status/release exposes the foreground input lease as a facade route without adding raw control_lease_* tools to the public surface. operation=route_memory_status reads and operation=route_memory_clear deletes the learned per-control delivery-tier memory (CF_KV route-memory/v1/<exe>/) for one app (exe) or every app. Raw foreground primitives remain profile-gated; this facade only delegates to capability-preserving routes and returns the action/lease readback source of truth.",
        input_schema = act_input_schema()
    )]
    pub async fn act(
        &self,
        params: Parameters<ActParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActResponse>, ErrorData> {
        let params = params.0;
        let operation = params.operation;
        let operation_name = act_operation_name(operation);
        let actor_session_id =
            super::context::mcp_session_id_from_request_context(&request_context)?;
        let trace_verb = params
            .action
            .as_ref()
            .map(|action| action.verb.as_str())
            .unwrap_or("");
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act",
            operation = act_operation_name(params.operation),
            verb = trace_verb,
            "tool.invocation kind=act"
        );
        let command_payload = act_command_audit_payload(&params);
        let command_before = act_command_audit_before(operation);
        self.command_audit_intent(super::command_audit::CommandAuditInput::mcp(
            "act",
            operation_name,
            actor_session_id.clone(),
            None,
            command_payload.clone(),
            command_before.clone(),
            Value::Null,
            "pending",
        ))?;

        let result: Result<ActResponse, ErrorData> = async {
            if params.exe.is_some() && !route_memory::is_route_memory_operation(operation) {
                return Err(act_facade_error(
                    operation,
                    format!(
                        "act operation={} rejects exe; exe is only valid with route_memory_status or route_memory_clear",
                        act_operation_name(operation)
                    ),
                    "remove exe, or call act operation=route_memory_status / route_memory_clear",
                    "exe",
                ));
            }
            match operation {
                ActOperation::Invoke => {
                    validate_act_invoke_params(&params)?;
                    let action_params = require_act_action(&params, ActOperation::Invoke)?;
                    // A same-session action must not run under an in-flight
                    // foreground escalation's temporary profile and lease.
                    let _hold = actor_session_id
                        .as_deref()
                        .map(|session_id| {
                            super::act_foreground_slot::admit_session_operation(
                                session_id,
                                "act operation=invoke",
                            )
                        })
                        .transpose()?;
                    let action = self
                        .target_act(Parameters(action_params), request_context)
                        .await?
                        .0;
                    Ok(ActResponse {
                        operation: ActOperation::Invoke,
                        source_of_truth: ACT_FACADE_SOURCE_OF_TRUTH.to_owned(),
                        action: Some(action),
                        foreground: None,
                        lease: None,
                        route_memory: None,
                    })
                }
                ActOperation::Foreground => {
                    validate_act_foreground_params(&params)?;
                    let action_params = require_act_action(&params, ActOperation::Foreground)?;
                    let reason = params.reason.clone().ok_or_else(|| {
                        act_facade_error(
                            ActOperation::Foreground,
                            "act operation=foreground requires reason",
                            "pass a non-empty reason explaining why the audited foreground lane is required",
                            "reason",
                        )
                    })?;
                    let action_start_ns = super::user_activity::unix_now_ns();
                    let response = self
                        .act_foreground(
                            Parameters(ActForegroundParams {
                                reason,
                                action: action_params,
                                ttl_ms: params.ttl_ms,
                                owner_approved: params.owner_approved,
                            }),
                            request_context,
                        )
                        .await
                        .map_err(|error| owner_return_error(error, action_start_ns))?
                        .0;
                    Ok(ActResponse {
                        operation: ActOperation::Foreground,
                        source_of_truth: ACT_FACADE_SOURCE_OF_TRUTH.to_owned(),
                        action: Some(response.action),
                        foreground: Some(response.escalation),
                        lease: None,
                        route_memory: None,
                    })
                }
                ActOperation::LeaseAcquire => {
                    validate_act_lease_acquire_params(&params)?;
                    let ttl_ms = params
                        .ttl_ms
                        .unwrap_or(synapse_action::DEFAULT_LEASE_TTL_MS);
                    let lease = self
                        .control_lease_acquire(
                            Parameters(super::lease_tools::ControlLeaseAcquireParams { ttl_ms }),
                            request_context,
                        )
                        .await?
                        .0;
                    Ok(ActResponse {
                        operation: ActOperation::LeaseAcquire,
                        source_of_truth: ACT_FACADE_SOURCE_OF_TRUTH.to_owned(),
                        action: None,
                        foreground: None,
                        lease: Some(lease),
                        route_memory: None,
                    })
                }
                ActOperation::LeaseStatus => {
                    validate_act_lease_read_params(&params, ActOperation::LeaseStatus)?;
                    let lease = self.control_lease_status(request_context).await?.0;
                    Ok(ActResponse {
                        operation: ActOperation::LeaseStatus,
                        source_of_truth: ACT_FACADE_SOURCE_OF_TRUTH.to_owned(),
                        action: None,
                        foreground: None,
                        lease: Some(lease),
                        route_memory: None,
                    })
                }
                ActOperation::LeaseRelease => {
                    validate_act_lease_read_params(&params, ActOperation::LeaseRelease)?;
                    let lease = self.control_lease_release(request_context).await?.0;
                    Ok(ActResponse {
                        operation: ActOperation::LeaseRelease,
                        source_of_truth: ACT_FACADE_SOURCE_OF_TRUTH.to_owned(),
                        action: None,
                        foreground: None,
                        lease: Some(lease),
                        route_memory: None,
                    })
                }
                ActOperation::RouteMemoryStatus | ActOperation::RouteMemoryClear => {
                    route_memory::validate_route_memory_params(&params, operation)?;
                    let readback = if operation == ActOperation::RouteMemoryClear {
                        route_memory::route_memory_clear(self, params.exe.as_deref())?
                    } else {
                        route_memory::route_memory_status(self, params.exe.as_deref())?
                    };
                    Ok(ActResponse {
                        operation,
                        source_of_truth: readback.source_of_truth.clone(),
                        action: None,
                        foreground: None,
                        lease: None,
                        route_memory: Some(readback),
                    })
                }
            }
        }
        .await;

        match result {
            Ok(response) => {
                // #10: an outer Ok can wrap a refused/failed delegated action;
                // audit the delegated outcome, keep the structured response.
                let nested_failure = response.action.as_ref().and_then(|action| {
                    super::act_outcome::nested_action_failure(
                        action.ok,
                        &action.status,
                        &action.delegated_tool,
                        &action.result,
                    )
                });
                let mut audit = super::command_audit::CommandAuditInput::mcp(
                    "act",
                    operation_name,
                    actor_session_id.clone(),
                    None,
                    command_payload,
                    command_before,
                    act_command_audit_success_after(&response),
                    if nested_failure.is_some() {
                        "error"
                    } else {
                        "ok"
                    },
                );
                if let Some(failure) = nested_failure {
                    audit = audit.with_error(super::command_audit::CommandAuditError {
                        code: failure.code,
                        message: failure.message,
                        data: Some(failure.data),
                    });
                }
                self.command_audit_final(audit)?;
                Ok(Json(response))
            }
            Err(error) => {
                self.command_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "act",
                        operation_name,
                        actor_session_id,
                        None,
                        command_payload,
                        command_before,
                        act_command_audit_error_after(operation),
                        "error",
                    )
                    .with_error(
                        super::command_audit::command_audit_error_from_error_data(&error),
                    ),
                )?;
                Err(error)
            }
        }
    }

    #[tool(
        description = "High-level capability-preserving computer-use router (#1005/#1033/#1207/#1219/#1261/#1267/#1299/#1300). One verb, routed to the correct session-targeted primitive: background/target-scoped when sufficient, agent_logical_foreground/foreground_lane when foreground-equivalent semantics are required, and never implicit fallback to the human OS foreground. verb=read observes the target; verb=screenshot captures it; verb=navigate drives the owned browser target (Chrome bridge/CDP); verb=set_field replaces a web/UIA field's text by element id via target-capable tiers, by native/UIA role/name/automation_id resolved at action time, or by CSS selector through the safe normal-Chrome bridge; verb=insert_text replaces the current selection/caret text on an observed native editable element_id via exact native readback, or types text at the current caret after an optional target focus/click; verb=append_text appends to an observed native editable element_id via exact native readback, or moves the current caret to the end with Ctrl+End and types text; verb=set_selection sets an exact start/end selection on an observed web/native editable element; verb=click clicks by exact name and role on a native window or by observed element_id via the UIA pattern chain (Invoke, Toggle, SelectionItem, ExpandCollapse, LegacyIAccessible), by selector/role/name DOM action on browser targets, or by x/y coordinate on the owned target; verb=tap touch-taps a browser target element or viewport coordinate with Input.dispatchTouchEvent touchStart/touchEnd through raw CDP or the normal-profile Chrome bridge cdpInput lane, and never falls back to mouse click; verb=dispatch_event dispatches a caller-specified DOM event_type with event_init directly on a matched element through the session-owned normal Chrome bridge, bypassing actionability and reporting dispatchEvent's default_allowed result; verb=clear empties a matched editable element and fires input/change; verb=focus calls DOM.focus and verifies activeElement; verb=blur calls DOM.blur and verifies activeElement moved away; verb=select_text/selectText selects all text in the matched element and verifies the selection; verb=check/uncheck set the requested state and read it back via UIA TogglePattern on native windows or the DOM checked property on browser tabs; verb=type optionally focuses x/y then types text into the session-owned browser active element or leased foreground target; verb=key presses a raw key/chord such as Ctrl+End or Tab; verb=press presses a named button/link in the session-owned tab, or a raw key/chord when key/keys is supplied; verb=select chooses native <select> option(s) by value, label, or zero-based index via option/value/option_label/option_index/options[] and fires input/change; verb=submit calls HTMLFormElement.requestSubmit() for a matched form/submitter; verb=save persists an already-owned Notepad target to an existing file path and verifies file bytes as the Source of Truth; verb=cleanup_notepad_tabs removes stale restored tabs from an owned hidden-desktop Notepad target while keeping the requested file tab; verb=scroll takes args=[dx, dy] wheel notches, e.g. args=[0,-1] is one notch down; verb=run_shell runs a command in the session workspace; verb=focus_window intentionally activates the session target's top-level HWND only after the session is already break_glass/full_capability and holds the foreground input lease, so Codex clients can use an existing target_act schema when they cannot hot-add act_focus_window after tools/list_changed; verb=set_window_bounds moves/resizes the bound top-level window (native Window target, or the browser window behind a Cdp target) via background-safe SetWindowPos without activation, accepts x/y and/or width/height, and returns requested-vs-actual outer bounds (GetWindowRect readback) plus minimized state and size_satisfied so responsive-UI/layout FSV can drive a window through boundary sizes. Prefer this over raw act_* primitives: it inherits target resolution, action audit, lane/lease guards, and structured refusals, so a normal session can keep valid foreground-equivalent capability without seizing the human foreground. Mutating failures are returned as ok=false with status=verify_needed/refused/error and the original structured error in result; no optimistic success. Bind a target first with set_target (discover one with window_list/cdp_open_tab)."
    )]
    pub async fn target_act(
        &self,
        params: Parameters<TargetActParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<TargetActResponse>, ErrorData> {
        let params = params.0;
        let verb = params.verb.as_str().to_owned();
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "target_act",
            verb = verb.as_str(),
            "tool.invocation kind=target_act"
        );

        // #2348: the backend of a DOM-style verb (`ActRoute`): the Chrome bridge
        // only for a Chrome bridge tab; what the target cannot serve is refused
        // here, before any lane runs.
        let act_backend = match ActVerb::of(&params) {
            Some(act_verb) => {
                let target =
                    match super::context::mcp_session_id_from_request_context(&request_context)? {
                        Some(session_id) => self.session_target(Some(&session_id))?,
                        None => None,
                    };
                let target_hwnd = match &target {
                    Some(SessionTarget::Window { hwnd }) => Some(*hwnd),
                    Some(SessionTarget::Cdp { window_hwnd, .. }) => Some(*window_hwnd),
                    None => None,
                };
                #[cfg(windows)]
                let raw_cdp_endpoint = target_hwnd
                    .is_some_and(|hwnd| synapse_a11y::endpoint_for_window(hwnd).is_some());
                // #79: does the window target host a WebView2 page while another
                // window is in front? (A failed foreground read counts as behind.)
                #[cfg(windows)]
                let webview_behind = match &target {
                    Some(SessionTarget::Window { hwnd }) if !raw_cdp_endpoint => {
                        let in_front = matches!(
                            (
                                synapse_a11y::window_root_identity(*hwnd),
                                synapse_a11y::foreground_root_identity(),
                            ),
                            (Ok(own), Ok(front)) if own.root_hwnd == front.root_hwnd
                        );
                        !in_front
                            && synapse_a11y::webview2_cdp_diagnostics_for_window(*hwnd).is_some()
                    }
                    _ => false,
                };
                #[cfg(not(windows))]
                let (raw_cdp_endpoint, webview_behind) = {
                    let _ = target_hwnd;
                    (false, false)
                };
                let route = ActRoute::new(
                    act_verb,
                    ActTarget::of(target.as_ref(), raw_cdp_endpoint, webview_behind),
                    ActLocator::of(&params),
                    params.option_choice().is_ok(),
                );
                match route.backend() {
                    ActBackend::Refused { code, text } => return Err(mcp_error(code, text)),
                    backend => Some(backend),
                }
            }
            None => None,
        };

        // #2326: route memory of a native element click, planned before
        // dispatch and recorded at the reply tail.
        let mut click_route_memory: Option<route_memory::ClickRouteMemory> = None;
        let (delegated_tool, ok, status, result) = match verb.as_str() {
            "read" => {
                let session_id = target_act_session_id(&request_context, "read")?;
                let target = self.session_target(Some(&session_id))?;
                let request_details = json!({
                    "session_id": &session_id,
                    "verb": "read",
                    "requires_agent_logical_foreground": true,
                    "no_human_os_foreground_fallback": true,
                });
                match target_act_read_delegated_tool(target.as_ref()) {
                    Ok("cdp_target_info") => {
                        let response = self
                            .cdp_target_info(
                                Parameters(CdpTargetInfoParams {
                                    window_hwnd: None,
                                    cdp_target_id: None,
                                }),
                                request_context,
                            )
                            .await?;
                        (
                            "cdp_target_info",
                            true,
                            TARGET_ACT_STATUS_OK,
                            target_act_result(&response.0)?,
                        )
                    }
                    Ok("observe") => {
                        let response = self
                            .observe(Parameters(ObserveParams::default()), request_context)
                            .await?;
                        (
                            "observe",
                            true,
                            TARGET_ACT_STATUS_OK,
                            target_act_result(&response.0)?,
                        )
                    }
                    Ok(other) => {
                        return Err(mcp_error(
                            error_codes::TOOL_INTERNAL_ERROR,
                            format!("target_act read resolved unknown delegated tool {other:?}"),
                        ));
                    }
                    Err(error) => {
                        self.audit_action_denied_with_details_for_session(
                            "target_act",
                            &error,
                            &request_details,
                            &session_id,
                        );
                        (
                            "observe",
                            false,
                            target_act_error_status(&error),
                            target_act_error_result("target_act", error),
                        )
                    }
                }
            }
            "screenshot" => {
                let path = require_param(params.path, "screenshot", "path")?;
                let session_id = target_act_session_id(&request_context, "screenshot")?;
                let target = self.session_target(Some(&session_id))?;
                let request_details = json!({
                    "session_id": &session_id,
                    "verb": "screenshot",
                    "path": &path,
                    "requires_agent_logical_foreground": true,
                    "no_human_os_foreground_fallback": true,
                });
                match target {
                    Some(SessionTarget::Cdp {
                        window_hwnd,
                        cdp_target_id,
                    }) => {
                        let activated = self
                            .cdp_activate_tab(
                                Parameters(CdpActivateTabParams {
                                    window_hwnd: Some(window_hwnd),
                                    cdp_target_id: Some(cdp_target_id),
                                    wait_timeout_ms: params.wait_timeout_ms,
                                }),
                                request_context.clone(),
                            )
                            .await?;
                        let response = self
                            .capture_screenshot(
                                Parameters(CaptureScreenshotParams {
                                    path,
                                    region: None,
                                    window_hwnd: Some(window_hwnd),
                                    overwrite: true,
                                    max_pixels: None,
                                    max_long_edge: None,
                                }),
                                request_context,
                            )
                            .await?;
                        let mut result = target_act_result(&response.0)?;
                        if let Some(object) = result.as_object_mut() {
                            object.insert(
                                "target_act_visual_route".to_owned(),
                                json!("cdp_activate_tab_then_passive_window_capture"),
                            );
                            object.insert(
                                "activated_target".to_owned(),
                                target_act_result(&activated.0)?,
                            );
                        }
                        ("capture_screenshot", true, TARGET_ACT_STATUS_OK, result)
                    }
                    Some(SessionTarget::Window { .. }) => target_act_delegate_response(
                        "capture_screenshot",
                        self.capture_screenshot(
                            Parameters(CaptureScreenshotParams {
                                path,
                                region: None,
                                window_hwnd: None,
                                overwrite: true,
                                max_pixels: None,
                                max_long_edge: None,
                            }),
                            request_context,
                        )
                        .await,
                    )?,
                    None => {
                        let error = mcp_error(
                            error_codes::TARGET_NOT_SET,
                            "target_act verb=screenshot requires this MCP session to have an agent_logical_foreground/foreground_lane target; refusing capture_screenshot's legacy human OS foreground fallback",
                        );
                        self.audit_action_denied_with_details_for_session(
                            "target_act",
                            &error,
                            &request_details,
                            &session_id,
                        );
                        (
                            "capture_screenshot",
                            false,
                            target_act_error_status(&error),
                            target_act_error_result("target_act", error),
                        )
                    }
                }
            }
            "navigate" => {
                let url = require_param(params.url, "navigate", "url")?;
                let response = self
                    .cdp_navigate_tab(
                        Parameters(CdpNavigateTabParams {
                            window_hwnd: None,
                            cdp_target_id: None,
                            action: CdpNavigateAction::Navigate,
                            url: Some(url),
                            wait_timeout_ms: None,
                            ignore_cache: None,
                        }),
                        request_context,
                    )
                    .await?;
                (
                    "cdp_navigate_tab",
                    true,
                    TARGET_ACT_STATUS_OK,
                    target_act_result(&response.0)?,
                )
            }
            "set_field" => {
                if target_act_coordinate(&params)?.is_some() {
                    return Err(mcp_error(
                        error_codes::TOOL_PARAMS_INVALID,
                        "target_act verb=set_field does not accept x/y because set_field is a replacement operation; use verb=type with x/y for coordinate focus + keyboard text",
                    ));
                }
                target_act_set_field_with(&params, |route, text| async move {
                    Ok(match route {
                        TargetActSetFieldTarget::Browser {
                            selector,
                            element_id,
                        } => {
                            // Background-safe web field replace in the user's normal Chrome
                            // via the safe bridge (no foreground, no DOM/action debugger attach,
                            // no UIA) — the #1000/#1005 path for forms perceived UIA-only.
                            let response = self
                                .browser_set_value(
                                    Parameters(BrowserSetValueParams {
                                        text,
                                        selector,
                                        element_id,
                                        active_element: false,
                                        cdp_target_id: None,
                                        window_hwnd: None,
                                    }),
                                    request_context,
                                )
                                .await;
                            target_act_delegate_response("browser_set_value", response)?
                        }
                        TargetActSetFieldTarget::Native {
                            element_id,
                            locator,
                        } => {
                            // #2406: an element named by id must be inside this
                            // session's target (`ElementScope`); a refusal sends
                            // nothing.
                            if let Some(scoped_id) = element_id.as_ref()
                                && let ScopeVerdict::Refused { code, text } =
                                    target_act_element_scope(
                                        self,
                                        &request_context,
                                        scoped_id,
                                        "set_field",
                                    )
                                    .await?
                                    .verdict("set_field")
                            {
                                return Err(mcp_error(code, text));
                            }
                            let response = self
                                .act_set_field_text(
                                    Parameters(ActSetFieldTextParams {
                                        element_id,
                                        locator,
                                        text,
                                        verify_timeout_ms: default_verify_timeout_ms(),
                                        auto_wait: params.auto_wait,
                                        auto_wait_timeout_ms: params.auto_wait_timeout_ms,
                                    }),
                                    request_context,
                                )
                                .await;
                            target_act_delegate_response("act_set_field_text", response)?
                        }
                    })
                })
                .await?
            }
            "insert_text" => {
                target_act_insert_or_append_text(self, &params, &request_context, false).await?
            }
            "append_text" => {
                target_act_insert_or_append_text(self, &params, &request_context, true).await?
            }
            "set_selection" => target_act_set_selection(self, &params, &request_context).await?,
            action @ ("click" | "dblclick") => {
                if target_act_coordinate(&params)?.is_some() {
                    if target_act_has_any_locator(&params) {
                        return Err(mcp_error(
                            error_codes::TOOL_PARAMS_INVALID,
                            format!(
                                "target_act verb={action} accepts either x/y coordinates or an element/DOM locator, not both"
                            ),
                        ));
                    }
                    target_act_coordinate_click(self, action, &params, &request_context).await?
                } else if action == "click"
                    && let Some(result) =
                        target_act_try_native_pattern(self, &params, &request_context).await?
                {
                    result
                } else if target_act_has_dom_locator(&params)
                    && (act_backend == Some(ActBackend::ChromeBridge)
                        || params
                            .element_id
                            .as_deref()
                            .is_none_or(|id| id.trim().is_empty()))
                {
                    let bridge_action: &'static str = if action == "dblclick" {
                        "dblclick"
                    } else {
                        "click"
                    };
                    target_act_dom_locator_pointer(
                        self,
                        bridge_action,
                        bridge_action,
                        &params,
                        &request_context,
                    )
                    .await?
                } else {
                    let element_id =
                        require_param(params.element_id.clone(), action, "element_id")?;
                    if let Some(element_id) = target_act_legacy_click_element_id(&element_id)? {
                        if target_act_click_position(&params)?.is_some() {
                            return Err(mcp_error(
                                error_codes::TOOL_PARAMS_INVALID,
                                format!(
                                    "target_act verb={action} position is supported for browser DOM locators, not observed native/UIA element_id clicks"
                                ),
                            ));
                        }
                        // #2406: the element must be inside this session's target
                        // (`ElementScope`); a refusal sends nothing, like the
                        // route refusal at the top of `target_act`.
                        if let ScopeVerdict::Refused { code, text } =
                            target_act_element_scope(self, &request_context, &element_id, action)
                                .await?
                                .verdict(action)
                        {
                            return Err(mcp_error(code, text));
                        }
                        let clicks = target_act_click_count_for_action(action, params.clicks)?;
                        let mut click_params = target_act_click_params(
                            element_id.clone(),
                            clicks,
                            params.button,
                            &params.modifiers,
                        )?;
                        click_route_memory = route_memory::ClickRouteMemory::plan(
                            self,
                            action,
                            &element_id,
                            &click_params,
                        );
                        click_params.coordinate_first = click_route_memory
                            .as_ref()
                            .is_some_and(route_memory::ClickRouteMemory::coordinate_first);
                        let response = self
                            .act_click(Parameters(click_params), request_context)
                            .await;
                        target_act_delegate_response("act_click", response)?
                    } else {
                        target_act_browser_dom_action(self, action, &params, &request_context)
                            .await?
                    }
                }
            }
            "tap" => target_act_touch_tap(self, &params, &request_context).await?,
            "drag" => target_act_drag(self, &params, &request_context).await?,
            "hover" => target_act_hover(self, &params, &request_context).await?,
            "dispatch_event" | "dispatchevent" => {
                target_act_browser_dom_action(self, "dispatch_event", &params, &request_context)
                    .await?
            }
            "clear" => {
                target_act_browser_dom_primitive(self, "clear", &params, &request_context).await?
            }
            "focus" => {
                target_act_browser_dom_primitive(self, "focus", &params, &request_context).await?
            }
            "blur" => {
                target_act_browser_dom_primitive(self, "blur", &params, &request_context).await?
            }
            "select_text" | "selecttext" => {
                target_act_browser_dom_primitive(self, "select_text", &params, &request_context)
                    .await?
            }
            action @ ("check" | "uncheck") => {
                if let Some(result) =
                    target_act_try_native_pattern(self, &params, &request_context).await?
                {
                    result
                } else if act_backend == Some(ActBackend::ChromeBridge) {
                    target_act_browser_dom_action(self, action, &params, &request_context).await?
                } else {
                    return Err(mcp_error(
                        error_codes::TOOL_INTERNAL_ERROR,
                        format!(
                            "target_act verb={action}: the route chose {act_backend:?} but no native locator was built; nothing was sent"
                        ),
                    ));
                }
            }
            "type" => {
                if target_act_has_any_locator(&params) {
                    return Err(mcp_error(
                        error_codes::TOOL_PARAMS_INVALID,
                        "target_act verb=type accepts text plus optional x/y coordinate; use verb=set_field/click for element, selector, role, name, value, or option locators",
                    ));
                }
                let text = require_param(params.text.clone(), "type", "text")?;
                let verify_target_window_hwnd =
                    target_act_optional_session_target_window_hwnd(self, &request_context)?;
                let type_params = target_act_type_params(
                    text,
                    params.wait_timeout_ms,
                    verify_target_window_hwnd,
                )?;
                // Validate the typing request before a coordinate focus click or any
                // target/profile routing. Only the absolute size cap applies here: the
                // route (background CDP/bridge vs SendInput) is not known yet, and the
                // foreground time budget must not block background typing (#50). A
                // coordinate focus click is a foreground side effect, so with x/y the
                // foreground budget is applied before it; the foreground act_type path
                // enforces it again otherwise.
                if target_act_coordinate(&params)?.is_some() {
                    crate::m2::action_from_foreground_type_params(&type_params)?;
                } else {
                    crate::m2::action_from_type_params(&type_params)?;
                }
                #[cfg(windows)]
                let console = super::console_type::ConsoleType::for_request(
                    self.clone(),
                    super::context::mcp_session_id_from_request_context(&request_context)?,
                    type_params.text.clone(),
                    type_params.verify_timeout_ms,
                    target_act_coordinate(&params)?.is_some(),
                )?;
                #[cfg(windows)]
                let console_result = match console {
                    Some(console) => Some(console.execute().await),
                    None => None,
                };
                #[cfg(not(windows))]
                let console_result: Option<
                    Result<(bool, &'static str, Value), ErrorData>,
                > = None;
                if let Some(result) = console_result {
                    match result {
                        Ok((ok, status, value)) => ("console_type", ok, status, value),
                        Err(error) => {
                            target_act_delegate_response::<Value>("console_type", Err(error))?
                        }
                    }
                } else {
                    let coordinate_result = if target_act_coordinate(&params)?.is_some() {
                        let (delegated_tool, ok, status, result) =
                            target_act_coordinate_click(self, "click", &params, &request_context)
                                .await?;
                        if !ok {
                            return Ok(Json(TargetActResponse {
                                verb: verb.as_str().to_owned(),
                                ok,
                                status: status.to_owned(),
                                delivery_state: DeliveryClaim::without_result(ok, status)
                                    .state()
                                    .to_owned(),
                                delegated_tool: delegated_tool.to_owned(),
                                routing: target_act_routing_description(),
                                result,
                                route_memory: None,
                            }));
                        }
                        Some((delegated_tool, result))
                    } else {
                        None
                    };
                    let response = self
                        .act_type(Parameters(type_params), request_context)
                        .await;
                    let (_delegated_tool, ok, status, type_result) =
                        target_act_delegate_response("act_type", response)?;
                    if let Some((coordinate_tool, coordinate_result)) = coordinate_result {
                        (
                            "chrome_debugger_bridge.coordinateClick+act_type",
                            ok,
                            status,
                            json!({
                                "coordinate_delegated_tool": coordinate_tool,
                                "coordinate_click": coordinate_result,
                                "type": type_result,
                            }),
                        )
                    } else {
                        ("act_type", ok, status, type_result)
                    }
                }
            }
            "key" | "key_chord" => target_act_key_press(self, &params, &request_context).await?,
            "press" => {
                if target_act_has_key_chord(&params) {
                    target_act_key_press(self, &params, &request_context).await?
                } else {
                    // press a named button/link = a real trusted click on the
                    // bridge target; keep the synthetic "press" DOM-action only as
                    // the raw-CDP/native fallback (#1348 headline #2).
                    target_act_dom_locator_pointer(
                        self,
                        "click",
                        "press",
                        &params,
                        &request_context,
                    )
                    .await?
                }
            }
            "select" => match act_backend {
                // #2348/#2365: native and raw-CDP select pick ONE option.
                Some(ActBackend::NativeAccessibility | ActBackend::RawCdp)
                    if !params.options.is_empty() =>
                {
                    return Err(mcp_error(
                        error_codes::TOOL_PARAMS_INVALID,
                        "target_act verb=select picks one option on this target: options[] (multi-select) is not supported; give option, option_label, value or option_index; nothing was sent",
                    ));
                }
                Some(ActBackend::NativeAccessibility) => {
                    match target_act_try_native_pattern(self, &params, &request_context).await? {
                        Some(result) => result,
                        None => {
                            return Err(mcp_error(
                                error_codes::TOOL_INTERNAL_ERROR,
                                "target_act verb=select: the route chose native accessibility but no native locator was built; nothing was sent",
                            ));
                        }
                    }
                }
                Some(ActBackend::RawCdp) => {
                    target_act_raw_cdp_select(self, &params, &request_context).await?
                }
                _ => {
                    target_act_browser_dom_action(self, "select", &params, &request_context).await?
                }
            },
            "submit" => {
                target_act_browser_dom_action(self, "submit", &params, &request_context).await?
            }
            "save" => target_act_save(self, &params, &request_context).await?,
            "cleanup_notepad_tabs" => {
                target_act_cleanup_notepad_tabs(self, &params, &request_context).await?
            }
            "run_shell" => {
                let session_id = target_act_session_id(&request_context, "run_shell")?;
                self.admit_tool_call_for_profile("act_run_shell", Some(&session_id))?;
                let command = require_param(params.command, "run_shell", "command")?;
                let response = self
                    .act_run_shell(
                        Parameters(ActRunShellParams {
                            command,
                            args: params.args,
                            working_dir: params.working_dir,
                            original_working_dir: None,
                            env: BTreeMap::new(),
                            timeout_ms: params
                                .timeout_ms
                                .unwrap_or(DEFAULT_TARGET_ACT_SHELL_TIMEOUT_MS),
                            execution_mode: ActRunShellExecutionMode::Inline,
                            durable_timeout_ms: None,
                            idempotency_key: None,
                        }),
                        request_context,
                    )
                    .await?;
                (
                    "act_run_shell",
                    true,
                    TARGET_ACT_STATUS_OK,
                    target_act_result(&response.0)?,
                )
            }
            "focus_window" => {
                let session_id = target_act_session_id(&request_context, "focus_window")?;
                let request_details = json!({
                    "session_id": &session_id,
                    "verb": "focus_window",
                    "delegated_tool": "act_focus_window",
                    "requires_tool_profile": "break_glass_or_full_capability",
                    "requires_foreground_input_lease": true,
                    "target_source": "session_target",
                    "no_human_os_foreground_fallback": true,
                });
                // The facade is public; act_focus_window is intentionally
                // hidden from tools/list. Check the foreground capability of
                // the active profile, not visibility of its internal delegate.
                if let Err(error) =
                    self.tool_profile_snapshot(Some(&session_id))
                        .and_then(|snapshot| {
                            snapshot
                                .profile
                                .allows_foreground_tier()
                                .then_some(())
                                .ok_or_else(|| {
                                    mcp_error(
                                        error_codes::TOOL_PROFILE_POLICY_DENIED,
                                        "act focus_window requires break_glass or full_capability",
                                    )
                                })
                        })
                {
                    self.audit_action_denied_with_details_for_session(
                        "target_act",
                        &error,
                        &request_details,
                        &session_id,
                    );
                    (
                        "act_focus_window",
                        false,
                        target_act_error_status(&error),
                        target_act_error_result("act_focus_window", error),
                    )
                } else {
                    let target = self.session_target(Some(&session_id))?;
                    let target = match target {
                        Some(target) => target,
                        None => {
                            let error = target_act_focus_window_missing_target_error();
                            self.audit_action_denied_with_details_for_session(
                                "target_act",
                                &error,
                                &request_details,
                                &session_id,
                            );
                            return Ok(Json(TargetActResponse {
                                verb: verb.as_str().to_owned(),
                                ok: false,
                                status: target_act_error_status(&error).to_owned(),
                                delivery_state: DeliveryClaim::without_result(
                                    false,
                                    target_act_error_status(&error),
                                )
                                .state()
                                .to_owned(),
                                delegated_tool: "act_focus_window".to_owned(),
                                routing: target_act_routing_description(),
                                result: target_act_error_result("act_focus_window", error),
                                route_memory: None,
                            }));
                        }
                    };
                    if let Err(error) =
                        target_act_focus_window_preflight(self, &session_id, &target)
                    {
                        self.audit_action_denied_with_details_for_session(
                            "target_act",
                            &error,
                            &request_details,
                            &session_id,
                        );
                        return Ok(Json(TargetActResponse {
                            verb: verb.as_str().to_owned(),
                            ok: false,
                            status: target_act_error_status(&error).to_owned(),
                            delivery_state: DeliveryClaim::without_result(
                                false,
                                target_act_error_status(&error),
                            )
                            .state()
                            .to_owned(),
                            delegated_tool: "act_focus_window".to_owned(),
                            routing: target_act_routing_description(),
                            result: target_act_error_result("act_focus_window", error),
                            route_memory: None,
                        }));
                    }
                    let focus_params = match target_act_focus_window_params(Some(&target)) {
                        Ok(params) => params,
                        Err(error) => {
                            self.audit_action_denied_with_details_for_session(
                                "target_act",
                                &error,
                                &request_details,
                                &session_id,
                            );
                            return Ok(Json(TargetActResponse {
                                verb: verb.as_str().to_owned(),
                                ok: false,
                                status: target_act_error_status(&error).to_owned(),
                                delivery_state: DeliveryClaim::without_result(
                                    false,
                                    target_act_error_status(&error),
                                )
                                .state()
                                .to_owned(),
                                delegated_tool: "act_focus_window".to_owned(),
                                routing: target_act_routing_description(),
                                result: target_act_error_result("act_focus_window", error),
                                route_memory: None,
                            }));
                        }
                    };
                    let response = self
                        .act_focus_window(Parameters(focus_params), request_context)
                        .await;
                    target_act_delegate_response("act_focus_window", response)?
                }
            }
            "scroll" => target_act_scroll(self, &params, &request_context).await?,
            "set_window_bounds" => target_act_set_window_bounds(self, &params, &request_context)?,
            other => return Err(target_act_unknown_verb_error(other)),
        };

        let delivery_state = DeliveryClaim::new(verb.as_str(), ok, status, &result).state();
        // #2326: a route-memory write error never changes this reply.
        let route_memory =
            click_route_memory.and_then(|memory| memory.record(delivery_state, &result));
        Ok(Json(TargetActResponse {
            verb: verb.as_str().to_owned(),
            ok,
            status: status.to_owned(),
            delivery_state: delivery_state.to_owned(),
            delegated_tool: delegated_tool.to_owned(),
            routing: target_act_routing_description(),
            result,
            route_memory,
        }))
    }

    #[tool(
        description = "Run one target_act action under a single, fully-audited real-OS-foreground escalation (#1352). Given a non-empty `reason` and an `action` (a target_act params object incl. its `verb`), this atomically: acquires the foreground input lease (ttl_ms, default 30000), sets profile=break_glass, runs the action via target_act, then RESTORES the prior profile and releases the lease if it newly acquired it — so a session does not have to hand-stitch control_lease_acquire + tool_profile_set + the action + restore and guess the ordering. verb=focus_window intentionally keeps the new foreground window; other focused actions restore the prior foreground unless the human moved it. Returns the action result plus an escalation readback (acquired_lease, released_lease, prior/restored profile). The prior profile is restored even when the action fails. Use for actions that genuinely need the hardware foreground tier (e.g. Shift+selection, app accelerators); prefer plain target_act for background-capable work."
    )]
    pub async fn act_foreground(
        &self,
        params: Parameters<ActForegroundParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ActForegroundResponse>, ErrorData> {
        let params = params.0;
        let (reason, ttl_ms) = validate_act_foreground_entry(&params.reason, params.ttl_ms)?;
        let session_id = target_act_session_id(&request_context, "act_foreground")?;
        // #258: refuse while the human is (or may be) active, before the slot,
        // lease, profile or any input. An owner approval is scoped to this
        // call's escalation task only.
        let owner_approval =
            self.admit_foreground_escalation(&session_id, params.owner_approved == Some(true))?;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "act_foreground",
            verb = params.action.verb.as_str(),
            "tool.invocation kind=act_foreground"
        );
        // The escalation runs on its own task: if the client cancels this
        // request, the task still reaches the unwind instead of leaving the
        // session escalated and the lease held.
        let service = self.clone();
        let scoped_session_id = crate::http::current_mcp_session_id();
        // #299: the escalation task acts as the same agent as this call.
        let scoped_agent = super::agent_scope::AgentScope::current();
        tokio::spawn(crate::http::with_current_mcp_session_id(
            scoped_session_id,
            super::agent_scope::AgentScope::scope(scoped_agent, async move {
                let escalation = service.act_foreground_escalated(
                    params.action,
                    reason,
                    ttl_ms,
                    session_id,
                    request_context,
                );
                super::owner_override::ApprovedCall::scope(owner_approval, escalation).await
            }),
        ))
        .await
        .map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("act operation=foreground escalation task failed: {error}"),
            )
        })?
        .map(Json)
    }
}

/// Entry validation for `act operation=foreground`, run before any lease or
/// profile state changes. The reason is normalized with the same bound
/// `tool_profile_set` enforces, so an over-long reason is refused without side
/// effects; a blank reason is refused with `TOOL_PARAMS_INVALID`. Returns the
/// normalized reason and the validated lease TTL (default 30000 ms).
pub(super) fn validate_act_foreground_entry(
    reason: &str,
    ttl_ms: Option<u64>,
) -> Result<(String, u64), ErrorData> {
    let Some(reason) = super::tool_profiles::normalize_reason(Some(reason))? else {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "act_foreground requires a non-empty reason for the foreground escalation audit",
        ));
    };
    let ttl_ms = ttl_ms.unwrap_or(30_000);
    super::lease_tools::validate_lease_ttl_ms("act_foreground", ttl_ms)?;
    Ok((reason, ttl_ms))
}

/// Outcome of the `act operation=foreground` action step: the foreground
/// context-restore guard (when one was taken) and the action result.
pub(super) type ActForegroundStepOutcome = (
    Option<crate::m2::ForegroundInputLeaseGuard>,
    Result<Json<TargetActResponse>, ErrorData>,
);

impl SynapseService {
    /// Acquire → escalate → act → unwind, with the unwind on every exit.
    async fn act_foreground_escalated(
        &self,
        action: TargetActParams,
        reason: String,
        ttl_ms: u64,
        session_id: String,
        request_context: RequestContext<RoleServer>,
    ) -> Result<ActForegroundResponse, ErrorData> {
        let session_ref = session_id.as_str();
        let context_ref = &request_context;
        self.act_foreground_escalated_with(
            action,
            reason,
            ttl_ms,
            session_ref,
            context_ref,
            move |action| self.act_foreground_action_step(action, ttl_ms, session_ref, context_ref),
        )
        .await
    }

    /// [`Self::act_foreground_escalated`] with an injected action step
    /// (`run_action`). The lease acquire, break_glass escalation and unwind
    /// run live; the unwind runs on every exit, including a failed step.
    pub(super) async fn act_foreground_escalated_with<F, Fut>(
        &self,
        action: TargetActParams,
        reason: String,
        ttl_ms: u64,
        session_id: &str,
        request_context: &RequestContext<RoleServer>,
        run_action: F,
    ) -> Result<ActForegroundResponse, ErrorData>
    where
        F: FnOnce(TargetActParams) -> Fut,
        Fut: Future<Output = ActForegroundStepOutcome>,
    {
        let _in_flight = super::act_foreground_slot::ActForegroundSlot::claim(session_id)?;
        // Checked after claiming the slot: teardown marks a session terminated
        // (and shutdown marks the daemon draining) before it waits on the
        // slot, so a late escalation is refused here instead of racing cleanup.
        if self.session_closing(session_id) {
            return Err(mcp_error(
                error_codes::HTTP_SESSION_INVALID,
                format!(
                    "act operation=foreground refused: session {session_id} is terminated or the daemon is draining"
                ),
            ));
        }
        let prior_profile = self.tool_profile_snapshot(Some(session_id))?.profile;

        // 1) acquire/renew the foreground input lease. Whether this call newly
        //    acquired it comes from the acquisition itself, not a pre-read that
        //    could race lease expiry.
        let acquired = self
            .control_lease_acquire_inner(
                Parameters(super::lease_tools::ControlLeaseAcquireParams { ttl_ms }),
                request_context.clone(),
            )
            .await?
            .0
            .outcome
            == "acquired";
        let unwind = super::act_foreground_unwind::ForegroundUnwind {
            session_id,
            prior_profile,
            reason: &reason,
            acquired,
        };

        // 2) escalate to break_glass (requires the lease we just took + confirm + reason).
        //    Any failure (even after the profile row was written) unwinds both
        //    the profile and a newly acquired lease.
        if let Err(error) = self
            .tool_profile_set_for_foreground(
                super::tool_profiles::ToolProfileSetParams {
                    profile: super::tool_profiles::ToolProfileKind::BreakGlass,
                    reason: Some(reason.clone()),
                    confirm_break_glass: true,
                },
                request_context,
            )
            .await
        {
            let outcome = self.unwind_act_foreground(&unwind, request_context).await;
            if !outcome.complete {
                return Err(outcome.incomplete_error(&unwind));
            }
            return Err(error);
        }

        // 3) An explicit keyboard chord needs real foreground delivery. Preserve the
        // caller's context before deliberate focus, then use only native window
        // facts plus the verified focus operation. In particular, do not make
        // SendInput wait on a broad UIA semantic snapshot: WebView providers can
        // time out even when the claimed HWND is live, visible, and foreground.
        //
        // The action is bounded by the lease TTL plus slack (#53): the
        // escalation holds the daemon-wide escalation lock and session fence
        // until the unwind below, so a hung action must not outlive them. On
        // expiry the action is abandoned and the unwind still runs.
        //
        // Gate semantics (#53 part 1): `act operation=foreground` is the
        // sanctioned one-shot path from a lower profile to break_glass, so
        // requiring break_glass first would be circular. Its gate is the
        // explicit non-empty `reason`, validated at the tool entry before any
        // lease or profile state changes, plus the audited break_glass
        // profile set and the lease-ownership proof below.
        let (foreground_context_restore, action_result) = run_action(action).await;

        // Restore while this session still owns the foreground lease. The guard
        // independently refuses to overwrite a human foreground change.
        drop(foreground_context_restore);

        // 4) restore the prior profile and 5) release a newly acquired lease.
        let outcome = self.unwind_act_foreground(&unwind, request_context).await;

        if !outcome.complete {
            return Err(outcome.incomplete_error(&unwind));
        }
        let action = action_result?.0;
        Ok(ActForegroundResponse {
            ok: action.ok,
            escalation: ActForegroundEscalation {
                reason,
                acquired_lease: acquired,
                released_lease: outcome.released_lease,
                prior_profile: prior_profile.as_str().to_owned(),
                escalated_to: "break_glass".to_owned(),
                restored_profile: outcome.restored_profile,
                profile_restored: outcome.profile_restored,
            },
            action,
        })
    }
}

impl SynapseService {
    /// Live action step of `act operation=foreground` (step 3 of
    /// [`Self::act_foreground_escalated_with`]): focus the claimed target when
    /// the verb needs it, then run the action within the lease-TTL deadline.
    async fn act_foreground_action_step(
        &self,
        action: TargetActParams,
        ttl_ms: u64,
        session_id: &str,
        request_context: &RequestContext<RoleServer>,
    ) -> ActForegroundStepOutcome {
        let action_deadline = super::act_foreground_deadline::act_foreground_deadline(ttl_ms);
        let needs_foreground_key_delivery = target_act_has_key_chord(&action);
        // #162: every real-input verb (click/type/press/scroll...) in this lane
        // focuses the claimed target with the verified act_focus_window step and
        // then delivers through the foreground tier, instead of reaching the
        // background WebView2 preflight that belongs to operation=invoke.
        let needs_claimed_focus =
            super::act_foreground_focus::act_foreground_requires_claimed_focus(
                true,
                action.verb.as_str(),
                needs_foreground_key_delivery,
            );
        if !needs_claimed_focus {
            // focus_window intentionally persists the foreground change (#217).
            // This branch therefore acquires no context-restore guard.
            return (
                None,
                super::act_foreground_deadline::run_within_deadline(
                    action_deadline,
                    self.target_act(Parameters(action), request_context.clone()),
                )
                .await,
            );
        }
        match crate::m2::acquire_foreground_input_lease_with_ttl(
            "act_foreground",
            Some(session_id),
            ttl_ms,
        ) {
            Ok(mut guard) => {
                let action_result = super::act_foreground_deadline::run_within_deadline(
                    action_deadline,
                    self.act_foreground_focused_action(
                        action,
                        &mut guard,
                        needs_foreground_key_delivery,
                        session_id,
                        request_context,
                    ),
                )
                .await;
                (Some(guard), action_result)
            }
            // The profile/outer lease below must still unwind if a second
            // foreground guard cannot be obtained. Do not early-return with
            // this session left in break_glass.
            Err(error) => (None, Err(error)),
        }
    }

    /// Focus the claimed target, record that focus as the lane's own on
    /// `guard`, then run the action through the foreground tier.
    async fn act_foreground_focused_action(
        &self,
        mut action: TargetActParams,
        guard: &mut crate::m2::ForegroundInputLeaseGuard,
        needs_foreground_key_delivery: bool,
        session_id: &str,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<Json<TargetActResponse>, ErrorData> {
        if let Err(error) = self
            .act_foreground_focus_key_target(session_id, request_context)
            .await
        {
            // #9: a transient activation refusal is a typed
            // deferral that keeps the target, not an error.
            return if crate::m2::foreground_deferral::is_deferred(&error) {
                Ok(Json(target_act_deferred_response(
                    action.verb.as_str(),
                    error,
                )))
            } else {
                Err(error)
            };
        }
        // #217: this focus change is the lane's own, not a
        // human move; the guard restores the prior window.
        guard.record_own_foreground_focus();
        action.force_foreground_key_delivery = needs_foreground_key_delivery;
        self.target_act(Parameters(action), request_context.clone())
            .await
    }

    async fn act_foreground_focus_key_target(
        &self,
        session_id: &str,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let target = self
            .session_target(Some(session_id))?
            .ok_or_else(target_act_focus_window_missing_target_error)?;
        // Foreground activation is stronger than the normal mutating-action
        // conflict check: the requesting session must itself own this exact
        // target claim. Otherwise an unclaimed session could seize focus and
        // emit trusted input merely because no other session conflicted.
        target_act_focus_window_preflight(self, session_id, &target)?;
        let focus_params = target_act_focus_window_params(Some(&target))?;
        self.act_focus_window(Parameters(focus_params), request_context.clone())
            .await?;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TargetActSaveResponse {
    ok: bool,
    method: String,
    backend_tier_used: String,
    required_foreground: bool,
    source_of_truth: String,
    path: String,
    target_hwnd: i64,
    target_process_name: String,
    target_window_title: String,
    before_len: u64,
    after_len: u64,
    before_sha256: String,
    after_sha256: String,
    changed: bool,
    expected_text_sha256: Option<String>,
    expected_text_matched: Option<bool>,
    attempts: Vec<TargetActSaveAttempt>,
    postcondition: crate::m2::postcondition::ActPostcondition,
    elapsed_ms: u32,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TargetActSaveAttempt {
    method: String,
    command_source: String,
    sent: bool,
    detail: String,
    win32_result: Option<usize>,
}

struct TargetActFileSnapshot {
    len: u64,
    sha256: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TargetActCleanupNotepadTabsResponse {
    ok: bool,
    method: String,
    backend_tier_used: String,
    required_foreground: bool,
    source_of_truth: String,
    target_hwnd: i64,
    target_process_name: String,
    target_window_title_before: String,
    target_window_title_after: String,
    keep_path: String,
    keep_file_name: String,
    modified_stale_policy: String,
    desktop_names: Vec<String>,
    active_desktop_name: String,
    before_tabs: Vec<TargetActNotepadTabReadback>,
    closed_tabs: Vec<TargetActNotepadTabCloseAttempt>,
    after_tabs: Vec<TargetActNotepadTabReadback>,
    before_signature: String,
    after_signature: String,
    stale_tabs_before: u32,
    stale_tabs_after: u32,
    modified_stale_tabs_discarded: u32,
    postcondition: crate::m2::postcondition::ActPostcondition,
    elapsed_ms: u32,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TargetActNotepadTabReadback {
    element_id: ElementId,
    name: String,
    document_name: String,
    modified: bool,
    keep: bool,
    bbox: Rect,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TargetActNotepadTabCloseAttempt {
    tab: TargetActNotepadTabReadback,
    selected: bool,
    scrolled_into_view: bool,
    stale_bbox_before: Rect,
    stale_bbox_after_select: Rect,
    close_method: String,
    close_button_element_id: Option<ElementId>,
    close_invoked: bool,
    discard_button_element_id: Option<ElementId>,
    discarded_modified: bool,
    tabs_after: Vec<TargetActNotepadTabReadback>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum TargetActModifiedStalePolicy {
    DiscardModified,
    RefuseModified,
}

impl TargetActModifiedStalePolicy {
    const fn as_str(self) -> &'static str {
        match self {
            Self::DiscardModified => "discard_modified",
            Self::RefuseModified => "refuse_modified",
        }
    }
}

#[derive(Clone)]
struct TargetActNotepadSnapshot {
    desktop_names: Vec<String>,
    active_desktop_name: String,
    context: synapse_core::ForegroundContext,
    nodes: Vec<AccessibleNode>,
    tabs: Vec<TargetActNotepadTabReadback>,
}

#[derive(Clone)]
struct TargetActHiddenWindowSnapshot {
    hwnd: i64,
    nodes: Vec<AccessibleNode>,
}

async fn target_act_cleanup_notepad_tabs(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let result = target_act_cleanup_notepad_tabs_impl(service, params, request_context).await;
    let _ = service.audit_action_result_for_request("target_act", &result, request_context);
    match result {
        Ok(response) => Ok((
            "target_window_notepad_tab_cleanup",
            true,
            TARGET_ACT_STATUS_OK,
            target_act_result(&response)?,
        )),
        Err(error) => Ok((
            "target_window_notepad_tab_cleanup",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_window_notepad_tab_cleanup", error),
        )),
    }
}

async fn target_act_cleanup_notepad_tabs_impl(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<TargetActCleanupNotepadTabsResponse, ErrorData> {
    let started = Instant::now();
    target_act_validate_cleanup_notepad_tabs_params(params)?;
    let session_id = target_act_session_id(request_context, "cleanup_notepad_tabs")?;
    let keep_path = require_param(params.path.clone(), "cleanup_notepad_tabs", "path")?;
    let keep_path =
        canonical_existing_file_for_target_act_path("cleanup_notepad_tabs", Path::new(&keep_path))?;
    let keep_file_name = target_act_file_name(&keep_path, "cleanup_notepad_tabs")?;
    let modified_policy = target_act_modified_stale_policy(params.value.as_deref())?;
    let verify_timeout_ms = target_act_cleanup_notepad_tabs_verify_timeout(params.wait_timeout_ms)?;
    let target = service.session_target(Some(&session_id))?.ok_or_else(|| {
        mcp_error(
            error_codes::TARGET_NOT_SET,
            "target_act verb=cleanup_notepad_tabs requires this MCP session to have an owned hidden-desktop Notepad window target; call act_launch/set_target first",
        )
    })?;
    let hwnd = match target {
        SessionTarget::Window { hwnd } => hwnd,
        SessionTarget::Cdp { .. } => {
            return Err(mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                "target_act verb=cleanup_notepad_tabs supports native Notepad window targets only",
            ));
        }
    };
    service.ensure_target_claim_allows_session("target_act", &session_id, &target)?;

    let before = target_act_hidden_notepad_snapshot(service, &session_id, hwnd, &keep_file_name)?;
    if !before
        .context
        .process_name
        .eq_ignore_ascii_case("notepad.exe")
    {
        return Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "target_act verb=cleanup_notepad_tabs only supports Notepad hidden targets; target 0x{hwnd:x} is process {} title {:?}",
                before.context.process_name, before.context.window_title
            ),
        ));
    }
    if !target_act_notepad_title_matches_path(&before.context.window_title, &keep_path) {
        return Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "target_act verb=cleanup_notepad_tabs refused: Notepad target title {:?} does not match keep file SoT {}",
                before.context.window_title,
                keep_path.display()
            ),
        ));
    }
    target_act_validate_notepad_keep_tab(&before.tabs, &keep_file_name)?;
    let before_signature = target_act_notepad_tabs_signature(&before.tabs);
    let before_tabs = before.tabs.clone();
    let target_window_title_before = before.context.window_title.clone();
    let stale_tabs_before = target_act_stale_notepad_tab_count(&before.tabs);
    let request_details = json!({
        "session_id": &session_id,
        "verb": "cleanup_notepad_tabs",
        "delegated_tool": "target_window_notepad_tab_cleanup",
        "target_hwnd": hwnd,
        "target_process_name": before.context.process_name,
        "target_window_title": before.context.window_title,
        "keep_path": keep_path.display().to_string(),
        "keep_file_name": keep_file_name,
        "desktop_names": before.desktop_names,
        "source_of_truth": TARGET_ACT_CLEANUP_NOTEPAD_TABS_SOURCE_OF_TRUTH,
        "before_signature": before_signature,
        "stale_tabs_before": stale_tabs_before,
        "modified_stale_policy": modified_policy.as_str(),
        "verify_timeout_ms": verify_timeout_ms,
        "requires_session_owned_hidden_desktop": true,
        "required_foreground": false,
        "no_human_os_foreground_fallback": true,
    });
    service.audit_action_started_with_details_for_request(
        "target_act",
        &request_details,
        request_context,
    )?;

    let run = TargetActCleanupNotepadTabsRun {
        service,
        session_id: &session_id,
        hwnd,
        started,
        keep_path,
        keep_file_name,
        modified_policy,
        verify_timeout_ms,
        before_signature,
        before_tabs,
        target_window_title_before,
        stale_tabs_before,
    };
    target_act_cleanup_notepad_tabs_after_started(run, before).await
}

/// Inputs of one `cleanup_notepad_tabs` run, all resolved before its started
/// audit row is written.
struct TargetActCleanupNotepadTabsRun<'a> {
    service: &'a SynapseService,
    session_id: &'a str,
    hwnd: i64,
    started: Instant,
    keep_path: PathBuf,
    keep_file_name: String,
    modified_policy: TargetActModifiedStalePolicy,
    verify_timeout_ms: u32,
    before_signature: String,
    before_tabs: Vec<TargetActNotepadTabReadback>,
    target_window_title_before: String,
    stale_tabs_before: u32,
}

/// The part of `target_act_cleanup_notepad_tabs_impl` after its started row.
/// Every outcome, early `?` failures included, is returned through the impl
/// to `target_act_cleanup_notepad_tabs`, which writes the closing row with
/// that result (#198).
async fn target_act_cleanup_notepad_tabs_after_started(
    run: TargetActCleanupNotepadTabsRun<'_>,
    before: TargetActNotepadSnapshot,
) -> Result<TargetActCleanupNotepadTabsResponse, ErrorData> {
    let (before, attempts) = target_act_cleanup_close_stale_notepad_tabs(&run, before).await?;
    let after = target_act_hidden_notepad_snapshot(
        run.service,
        run.session_id,
        run.hwnd,
        &run.keep_file_name,
    )?;
    target_act_validate_notepad_keep_tab(&after.tabs, &run.keep_file_name)?;
    let stale_tabs_after = target_act_stale_notepad_tab_count(&after.tabs);
    if stale_tabs_after != 0 {
        return Err(target_act_cleanup_notepad_tabs_postcondition_error(
            "stale tabs remained after cleanup",
            &run.keep_path,
            &run.keep_file_name,
            &run.before_signature,
            &before,
            &after,
            &attempts,
            run.verify_timeout_ms,
        ));
    }
    Ok(target_act_cleanup_notepad_tabs_response(
        run,
        after,
        attempts,
        stale_tabs_after,
    ))
}

/// Last snapshot and close attempts of a stale-tab close loop.
type TargetActCleanupNotepadTabsProgress = (
    TargetActNotepadSnapshot,
    Vec<TargetActNotepadTabCloseAttempt>,
);

/// Why the stale-tab close loop must stop before closing another tab.
fn target_act_cleanup_stop_detail(attempts: usize, deadline: Instant) -> Option<&'static str> {
    if attempts > 64 {
        Some("tab cleanup iteration limit reached")
    } else if Instant::now() >= deadline {
        Some("tab cleanup timed out before all stale tabs were removed")
    } else {
        None
    }
}

/// Closes stale tabs until none remain; returns the last snapshot and the
/// close attempts.
async fn target_act_cleanup_close_stale_notepad_tabs(
    run: &TargetActCleanupNotepadTabsRun<'_>,
    mut before: TargetActNotepadSnapshot,
) -> Result<TargetActCleanupNotepadTabsProgress, ErrorData> {
    let deadline = Instant::now() + Duration::from_millis(u64::from(run.verify_timeout_ms));
    let mut attempts = Vec::new();
    loop {
        target_act_validate_notepad_keep_tab(&before.tabs, &run.keep_file_name)?;
        if run.modified_policy == TargetActModifiedStalePolicy::RefuseModified
            && let Some(stale) = target_act_first_modified_stale_notepad_tab(&before.tabs)
        {
            return Err(target_act_refuse_modified_stale_tab_error(&stale));
        }
        let Some(stale) = target_act_next_stale_notepad_tab(&before.tabs) else {
            break;
        };
        if let Some(detail) = target_act_cleanup_stop_detail(attempts.len(), deadline) {
            return Err(target_act_cleanup_notepad_tabs_postcondition_error(
                detail,
                &run.keep_path,
                &run.keep_file_name,
                &run.before_signature,
                &before,
                &before,
                &attempts,
                run.verify_timeout_ms,
            ));
        }
        let attempt = target_act_cleanup_close_notepad_tab(
            run.service,
            run.session_id,
            run.hwnd,
            &run.keep_file_name,
            stale,
            run.modified_policy,
            deadline,
        )
        .await?;
        attempts.push(attempt);
        before = target_act_hidden_notepad_snapshot(
            run.service,
            run.session_id,
            run.hwnd,
            &run.keep_file_name,
        )?;
    }
    Ok((before, attempts))
}

fn target_act_cleanup_notepad_tabs_response(
    run: TargetActCleanupNotepadTabsRun<'_>,
    after: TargetActNotepadSnapshot,
    attempts: Vec<TargetActNotepadTabCloseAttempt>,
    stale_tabs_after: u32,
) -> TargetActCleanupNotepadTabsResponse {
    let after_signature = target_act_notepad_tabs_signature(&after.tabs);
    let changed = run.before_signature != after_signature;
    let modified_stale_tabs_discarded = attempts
        .iter()
        .filter(|attempt| attempt.discarded_modified)
        .count()
        .try_into()
        .unwrap_or(u32::MAX);
    TargetActCleanupNotepadTabsResponse {
        ok: true,
        method: "uia_select_tab_then_invoke_close_tab".to_owned(),
        backend_tier_used: "uia_invoke_hidden_desktop_target".to_owned(),
        required_foreground: false,
        source_of_truth: TARGET_ACT_CLEANUP_NOTEPAD_TABS_SOURCE_OF_TRUTH.to_owned(),
        target_hwnd: run.hwnd,
        target_process_name: after.context.process_name.clone(),
        target_window_title_before: run.target_window_title_before,
        target_window_title_after: after.context.window_title.clone(),
        keep_path: run.keep_path.display().to_string(),
        keep_file_name: run.keep_file_name,
        modified_stale_policy: run.modified_policy.as_str().to_owned(),
        desktop_names: after.desktop_names.clone(),
        active_desktop_name: after.active_desktop_name.clone(),
        before_tabs: run.before_tabs,
        closed_tabs: attempts,
        after_tabs: after.tabs,
        before_signature: run.before_signature.clone(),
        after_signature: after_signature.clone(),
        stale_tabs_before: run.stale_tabs_before,
        stale_tabs_after,
        modified_stale_tabs_discarded,
        postcondition: crate::m2::postcondition::ActPostcondition {
            related_window_observed: None,
            status: "verified_state".to_owned(),
            observed_delta: Some(changed),
            source_of_truth: Some(TARGET_ACT_CLEANUP_NOTEPAD_TABS_SOURCE_OF_TRUTH.to_owned()),
            before_signature: Some(run.before_signature),
            after_signature: Some(after_signature),
            detail: Some("target_act cleanup_notepad_tabs verified stale tab count is zero on the session-owned hidden Notepad target".to_owned()),
        },
        elapsed_ms: u32::try_from(run.started.elapsed().as_millis()).unwrap_or(u32::MAX),
    }
}

async fn target_act_cleanup_close_notepad_tab(
    service: &SynapseService,
    session_id: &str,
    hwnd: i64,
    keep_file_name: &str,
    stale: TargetActNotepadTabReadback,
    modified_policy: TargetActModifiedStalePolicy,
    deadline: Instant,
) -> Result<TargetActNotepadTabCloseAttempt, ErrorData> {
    let stale_bbox_before = stale.bbox;
    let mut action_stale = stale.clone();
    let mut scrolled_into_view = false;
    if !target_act_rect_has_area(action_stale.bbox) {
        target_act_scroll_uia_element_into_view(
            "cleanup_notepad_tabs.scroll_stale_tab_into_view",
            &action_stale.element_id,
        )?;
        scrolled_into_view = true;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let scrolled =
            target_act_hidden_notepad_snapshot(service, session_id, hwnd, keep_file_name)?;
        action_stale =
            target_act_matching_notepad_tab(&scrolled.tabs, &stale).ok_or_else(|| {
                mcp_error(
                    error_codes::ACTION_POSTCONDITION_FAILED,
                    format!(
                        "target_act verb=cleanup_notepad_tabs scrolled stale hidden Notepad tab {:?}, but the tab was absent from the post-scroll SoT",
                        stale.name
                    ),
                )
            })?;
    }
    target_act_invoke_uia_element(
        "cleanup_notepad_tabs.select_stale_tab",
        &action_stale.element_id,
    )?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut selected =
        target_act_hidden_notepad_snapshot(service, session_id, hwnd, keep_file_name)?;
    let mut selected_stale =
        target_act_matching_notepad_tab(&selected.tabs, &action_stale).ok_or_else(|| {
            mcp_error(
                error_codes::ACTION_POSTCONDITION_FAILED,
                format!(
                    "target_act verb=cleanup_notepad_tabs selected stale tab {:?}, but the tab was absent from the selected hidden Notepad SoT",
                    stale.name
                ),
            )
        })?;
    if !target_act_rect_has_area(selected_stale.bbox) {
        target_act_scroll_uia_element_into_view(
            "cleanup_notepad_tabs.scroll_selected_stale_tab_into_view",
            &selected_stale.element_id,
        )?;
        scrolled_into_view = true;
        tokio::time::sleep(Duration::from_millis(100)).await;
        selected = target_act_hidden_notepad_snapshot(service, session_id, hwnd, keep_file_name)?;
        selected_stale =
            target_act_matching_notepad_tab(&selected.tabs, &action_stale).ok_or_else(|| {
                mcp_error(
                    error_codes::ACTION_POSTCONDITION_FAILED,
                    format!(
                        "target_act verb=cleanup_notepad_tabs scrolled selected stale tab {:?}, but the tab was absent from the post-scroll SoT",
                        stale.name
                    ),
                )
            })?;
    }
    if !target_act_notepad_title_matches_document(
        &selected.context.window_title,
        &stale.document_name,
    ) {
        return Err(mcp_error(
            error_codes::ACTION_POSTCONDITION_FAILED,
            format!(
                "target_act verb=cleanup_notepad_tabs selected stale tab {:?}, but Notepad title readback stayed {:?}",
                stale.name, selected.context.window_title
            ),
        ));
    }
    let (close_method, close_button_element_id) = if let Some(close_button) =
        target_act_notepad_close_tab_button(&selected.nodes, &selected_stale)
    {
        let close_button_element_id = Some(close_button.element_id.clone());
        target_act_invoke_uia_element(
            "cleanup_notepad_tabs.invoke_close_tab",
            &close_button.element_id,
        )?;
        ("uia_close_tab_button".to_owned(), close_button_element_id)
    } else if let Some(close_menu_item) = target_act_notepad_close_tab_menu_item_for_hidden_desktop(
        service,
        session_id,
        hwnd,
        &selected.nodes,
    )
    .await?
    {
        let close_button_element_id = Some(close_menu_item.element_id.clone());
        target_act_invoke_uia_element(
            "cleanup_notepad_tabs.invoke_file_close_tab",
            &close_menu_item.element_id,
        )?;
        (
            "uia_file_menu_close_tab".to_owned(),
            close_button_element_id,
        )
    } else {
        let close_glyph = target_act_notepad_close_tab_glyph(&selected.nodes, &selected_stale);
        return Err(mcp_error(
            error_codes::ACTION_ELEMENT_NOT_RESOLVED,
            format!(
                "target_act verb=cleanup_notepad_tabs could not find a UIA Close Tab button or File > Close tab menu item for stale hidden Notepad tab {:?}; close_glyph_present={}",
                stale.name,
                close_glyph.is_some()
            ),
        ));
    };
    tokio::time::sleep(Duration::from_millis(150)).await;

    let mut discard_button_element_id = None;
    let mut discarded_modified = false;
    let mut after_close =
        target_act_hidden_notepad_snapshot(service, session_id, hwnd, keep_file_name)?;
    if target_act_notepad_tab_still_present(&after_close.tabs, &stale) && stale.modified {
        if modified_policy == TargetActModifiedStalePolicy::DiscardModified {
            let discard_button = target_act_notepad_discard_button_for_hidden_desktop(
                service,
                session_id,
                hwnd,
                &after_close.nodes,
            )
            .map_err(|error| {
                mcp_error(
                    error_codes::ACTION_ELEMENT_NOT_RESOLVED,
                    format!(
                        "target_act verb=cleanup_notepad_tabs close of modified stale tab {:?} did not remove the tab and no discard button was visible in the hidden target/popup SoT: {error}",
                        stale.name
                    ),
                )
            })?;
            discard_button_element_id = Some(discard_button.element_id.clone());
            target_act_invoke_uia_element(
                "cleanup_notepad_tabs.discard_modified_stale_tab",
                &discard_button.element_id,
            )?;
            discarded_modified = true;
            tokio::time::sleep(Duration::from_millis(150)).await;
            after_close =
                target_act_hidden_notepad_snapshot(service, session_id, hwnd, keep_file_name)?;
        }
    }
    while target_act_notepad_tab_still_present(&after_close.tabs, &stale) {
        if Instant::now() >= deadline {
            return Err(mcp_error(
                error_codes::ACTION_POSTCONDITION_FAILED,
                format!(
                    "target_act verb=cleanup_notepad_tabs stale tab {:?} remained after Close Tab",
                    stale.name
                ),
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        after_close =
            target_act_hidden_notepad_snapshot(service, session_id, hwnd, keep_file_name)?;
    }

    Ok(TargetActNotepadTabCloseAttempt {
        tab: stale,
        selected: true,
        scrolled_into_view,
        stale_bbox_before,
        stale_bbox_after_select: selected_stale.bbox,
        close_method,
        close_button_element_id,
        close_invoked: true,
        discard_button_element_id,
        discarded_modified,
        tabs_after: after_close.tabs,
    })
}

fn target_act_validate_cleanup_notepad_tabs_params(
    params: &TargetActParams,
) -> Result<(), ErrorData> {
    if params.url.as_ref().is_some_and(|value| !value.is_empty())
        || params
            .command
            .as_ref()
            .is_some_and(|value| !value.is_empty())
        || !params.args.is_empty()
        || params
            .working_dir
            .as_ref()
            .is_some_and(|value| !value.is_empty())
        || params.text.as_ref().is_some_and(|value| !value.is_empty())
        || params
            .option
            .as_ref()
            .is_some_and(|value| !value.is_empty())
        || params.clicks.is_some()
        || params
            .element_id
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        || params
            .selector
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        || params
            .role
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        || params
            .name
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        || target_act_coordinate(params)?.is_some()
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=cleanup_notepad_tabs accepts only path, optional value=discard_modified|refuse_modified, and optional wait_timeout_ms",
        ));
    }
    Ok(())
}

fn target_act_cleanup_notepad_tabs_verify_timeout(value: Option<u64>) -> Result<u32, ErrorData> {
    let timeout = value.unwrap_or(u64::from(
        DEFAULT_TARGET_ACT_CLEANUP_NOTEPAD_TABS_TIMEOUT_MS,
    ));
    if !(250..=30_000).contains(&timeout) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb=cleanup_notepad_tabs wait_timeout_ms must be 250..=30000, got {timeout}"
            ),
        ));
    }
    Ok(u32::try_from(timeout).unwrap_or(DEFAULT_TARGET_ACT_CLEANUP_NOTEPAD_TABS_TIMEOUT_MS))
}

fn target_act_modified_stale_policy(
    value: Option<&str>,
) -> Result<TargetActModifiedStalePolicy, ErrorData> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        None | Some("discard_modified") => Ok(TargetActModifiedStalePolicy::DiscardModified),
        Some("refuse_modified") => Ok(TargetActModifiedStalePolicy::RefuseModified),
        Some(other) => Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb=cleanup_notepad_tabs value must be discard_modified or refuse_modified, got {other:?}"
            ),
        )),
    }
}

fn target_act_hidden_notepad_snapshot(
    service: &SynapseService,
    session_id: &str,
    hwnd: i64,
    keep_file_name: &str,
) -> Result<TargetActNotepadSnapshot, ErrorData> {
    let Some(hidden_desktop) = service.session_hidden_desktop_readback(session_id)? else {
        return Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "target_act verb=cleanup_notepad_tabs requires session-owned hidden desktop resources; session {session_id:?} has none"
            ),
        ));
    };
    let mut misses = Vec::new();
    for desktop_name in &hidden_desktop.desktop_names {
        match crate::desktop_worker::hidden_desktop_window_snapshot(desktop_name, hwnd, 16) {
            Ok(snapshot) => {
                let tabs = target_act_notepad_tabs_from_nodes(&snapshot.tree.nodes, keep_file_name);
                return Ok(TargetActNotepadSnapshot {
                    desktop_names: hidden_desktop.desktop_names.clone(),
                    active_desktop_name: desktop_name.clone(),
                    context: snapshot.context,
                    nodes: snapshot.tree.nodes,
                    tabs,
                });
            }
            Err(error) if target_act_hidden_worker_target_miss(&error) => {
                misses.push(desktop_name.clone());
            }
            Err(error) => return Err(error),
        }
    }
    Err(mcp_error(
        error_codes::TARGET_WINDOW_NOT_FOUND,
        format!(
            "target_act verb=cleanup_notepad_tabs target HWND 0x{hwnd:x} was not found on this session's hidden desktop(s): {misses:?}"
        ),
    ))
}

fn target_act_hidden_worker_target_miss(error: &ErrorData) -> bool {
    matches!(
        target_act_error_code(error),
        Some(error_codes::TARGET_WINDOW_NOT_FOUND)
    )
}

fn target_act_notepad_tabs_from_nodes(
    nodes: &[AccessibleNode],
    keep_file_name: &str,
) -> Vec<TargetActNotepadTabReadback> {
    nodes
        .iter()
        .filter_map(|node| {
            let role = target_act_normalized_role(&node.role);
            if role != "tabitem" && role != "tab" {
                return None;
            }
            let (document_name, modified) = target_act_parse_notepad_tab_name(&node.name)?;
            let keep = target_act_names_equal(&document_name, keep_file_name);
            Some(TargetActNotepadTabReadback {
                element_id: node.element_id.clone(),
                name: node.name.trim().to_owned(),
                document_name,
                modified,
                keep,
                bbox: node.bbox,
            })
        })
        .collect()
}

fn target_act_parse_notepad_tab_name(name: &str) -> Option<(String, bool)> {
    let name = name.trim();
    for (suffix, modified) in [(". Modified.", true), (". Unmodified.", false)] {
        if let Some(document_name) = name.strip_suffix(suffix) {
            let document_name = document_name.trim().trim_start_matches('*').to_owned();
            if !document_name.is_empty() {
                return Some((document_name, modified));
            }
        }
    }
    None
}

fn target_act_validate_notepad_keep_tab(
    tabs: &[TargetActNotepadTabReadback],
    keep_file_name: &str,
) -> Result<(), ErrorData> {
    if tabs.is_empty() {
        return Ok(());
    }
    let keep_count = tabs.iter().filter(|tab| tab.keep).count();
    if keep_count == 1 {
        return Ok(());
    }
    Err(mcp_error(
        error_codes::ACTION_TARGET_INVALID,
        format!(
            "target_act verb=cleanup_notepad_tabs expected exactly one keep tab for {keep_file_name:?}, found {keep_count}; tabs={}",
            target_act_notepad_tab_names(tabs).join(" | ")
        ),
    ))
}

fn target_act_stale_notepad_tab_count(tabs: &[TargetActNotepadTabReadback]) -> u32 {
    tabs.iter()
        .filter(|tab| !tab.keep)
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

fn target_act_first_modified_stale_notepad_tab(
    tabs: &[TargetActNotepadTabReadback],
) -> Option<TargetActNotepadTabReadback> {
    tabs.iter().find(|tab| !tab.keep && tab.modified).cloned()
}

fn target_act_refuse_modified_stale_tab_error(stale: &TargetActNotepadTabReadback) -> ErrorData {
    mcp_error(
        error_codes::ACTION_TARGET_INVALID,
        format!(
            "target_act verb=cleanup_notepad_tabs found modified stale hidden Notepad tab {:?} and value=refuse_modified was requested",
            stale.name
        ),
    )
}

fn target_act_next_stale_notepad_tab(
    tabs: &[TargetActNotepadTabReadback],
) -> Option<TargetActNotepadTabReadback> {
    tabs.iter()
        .filter(|tab| !tab.keep)
        .min_by_key(|tab| {
            (
                i32::from(!target_act_rect_has_area(tab.bbox)),
                tab.bbox.x,
                tab.bbox.y,
                tab.name.clone(),
            )
        })
        .cloned()
}

fn target_act_notepad_tab_still_present(
    tabs: &[TargetActNotepadTabReadback],
    stale: &TargetActNotepadTabReadback,
) -> bool {
    tabs.iter().any(|tab| {
        tab.element_id == stale.element_id
            || (target_act_names_equal(&tab.document_name, &stale.document_name)
                && tab.modified == stale.modified)
    })
}

fn target_act_matching_notepad_tab(
    tabs: &[TargetActNotepadTabReadback],
    stale: &TargetActNotepadTabReadback,
) -> Option<TargetActNotepadTabReadback> {
    tabs.iter()
        .find(|tab| tab.element_id == stale.element_id)
        .or_else(|| {
            tabs.iter().find(|tab| {
                target_act_names_equal(&tab.document_name, &stale.document_name)
                    && tab.modified == stale.modified
            })
        })
        .cloned()
}

fn target_act_notepad_close_tab_button(
    nodes: &[AccessibleNode],
    stale: &TargetActNotepadTabReadback,
) -> Option<AccessibleNode> {
    nodes
        .iter()
        .filter(|node| {
            node.enabled
                && target_act_normalized_role(&node.role) == "button"
                && target_act_normalized_name(&node.name) == "closetab"
                && node.patterns.contains(&UiaPattern::Invoke)
        })
        .min_by_key(|node| target_act_rect_distance_score(node.bbox, stale))
        .cloned()
}

fn target_act_notepad_close_tab_glyph(
    nodes: &[AccessibleNode],
    stale: &TargetActNotepadTabReadback,
) -> Option<AccessibleNode> {
    nodes
        .iter()
        .filter(|node| {
            if !node.enabled || target_act_normalized_role(&node.role) != "text" {
                return false;
            }
            let bbox = node.bbox;
            if bbox.w <= 0 || bbox.h <= 0 || bbox.w > 40 || bbox.h > 40 {
                return false;
            }
            let (center_x, center_y) = target_act_rect_center(bbox);
            let left = i64::from(stale.bbox.x);
            let right = left + i64::from(stale.bbox.w);
            let top = i64::from(stale.bbox.y);
            let bottom = top + i64::from(stale.bbox.h);
            center_x > left + i64::from(stale.bbox.w) / 2
                && center_x <= right
                && center_y >= top
                && center_y <= bottom
                && !target_act_names_equal(&node.name, &stale.document_name)
        })
        .min_by_key(|node| target_act_rect_distance_score(node.bbox, stale))
        .cloned()
}

async fn target_act_notepad_close_tab_menu_item_for_hidden_desktop(
    service: &SynapseService,
    session_id: &str,
    target_hwnd: i64,
    target_nodes: &[AccessibleNode],
) -> Result<Option<AccessibleNode>, ErrorData> {
    let Some(file_menu) = target_act_notepad_file_menu_item(target_nodes) else {
        return Ok(None);
    };
    target_act_invoke_uia_element("cleanup_notepad_tabs.open_file_menu", &file_menu.element_id)?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    if let Some(menu_item) = target_act_notepad_close_tab_menu_item(target_nodes) {
        return Ok(Some(menu_item));
    }
    let snapshots =
        target_act_hidden_desktop_auxiliary_snapshots(service, session_id, target_hwnd)?;
    for snapshot in snapshots {
        if let Some(menu_item) = target_act_notepad_close_tab_menu_item(&snapshot.nodes) {
            return Ok(Some(menu_item));
        }
    }
    Ok(None)
}

fn target_act_notepad_file_menu_item(nodes: &[AccessibleNode]) -> Option<AccessibleNode> {
    nodes
        .iter()
        .find(|node| {
            node.enabled
                && target_act_normalized_role(&node.role) == "menuitem"
                && target_act_normalized_name(&node.name) == "file"
                && node.patterns.contains(&UiaPattern::ExpandCollapse)
        })
        .cloned()
}

fn target_act_notepad_close_tab_menu_item(nodes: &[AccessibleNode]) -> Option<AccessibleNode> {
    nodes
        .iter()
        .find(|node| {
            if !node.enabled {
                return false;
            }
            let role = target_act_normalized_role(&node.role);
            if role != "menuitem" && role != "button" && role != "listitem" {
                return false;
            }
            target_act_normalized_name(&node.name).contains("closetab")
                && node.patterns.contains(&UiaPattern::Invoke)
        })
        .cloned()
}

fn target_act_notepad_discard_button(nodes: &[AccessibleNode]) -> Option<AccessibleNode> {
    nodes
        .iter()
        .find(|node| {
            if !node.enabled || target_act_normalized_role(&node.role) != "button" {
                return false;
            }
            let name = node.name.to_ascii_lowercase().replace('\u{2019}', "'");
            name.contains("discard")
                || name.contains("close without saving")
                || name.contains("do not save")
                || ((name.contains("don't") || name.contains("dont")) && name.contains("save"))
        })
        .cloned()
}

fn target_act_notepad_discard_button_for_hidden_desktop(
    service: &SynapseService,
    session_id: &str,
    target_hwnd: i64,
    target_nodes: &[AccessibleNode],
) -> Result<AccessibleNode, String> {
    if let Some(button) = target_act_notepad_discard_button(target_nodes) {
        return Ok(button);
    }
    let snapshots = target_act_hidden_desktop_auxiliary_snapshots(service, session_id, target_hwnd)
        .map_err(|error| error.to_string())?;
    let mut inspected = Vec::new();
    for snapshot in snapshots {
        inspected.push(format!("0x{:x}", snapshot.hwnd));
        if let Some(button) = target_act_notepad_discard_button(&snapshot.nodes) {
            return Ok(button);
        }
    }
    Err(format!(
        "inspected auxiliary hidden HWNDs [{}]",
        inspected.join(", ")
    ))
}

fn target_act_hidden_desktop_auxiliary_snapshots(
    service: &SynapseService,
    session_id: &str,
    target_hwnd: i64,
) -> Result<Vec<TargetActHiddenWindowSnapshot>, ErrorData> {
    let Some(hidden_desktop) = service.session_hidden_desktop_readback(session_id)? else {
        return Ok(Vec::new());
    };
    let mut snapshots = Vec::new();
    for desktop_name in &hidden_desktop.desktop_names {
        for hwnd in crate::desktop_worker::hidden_desktop_window_hwnds(desktop_name)? {
            if hwnd == target_hwnd {
                continue;
            }
            match crate::desktop_worker::hidden_desktop_window_snapshot(desktop_name, hwnd, 8) {
                Ok(snapshot) => snapshots.push(TargetActHiddenWindowSnapshot {
                    hwnd,
                    nodes: snapshot.tree.nodes,
                }),
                Err(error) if target_act_hidden_worker_target_miss(&error) => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(snapshots)
}

fn target_act_invoke_uia_element(
    operation: &'static str,
    element_id: &ElementId,
) -> Result<(), ErrorData> {
    synapse_action::invoke_element(element_id).map_err(|error| {
        mcp_error(
            error.code(),
            format!("target_act {operation} failed for element {element_id}: {error}"),
        )
    })
}

fn target_act_scroll_uia_element_into_view(
    operation: &'static str,
    element_id: &ElementId,
) -> Result<(), ErrorData> {
    synapse_a11y::scroll_element_into_view(element_id).map_err(|error| {
        mcp_error(
            error.code(),
            format!("target_act {operation} failed for element {element_id}: {error}"),
        )
    })
}

fn target_act_notepad_title_matches_document(title: &str, document_name: &str) -> bool {
    let lowered_title = title.to_ascii_lowercase();
    lowered_title.contains("notepad")
        && lowered_title.contains(
            document_name
                .trim()
                .trim_start_matches('*')
                .to_ascii_lowercase()
                .as_str(),
        )
}

fn target_act_notepad_tabs_signature(tabs: &[TargetActNotepadTabReadback]) -> String {
    let payload = tabs
        .iter()
        .map(|tab| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                tab.element_id,
                tab.document_name,
                tab.modified,
                tab.keep,
                tab.bbox.x,
                tab.bbox.y,
                tab.bbox.w,
                tab.bbox.h
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    crate::content_digest::keyed_digest_hex(payload.as_bytes())
}

fn target_act_cleanup_notepad_tabs_postcondition_error(
    detail: &str,
    keep_path: &Path,
    keep_file_name: &str,
    before_signature: &str,
    before: &TargetActNotepadSnapshot,
    after: &TargetActNotepadSnapshot,
    attempts: &[TargetActNotepadTabCloseAttempt],
    verify_timeout_ms: u32,
) -> ErrorData {
    let after_signature = target_act_notepad_tabs_signature(&after.tabs);
    crate::m2::postcondition::postcondition_failed_error(
        "target_act",
        TARGET_ACT_CLEANUP_NOTEPAD_TABS_SOURCE_OF_TRUTH,
        detail,
        before_signature.to_owned(),
        after_signature,
        json!({
            "path": keep_path.display().to_string(),
            "keep_file_name": keep_file_name,
            "before_tabs": before.tabs,
            "after_tabs": after.tabs,
            "attempts": attempts,
            "verify_timeout_ms": verify_timeout_ms,
        }),
    )
}

fn target_act_notepad_tab_names(tabs: &[TargetActNotepadTabReadback]) -> Vec<String> {
    tabs.iter().map(|tab| tab.name.clone()).collect()
}

fn target_act_file_name(path: &Path, verb: &str) -> Result<String, ErrorData> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "target_act verb={verb} path must have a UTF-8 file name: {}",
                    path.display()
                ),
            )
        })
}

fn target_act_names_equal(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

fn target_act_normalized_role(role: &str) -> String {
    role.chars()
        .filter(|character| !character.is_whitespace() && *character != '_' && *character != '-')
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn target_act_normalized_name(name: &str) -> String {
    name.chars()
        .filter(|character| {
            !character.is_whitespace()
                && *character != '_'
                && *character != '-'
                && *character != '.'
        })
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn target_act_rect_distance_score(rect: Rect, stale: &TargetActNotepadTabReadback) -> i64 {
    let (rect_x, rect_y) = target_act_rect_center(rect);
    let (tab_x, tab_y) = target_act_rect_center(stale.bbox);
    (rect_x - tab_x).abs() + (rect_y - tab_y).abs()
}

fn target_act_rect_has_area(rect: Rect) -> bool {
    rect.w > 0 && rect.h > 0
}

fn target_act_rect_center(rect: Rect) -> (i64, i64) {
    (
        i64::from(rect.x) + i64::from(rect.w) / 2,
        i64::from(rect.y) + i64::from(rect.h) / 2,
    )
}

async fn target_act_save(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let result = target_act_save_impl(service, params, request_context).await;
    let _ = service.audit_action_result_for_request("target_act", &result, request_context);
    match result {
        Ok(response) => Ok((
            "target_window_save",
            true,
            TARGET_ACT_STATUS_OK,
            target_act_result(&response)?,
        )),
        Err(error) => Ok((
            "target_window_save",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_window_save", error),
        )),
    }
}

async fn target_act_save_impl(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<TargetActSaveResponse, ErrorData> {
    let started = Instant::now();
    target_act_validate_save_params(params)?;
    let session_id = target_act_session_id(request_context, "save")?;
    let path = require_param(params.path.clone(), "save", "path")?;
    let expected_text = params.text.as_deref();
    let verify_timeout_ms = target_act_save_verify_timeout(params.wait_timeout_ms)?;
    let target = service.session_target(Some(&session_id))?.ok_or_else(|| {
        mcp_error(
            error_codes::TARGET_NOT_SET,
            "target_act verb=save requires this MCP session to have an owned window target; call act_launch/set_target first",
        )
    })?;
    let hwnd = match target {
        SessionTarget::Window { hwnd } => hwnd,
        SessionTarget::Cdp { .. } => {
            return Err(mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                "target_act verb=save currently supports native window targets only; browser targets must use browser/CDP storage or download-specific tools",
            ));
        }
    };
    service.ensure_target_claim_allows_session("target_act", &session_id, &target)?;

    let context = synapse_a11y::foreground_context(hwnd).map_err(|error| {
        mcp_error(
            error.code(),
            format!("target_act verb=save target HWND 0x{hwnd:x} context readback failed: {error}"),
        )
    })?;
    if !context.process_name.eq_ignore_ascii_case("notepad.exe") {
        return Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "target_act verb=save only supports Notepad window targets for #1275; target 0x{hwnd:x} is process {} title {:?}",
                context.process_name, context.window_title
            ),
        ));
    }

    let path = canonical_existing_file_for_target_act_save(Path::new(&path))?;
    if !target_act_notepad_title_matches_path(&context.window_title, &path) {
        return Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "target_act verb=save refused: Notepad target title {:?} does not match file SoT {}",
                context.window_title,
                path.display()
            ),
        ));
    }

    let before = target_act_file_snapshot(&path)?;
    let expected_sha256 = expected_text.map(crate::m2::postcondition::text_signature);
    let request_details = json!({
        "session_id": &session_id,
        "verb": "save",
        "delegated_tool": "target_window_save",
        "target_hwnd": hwnd,
        "target_process_name": context.process_name,
        "target_window_title": context.window_title,
        "path": path.display().to_string(),
        "source_of_truth": TARGET_ACT_SAVE_SOURCE_OF_TRUTH,
        "before_len": before.len,
        "before_sha256": before.sha256,
        "expected_text_sha256": expected_sha256,
        "verify_timeout_ms": verify_timeout_ms,
        "requires_agent_logical_foreground": true,
        "required_foreground": false,
        "no_human_os_foreground_fallback": true,
    });
    service.audit_action_started_with_details_for_request(
        "target_act",
        &request_details,
        request_context,
    )?;
    target_act_save_after_started(
        started,
        &path,
        hwnd,
        &context.process_name,
        &context.window_title,
        before,
        expected_text,
        expected_sha256,
        verify_timeout_ms,
    )
    .await
}

/// The part of `target_act_save_impl` after its started row. Every outcome,
/// early `?` failures included, is returned through `target_act_save_impl`
/// to `target_act_save`, which writes the closing row with that result (#198).
async fn target_act_save_after_started(
    started: Instant,
    path: &Path,
    hwnd: i64,
    process_name: &str,
    window_title: &str,
    before: TargetActFileSnapshot,
    expected_text: Option<&str>,
    expected_sha256: Option<String>,
    verify_timeout_ms: u32,
) -> Result<TargetActSaveResponse, ErrorData> {
    let mut attempts = Vec::new();
    for source in [
        NotepadSaveCommandSource::Menu,
        NotepadSaveCommandSource::Accelerator,
    ] {
        let attempt = send_notepad_save_command(hwnd, source)?;
        attempts.push(attempt);
        if let Some(after) =
            poll_target_act_save_file(path, &before, expected_text, verify_timeout_ms).await?
        {
            return Ok(target_act_save_response(
                started,
                path,
                hwnd,
                process_name,
                window_title,
                before,
                after,
                expected_sha256,
                expected_text,
                attempts,
            ));
        }
    }

    let after = target_act_file_snapshot(path)?;
    Err(target_act_save_postcondition_error(
        path,
        &before,
        &after,
        expected_text,
        verify_timeout_ms,
        &attempts,
    ))
}

fn target_act_validate_save_params(params: &TargetActParams) -> Result<(), ErrorData> {
    if params.url.as_ref().is_some_and(|value| !value.is_empty())
        || params
            .command
            .as_ref()
            .is_some_and(|value| !value.is_empty())
        || !params.args.is_empty()
        || params
            .working_dir
            .as_ref()
            .is_some_and(|value| !value.is_empty())
        || target_act_has_any_locator(params)
        || target_act_coordinate(params)?.is_some()
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=save accepts only path, optional text as expected file contents, and optional wait_timeout_ms",
        ));
    }
    Ok(())
}

fn target_act_save_verify_timeout(value: Option<u64>) -> Result<u32, ErrorData> {
    let timeout = value.unwrap_or(u64::from(DEFAULT_TARGET_ACT_SAVE_TIMEOUT_MS));
    if !(50..=10_000).contains(&timeout) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb=save wait_timeout_ms must be 50..=10000, got {timeout}"),
        ));
    }
    Ok(u32::try_from(timeout).unwrap_or(DEFAULT_TARGET_ACT_SAVE_TIMEOUT_MS))
}

fn canonical_existing_file_for_target_act_save(path: &Path) -> Result<PathBuf, ErrorData> {
    canonical_existing_file_for_target_act_path("save", path)
}

fn canonical_existing_file_for_target_act_path(
    verb: &str,
    path: &Path,
) -> Result<PathBuf, ErrorData> {
    if path.as_os_str().is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb={verb} path must not be empty"),
        ));
    }
    let metadata = fs::metadata(path).map_err(|error| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={verb} requires an existing file path Source of Truth; {} could not be read: {error}",
                path.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={verb} path must be an existing file, got {}",
                path.display()
            ),
        ));
    }
    path.canonicalize().map_err(|error| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={verb} failed to canonicalize file SoT {}: {error}",
                path.display()
            ),
        )
    })
}

fn target_act_notepad_title_matches_path(title: &str, path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let lowered_title = title.to_ascii_lowercase();
    lowered_title.contains("notepad")
        && lowered_title.contains(file_name.to_ascii_lowercase().as_str())
}

fn target_act_file_snapshot(path: &Path) -> Result<TargetActFileSnapshot, ErrorData> {
    let bytes = fs::read(path).map_err(|error| {
        mcp_error(
            error_codes::ACTION_POSTCONDITION_FAILED,
            format!(
                "target_act verb=save file SoT read failed for {}: {error}",
                path.display()
            ),
        )
    })?;
    Ok(TargetActFileSnapshot {
        len: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        sha256: crate::content_digest::keyed_digest_hex(&bytes),
        bytes,
    })
}

async fn poll_target_act_save_file(
    path: &Path,
    before: &TargetActFileSnapshot,
    expected_text: Option<&str>,
    timeout_ms: u32,
) -> Result<Option<TargetActFileSnapshot>, ErrorData> {
    let deadline = Instant::now() + Duration::from_millis(u64::from(timeout_ms));
    loop {
        let after = target_act_file_snapshot(path)?;
        if target_act_save_satisfied(before, &after, expected_text) {
            return Ok(Some(after));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        tokio::time::sleep(Duration::from_millis(TARGET_ACT_SAVE_POLL_INTERVAL_MS)).await;
    }
}

fn target_act_save_satisfied(
    before: &TargetActFileSnapshot,
    after: &TargetActFileSnapshot,
    expected_text: Option<&str>,
) -> bool {
    if let Some(expected_text) = expected_text {
        return after.bytes == expected_text.as_bytes();
    }
    after.sha256 != before.sha256
}

fn target_act_save_response(
    started: Instant,
    path: &Path,
    hwnd: i64,
    process_name: &str,
    window_title: &str,
    before: TargetActFileSnapshot,
    after: TargetActFileSnapshot,
    expected_sha256: Option<String>,
    expected_text: Option<&str>,
    attempts: Vec<TargetActSaveAttempt>,
) -> TargetActSaveResponse {
    let changed = before.sha256 != after.sha256;
    let expected_text_matched = expected_text.map(|expected| after.bytes == expected.as_bytes());
    let detail = if expected_text_matched == Some(true) {
        "target_act save verified file bytes equal expected text"
    } else if changed {
        "target_act save verified file bytes changed"
    } else {
        "target_act save verified file bytes already matched expected text"
    };
    TargetActSaveResponse {
        ok: true,
        method: attempts
            .last()
            .map(|attempt| attempt.method.clone())
            .unwrap_or_else(|| "notepad_wm_command_save".to_owned()),
        backend_tier_used: "win32_wm_command".to_owned(),
        required_foreground: false,
        source_of_truth: TARGET_ACT_SAVE_SOURCE_OF_TRUTH.to_owned(),
        path: path.display().to_string(),
        target_hwnd: hwnd,
        target_process_name: process_name.to_owned(),
        target_window_title: window_title.to_owned(),
        before_len: before.len,
        after_len: after.len,
        before_sha256: before.sha256.clone(),
        after_sha256: after.sha256.clone(),
        changed,
        expected_text_sha256: expected_sha256,
        expected_text_matched,
        attempts,
        postcondition: crate::m2::postcondition::ActPostcondition {
            related_window_observed: None,
            status: "verified_state".to_owned(),
            observed_delta: Some(changed),
            source_of_truth: Some(TARGET_ACT_SAVE_SOURCE_OF_TRUTH.to_owned()),
            before_signature: Some(before.sha256),
            after_signature: Some(after.sha256),
            detail: Some(detail.to_owned()),
        },
        elapsed_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
    }
}

fn target_act_save_postcondition_error(
    path: &Path,
    before: &TargetActFileSnapshot,
    after: &TargetActFileSnapshot,
    expected_text: Option<&str>,
    verify_timeout_ms: u32,
    attempts: &[TargetActSaveAttempt],
) -> ErrorData {
    let expected_text_sha256 = expected_text.map(crate::m2::postcondition::text_signature);
    let expected_text_matched = expected_text.map(|expected| after.bytes == expected.as_bytes());
    let detail = if expected_text.is_some() {
        "file bytes did not equal expected text after target-scoped Notepad save"
    } else {
        "file bytes did not change after target-scoped Notepad save; pass text to verify an already-matching save"
    };
    crate::m2::postcondition::postcondition_failed_error(
        "target_act",
        TARGET_ACT_SAVE_SOURCE_OF_TRUTH,
        detail,
        before.sha256.clone(),
        after.sha256.clone(),
        json!({
            "path": path.display().to_string(),
            "before_len": before.len,
            "after_len": after.len,
            "expected_text_sha256": expected_text_sha256,
            "expected_text_matched": expected_text_matched,
            "verify_timeout_ms": verify_timeout_ms,
            "attempts": attempts,
        }),
    )
}

#[derive(Copy, Clone)]
enum NotepadSaveCommandSource {
    Menu,
    Accelerator,
}

impl NotepadSaveCommandSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Menu => "menu",
            Self::Accelerator => "accelerator",
        }
    }

    const fn wparam(self) -> usize {
        const NOTEPAD_IDM_SAVE: usize = 3;
        match self {
            Self::Menu => NOTEPAD_IDM_SAVE,
            Self::Accelerator => (1_usize << 16) | NOTEPAD_IDM_SAVE,
        }
    }
}

#[cfg(windows)]
fn send_notepad_save_command(
    hwnd: i64,
    source: NotepadSaveCommandSource,
) -> Result<TargetActSaveAttempt, ErrorData> {
    use std::ffi::c_void;
    use windows::Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::{
            SMTO_ABORTIFHUNG, SMTO_ERRORONEXIT, SendMessageTimeoutW, WM_COMMAND,
        },
    };

    synapse_action::delivery_target::ensure_live_window(hwnd, "target_act verb=save target")
        .map_err(|error| crate::m2::action_error_to_mcp(&error))?;
    let hwnd = HWND(hwnd as *mut c_void);
    let mut win32_result = 0_usize;
    // SAFETY: `hwnd` was just checked non-null and live (a stale handle makes the call fail, not
    // fault); WM_COMMAND carries a command id and a zero lparam, so no pointers cross the call;
    // `win32_result` is a live local out-param.
    let lresult = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_COMMAND,
            WPARAM(source.wparam()),
            LPARAM(0),
            SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT,
            500,
            Some(&raw mut win32_result),
        )
    };
    let sent = lresult.0 != 0;
    Ok(TargetActSaveAttempt {
        method: format!("notepad_wm_command_save_{}", source.as_str()),
        command_source: source.as_str().to_owned(),
        sent,
        detail: if sent {
            "WM_COMMAND returned before timeout".to_owned()
        } else {
            format!(
                "SendMessageTimeoutW(WM_COMMAND save/{}) failed or timed out: {}",
                source.as_str(),
                windows::core::Error::from_thread()
            )
        },
        win32_result: sent.then_some(win32_result),
    })
}

#[cfg(not(windows))]
fn send_notepad_save_command(
    _hwnd: i64,
    _source: NotepadSaveCommandSource,
) -> Result<TargetActSaveAttempt, ErrorData> {
    Err(mcp_error(
        error_codes::ACTION_TARGET_INVALID,
        "target_act verb=save Notepad WM_COMMAND route is only available on Windows",
    ))
}

fn target_act_session_id(
    request_context: &RequestContext<RoleServer>,
    verb: &str,
) -> Result<String, ErrorData> {
    super::context::mcp_session_id_from_request_context(request_context)?.ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb={verb} requires an MCP session id"),
        )
    })
}

async fn target_act_browser_dom_primitive(
    service: &SynapseService,
    action: &'static str,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    target_act_validate_dom_primitive_params(action, params)?;
    if let Some(raw_element_id) = params
        .element_id
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        match ElementId::parse(raw_element_id) {
            Ok(element_id) => {
                if synapse_a11y::cdp_backend_from_element_id(&element_id).is_some() {
                    return target_act_cdp_dom_primitive(
                        service,
                        action,
                        element_id,
                        request_context,
                    )
                    .await;
                }
                return Err(mcp_error(
                    error_codes::ACTION_TARGET_INVALID,
                    format!(
                        "target_act verb={action} supports browser DOM/CDP targets only; native/UIA element_id {raw_element_id:?} is not supported by this primitive"
                    ),
                ));
            }
            Err(error) if !target_act_click_element_id_can_be_dom_id(raw_element_id) => {
                return Err(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    format!("target_act verb={action} element_id is invalid: {error}"),
                ));
            }
            Err(_) => {}
        }
    }
    target_act_browser_dom_action(service, action, params, request_context).await
}

fn target_act_validate_dom_primitive_params(
    action: &str,
    params: &TargetActParams,
) -> Result<(), ErrorData> {
    if target_act_coordinate(params)?.is_some() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb={action} does not accept x/y coordinates"),
        ));
    }
    if target_act_has_key_chord(params) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={action} does not accept key(s); use verb=key for raw keyboard chords"
            ),
        ));
    }
    if params.clicks.is_some() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb={action} does not accept clicks"),
        ));
    }
    if params.text.as_ref().is_some_and(|value| !value.is_empty()) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb={action} does not accept text"),
        ));
    }
    if params
        .option
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
        || params
            .option_label
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        || params.option_index.is_some()
        || !params.options.is_empty()
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb={action} does not accept select option fields"),
        ));
    }
    if params
        .event_type
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
        || params.event_init.is_some()
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb={action} does not accept event_type/event_init"),
        ));
    }
    if params.selection_start.is_some() || params.selection_end.is_some() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={action} selects the whole element and does not accept selection_start/selection_end"
            ),
        ));
    }
    target_act_validate_dom_locator(action, params)
}

#[cfg(windows)]
async fn target_act_cdp_dom_primitive(
    service: &SynapseService,
    action: &'static str,
    element_id: ElementId,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let session_id = target_act_session_id(request_context, action)?;
    let backend_node_id =
        synapse_a11y::cdp_backend_from_element_id(&element_id).ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!("target_act verb={action} element_id is not a CDP-backed web element"),
            )
        })?;
    let element_target_id =
        synapse_a11y::cdp_target_from_element_id(&element_id).ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "target_act verb={action} web element_id must include an embedded CDP target id; re-resolve it against the owned tab"
                ),
            )
        })?;
    let element_hwnd = element_id
        .parts()
        .map_err(|error| {
            mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!("target_act verb={action} element_id is invalid: {error}"),
            )
        })?
        .hwnd;
    let request_details = json!({
        "session_id": &session_id,
        "verb": action,
        "element_id": element_id.to_string(),
        "element_hwnd": element_hwnd,
        "backend_node_id": backend_node_id,
        "element_cdp_target_id": &element_target_id,
        "delegated_tool": "synapse_a11y.cdp_dom_primitive_node",
        "required_foreground": false,
    });
    let Some(target) = service.session_target(Some(&session_id))? else {
        let error = mcp_error(
            error_codes::TARGET_NOT_SET,
            format!(
                "target_act verb={action} requires this MCP session to own a CDP browser target; bind one with cdp_open_tab/set_target first"
            ),
        );
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "synapse_a11y.cdp_dom_primitive_node",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    };
    let (window_hwnd, cdp_target_id) = match &target {
        SessionTarget::Cdp {
            window_hwnd,
            cdp_target_id,
        } => (*window_hwnd, cdp_target_id.clone()),
        SessionTarget::Window { .. } => {
            let error = mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                format!(
                    "target_act verb={action} with an observed web element_id requires a session-owned CDP target, not a native/window target"
                ),
            );
            service.audit_action_denied_with_details_for_session(
                "target_act",
                &error,
                &request_details,
                &session_id,
            );
            return Ok((
                "synapse_a11y.cdp_dom_primitive_node",
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        }
    };
    if window_hwnd != element_hwnd {
        let error = mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "target_act verb={action} element belongs to browser HWND 0x{element_hwnd:x}, but this session target is HWND 0x{window_hwnd:x}"
            ),
        );
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "synapse_a11y.cdp_dom_primitive_node",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    }
    if let Err(error) =
        service.ensure_target_claim_allows_session("target_act", &session_id, &target)
    {
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "synapse_a11y.cdp_dom_primitive_node",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    }
    let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) else {
        let error = mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "target_act verb={action} requires a raw CDP debugging endpoint for window 0x{window_hwnd:x}; re-resolve by selector/role/name to use the normal Chrome bridge path"
            ),
        );
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "synapse_a11y.cdp_dom_primitive_node",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    };
    let routed_target_id = match target_act_validate_observed_cdp_element_target(
        &endpoint,
        window_hwnd,
        &cdp_target_id,
        &element_target_id,
        action,
    )
    .await
    {
        Ok(()) => element_target_id.clone(),
        Err(error) => {
            service.audit_action_denied_with_details_for_session(
                "target_act",
                &error,
                &request_details,
                &session_id,
            );
            return Ok((
                "synapse_a11y.cdp_dom_primitive_node",
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        }
    };
    let request_details = json!({
        "session_id": &session_id,
        "verb": action,
        "element_id": element_id.to_string(),
        "element_hwnd": element_hwnd,
        "backend_node_id": backend_node_id,
        "session_cdp_target_id": &cdp_target_id,
        "element_cdp_target_id": &element_target_id,
        "dispatch_cdp_target_id": &routed_target_id,
        "delegated_tool": "synapse_a11y.cdp_dom_primitive_node",
        "required_foreground": false,
    });
    service.audit_action_started_with_details_for_session(
        "target_act",
        &request_details,
        &session_id,
    )?;
    let result =
        synapse_a11y::cdp_dom_primitive_node(&endpoint, &routed_target_id, backend_node_id, action)
            .await
            .map_err(|error| {
                mcp_error(
                    error.code(),
                    format!("target_act CDP {action} failed: {error}"),
                )
            });
    service.audit_action_result_for_session("target_act", &result, &session_id)?;
    match result {
        Ok(readback) => {
            let mut value = target_act_result(&readback)?;
            if let Some(object) = value.as_object_mut() {
                object.insert("element_id".to_owned(), json!(element_id.to_string()));
                object.insert("backend_node_id".to_owned(), json!(backend_node_id));
                object.insert("target_id".to_owned(), json!(routed_target_id));
                object.insert("session_target_id".to_owned(), json!(cdp_target_id));
                object.insert("window_hwnd".to_owned(), json!(window_hwnd));
            }
            Ok((
                "synapse_a11y.cdp_dom_primitive_node",
                true,
                TARGET_ACT_STATUS_OK,
                value,
            ))
        }
        Err(error) => Ok((
            "synapse_a11y.cdp_dom_primitive_node",
            false,
            target_act_error_status(&error),
            target_act_error_result("synapse_a11y.cdp_dom_primitive_node", error),
        )),
    }
}

#[cfg(not(windows))]
async fn target_act_cdp_dom_primitive(
    _service: &SynapseService,
    action: &'static str,
    _element_id: ElementId,
    _request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let error = mcp_error(
        error_codes::ACTION_TARGET_INVALID,
        format!(
            "target_act verb={action} observed CDP element_id primitives require Windows CDP action support"
        ),
    );
    Ok((
        "synapse_a11y.cdp_dom_primitive_node",
        false,
        target_act_error_status(&error),
        target_act_error_result("synapse_a11y.cdp_dom_primitive_node", error),
    ))
}

/// #2348: `act select` on a raw-CDP page, or on the raw-CDP page inside a
/// native window (a web element_id from find names its page). The element is
/// found the way a raw-CDP click finds it; `synapse_a11y::cdp_select_option`
/// (#2365) picks the option and reads the page's `selectedIndex` back.
#[cfg(windows)]
async fn target_act_raw_cdp_select(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    const TOOL: &str = "synapse_a11y.cdp_select_option";
    let session_id = target_act_session_id(request_context, "select")?;
    let choice = params.option_choice().map_err(|text| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb=select: {text}"),
        )
    })?;
    let Some(target) = service.session_target(Some(&session_id))? else {
        return Err(mcp_error(
            error_codes::TARGET_NOT_SET,
            "target_act verb=select requires this MCP session to own a target; set one with set_target (target operation=set) or cdp_open_tab and claim it (target operation=claim) first; refusing to use the human foreground tab",
        ));
    };
    let (window_hwnd, page_target_id) = match &target {
        SessionTarget::Cdp {
            window_hwnd,
            cdp_target_id,
        } => (*window_hwnd, Some(cdp_target_id.clone())),
        // #2406: on a window target the page is the window's own page (read
        // below from its endpoint), never the page the element id names.
        SessionTarget::Window { hwnd } => (*hwnd, None),
    };
    let request_details = json!({
        "session_id": &session_id,
        "verb": "select",
        "window_hwnd": window_hwnd,
        "delegated_tool": TOOL,
        "required_foreground": false,
    });
    let found = async {
        service.ensure_target_claim_allows_session("target_act", &session_id, &target)?;
        let endpoint = synapse_a11y::endpoint_for_window(window_hwnd).ok_or_else(|| {
            mcp_error(
                error_codes::A11Y_CDP_UNREACHABLE,
                format!(
                    "target_act verb=select found no raw CDP endpoint for window 0x{window_hwnd:x}; nothing was sent"
                ),
            )
        })?;
        // #2406: an element named by id must be inside this session's target.
        if let Some(element_id) = params
            .element_id
            .as_deref()
            .and_then(|raw| ElementId::parse(raw.trim()).ok())
            && let ScopeVerdict::Refused { code, text } =
                target_act_element_scope(service, request_context, &element_id, "select")
                    .await?
                    .verdict("select")
        {
            return Err(mcp_error(code, text));
        }
        let page_target_id = match page_target_id {
            Some(page_target_id) => page_target_id,
            None => ScopeTarget::new(
                Some(target.clone()),
                target_act_window_pages(&endpoint).await?,
            )
            .page()
            .map(str::to_owned)
            .ok_or_else(|| {
                mcp_error(
                    error_codes::ACTION_TARGET_INVALID,
                    format!(
                        "target_act verb=select refused: the page of the session target window HWND 0x{window_hwnd:x} cannot be told (its raw CDP endpoint does not list exactly one page); target the page itself with cdp_open_tab first; nothing was sent"
                    ),
                )
            })?,
        };
        // `value` and `option` name the option here, not the element.
        let locator = TargetActParams {
            value: None,
            option: None,
            option_label: None,
            ..params.clone()
        };
        let element =
            target_act_tap_element(&endpoint, window_hwnd, &page_target_id, &locator, "select")
                .await?;
        Ok::<_, ErrorData>((endpoint, element))
    }
    .await;
    let (endpoint, element) = match found {
        Ok(found) => found,
        Err(error) => {
            service.audit_action_denied_with_details_for_session(
                "target_act",
                &error,
                &request_details,
                &session_id,
            );
            return Ok((
                TOOL,
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        }
    };
    let request_details = json!({
        "session_id": &session_id,
        "verb": "select",
        "window_hwnd": window_hwnd,
        "cdp_target_id": &element.target_id,
        "backend_node_id": element.backend_node_id,
        "resolved_by": &element.resolved_by,
        "delegated_tool": TOOL,
        "required_foreground": false,
    });
    service.audit_action_started_with_details_for_session(
        "target_act",
        &request_details,
        &session_id,
    )?;
    let result = synapse_a11y::cdp_select_option(
        &endpoint,
        &element.target_id,
        element.backend_node_id,
        &choice,
    )
    .await
    .map(|readback| {
        json!({
            "option": {
                "index": readback.index(),
                "label": readback.label(),
                "value": readback.value(),
            },
            "source_of_truth": "raw_cdp.select.selectedIndex",
            "postcondition": {
                "status": "verified_state",
                "source_of_truth": "raw_cdp.select.selectedIndex",
            },
            "window_hwnd": window_hwnd,
            "target_id": &element.target_id,
            "backend_node_id": element.backend_node_id,
            "element_id": &element.element_id,
            "resolved_by": &element.resolved_by,
            "readback_backend": "raw_cdp",
        })
    })
    .map_err(|error| {
        mcp_error(
            error.code(),
            format!("target_act verb=select via raw CDP failed: {error}"),
        )
    });
    service.audit_action_result_for_session("target_act", &result, &session_id)?;
    match result {
        Ok(value) => Ok((TOOL, true, TARGET_ACT_STATUS_OK, value)),
        Err(error) => Ok((
            TOOL,
            false,
            target_act_error_status(&error),
            target_act_error_result(TOOL, error),
        )),
    }
}

#[cfg(not(windows))]
async fn target_act_raw_cdp_select(
    _service: &SynapseService,
    _params: &TargetActParams,
    _request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let error = mcp_error(
        error_codes::ACTION_TARGET_INVALID,
        "target_act verb=select on a raw-CDP page requires Windows CDP action support",
    );
    Ok((
        "synapse_a11y.cdp_select_option",
        false,
        target_act_error_status(&error),
        target_act_error_result("synapse_a11y.cdp_select_option", error),
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativePatternAction {
    /// A click through the UIA pattern that fits the control (#107 part b).
    Click,
    SetChecked(bool),
    /// #2348: pick one option of a native list or combo box.
    Select,
}

#[derive(Debug, PartialEq, Eq)]
enum NativePatternLocator<'a> {
    ElementId(ElementId),
    NameAndRole {
        name: &'a str,
        role: &'a str,
    },
    /// #2348: name alone, looked up like name + role with no role.
    Name(&'a str),
}

fn target_act_native_pattern_route<'a>(
    target: Option<&SessionTarget>,
    params: &'a TargetActParams,
) -> Option<(i64, NativePatternAction, NativePatternLocator<'a>)> {
    let Some(SessionTarget::Window { hwnd }) = target else {
        return None;
    };
    let action = match params.verb.as_str() {
        "click" => NativePatternAction::Click,
        "check" => NativePatternAction::SetChecked(true),
        "uncheck" => NativePatternAction::SetChecked(false),
        _ => return None,
    };
    if [
        params.selector.as_deref(),
        params.value.as_deref(),
        params.option.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.trim().is_empty())
    {
        return None;
    }
    let locator = if let Some(id) = params
        .element_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
    {
        // An observed native element_id click keeps the existing act_click UIA
        // pattern chain (Invoke, Toggle, SelectionItem, ExpandCollapse, Legacy).
        if action == NativePatternAction::Click {
            return None;
        }
        let id = ElementId::parse(id).ok()?;
        if synapse_a11y::cdp_backend_from_element_id(&id).is_some() {
            return None;
        }
        NativePatternLocator::ElementId(id)
    } else {
        let name = params.name.as_deref()?.trim();
        let role = params.role.as_deref()?.trim();
        if name.is_empty() || role.is_empty() {
            return None;
        }
        NativePatternLocator::NameAndRole { name, role }
    };
    Some((*hwnd, action, locator))
}

async fn target_act_try_native_pattern(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<Option<(&'static str, bool, &'static str, Value)>, ErrorData> {
    let Some(session_id) = super::context::mcp_session_id_from_request_context(request_context)?
    else {
        return Ok(None);
    };
    let target = service.session_target(Some(&session_id))?;
    // #2348: the #300 route first; then click / check / uncheck by name alone
    // and select by element id, name + role or name (`ActRoute` sent them here).
    let Some((window_hwnd, action, locator)) =
        target_act_native_pattern_route(target.as_ref(), params).or_else(|| {
            let Some(SessionTarget::Window { hwnd }) = target.as_ref() else {
                return None;
            };
            let action = match params.verb.as_str() {
                "click" => NativePatternAction::Click,
                "check" => NativePatternAction::SetChecked(true),
                "uncheck" => NativePatternAction::SetChecked(false),
                "select" => NativePatternAction::Select,
                _ => return None,
            };
            let element_id = params
                .element_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let name = params
                .name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let role = params
                .role
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let locator = match (element_id, name, role) {
                (Some(id), _, _) if action != NativePatternAction::Click => {
                    NativePatternLocator::ElementId(ElementId::parse(id).ok()?)
                }
                (Some(_), _, _) | (None, None, _) => return None,
                (None, Some(name), Some(role)) => NativePatternLocator::NameAndRole { name, role },
                (None, Some(name), None) => NativePatternLocator::Name(name),
            };
            Some((*hwnd, action, locator))
        })
    else {
        return Ok(None);
    };
    let verb = params.verb.as_str();
    if target_act_coordinate(params)?.is_some()
        || target_act_has_click_position_input(params)
        || params.clicks.is_some_and(|clicks| clicks != 1)
        || !matches!(params.button, None | Some(TargetActMouseButton::Left))
        || !params.modifiers.is_empty()
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={verb} on a native UIA element requires an unmodified, non-coordinate action"
            ),
        ));
    }
    let (delegated_tool, pattern) = match action {
        NativePatternAction::Click => (
            "synapse_a11y.click_element_action",
            "click pattern (Invoke, Toggle, SelectionItem, ExpandCollapse or LegacyIAccessible)",
        ),
        NativePatternAction::SetChecked(_) => ("synapse_a11y.set_element_checked", "TogglePattern"),
        NativePatternAction::Select => (
            "synapse_a11y.select_element_option",
            "option pick (a classic combo box through its own list messages, else the SelectionItemPattern of an option, opening a closed list first)",
        ),
    };
    let request_details = json!({
        "session_id": &session_id,
        "verb": verb,
        "window_hwnd": window_hwnd,
        "required_foreground": false,
    });
    let target = SessionTarget::Window { hwnd: window_hwnd };
    let resolution = (|| {
        service.ensure_target_claim_allows_session("target_act", &session_id, &target)?;
        let element_id = match locator {
            NativePatternLocator::ElementId(id) => id,
            NativePatternLocator::Name(name) => service.resolve_native_action_locator(
                "target_act",
                &ActSetFieldTextLocator {
                    window_hwnd: Some(window_hwnd),
                    role: None,
                    name: Some(name.to_owned()),
                    name_substring: None,
                    automation_id: None,
                },
                window_hwnd,
                "pre_dispatch",
            )?,
            NativePatternLocator::NameAndRole { name, role } => service
                .resolve_native_action_locator(
                    "target_act",
                    &ActSetFieldTextLocator {
                        window_hwnd: Some(window_hwnd),
                        role: Some(role.to_owned()),
                        name: Some(name.to_owned()),
                        name_substring: None,
                        automation_id: None,
                    },
                    window_hwnd,
                    "pre_dispatch",
                )?,
        };
        // 300f: an element of another window is refused before any UIA call.
        ElementRoot::read(&element_id, &format!("target_act verb={verb}"))?
            .ensure_in(window_hwnd)?;
        Ok(element_id)
    })();
    let element_id = match resolution {
        Ok(element_id) => element_id,
        Err(error) => {
            if params.secret_safe {
                audit_action_redacted_error_for_session(
                    service,
                    delegated_tool,
                    &error,
                    &session_id,
                )?;
            } else {
                service.audit_action_denied_with_details_for_session(
                    "target_act",
                    &error,
                    &request_details,
                    &session_id,
                );
            }
            return Ok(Some((
                delegated_tool,
                false,
                target_act_error_status(&error),
                target_act_maybe_secret_safe_error_result(
                    delegated_tool,
                    error,
                    params.secret_safe,
                ),
            )));
        }
    };
    let request_details = json!({
        "session_id": &session_id,
        "verb": verb,
        "window_hwnd": window_hwnd,
        "element_id": &element_id,
        "required_foreground": false,
    });
    service.audit_action_started_with_details_for_session(
        "target_act",
        &request_details,
        &session_id,
    )?;
    // #300: a native UIA provider (WinForms) may activate its own window while serving
    // the pattern call; read the foreground root around the call. #316: before-read,
    // call and settle run inside the daemon-wide native foreground section.
    let foreground_section = native_foreground_section().await;
    let foreground_before = synapse_a11y::foreground_root_identity().ok();
    let result = match action {
        NativePatternAction::Select => match params.option_choice() {
            Ok(choice) => synapse_a11y::select_element_option(&element_id, &choice).map(|readback| {
                // #2348 round 2: the delivery `OptionRoute` chose names its own readback.
                let delivery = readback.delivery();
                json!({
                    "pattern": pattern,
                    "delivery": delivery,
                    "source_of_truth": delivery.source_of_truth(),
                    "window_hwnd": window_hwnd,
                    "element_id": &element_id,
                    "changed": !readback.was_selected(),
                    "option": readback,
                    "postcondition": {
                        "status": "verified_state",
                        "source_of_truth": delivery.source_of_truth(),
                    },
                })
            }),
            Err(detail) => Err(synapse_a11y::A11yError::OptionRefused { detail }),
        },
        NativePatternAction::Click => synapse_a11y::click_element_action(&element_id).map(|action| {
            super::native_click_evidence::PatternClickEvidence::of(action)
                .reply(window_hwnd, &element_id)
        }),
        NativePatternAction::SetChecked(checked) => {
            synapse_a11y::set_element_checked(&element_id, checked).map(|(before, after)| json!({
                "pattern": pattern,
                "delivery": "uia_toggle_pattern",
                "source_of_truth": "uia_toggle_pattern.toggle_state",
                "window_hwnd": window_hwnd,
                "element_id": &element_id,
                "changed": before != after,
                "before_state": before,
                "after_state": after,
                "postcondition": {
                    "status": "verified_state",
                    "source_of_truth": "uia_toggle_pattern.toggle_state",
                },
            }))
        }
    }
    .map_err(|error| {
        use crate::m2::postcondition::{mark_input_delivered, mark_input_possibly_delivered};

        let detail = if error.code() == error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED {
            mcp_error(
                error.code(),
                format!(
                    "target_act verb={verb} refused: element {element_id} does not expose a UIA {pattern}; no click, coordinate or SendInput fallback"
                ),
            )
        } else {
            mcp_error(error.code(), format!("target_act verb={verb} via the UIA {pattern} failed: {error}"))
        };
        match error {
            synapse_a11y::A11yError::ClickDispatch { delivered: true, .. } => mark_input_delivered(detail),
            synapse_a11y::A11yError::ClickDispatch { .. } => mark_input_possibly_delivered(detail),
            _ => detail,
        }
    });
    let foreground_after = synapse_a11y::foreground_root_identity().ok();
    let (outcome, foreground) = settle_native_pattern_foreground(
        "target_act",
        &session_id,
        window_hwnd,
        foreground_before,
        foreground_after,
    );
    drop(foreground_section);
    let action_source_of_truth = result
        .as_ref()
        .ok()
        .and_then(|value| value.get("source_of_truth"))
        .and_then(Value::as_str)
        .unwrap_or(delegated_tool)
        .to_owned();
    let mut result = result
        .map(|mut value| {
            foreground.merge_into(&mut value);
            value
        })
        .map_err(|mut error| {
            if let Some(data) = error.data.as_mut() {
                foreground.merge_into(data);
            }
            error
        });
    // The target took the foreground and the prior window was not put back: the
    // same ACTION_FOREGROUND_LOST error the guarded click path returns.
    if let Some(code) = background_click_foreground_error(outcome, foreground.restored()) {
        let lost = background_click_foreground_lost_error(
            "target_act",
            code,
            &action_source_of_truth,
            BackgroundTargetForegroundGuard {
                element_hwnd: element_id.parts().map_or(window_hwnd, |parts| parts.hwnd),
                root_hwnd: window_hwnd,
            },
            foreground_before,
            foreground_after,
            &foreground,
        );
        result = Err(merge_background_foreground_error(result, lost));
    }
    audit_action_result_maybe_redacted_for_session(
        service,
        delegated_tool,
        &result,
        &session_id,
        params.secret_safe,
    )?;
    Ok(Some(match result {
        Ok(value) => (delegated_tool, true, TARGET_ACT_STATUS_OK, value),
        Err(error) => (
            delegated_tool,
            false,
            target_act_error_status(&error),
            target_act_maybe_secret_safe_error_result(delegated_tool, error, params.secret_safe),
        ),
    }))
}

#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct NativeForegroundEvidence {
    foreground_outcome: &'static str,
    foreground_root_hwnd_before: Option<i64>,
    foreground_root_hwnd_after: Option<i64>,
    foreground_restore: NativeForegroundRestore,
    foreground_untouched: bool,
}

impl NativeForegroundEvidence {
    pub(super) const fn required_foreground(&self) -> bool {
        !self.foreground_untouched
    }

    pub(super) fn restored(&self) -> Option<bool> {
        self.foreground_restore
            .readback
            .as_ref()
            .map(|readback| readback.result == "restored")
    }

    pub(super) fn merge_into(&self, target: &mut Value) {
        let mut fields = json!(self);
        fields["required_foreground"] = json!(self.required_foreground());
        target_act_merge_object_fields(target, &fields);
    }
}

#[derive(Debug, Serialize, JsonSchema)]
struct NativeForegroundRestore {
    attempted: bool,
    #[serde(flatten)]
    readback: Option<NativeForegroundRestoreReadback>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct NativeForegroundRestoreReadback {
    result: &'static str,
    restore_to_root_hwnd: i64,
    foreground_root_hwnd_after_restore: Option<i64>,
}

/// #316: the one owner of "the window that was in front before Synapse's background
/// actions started". Every native UIA pattern action that may take the OS foreground
/// (target_act native patterns and guarded act tools) reads its foreground
/// "before", runs the pattern call and settles/restores inside this daemon-wide
/// section, so a concurrent action never samples a window another action's target
/// just brought forward, and each restore puts back the real prior window before the
/// next action starts. Only these foreground-taking native pattern actions take it;
/// other background routes are not serialized.
///
/// Lock order: this section first, then the synapse-a11y UIA worker slot and its
/// `job_lock`. No UIA worker job ever takes this section, so the two cannot deadlock.
/// Fair (FIFO) async mutex: the guard is `Send` and may be held across awaits.
pub(super) async fn native_foreground_section() -> tokio::sync::MutexGuard<'static, ()> {
    static SECTION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    SECTION.lock().await
}

/// Classify the foreground after native UIA dispatch and restore it only when
/// the target took it from a prior window (#300, #316).
pub(super) fn settle_native_pattern_foreground(
    tool: &'static str,
    session_id: &str,
    target_root: i64,
    before: Option<synapse_a11y::WindowRootIdentity>,
    after: Option<synapse_a11y::WindowRootIdentity>,
) -> (NativeForegroundOutcome, NativeForegroundEvidence) {
    let before_root = before.map(|identity| identity.root_hwnd);
    let after_root = after.map(|identity| identity.root_hwnd);
    let outcome = native_pattern_foreground_outcome(before_root, after_root, target_root);
    let readback = match (outcome.restore_target(), before, after) {
        (Some(restore_to_root_hwnd), Some(prior), Some(taken)) => {
            let result =
                crate::m2::restore_foreground_taken_by_target(tool, session_id, prior, taken);
            Some(NativeForegroundRestoreReadback {
                result,
                restore_to_root_hwnd,
                foreground_root_hwnd_after_restore: synapse_a11y::foreground_root_identity()
                    .ok()
                    .map(|identity| identity.root_hwnd),
            })
        }
        _ => None,
    };
    let evidence = NativeForegroundEvidence {
        foreground_outcome: outcome.as_str(),
        foreground_root_hwnd_before: before_root,
        foreground_root_hwnd_after: after_root,
        foreground_restore: NativeForegroundRestore {
            attempted: readback.is_some(),
            readback,
        },
        foreground_untouched: outcome.foreground_untouched(),
    };
    (outcome, evidence)
}

/// Copies every field of the `fields` object into `target` when both are objects.
fn target_act_merge_object_fields(target: &mut Value, fields: &Value) {
    if let (Some(target), Some(fields)) = (target.as_object_mut(), fields.as_object()) {
        for (key, value) in fields {
            target.insert(key.clone(), value.clone());
        }
    }
}

async fn target_act_browser_dom_action(
    service: &SynapseService,
    action: &str,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let session_id = target_act_session_id(request_context, action)?;
    let is_click = target_act_is_click_like_dom_action(action);
    if target_act_coordinate(params)?.is_some() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={action} does not accept x/y; use verb=click or verb=type for coordinate fallback"
            ),
        ));
    }
    target_act_validate_dom_locator(action, params)?;
    crate::m2::validate_auto_wait_timeout(
        "target_act",
        params.auto_wait,
        params.auto_wait_timeout_ms,
    )?;
    let wait_timeout_ms = target_act_dom_wait_timeout(params.wait_timeout_ms)?;
    let click_count = target_act_dom_click_count(action, params.clicks)?;
    let click_position = target_act_click_position(params)?;
    let click_modifiers = target_act_click_modifiers_bridge_value(&params.modifiers)?;
    let request_details = json!({
        "session_id": &session_id,
        "verb": action,
        "selector_present": params.selector.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "element_id_present": params.element_id.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "role": params.role.as_deref(),
        "name_present": params.name.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "value_present": params.value.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "option_present": params.option.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "option_label_present": params.option_label.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "option_index": params.option_index,
        "options_count": params.options.len(),
        "event_type": params.event_type.as_deref(),
        "event_init_present": params.event_init.is_some(),
        "button": params.button.map(TargetActMouseButton::as_str),
        "modifiers": &click_modifiers,
        "position": click_position.map(|(x, y)| json!({ "x": x, "y": y })),
        "clicks": click_count,
        "wait_timeout_ms": wait_timeout_ms,
        "auto_wait": params.auto_wait,
        "auto_wait_timeout_ms": params.auto_wait_timeout_ms,
        "secret_safe": params.secret_safe,
        "required_foreground": false,
    });
    let (window_hwnd, cdp_target_id) = match service.audit_cdp_target_resolution_result(
        "target_act",
        &session_id,
        &request_details,
        service.resolve_cdp_tab_mutation_target("target_act", &session_id, None, None),
    ) {
        Ok(resolved) => resolved,
        Err(error) => {
            return Ok((
                "chrome_debugger_bridge.domAction",
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        }
    };
    let target = SessionTarget::Cdp {
        window_hwnd,
        cdp_target_id: cdp_target_id.clone(),
    };
    if let Err(error) =
        service.ensure_target_claim_allows_session("target_act", &session_id, &target)
    {
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "chrome_debugger_bridge.domAction",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    }
    let request_details = json!({
        "session_id": &session_id,
        "verb": action,
        "window_hwnd": window_hwnd,
        "cdp_target_id": &cdp_target_id,
        "selector_present": params.selector.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "element_id_present": params.element_id.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "role": params.role.as_deref(),
        "name_present": params.name.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "value_present": params.value.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "option_present": params.option.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "option_label_present": params.option_label.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "option_index": params.option_index,
        "options_count": params.options.len(),
        "event_type": params.event_type.as_deref(),
        "event_init_present": params.event_init.is_some(),
        "button": params.button.map(TargetActMouseButton::as_str),
        "modifiers": &click_modifiers,
        "position": click_position.map(|(x, y)| json!({ "x": x, "y": y })),
        "clicks": click_count,
        "wait_timeout_ms": wait_timeout_ms,
        "auto_wait": params.auto_wait,
        "auto_wait_timeout_ms": params.auto_wait_timeout_ms,
        "secret_safe": params.secret_safe,
        "required_foreground": false,
    });
    // #198: encode the options before the started row so its failure cannot
    // strand an unclosed started row.
    let options_value = (!params.options.is_empty())
        .then(|| serde_json::to_value(&params.options))
        .transpose()
        .map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("target_act select options encode failed: {error}"),
            )
        })?;
    service.audit_action_started_with_details_for_session(
        "target_act",
        &request_details,
        &session_id,
    )?;
    let mut result = crate::chrome_debugger_bridge::dom_action(
        crate::chrome_debugger_bridge::ChromeDebuggerDomActionRequest {
            hwnd: window_hwnd,
            target_id: &cdp_target_id,
            action,
            selector: params.selector.as_deref(),
            element_id: params.element_id.as_deref(),
            role: params.role.as_deref(),
            name: params.name.as_deref(),
            value: params.value.as_deref(),
            option: params.option.as_deref(),
            option_label: params.option_label.as_deref(),
            option_index: params.option_index,
            options: options_value.as_ref(),
            event_type: params.event_type.as_deref(),
            event_init: params.event_init.as_ref(),
            clicks: click_count,
            button: params.button.map(TargetActMouseButton::as_str),
            modifiers: Some(&click_modifiers),
            position_x: click_position.map(|(x, _)| x),
            position_y: click_position.map(|(_, y)| y),
            wait_timeout_ms,
            auto_wait: params.auto_wait,
            auto_wait_timeout_ms: params.auto_wait_timeout_ms,
            suppress_page_text: params.secret_safe,
        },
    )
    .await
    .map_err(|error| {
        if is_click {
            crate::m2::remote_bridge_click_error(&error)
        } else {
            mcp_error(error.code(), error.detail().to_owned())
        }
    });
    if let Ok(value) = result.as_mut() {
        if let Err(error) =
            target_act_register_created_popup_tabs(service, &session_id, window_hwnd, value)
        {
            result = Err(if is_click {
                crate::m2::remote_click_error(error, true)
            } else {
                error
            });
        }
    }
    audit_action_result_maybe_redacted_for_session(
        service,
        "chrome_debugger_bridge.domAction",
        &result,
        &session_id,
        params.secret_safe,
    )
    .map_err(|error| {
        if is_click {
            crate::m2::carry_remote_click_delivery(error, &result)
        } else {
            error
        }
    })?;
    match result {
        Ok(value) => {
            let value = target_act_maybe_secret_safe_result(
                "chrome_debugger_bridge.domAction",
                value,
                params.secret_safe,
            )
            .map_err(|error| {
                if is_click {
                    crate::m2::remote_click_error(error, true)
                } else {
                    error
                }
            })?;
            Ok((
                "chrome_debugger_bridge.domAction",
                true,
                TARGET_ACT_STATUS_OK,
                value,
            ))
        }
        Err(error) => Ok((
            "chrome_debugger_bridge.domAction",
            false,
            target_act_error_status(&error),
            target_act_maybe_secret_safe_error_result(
                "chrome_debugger_bridge.domAction",
                error,
                params.secret_safe,
            ),
        )),
    }
}

fn target_act_register_created_popup_tabs(
    service: &SynapseService,
    session_id: &str,
    window_hwnd: i64,
    action_value: &mut Value,
) -> Result<(), ErrorData> {
    let tabs = action_value
        .get("created_popup_tabs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if tabs.is_empty() {
        return Ok(());
    }

    let reported_extension_id = action_value
        .get("extension_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut registered = Vec::with_capacity(tabs.len());
    for (index, tab) in tabs.iter().enumerate() {
        let target_id = tab
            .get("target_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                mcp_error(
                    error_codes::ACTION_POSTCONDITION_FAILED,
                    format!(
                        "target_act DOM action created popup tab at index {index}, but the bridge did not return target_id"
                    ),
                )
            })?;
        validate_cdp_target_id(target_id)?;
        // Multi-browser: the owner endpoint names the tab's own browser row.
        let endpoint = synapse_browsers::BrowserRegistry::global()
            .bridge_endpoint_for_reported_target(
                target_id,
                reported_extension_id.as_deref(),
                synapse_browsers::BridgeLane::Tabs,
            );
        let target_url = tab
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let chrome_window_id = tab.get("chrome_window_id").and_then(Value::as_i64);
        let owner_key = service.register_cdp_target_owner(CdpTargetOwner {
            session_id: session_id.to_owned(),
            window_hwnd,
            endpoint: endpoint.clone(),
            chrome_window_id,
            capture_window_hwnd: None,
            cdp_target_id: target_id.to_owned(),
            requested_url: target_url.clone(),
            target_url,
            created_at_unix_ms: target_act_unix_ms_now(),
        })?;
        tracing::info!(
            code = "TARGET_ACT_POPUP_TAB_OWNER_REGISTERED",
            session_id = %session_id,
            hwnd = window_hwnd,
            endpoint = %endpoint,
            cdp_target_id = %target_id,
            owner_key = %owner_key,
            chrome_window_id = chrome_window_id.unwrap_or_default(),
            "target_act registered bridge-created popup tab owner"
        );
        registered.push(json!({
            "target_id": target_id,
            "owner_key": owner_key,
            "endpoint": endpoint,
            "window_hwnd": window_hwnd,
            "chrome_window_id": chrome_window_id,
        }));
    }
    if let Some(object) = action_value.as_object_mut() {
        object.insert("created_popup_tab_owners".to_owned(), json!(registered));
    }
    Ok(())
}

fn target_act_unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn raw_cdp_click_css_point(
    space: TargetActCoordinateSpace,
    x: i32,
    y: i32,
    client_physical: Option<synapse_core::Point>,
    width: f64,
    height: f64,
    dpr: f64,
) -> Result<synapse_a11y::CdpActionPoint, ErrorData> {
    let (x, y) =
        if space == TargetActCoordinateSpace::Viewport {
            (f64::from(x), f64::from(y))
        } else {
            let client = client_physical.ok_or_else(|| mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            "target_act raw-CDP coordinate click could not read the host client-area origin",
        ))?;
            (f64::from(client.x) / dpr, f64::from(client.y) / dpr)
        };
    if !dpr.is_finite()
        || dpr <= 0.0
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || !x.is_finite()
        || !y.is_finite()
        || x < 0.0
        || y < 0.0
        || x >= width
        || y >= height
    {
        return Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "target_act raw-CDP coordinate click is outside the page viewport: point=({x},{y}), viewport=({width},{height}), device_pixel_ratio={dpr}"
            ),
        ));
    }
    Ok(synapse_a11y::CdpActionPoint { x, y })
}

#[cfg(windows)]
async fn target_act_raw_cdp_coordinate_click(
    endpoint: &str,
    window_hwnd: i64,
    target_id: &str,
    coordinate: TargetActCoordinate,
    clicks: u8,
    params: &TargetActParams,
) -> Result<Value, ErrorData> {
    let viewport = synapse_a11y::cdp_evaluate_expression(
        endpoint,
        target_id,
        "({ width: innerWidth, height: innerHeight, dpr: devicePixelRatio })",
        false,
        true,
    )
    .await
    .map_err(|error| {
        mcp_error(
            error.code(),
            format!("target_act raw-CDP viewport readback failed: {error}"),
        )
    })?;
    let width = viewport.value.get("width").and_then(Value::as_f64);
    let height = viewport.value.get("height").and_then(Value::as_f64);
    let dpr = viewport.value.get("dpr").and_then(Value::as_f64);
    let (Some(width), Some(height), Some(dpr)) = (width, height, dpr) else {
        return Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            "target_act raw-CDP viewport metrics are missing",
        ));
    };
    let client_physical = if coordinate.space == TargetActCoordinateSpace::Viewport {
        None
    } else {
        let screen = target_act_window_coordinate_to_screen_point(window_hwnd, coordinate)?;
        let origin = synapse_a11y::client_origin_screen(window_hwnd).ok_or_else(|| {
            mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                "target_act raw-CDP coordinate click could not read the host client-area origin",
            )
        })?;
        Some(synapse_core::Point {
            x: screen.x.saturating_sub(origin.x),
            y: screen.y.saturating_sub(origin.y),
        })
    };
    let point = raw_cdp_click_css_point(
        coordinate.space,
        coordinate.x,
        coordinate.y,
        client_physical,
        width,
        height,
        dpr,
    )?;
    let before = synapse_a11y::cdp_capture_page_bgra(endpoint, target_id, None)
        .await
        .map_err(|error| {
            mcp_error(
                error.code(),
                format!("raw-CDP pre-click capture failed: {error}"),
            )
        })?;
    let button = match params.button {
        Some(TargetActMouseButton::Right) => synapse_a11y::CdpMouseButton::Right,
        Some(TargetActMouseButton::Middle) => synapse_a11y::CdpMouseButton::Middle,
        Some(TargetActMouseButton::Left) | None => synapse_a11y::CdpMouseButton::Left,
    };
    crate::m2::remote_click_with(async {
        synapse_a11y::cdp_click_point_target(
            endpoint,
            target_id,
            point,
            button,
            i64::from(clicks),
            target_act_click_modifiers_cdp_mask(&params.modifiers),
        )
        .await
        .map_err(|error| {
            crate::m2::remote_cdp_click_failure(
                &error,
                mcp_error(
                    error.code(),
                    format!("raw-CDP coordinate click failed: {error}"),
                ),
            )
        })
    })
    .await?;
    tokio::time::sleep(Duration::from_millis(250)).await;
    let mut after = synapse_a11y::cdp_capture_page_bgra(endpoint, target_id, None)
        .await
        .map_err(|error| {
            crate::m2::remote_click_error(
                mcp_error(
                    error.code(),
                    format!("raw-CDP post-click capture failed: {error}"),
                ),
                true,
            )
        })?;
    if before.bgra == after.bgra {
        tokio::time::sleep(Duration::from_millis(500)).await;
        after = synapse_a11y::cdp_capture_page_bgra(endpoint, target_id, None)
            .await
            .map_err(|error| {
                crate::m2::remote_click_error(
                    mcp_error(
                        error.code(),
                        format!("raw-CDP post-click retry capture failed: {error}"),
                    ),
                    true,
                )
            })?;
    }
    let before_hash = Sha256::digest(&before.bgra)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let after_hash = Sha256::digest(&after.bgra)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if before_hash == after_hash {
        return Err(crate::m2::remote_click_error(
            mcp_error(
                error_codes::ACTION_NO_OBSERVED_DELTA,
                format!(
                    "raw-CDP coordinate click reached target {target_id} but page pixels did not change; before_sha256={before_hash} after_sha256={after_hash}"
                ),
            ),
            true,
        ));
    }
    Ok(json!({
        "ok": true,
        "target_id": target_id,
        "window_hwnd": window_hwnd,
        "viewport_point": point,
        "click_count": clicks,
        "readback_backend": "raw_cdp_page_pixels",
        "before_sha256": before_hash,
        "after_sha256": after_hash,
        "method": "Input.dispatchMouseEvent(mouseMoved,mousePressed,mouseReleased)",
        "required_foreground": false,
    }))
}

/// Raw-CDP coordinate click that writes its own started row and closes it
/// with the click result on every exit (#198).
#[cfg(windows)]
async fn target_act_raw_cdp_coordinate_click_audited(
    service: &SynapseService,
    endpoint: &str,
    window_hwnd: i64,
    target_id: &str,
    coordinate: TargetActCoordinate,
    clicks: u8,
    params: &TargetActParams,
    session_id: &str,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let delegated_tool = "synapse_a11y.cdp_click_point_target";
    service.audit_action_started_with_details_for_session(
        "target_act",
        &json!({
            "session_id": session_id,
            "verb": "coordinate_click",
            "delegated_tool": delegated_tool,
            "window_hwnd": window_hwnd,
            "cdp_target_id": target_id,
            "required_foreground": false,
        }),
        session_id,
    )?;
    let result = target_act_raw_cdp_coordinate_click(
        endpoint,
        window_hwnd,
        target_id,
        coordinate,
        clicks,
        params,
    )
    .await;
    audit_action_result_maybe_redacted_for_session(
        service,
        delegated_tool,
        &result,
        session_id,
        params.secret_safe,
    )
    .map_err(|error| crate::m2::carry_remote_click_delivery(error, &result))?;
    Ok(match result {
        Ok(value) => (delegated_tool, true, TARGET_ACT_STATUS_OK, value),
        Err(error) => (
            delegated_tool,
            false,
            target_act_error_status(&error),
            target_act_error_result(delegated_tool, error),
        ),
    })
}

async fn target_act_coordinate_click(
    service: &SynapseService,
    action: &str,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let session_id = target_act_session_id(request_context, "coordinate_click")?;
    let coordinate = target_act_coordinate(params)?.ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act coordinate click requires both x and y",
        )
    })?;
    if !target_act_coordinate_uses_nested_position(params)
        && target_act_click_position(params)?.is_some()
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={action} position is for element-relative browser DOM clicks; coordinate clicks already use x/y unless coordinate_space is set and x/y are omitted"
            ),
        ));
    }
    let clicks = target_act_click_count_for_action(action, params.clicks)?;
    let wait_timeout_ms = target_act_dom_wait_timeout(params.wait_timeout_ms)?;
    let click_modifiers = target_act_click_modifiers_bridge_value(&params.modifiers)?;
    let request_details = json!({
        "session_id": &session_id,
        "verb": "coordinate_click",
        "action": action,
        "x": coordinate.x,
        "y": coordinate.y,
        "coordinate_space": coordinate.space.as_bridge_str(),
        "clicks": clicks,
        "button": params.button.map(TargetActMouseButton::as_str),
        "modifiers": &click_modifiers,
        "wait_timeout_ms": wait_timeout_ms,
        "secret_safe": params.secret_safe,
        "requires_session_target": true,
        "no_human_os_foreground_fallback": true,
    });
    let Some(target) = service.session_target(Some(&session_id))? else {
        let error = mcp_error(
            error_codes::TARGET_NOT_SET,
            "target_act coordinate click requires this MCP session to own an agent_logical_foreground/foreground_lane target; bind a target with set_target or cdp_open_tab first",
        );
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "target_act",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    };
    if let Err(error) =
        service.ensure_target_claim_allows_session("target_act", &session_id, &target)
    {
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "target_act",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    }
    match target {
        SessionTarget::Cdp {
            window_hwnd,
            cdp_target_id,
        } => {
            if let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) {
                return target_act_raw_cdp_coordinate_click_audited(
                    service,
                    &endpoint,
                    window_hwnd,
                    &cdp_target_id,
                    coordinate,
                    clicks,
                    params,
                    &session_id,
                )
                .await;
            }
            let request_details = json!({
                "session_id": &session_id,
                "verb": "coordinate_click",
                "delegated_tool": "chrome_debugger_bridge.coordinateClick",
                "window_hwnd": window_hwnd,
                "cdp_target_id": &cdp_target_id,
                "x": coordinate.x,
                "y": coordinate.y,
                "coordinate_space": coordinate.space.as_bridge_str(),
                "clicks": clicks,
                "button": params.button.map(TargetActMouseButton::as_str),
                "modifiers": &click_modifiers,
                "wait_timeout_ms": wait_timeout_ms,
                "secret_safe": params.secret_safe,
                "required_foreground": false,
            });
            service.audit_action_started_with_details_for_session(
                "target_act",
                &request_details,
                &session_id,
            )?;
            let result = crate::chrome_debugger_bridge::coordinate_click(
                crate::chrome_debugger_bridge::ChromeDebuggerCoordinateClickRequest {
                    hwnd: window_hwnd,
                    target_id: &cdp_target_id,
                    x: coordinate.x,
                    y: coordinate.y,
                    coordinate_space: coordinate.space.as_bridge_str(),
                    clicks,
                    button: params.button.map(TargetActMouseButton::as_str),
                    modifiers: Some(&click_modifiers),
                    wait_timeout_ms,
                    suppress_page_text: params.secret_safe,
                },
            )
            .await
            .map_err(|error| crate::m2::remote_bridge_click_error(&error));
            audit_action_result_maybe_redacted_for_session(
                service,
                "chrome_debugger_bridge.coordinateClick",
                &result,
                &session_id,
                params.secret_safe,
            )
            .map_err(|error| crate::m2::carry_remote_click_delivery(error, &result))?;
            match result {
                Ok(value) => {
                    let value = target_act_maybe_secret_safe_result(
                        "chrome_debugger_bridge.coordinateClick",
                        value,
                        params.secret_safe,
                    )
                    .map_err(|error| crate::m2::remote_click_error(error, true))?;
                    Ok((
                        "chrome_debugger_bridge.coordinateClick",
                        true,
                        TARGET_ACT_STATUS_OK,
                        value,
                    ))
                }
                Err(error) => Ok((
                    "chrome_debugger_bridge.coordinateClick",
                    false,
                    target_act_error_status(&error),
                    target_act_maybe_secret_safe_error_result(
                        "chrome_debugger_bridge.coordinateClick",
                        error,
                        params.secret_safe,
                    ),
                )),
            }
        }
        SessionTarget::Window { hwnd } => {
            let is_webview_host = synapse_a11y::foreground_context(hwnd)
                .is_ok_and(|context| !synapse_a11y::is_chromium_family(&context.process_name));
            if let Some(endpoint) = is_webview_host
                .then(|| synapse_a11y::endpoint_for_window(hwnd))
                .flatten()
            {
                let pages = synapse_a11y::cdp_list_targets(&endpoint)
                    .await
                    .map_err(|error| {
                        mcp_error(
                            error.code(),
                            format!("target_act WebView2 page discovery failed: {error}"),
                        )
                    })?
                    .into_iter()
                    .filter(|target| target.target_type == "page")
                    .collect::<Vec<_>>();
                if pages.len() != 1 {
                    let error = mcp_error(
                        error_codes::ACTION_TARGET_INVALID,
                        format!(
                            "target_act window coordinate click requires exactly one WebView2 page for target HWND 0x{hwnd:x}; found {}",
                            pages.len()
                        ),
                    );
                    service.audit_action_denied_with_details_for_session(
                        "target_act",
                        &error,
                        &request_details,
                        &session_id,
                    );
                    return Ok((
                        "synapse_a11y.cdp_click_point_target",
                        false,
                        target_act_error_status(&error),
                        target_act_error_result("synapse_a11y.cdp_click_point_target", error),
                    ));
                }
                let page = &pages[0];
                return target_act_raw_cdp_coordinate_click_audited(
                    service,
                    &endpoint,
                    hwnd,
                    &page.target_id,
                    coordinate,
                    clicks,
                    params,
                    &session_id,
                )
                .await;
            }
            let target_root = synapse_a11y::top_level_root_hwnd(hwnd).unwrap_or(hwnd);
            let is_background = synapse_a11y::current_foreground_context()
                .ok()
                .and_then(|context| synapse_a11y::top_level_root_hwnd(context.hwnd).ok())
                != Some(target_root);
            if is_background
                && is_webview_host
                && let Some(diagnostics) = synapse_a11y::webview2_cdp_diagnostics_for_window(hwnd)
            {
                let error = mcp_error(
                    error_codes::A11Y_CDP_UNREACHABLE,
                    webview2_background_click_refusal_message(hwnd, diagnostics.detail.as_deref()),
                );
                service.audit_action_denied_with_details_for_session(
                    "target_act",
                    &error,
                    &request_details,
                    &session_id,
                );
                return Ok((
                    "target_act",
                    false,
                    target_act_error_status(&error),
                    target_act_error_result("target_act", error),
                ));
            }
            let point = match target_act_window_coordinate_to_screen_point(hwnd, coordinate) {
                Ok(point) => point,
                Err(error) => {
                    service.audit_action_denied_with_details_for_session(
                        "target_act",
                        &error,
                        &request_details,
                        &session_id,
                    );
                    return Ok((
                        "act_click",
                        false,
                        target_act_error_status(&error),
                        target_act_error_result("act_click", error),
                    ));
                }
            };
            let target_root_hwnd = match target_act_window_coordinate_target_root(hwnd) {
                Ok(root_hwnd) => root_hwnd,
                Err(error) => {
                    service.audit_action_denied_with_details_for_session(
                        "target_act",
                        &error,
                        &request_details,
                        &session_id,
                    );
                    return Ok((
                        "act_click",
                        false,
                        target_act_error_status(&error),
                        target_act_error_result("act_click", error),
                    ));
                }
            };
            let click_params = target_act_click_point_params(
                point,
                clicks,
                params.button,
                &params.modifiers,
                Some(target_root_hwnd),
            )?;
            let response = service
                .act_click(Parameters(click_params), request_context.clone())
                .await;
            target_act_delegate_response("act_click", response)
        }
    }
}

#[cfg(windows)]
#[derive(Clone, Debug)]
struct TargetActTapElement {
    backend_node_id: i64,
    target_id: String,
    resolved_by: String,
    match_count: Option<usize>,
    returned_count: Option<usize>,
    element_id: Option<String>,
}

#[cfg(windows)]
async fn target_act_touch_tap(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let session_id = target_act_session_id(request_context, "tap")?;
    crate::m2::validate_auto_wait_timeout(
        "target_act",
        params.auto_wait,
        params.auto_wait_timeout_ms,
    )?;
    if params.clicks.is_some() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=tap does not accept clicks; a touch tap is always one touchStart/touchEnd sequence",
        ));
    }
    let coordinate = target_act_coordinate(params)?;
    if coordinate.is_some() && target_act_has_any_locator(params) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=tap accepts either viewport x/y coordinates or an element/DOM locator, not both",
        ));
    }
    let request_details = json!({
        "session_id": &session_id,
        "verb": "tap",
        "coordinate_present": coordinate.is_some(),
        "selector_present": params.selector.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "element_id_present": params.element_id.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "role": params.role.as_deref(),
        "name_present": params.name.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "requires_cdp_input": true,
        "delegated_tool": "synapse_a11y.cdp_touch_tap_or_chrome_debugger_bridge.cdpInput",
        "method": "Input.dispatchTouchEvent",
        "non_touch_fallback": "none; use verb=click explicitly for mouse behavior",
        "required_foreground": false,
    });
    let Some(target) = service.session_target(Some(&session_id))? else {
        let error = mcp_error(
            error_codes::TARGET_NOT_SET,
            "target_act verb=tap requires this MCP session to own a raw-CDP browser target; bind one with cdp_open_tab/set_target first",
        );
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "synapse_a11y.cdp_touch_tap",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    };
    if let Err(error) =
        service.ensure_target_claim_allows_session("target_act", &session_id, &target)
    {
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "synapse_a11y.cdp_touch_tap",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    }
    let SessionTarget::Cdp {
        window_hwnd,
        cdp_target_id,
    } = target
    else {
        let error = mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            "target_act verb=tap supports raw-CDP browser targets only; native/window targets have no touch semantics, and the explicit non-touch fallback is verb=click",
        );
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "synapse_a11y.cdp_touch_tap",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    };
    let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) else {
        return target_act_bridge_cdp_input(
            service,
            "tap",
            window_hwnd,
            &cdp_target_id,
            coordinate,
            params,
            &request_details,
            &session_id,
        )
        .await;
    };

    let mut request_details = request_details;
    if let Some(object) = request_details.as_object_mut() {
        object.insert("window_hwnd".to_owned(), json!(window_hwnd));
        object.insert("cdp_target_id".to_owned(), json!(&cdp_target_id));
    }
    service.audit_action_started_with_details_for_session(
        "target_act",
        &request_details,
        &session_id,
    )?;
    let result =
        target_act_touch_tap_dispatch(&endpoint, window_hwnd, &cdp_target_id, coordinate, params)
            .await;
    service.audit_action_result_for_session("target_act", &result, &session_id)?;
    match result {
        Ok(result) => Ok((
            "synapse_a11y.cdp_touch_tap",
            true,
            TARGET_ACT_STATUS_OK,
            result,
        )),
        Err(error) => Ok((
            "synapse_a11y.cdp_touch_tap",
            false,
            target_act_error_status(&error),
            target_act_error_result("synapse_a11y.cdp_touch_tap", error),
        )),
    }
}

#[cfg(not(windows))]
async fn target_act_touch_tap(
    _service: &SynapseService,
    _params: &TargetActParams,
    _request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let error = mcp_error(
        error_codes::ACTION_TARGET_INVALID,
        "target_act verb=tap requires Windows raw-CDP action support for Input.dispatchTouchEvent; use verb=click explicitly for non-touch mouse behavior",
    );
    Ok((
        "synapse_a11y.cdp_touch_tap",
        false,
        target_act_error_status(&error),
        target_act_error_result("target_act", error),
    ))
}

/// #1349: background-safe target-bound window move/resize. Resolves the bound
/// window HWND (native Window target, or the browser window behind a Cdp target),
/// drives SetWindowPos without activation, and reads the resulting outer rect
/// back so the caller gets requested-vs-actual bounds (min-size constraints) and
/// minimized state. No human-foreground fallback, no implicit target.
#[cfg(windows)]
fn target_act_set_window_bounds(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    const DELEGATE: &str = "synapse_a11y.set_window_bounds";
    let session_id = target_act_session_id(request_context, "set_window_bounds")?;
    if params.selector.is_some()
        || params.element_id.is_some()
        || params.role.is_some()
        || params.name.is_some()
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=set_window_bounds operates on the bound window target; it does not accept selector/element_id/role/name",
        ));
    }
    if params.width.is_none() && params.height.is_none() && params.x.is_none() && params.y.is_none()
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=set_window_bounds requires at least one of x/y (move) or width/height (resize)",
        ));
    }
    if params.width.is_some_and(|value| value <= 0) || params.height.is_some_and(|value| value <= 0)
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=set_window_bounds width/height must be > 0 when supplied",
        ));
    }
    let request_details = json!({
        "session_id": &session_id,
        "verb": "set_window_bounds",
        "requested_x": params.x,
        "requested_y": params.y,
        "requested_width": params.width,
        "requested_height": params.height,
        "delegated_tool": DELEGATE,
        "method": "SetWindowPos(SWP_NOZORDER|SWP_NOACTIVATE)+GetWindowRect",
        "required_foreground": false,
    });
    let Some(target) = service.session_target(Some(&session_id))? else {
        let error = mcp_error(
            error_codes::TARGET_NOT_SET,
            "target_act verb=set_window_bounds requires a bound window target; bind one with window_list/set_target first",
        );
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            DELEGATE,
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    };
    if let Err(error) =
        service.ensure_target_claim_allows_session("target_act", &session_id, &target)
    {
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            DELEGATE,
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    }
    let window_hwnd = match &target {
        SessionTarget::Window { hwnd } => *hwnd,
        SessionTarget::Cdp { window_hwnd, .. } => *window_hwnd,
    };
    service.audit_action_started_with_details_for_session(
        "target_act",
        &request_details,
        &session_id,
    )?;
    let result = synapse_a11y::set_window_bounds(
        window_hwnd,
        params.x,
        params.y,
        params.width,
        params.height,
    )
    .map(|outcome| {
        let (ok, status, mut value) = target_act_window_bounds_response(params, &outcome);
        value["target_hwnd"] = json!(window_hwnd);
        (ok, status, value)
    })
    .map_err(|error| {
        mcp_error(
            error.code(),
            format!("target_act set_window_bounds failed for HWND {window_hwnd:#x}: {error}"),
        )
    });
    service.audit_action_result_for_session("target_act", &result, &session_id)?;
    match result {
        Ok((ok, status, value)) => Ok((DELEGATE, ok, status, value)),
        Err(error) => Ok((
            DELEGATE,
            false,
            target_act_error_status(&error),
            target_act_error_result(DELEGATE, error),
        )),
    }
}

#[cfg(not(windows))]
fn target_act_set_window_bounds(
    _service: &SynapseService,
    _params: &TargetActParams,
    _request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    Err(mcp_error(
        error_codes::ACTION_TARGET_INVALID,
        "target_act verb=set_window_bounds requires Windows SetWindowPos support",
    ))
}

#[cfg(windows)]
async fn target_act_hover(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let session_id = target_act_session_id(request_context, "hover")?;
    crate::m2::validate_auto_wait_timeout(
        "target_act",
        params.auto_wait,
        params.auto_wait_timeout_ms,
    )?;
    if target_act_coordinate(params)?.is_some() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=hover targets an element center; use selector, role/name, or observed CDP element_id",
        ));
    }
    if params.clicks.is_some()
        || params.button.is_some()
        || !params.modifiers.is_empty()
        || target_act_has_click_position_input(params)
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=hover does not accept click-only options clicks/clickCount, button, modifiers, or position",
        ));
    }
    let request_details = json!({
        "session_id": &session_id,
        "verb": "hover",
        "selector_present": params.selector.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "element_id_present": params.element_id.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "role": params.role.as_deref(),
        "name_present": params.name.as_ref().is_some_and(|value| !value.trim().is_empty()),
        "requires_cdp_input": true,
        "delegated_tool": "synapse_a11y.cdp_aim_node_or_chrome_debugger_bridge.cdpInput",
        "method": "Input.dispatchMouseEvent(mouseMoved)",
        "required_foreground": false,
    });
    let Some(target) = service.session_target(Some(&session_id))? else {
        let error = mcp_error(
            error_codes::TARGET_NOT_SET,
            "target_act verb=hover requires this MCP session to own a raw-CDP browser target; bind one with cdp_open_tab/set_target first",
        );
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "synapse_a11y.cdp_aim_node",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    };
    if let Err(error) =
        service.ensure_target_claim_allows_session("target_act", &session_id, &target)
    {
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "synapse_a11y.cdp_aim_node",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    }
    let SessionTarget::Cdp {
        window_hwnd,
        cdp_target_id,
    } = target
    else {
        let error = mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            "target_act verb=hover supports raw-CDP browser targets only; native/window targets cannot set browser :hover state without foreground mouse input",
        );
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "synapse_a11y.cdp_aim_node",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    };
    let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) else {
        return target_act_bridge_cdp_input(
            service,
            "hover",
            window_hwnd,
            &cdp_target_id,
            None,
            params,
            &request_details,
            &session_id,
        )
        .await;
    };

    let mut request_details = request_details;
    if let Some(object) = request_details.as_object_mut() {
        object.insert("window_hwnd".to_owned(), json!(window_hwnd));
        object.insert("cdp_target_id".to_owned(), json!(&cdp_target_id));
    }
    service.audit_action_started_with_details_for_session(
        "target_act",
        &request_details,
        &session_id,
    )?;
    let result = target_act_hover_dispatch(&endpoint, window_hwnd, &cdp_target_id, params).await;
    service.audit_action_result_for_session("target_act", &result, &session_id)?;
    match result {
        Ok(result) => Ok((
            "synapse_a11y.cdp_aim_node",
            true,
            TARGET_ACT_STATUS_OK,
            result,
        )),
        Err(error) => Ok((
            "synapse_a11y.cdp_aim_node",
            false,
            target_act_error_status(&error),
            target_act_error_result("synapse_a11y.cdp_aim_node", error),
        )),
    }
}

#[cfg(not(windows))]
async fn target_act_hover(
    _service: &SynapseService,
    _params: &TargetActParams,
    _request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let error = mcp_error(
        error_codes::ACTION_TARGET_INVALID,
        "target_act verb=hover requires Windows raw-CDP action support for Input.dispatchMouseEvent(mouseMoved)",
    );
    Ok((
        "synapse_a11y.cdp_aim_node",
        false,
        target_act_error_status(&error),
        target_act_error_result("target_act", error),
    ))
}

/// Route a DOM-locator click/dblclick/press to the REAL trusted-input lane
/// (#1348 headline #1/#2). For the normal Chrome bridge target this dispatches
/// chrome.debugger `Input.dispatchMouseEvent` (isTrusted=true) instead of the
/// synthetic `performClick` that guarded handlers ignore. Raw-CDP and
/// native/window targets keep the existing path (raw-CDP real mouse click is a
/// tracked follow-up). `bridge_action` is the cdpInput action ("click"/
/// "dblclick"); `fallback_action` is the DOM-action verb used when the real lane
/// does not apply (so verb=press keeps its named-button fallback semantics).
#[cfg(windows)]
async fn target_act_dom_locator_pointer(
    service: &SynapseService,
    bridge_action: &'static str,
    fallback_action: &'static str,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let session_id = target_act_session_id(request_context, bridge_action)?;
    let Some(target) = service.session_target(Some(&session_id))? else {
        return target_act_browser_dom_action(service, fallback_action, params, request_context)
            .await;
    };
    let SessionTarget::Cdp {
        window_hwnd,
        cdp_target_id,
    } = &target
    else {
        return target_act_browser_dom_action(service, fallback_action, params, request_context)
            .await;
    };
    // Raw-CDP endpoint present → real trusted mouse click via
    // synapse_a11y::cdp_click_node (Input.dispatchMouseEvent mouseMoved →
    // mousePressed → mouseReleased), NOT the bridge synthetic path (which rejects
    // a non-bridge raw-CDP target outright). #1348: closes the last
    // synthetic/broken input lane — raw-CDP locator clicks are now real and
    // trusted, mirroring the verb=tap raw-CDP path. Bridge-only targets fall
    // through to the cdpInput lane below.
    #[cfg(windows)]
    if let Some(endpoint) = synapse_a11y::endpoint_for_window(*window_hwnd) {
        let raw_details = json!({
            "session_id": &session_id,
            "verb": bridge_action,
            "lane": "synapse_a11y.cdp_click_node",
            "real_trusted_input": true,
            "required_foreground": false,
            "window_hwnd": *window_hwnd,
            "cdp_target_id": cdp_target_id,
            "secret_safe": params.secret_safe,
        });
        if let Err(error) =
            service.ensure_target_claim_allows_session("target_act", &session_id, &target)
        {
            service.audit_action_denied_with_details_for_session(
                "target_act",
                &error,
                &raw_details,
                &session_id,
            );
            return Ok((
                "synapse_a11y.cdp_click_node",
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        }
        service.audit_action_started_with_details_for_session(
            "target_act",
            &raw_details,
            &session_id,
        )?;
        let result = target_act_raw_cdp_click_dispatch(
            &endpoint,
            *window_hwnd,
            cdp_target_id,
            bridge_action,
            params,
        )
        .await;
        service
            .audit_action_result_for_session("target_act", &result, &session_id)
            .map_err(|error| crate::m2::carry_remote_click_delivery(error, &result))?;
        return match result {
            Ok(result) => Ok((
                "synapse_a11y.cdp_click_node",
                true,
                TARGET_ACT_STATUS_OK,
                result,
            )),
            Err(error) => Ok((
                "synapse_a11y.cdp_click_node",
                false,
                target_act_error_status(&error),
                target_act_error_result("synapse_a11y.cdp_click_node", error),
            )),
        };
    }
    let request_details = json!({
        "session_id": &session_id,
        "verb": bridge_action,
        "fallback_verb": fallback_action,
        "lane": "chrome_debugger_bridge.cdpInput",
        "real_trusted_input": true,
        "required_foreground": false,
    });
    if let Err(error) =
        service.ensure_target_claim_allows_session("target_act", &session_id, &target)
    {
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "chrome_debugger_bridge.cdpInput",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    }
    target_act_bridge_cdp_input(
        service,
        bridge_action,
        *window_hwnd,
        cdp_target_id,
        None,
        params,
        &request_details,
        &session_id,
    )
    .await
}

#[cfg(not(windows))]
async fn target_act_dom_locator_pointer(
    service: &SynapseService,
    _bridge_action: &'static str,
    fallback_action: &'static str,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    target_act_browser_dom_action(service, fallback_action, params, request_context).await
}

#[cfg(windows)]
async fn target_act_bridge_cdp_input(
    service: &SynapseService,
    action: &'static str,
    window_hwnd: i64,
    cdp_target_id: &str,
    coordinate: Option<TargetActCoordinate>,
    params: &TargetActParams,
    request_details: &Value,
    session_id: &str,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    target_act_validate_bridge_cdp_input(action, coordinate, params)?;
    // Mouse-click options apply only to the real-mouse click/dblclick lane;
    // hover/tap/drag ignore them (left None).
    let is_click = matches!(action, "click" | "dblclick");
    let carries_delivery_evidence = is_click || action == "wheel";
    let wheel_delta = if action == "wheel" {
        Some(parse_target_act_scroll_args(&params.args)?)
    } else {
        None
    };
    let click_count = if is_click {
        Some(target_act_click_count_for_action(action, params.clicks)?)
    } else {
        None
    };
    let click_button = if is_click {
        params.button.map(TargetActMouseButton::as_str)
    } else {
        None
    };
    let click_modifiers = if is_click {
        Some(target_act_click_modifiers_bridge_value(&params.modifiers)?)
    } else {
        None
    };
    // #198: resolve every fallible input before the started row is written.
    let wait_timeout_ms = target_act_dom_wait_timeout(params.wait_timeout_ms)?;
    let mut request_details = request_details.clone();
    if let Some(object) = request_details.as_object_mut() {
        object.insert("window_hwnd".to_owned(), json!(window_hwnd));
        object.insert("cdp_target_id".to_owned(), json!(cdp_target_id));
        object.insert(
            "delegated_tool".to_owned(),
            json!("chrome_debugger_bridge.cdpInput"),
        );
        object.insert("bridge_debugger_lane".to_owned(), json!("chrome.debugger"));
        object.insert("required_foreground".to_owned(), json!(false));
        object.insert("secret_safe".to_owned(), json!(params.secret_safe));
    }
    service.audit_action_started_with_details_for_session(
        "target_act",
        &request_details,
        session_id,
    )?;
    let coordinate_space = coordinate.map(|value| value.space.as_bridge_str());
    let request = crate::chrome_debugger_bridge::ChromeDebuggerCdpInputRequest {
        hwnd: window_hwnd,
        target_id: cdp_target_id,
        action,
        selector: params.selector.as_deref(),
        element_id: params.element_id.as_deref(),
        active_element: false,
        role: params.role.as_deref(),
        name: params.name.as_deref(),
        value: params.value.as_deref(),
        text: params.text.as_deref(),
        x: coordinate.map(|value| value.x),
        y: coordinate.map(|value| value.y),
        coordinate_space,
        source_selector: None,
        target_selector: None,
        drag_steps: None,
        drag_duration_ms: None,
        drag_data_mime_type: None,
        drag_data_text: None,
        button: click_button,
        modifiers: click_modifiers.as_ref(),
        clicks: click_count,
        wait_timeout_ms,
        auto_wait: params.auto_wait,
        auto_wait_timeout_ms: params.auto_wait_timeout_ms,
        suppress_page_text: params.secret_safe,
    };
    let result = match wheel_delta {
        Some((dx, dy)) => {
            crate::chrome_debugger_bridge::cdp_wheel_input(
                request,
                i64::from(dx) * 100,
                -i64::from(dy) * 100,
            )
            .await
        }
        None => crate::chrome_debugger_bridge::cdp_input(request).await,
    }
    .map_err(|error| {
        if carries_delivery_evidence {
            crate::m2::remote_bridge_click_error(&error)
        } else {
            mcp_error(error.code(), error.detail().to_owned())
        }
    });
    audit_action_result_maybe_redacted_for_session(
        service,
        "chrome_debugger_bridge.cdpInput",
        &result,
        session_id,
        params.secret_safe,
    )
    .map_err(|error| {
        if carries_delivery_evidence {
            crate::m2::carry_remote_click_delivery(error, &result)
        } else {
            error
        }
    })?;
    match result {
        Ok(value) => {
            let value = target_act_maybe_secret_safe_result(
                "chrome_debugger_bridge.cdpInput",
                value,
                params.secret_safe,
            )
            .map_err(|error| {
                if carries_delivery_evidence {
                    crate::m2::remote_click_error(error, true)
                } else {
                    error
                }
            })?;
            Ok((
                "chrome_debugger_bridge.cdpInput",
                true,
                TARGET_ACT_STATUS_OK,
                value,
            ))
        }
        Err(error) => Ok((
            "chrome_debugger_bridge.cdpInput",
            false,
            target_act_error_status(&error),
            target_act_maybe_secret_safe_error_result(
                "chrome_debugger_bridge.cdpInput",
                error,
                params.secret_safe,
            ),
        )),
    }
}

#[cfg(windows)]
fn target_act_validate_bridge_cdp_input(
    action: &str,
    coordinate: Option<TargetActCoordinate>,
    params: &TargetActParams,
) -> Result<(), ErrorData> {
    if let Some(coordinate) = coordinate
        && coordinate.space != TargetActCoordinateSpace::Viewport
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={action} coordinate input must use coordinate_space=viewport because CDP Input consumes viewport CSS pixels; got {}",
                coordinate.space.as_bridge_str()
            ),
        ));
    }
    let has_locator = params
        .element_id
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
        || target_act_has_dom_locator(params);
    if coordinate.is_none() && (action != "wheel" || has_locator) {
        target_act_validate_dom_locator(action, params)?;
    }
    if let Some(raw) = params
        .element_id
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        if ElementId::parse(raw).is_ok() {
            return Err(mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                format!(
                    "target_act verb={action} observed raw-CDP element_id {raw:?} requires a raw CDP endpoint; re-resolve the element through browser_locate for a chrome-tab:... bridge element id or use selector/role/name"
                ),
            ));
        }
        if !target_act_click_element_id_can_be_dom_id(raw) {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "target_act verb={action} element_id must be a chrome-tab:... bridge element id or plain DOM id for the normal Chrome bridge cdpInput lane"
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
async fn target_act_hover_dispatch(
    endpoint: &str,
    window_hwnd: i64,
    cdp_target_id: &str,
    params: &TargetActParams,
) -> Result<Value, ErrorData> {
    let element =
        target_act_tap_element(endpoint, window_hwnd, cdp_target_id, params, "hover").await?;
    let point = synapse_a11y::cdp_aim_node(
        endpoint,
        "",
        Some(&element.target_id),
        element.backend_node_id,
    )
    .await
    .map_err(|error| {
        mcp_error(
            error.code(),
            format!(
                "target_act hover Input.dispatchMouseEvent(mouseMoved) failed for backendNodeId {} in target {:?}: {error}",
                element.backend_node_id, element.target_id
            ),
        )
    })?;
    Ok(json!({
        "ok": true,
        "target_id": element.target_id,
        "point": point,
        "resolved_by": element.resolved_by,
        "backend_node_id": element.backend_node_id,
        "element_id": element.element_id,
        "match_count": element.match_count,
        "returned_count": element.returned_count,
        "window_hwnd": window_hwnd,
        "readback_backend": "raw_cdp",
        "method": "Input.dispatchMouseEvent(mouseMoved)",
        "scrolled_into_view_before_move": true
    }))
}

/// CDP `Input.dispatchMouseEvent` modifier bitmask: Alt=1, Ctrl=2, Meta=4,
/// Shift=8 (#1348 raw-CDP click).
fn target_act_click_modifiers_cdp_mask(modifiers: &[TargetActClickModifier]) -> i64 {
    let mut mask = 0;
    for modifier in modifiers {
        mask |= match modifier {
            TargetActClickModifier::Alt => 1,
            TargetActClickModifier::Ctrl => 2,
            TargetActClickModifier::Meta => 4,
            TargetActClickModifier::Shift => 8,
        };
    }
    mask
}

/// Real, TRUSTED raw-CDP mouse click/dblclick on a DOM-locator/element_id target
/// via `synapse_a11y::cdp_click_node` (Input.dispatchMouseEvent
/// mouseMoved→mousePressed→mouseReleased), mirroring `target_act_touch_tap_dispatch`
/// but with mouse instead of touch. isTrusted=true, so activation-guarded handlers
/// fire — the raw-CDP analogue of the bridge cdpInput real-click lane (#1348).
#[cfg(windows)]
async fn target_act_raw_cdp_click_dispatch(
    endpoint: &str,
    window_hwnd: i64,
    cdp_target_id: &str,
    action: &'static str,
    params: &TargetActParams,
) -> Result<Value, ErrorData> {
    let element =
        target_act_tap_element(endpoint, window_hwnd, cdp_target_id, params, action).await?;
    let click_count = i64::from(target_act_click_count_for_action(action, params.clicks)?);
    let button = match params.button {
        Some(TargetActMouseButton::Right) => synapse_a11y::CdpMouseButton::Right,
        Some(TargetActMouseButton::Middle) => synapse_a11y::CdpMouseButton::Middle,
        Some(TargetActMouseButton::Left) | None => synapse_a11y::CdpMouseButton::Left,
    };
    let modifiers = target_act_click_modifiers_cdp_mask(&params.modifiers);
    let point = crate::m2::remote_click_with(async {
        synapse_a11y::cdp_click_node(
            endpoint,
            "",
            Some(&element.target_id),
            element.backend_node_id,
            button,
            click_count,
            modifiers,
        )
        .await
        .map_err(|error| {
            crate::m2::remote_cdp_click_failure(
                &error,
                mcp_error(
                    error.code(),
                    format!(
                        "target_act {action} Input.dispatchMouseEvent failed for backendNodeId {} in target {:?}: {error}",
                        element.backend_node_id, element.target_id
                    ),
                ),
            )
        })
    })
    .await?;
    Ok(json!({
        "ok": true,
        "action": action,
        "target_id": element.target_id,
        "point": point,
        "resolved_by": element.resolved_by,
        "backend_node_id": element.backend_node_id,
        "element_id": element.element_id,
        "match_count": element.match_count,
        "returned_count": element.returned_count,
        "window_hwnd": window_hwnd,
        "readback_backend": "raw_cdp",
        "method": "Input.dispatchMouseEvent(mouseMoved,mousePressed,mouseReleased)",
        "click_count": click_count,
        "real_trusted_input": true,
        "scrolled_into_view_before_click": true
    }))
}

#[cfg(windows)]
async fn target_act_touch_tap_dispatch(
    endpoint: &str,
    window_hwnd: i64,
    cdp_target_id: &str,
    coordinate: Option<TargetActCoordinate>,
    params: &TargetActParams,
) -> Result<Value, ErrorData> {
    if let Some(coordinate) = coordinate {
        if coordinate.space != TargetActCoordinateSpace::Viewport {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "target_act verb=tap coordinate input must use coordinate_space=viewport because Input.dispatchTouchEvent consumes viewport CSS pixels; got {}",
                    coordinate.space.as_bridge_str()
                ),
            ));
        }
        let point = synapse_a11y::CdpActionPoint {
            x: f64::from(coordinate.x),
            y: f64::from(coordinate.y),
        };
        let tap = synapse_a11y::cdp_touch_tap_target(endpoint, cdp_target_id, point)
            .await
            .map_err(|error| {
                mcp_error(
                    error.code(),
                    format!(
                        "target_act tap Input.dispatchTouchEvent failed for target {cdp_target_id:?}: {error}"
                    ),
                )
            })?;
        let mut result = target_act_result(&tap)?;
        if let Some(object) = result.as_object_mut() {
            object.insert("ok".to_owned(), json!(true));
            object.insert("resolved_by".to_owned(), json!("viewport_coordinate"));
            object.insert("backend_node_id".to_owned(), Value::Null);
            object.insert("window_hwnd".to_owned(), json!(window_hwnd));
            object.insert("readback_backend".to_owned(), json!("raw_cdp"));
            object.insert("method".to_owned(), json!("Input.dispatchTouchEvent"));
        }
        return Ok(result);
    }

    let element =
        target_act_tap_element(endpoint, window_hwnd, cdp_target_id, params, "tap").await?;
    let tap = synapse_a11y::cdp_touch_tap_node(
        endpoint,
        "",
        Some(&element.target_id),
        element.backend_node_id,
    )
    .await
    .map_err(|error| {
        mcp_error(
            error.code(),
            format!(
                "target_act tap Input.dispatchTouchEvent failed for backendNodeId {} in target {:?}: {error}",
                element.backend_node_id, element.target_id
            ),
        )
    })?;
    let mut result = target_act_result(&tap)?;
    if let Some(object) = result.as_object_mut() {
        object.insert("ok".to_owned(), json!(true));
        object.insert("resolved_by".to_owned(), json!(element.resolved_by));
        object.insert("backend_node_id".to_owned(), json!(element.backend_node_id));
        object.insert("element_id".to_owned(), json!(element.element_id));
        object.insert("match_count".to_owned(), json!(element.match_count));
        object.insert("returned_count".to_owned(), json!(element.returned_count));
        object.insert("window_hwnd".to_owned(), json!(window_hwnd));
        object.insert("readback_backend".to_owned(), json!("raw_cdp"));
        object.insert("method".to_owned(), json!("Input.dispatchTouchEvent"));
    }
    Ok(result)
}

/// #2350 / #2359: `act verb=drag`. `DragRoute` picks the primitive from the
/// session target, `DragPath` checks the ends and the duration, then the
/// existing primitive runs as it is: `browser_drag` in this session's own
/// Chrome bridge tab (called directly, so the `browser_debugger` facade and
/// its profile gate are not on this path), or `act_stroke`, whose own route
/// strokes into this session's raw-CDP page, or sends real input with its
/// lease, its claim check on every window under the path and its focus
/// monitor. A refusal writes one "denied" audit row and sends nothing.
#[cfg(windows)]
async fn target_act_drag(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let session_id = target_act_session_id(request_context, "drag")?;
    let given = |value: Option<&String>| value.is_some_and(|value| !value.trim().is_empty());
    let request_details = json!({
        "session_id": &session_id,
        "verb": "drag",
        "element_id_present": given(params.element_id.as_ref()),
        "selector_present": given(params.selector.as_ref()),
        "x": params.x,
        "y": params.y,
        "to_element_id_present": given(params.to_element_id.as_ref()),
        "to_selector_present": given(params.to_selector.as_ref()),
        "to_x": params.to_x,
        "to_y": params.to_y,
        "duration_ms": params.duration_ms,
        "no_human_os_foreground_fallback": true,
    });
    let refuse = |error: ErrorData| {
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        Ok::<_, ErrorData>((
            "target_act",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ))
    };
    if params.coordinate_space.is_some()
        || target_act_has_click_position_input(params)
        || params.clicks.is_some()
        || params.button.is_some()
        || !params.modifiers.is_empty()
        || given(params.role.as_ref())
        || given(params.name.as_ref())
    {
        return refuse(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=drag takes its start as element_id, selector, or x and y and its end as to_element_id, to_selector, or to_x and to_y (points in the target's own coordinates); it does not accept coordinate_space, position, clicks, button, modifiers, role or name; nothing was sent",
        ));
    }
    let target = service.session_target(Some(&session_id))?;
    let target_hwnd = match &target {
        Some(SessionTarget::Window { hwnd }) => Some(*hwnd),
        Some(SessionTarget::Cdp { window_hwnd, .. }) => Some(*window_hwnd),
        None => None,
    };
    let endpoint = target_hwnd.and_then(synapse_a11y::endpoint_for_window);
    let claimed = match &target {
        Some(target) => service
            .target_claim_for_session(&session_id, target)?
            .is_some(),
        None => false,
    };
    let in_front = target_hwnd.is_some_and(|hwnd| {
        service
            .current_audit_foreground()
            .is_ok_and(|foreground| foreground.hwnd == hwnd)
    });
    // Drag keeps its own in-front rule (`DragRoute`): no WebView2 read here.
    let route = DragRoute::new(
        ActTarget::of(target.as_ref(), endpoint.is_some(), false),
        claimed,
        in_front,
    );
    let backend = match route.backend() {
        Ok(backend) => backend,
        Err(error) => return refuse(error),
    };
    let (Some(target), Some(target_hwnd)) = (target, target_hwnd) else {
        return refuse(mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            "target_act verb=drag has a route but no session target; nothing was sent",
        ));
    };
    // Another session's claim on this target refuses, as for the other
    // target_act actions (#2350 R5).
    if let Err(error) =
        service.ensure_target_claim_allows_session("target_act", &session_id, &target)
    {
        return refuse(error);
    }
    let area = match backend {
        DragBackend::BridgeDrag => DragArea::BridgeTab,
        DragBackend::RawCdpStroke => {
            match target_act_drag_viewport(endpoint.as_deref(), &target).await {
                Ok(viewport) => DragArea::Bounds(viewport),
                Err(error) => return refuse(error),
            }
        }
        DragBackend::RealInputStroke => match synapse_a11y::foreground_context(target_hwnd) {
            Ok(context) => DragArea::Bounds(context.window_bounds),
            Err(error) => {
                return refuse(mcp_error(
                    error.code(),
                    format!(
                        "target_act verb=drag target window HWND 0x{target_hwnd:x} bounds readback failed: {error}; nothing was sent"
                    ),
                ));
            }
        },
    };
    let from = match target_act_drag_end(
        service,
        request_context,
        area,
        &target,
        endpoint.as_deref(),
        (
            params.element_id.as_deref(),
            params.selector.as_deref(),
            params.x.zip(params.y),
        ),
    )
    .await
    {
        Ok(end) => end,
        Err(error) => return refuse(error),
    };
    let to = match target_act_drag_end(
        service,
        request_context,
        area,
        &target,
        endpoint.as_deref(),
        (
            params.to_element_id.as_deref(),
            params.to_selector.as_deref(),
            params.to_x.zip(params.to_y),
        ),
    )
    .await
    {
        Ok(end) => end,
        Err(error) => return refuse(error),
    };
    let path = match DragPath::new(from, to, params.duration_ms, area) {
        Ok(path) => path,
        Err(error) => return refuse(error),
    };
    match (backend, path.selectors(), path.points()) {
        (DragBackend::BridgeDrag, Some((from, to)), _) => {
            let response = service
                .browser_drag(
                    Parameters(super::browser_dnd::BrowserDndParams {
                        source_selector: from.to_owned(),
                        target_selector: to.to_owned(),
                        mode: None,
                        steps: None,
                        duration_ms: Some(u64::from(path.duration_ms())),
                        data_mime_type: None,
                        data_text: None,
                        // None + None: `browser_drag` takes this session's own
                        // target (`action_session_target_override`).
                        cdp_target_id: None,
                        window_hwnd: None,
                        wait_timeout_ms: params.wait_timeout_ms,
                        auto_wait: None,
                        auto_wait_timeout_ms: None,
                    }),
                    request_context.clone(),
                )
                .await;
            target_act_delegate_response("browser_drag", response)
        }
        (DragBackend::RawCdpStroke | DragBackend::RealInputStroke, _, Some((from, to))) => {
            let stroke = target_act_drag_stroke_params(from, to, path.duration_ms())?;
            let response = service
                .act_stroke(Parameters(stroke), request_context.clone())
                .await;
            target_act_delegate_response("act_stroke", response)
        }
        _ => refuse(mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            "target_act verb=drag path does not fit its route; nothing was sent",
        )),
    }
}

#[cfg(not(windows))]
async fn target_act_drag(
    _service: &SynapseService,
    _params: &TargetActParams,
    _request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let error = mcp_error(
        error_codes::ACTION_TARGET_INVALID,
        "target_act verb=drag requires Windows (bridge drag, raw-CDP stroke and real-input stroke); nothing was sent",
    );
    Ok((
        "target_act",
        false,
        target_act_error_status(&error),
        target_act_error_result("target_act", error),
    ))
}

/// #2350: one end of a drag as the caller named it (`named`: element id,
/// selector, x and y), with what is read about it now. An element's scope is
/// asked before its box is read: the page read scrolls the node into view,
/// and nothing is read on another window or page.
#[cfg(windows)]
async fn target_act_drag_end(
    service: &SynapseService,
    request_context: &RequestContext<RoleServer>,
    area: DragArea,
    target: &SessionTarget,
    endpoint: Option<&str>,
    named: (Option<&str>, Option<&str>, Option<(i32, i32)>),
) -> Result<Option<DragEnd>, ErrorData> {
    fn given(value: Option<&str>) -> Option<&str> {
        value.map(str::trim).filter(|value| !value.is_empty())
    }
    let raw_id = match (given(named.0), given(named.1), named.2) {
        (None, None, None) => return Ok(None),
        (None, Some(selector), None) => return Ok(Some(DragEnd::Selector(selector.to_owned()))),
        (None, None, Some((x, y))) => return Ok(Some(DragEnd::Point(Point { x, y }))),
        (Some(raw_id), None, None) => raw_id,
        _ => {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                "target_act verb=drag names each end one way: an element id, a selector, or x and y; nothing was sent",
            ));
        }
    };
    let element_id = ElementId::parse(raw_id).map_err(|error| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb=drag element id {raw_id:?} cannot be read as an element id from find: {error}; nothing was sent"
            ),
        )
    })?;
    let scope = target_act_element_scope(service, request_context, &element_id, "drag").await?;
    let DragArea::Bounds(_) = area else {
        // A Chrome bridge tab takes selectors only: `DragPath` refuses this
        // end, so no box is read.
        return Ok(Some(DragEnd::Element {
            scope,
            bounds: Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
        }));
    };
    if let ScopeVerdict::Refused { code, text } = scope.verdict("drag") {
        return Err(mcp_error(code, text));
    }
    let backend_node_id = synapse_a11y::cdp_backend_from_element_id(&element_id);
    let bounds = match (target, backend_node_id, endpoint) {
        (SessionTarget::Window { .. }, None, _) => synapse_a11y::element_bounding_rect(&element_id)
            .map_err(|error| {
                mcp_error(
                    error.code(),
                    format!(
                        "target_act verb=drag element {element_id} box readback failed: {error}; nothing was sent"
                    ),
                )
            })?,
        (SessionTarget::Cdp { cdp_target_id, .. }, Some(backend_node_id), Some(endpoint)) => {
            let page = synapse_a11y::cdp_target_from_element_id(&element_id)
                .unwrap_or_else(|| cdp_target_id.clone());
            let centre = synapse_a11y::cdp_node_viewport_center(
                endpoint,
                "",
                Some(page.as_str()),
                backend_node_id,
            )
            .await
            .map_err(|error| {
                mcp_error(
                    error.code(),
                    format!(
                        "target_act verb=drag element {element_id} centre readback failed: {error}; nothing was sent"
                    ),
                )
            })?;
            // Only the centre is read: a box of size 0 at the centre.
            #[allow(clippy::cast_possible_truncation)]
            let (x, y) = (centre.x.round() as i32, centre.y.round() as i32);
            Rect { x, y, w: 0, h: 0 }
        }
        (SessionTarget::Window { .. }, Some(_), _) => {
            return Err(mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                "target_act verb=drag on a native window takes UIA element ids from find; to drag between web elements, target their page (cdp_open_tab) first; nothing was sent",
            ));
        }
        _ => {
            return Err(mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                "target_act verb=drag on a raw-CDP page takes web element ids from find of that page; nothing was sent",
            ));
        }
    };
    Ok(Some(DragEnd::Element { scope, bounds }))
}

/// #2350: the viewport of the session's raw-CDP page in CSS pixels from 0,0
/// (the readback of `target_act_raw_cdp_coordinate_click`).
#[cfg(windows)]
async fn target_act_drag_viewport(
    endpoint: Option<&str>,
    target: &SessionTarget,
) -> Result<Rect, ErrorData> {
    let (Some(endpoint), SessionTarget::Cdp { cdp_target_id, .. }) = (endpoint, target) else {
        return Err(mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            "target_act verb=drag raw-CDP route without a page endpoint; nothing was sent",
        ));
    };
    let viewport = synapse_a11y::cdp_evaluate_expression(
        endpoint,
        cdp_target_id,
        "({ width: innerWidth, height: innerHeight })",
        false,
        true,
    )
    .await
    .map_err(|error| {
        mcp_error(
            error.code(),
            format!(
                "target_act verb=drag raw-CDP viewport readback failed: {error}; nothing was sent"
            ),
        )
    })?;
    let size = |key: &str| {
        viewport
            .value
            .get(key)
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
    };
    let (Some(w), Some(h)) = (size("width"), size("height")) else {
        return Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            "target_act verb=drag raw-CDP viewport metrics are missing; nothing was sent",
        ));
    };
    Ok(Rect { x: 0, y: 0, w, h })
}

/// #2350: `act_stroke` params for a left-button drag along a straight line.
/// `act_stroke` picks its own route from the session target: raw CDP for a
/// raw-CDP page (page coordinates), else real input (screen coordinates).
#[cfg(windows)]
fn target_act_drag_stroke_params(
    from: Point,
    to: Point,
    duration_ms: u32,
) -> Result<crate::m2::ActStrokeParams, ErrorData> {
    serde_json::from_value(json!({
        "path": {
            "kind": "line",
            "from": { "x": from.x, "y": from.y },
            "to": { "x": to.x, "y": to.y }
        },
        "button": "left",
        "duration_or_speed": { "kind": "duration_ms", "duration_ms": duration_ms }
    }))
    .map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("target_act failed to construct act_stroke params: {error}"),
        )
    })
}

/// #2406: reads what `ElementScope` needs to judge `element_id`: the session
/// target; a native element's top-level window (`ElementRoot`); for a page
/// element, the pages of the target window and the frames under the target
/// page. Reads only: nothing is sent to the element.
async fn target_act_element_scope(
    service: &SynapseService,
    request_context: &RequestContext<RoleServer>,
    element_id: &ElementId,
    verb: &str,
) -> Result<ElementScope, ErrorData> {
    let session_target = match super::context::mcp_session_id_from_request_context(request_context)?
    {
        Some(session_id) => service.session_target(Some(&session_id))?,
        None => None,
    };
    let page_element = synapse_a11y::cdp_backend_from_element_id(element_id).is_some();
    // With no target nothing is read: the answer is TARGET_NOT_SET.
    let root_hwnd = if page_element || session_target.is_none() {
        None
    } else {
        Some(ElementRoot::read(element_id, &format!("target_act verb={verb}"))?.root_hwnd())
    };
    #[cfg(windows)]
    let target_window = match &session_target {
        Some(SessionTarget::Window { hwnd }) => Some(*hwnd),
        Some(SessionTarget::Cdp { window_hwnd, .. }) => Some(*window_hwnd),
        None => None,
    };
    #[cfg(windows)]
    let endpoint = target_window
        .filter(|_| page_element)
        .and_then(synapse_a11y::endpoint_for_window);
    #[cfg(windows)]
    let window_pages = match (&session_target, &endpoint) {
        (Some(SessionTarget::Window { .. }), Some(endpoint)) => {
            target_act_window_pages(endpoint).await?
        }
        _ => Vec::new(),
    };
    #[cfg(not(windows))]
    let window_pages = Vec::new();
    let target = ScopeTarget::new(session_target, window_pages);
    // A frame list that cannot be read counts as no frames: an element of a
    // frame is then refused, never allowed.
    #[cfg(windows)]
    let page_frames = match (target.page(), &endpoint, target_window) {
        (Some(page), Some(endpoint), Some(window_hwnd)) => {
            synapse_a11y::cdp_list_frames(endpoint, window_hwnd, page)
                .await
                .map(|list| {
                    list.frames
                        .into_iter()
                        .map(|frame| frame.cdp_target_id)
                        .collect()
                })
                .unwrap_or_default()
        }
        _ => Vec::new(),
    };
    #[cfg(not(windows))]
    let page_frames = Vec::new();
    Ok(ElementScope::new(
        target,
        ScopeElement::new(element_id.as_str(), root_hwnd, page_frames),
    ))
}

/// #2406: the page ids a raw CDP endpoint lists (the page discovery of the
/// WebView2 branch of `target_act_coordinate_click`).
#[cfg(windows)]
async fn target_act_window_pages(endpoint: &str) -> Result<Vec<String>, ErrorData> {
    Ok(synapse_a11y::cdp_list_targets(endpoint)
        .await
        .map_err(|error| {
            mcp_error(
                error.code(),
                format!("target_act page discovery of the target window failed: {error}"),
            )
        })?
        .into_iter()
        .filter(|target| target.target_type == "page")
        .map(|target| target.target_id)
        .collect())
}

#[cfg(windows)]
async fn target_act_validate_observed_cdp_element_target(
    endpoint: &str,
    window_hwnd: i64,
    session_cdp_target_id: &str,
    element_cdp_target_id: &str,
    verb: &str,
) -> Result<(), ErrorData> {
    if target_act_cdp_target_matches_session_or_frame(
        session_cdp_target_id,
        element_cdp_target_id,
        &[],
    ) {
        return Ok(());
    }
    let frames = synapse_a11y::cdp_list_frames(endpoint, window_hwnd, session_cdp_target_id)
        .await
        .map_err(|error| {
            mcp_error(
                error.code(),
                format!(
                    "target_act verb={verb} could not validate CDP frame ownership for element target {element_cdp_target_id:?} under session target {session_cdp_target_id:?}: {error}"
                ),
            )
        })?;
    if target_act_cdp_target_matches_session_or_frame(
        session_cdp_target_id,
        element_cdp_target_id,
        &frames.frames,
    ) {
        return Ok(());
    }
    Err(mcp_error(
        error_codes::ACTION_ELEMENT_NOT_RESOLVED,
        format!(
            "target_act verb={verb} element_id belongs to CDP target {element_cdp_target_id:?}, but that target is not the owned tab {session_cdp_target_id:?} or any currently attached frame target under it; re-resolve the element because its frame may be stale"
        ),
    ))
}

fn target_act_cdp_target_matches_session_or_frame(
    session_cdp_target_id: &str,
    element_cdp_target_id: &str,
    frames: &[synapse_a11y::CdpFrameTreeEntry],
) -> bool {
    element_cdp_target_id.eq_ignore_ascii_case(session_cdp_target_id)
        || frames.iter().any(|frame| {
            frame
                .cdp_target_id
                .eq_ignore_ascii_case(element_cdp_target_id)
        })
}

#[cfg(windows)]
async fn target_act_tap_element(
    endpoint: &str,
    window_hwnd: i64,
    cdp_target_id: &str,
    params: &TargetActParams,
    verb: &'static str,
) -> Result<TargetActTapElement, ErrorData> {
    if let Some(raw) = params
        .element_id
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        let raw = raw.trim();
        if target_act_has_dom_locator(params) {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!("target_act verb={verb} accepts an element_id or a DOM locator, not both"),
            ));
        }
        if let Ok(element_id) = ElementId::parse(raw)
            && let Some(backend_node_id) = synapse_a11y::cdp_backend_from_element_id(&element_id)
        {
            let element_hwnd = element_id
                .parts()
                .map_err(|error| {
                    mcp_error(
                        error_codes::TOOL_PARAMS_INVALID,
                        format!("target_act verb={verb} element_id is invalid: {error}"),
                    )
                })?
                .hwnd;
            if element_hwnd != window_hwnd {
                return Err(mcp_error(
                    error_codes::ACTION_TARGET_INVALID,
                    format!(
                        "target_act verb={verb} element belongs to browser HWND 0x{element_hwnd:x}, but this session target is HWND 0x{window_hwnd:x}"
                    ),
                ));
            }
            let target_id = synapse_a11y::cdp_target_from_element_id(&element_id)
                .unwrap_or_else(|| cdp_target_id.to_owned());
            target_act_validate_observed_cdp_element_target(
                endpoint,
                window_hwnd,
                cdp_target_id,
                &target_id,
                verb,
            )
            .await?;
            return Ok(TargetActTapElement {
                backend_node_id,
                target_id,
                resolved_by: "observed_cdp_element_id".to_owned(),
                match_count: None,
                returned_count: None,
                element_id: Some(element_id.to_string()),
            });
        }
        if target_act_click_element_id_can_be_dom_id(raw) {
            let selector = target_act_dom_id_selector(raw)?;
            return target_act_locate_tap_element(
                endpoint,
                window_hwnd,
                cdp_target_id,
                synapse_a11y::CdpLocateRequest {
                    engine: synapse_a11y::CdpLocateEngine::Css,
                    query: selector,
                    strict: true,
                    limit: 2,
                    ..Default::default()
                },
                "dom_id",
                verb,
            )
            .await;
        }
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={verb} element_id must be an observed CDP web element id or a plain DOM id; use selector/role/name for DOM locator actions"
            ),
        ));
    }

    if let Some(selector) = params
        .selector
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return target_act_locate_tap_element(
            endpoint,
            window_hwnd,
            cdp_target_id,
            synapse_a11y::CdpLocateRequest {
                engine: synapse_a11y::CdpLocateEngine::Css,
                query: selector.to_owned(),
                strict: true,
                limit: 2,
                ..Default::default()
            },
            "selector",
            verb,
        )
        .await;
    }

    if let Some(role) = trimmed_non_empty_string(params.role.as_deref()) {
        return target_act_locate_tap_element(
            endpoint,
            window_hwnd,
            cdp_target_id,
            synapse_a11y::CdpLocateRequest {
                engine: synapse_a11y::CdpLocateEngine::Role,
                query: role,
                name: trimmed_non_empty_string(params.name.as_deref()),
                strict: true,
                limit: 2,
                ..Default::default()
            },
            "role",
            verb,
        )
        .await;
    }

    if let Some(name) = trimmed_non_empty_string(params.name.as_deref()) {
        return target_act_locate_tap_element(
            endpoint,
            window_hwnd,
            cdp_target_id,
            synapse_a11y::CdpLocateRequest {
                engine: synapse_a11y::CdpLocateEngine::Text,
                query: name,
                strict: true,
                limit: 2,
                ..Default::default()
            },
            "name_text",
            verb,
        )
        .await;
    }

    Err(mcp_error(
        error_codes::TOOL_PARAMS_INVALID,
        format!(
            "target_act verb={verb} requires an observed CDP element_id, selector, role, or name"
        ),
    ))
}

#[cfg(windows)]
async fn target_act_locate_tap_element(
    endpoint: &str,
    window_hwnd: i64,
    cdp_target_id: &str,
    request: synapse_a11y::CdpLocateRequest,
    resolved_by: &'static str,
    verb: &'static str,
) -> Result<TargetActTapElement, ErrorData> {
    let located = synapse_a11y::cdp_locate(endpoint, cdp_target_id, request)
        .await
        .map_err(|error| {
            mcp_error(
                error.code(),
                format!("target_act {verb} raw CDP locator resolution failed: {error}"),
            )
        })?;
    let Some(backend_node_id) = located.backend_node_ids.first().copied() else {
        return Err(mcp_error(
            error_codes::ACTION_ELEMENT_NOT_RESOLVED,
            format!(
                "target_act verb={verb} found no element for {} query {:?} in target {:?}",
                located.engine, located.query, located.target_id
            ),
        ));
    };
    Ok(TargetActTapElement {
        backend_node_id,
        target_id: located.target_id.clone(),
        resolved_by: resolved_by.to_owned(),
        match_count: Some(located.match_count),
        returned_count: Some(located.returned_count),
        element_id: Some(
            synapse_a11y::cdp_element_id_for_target(
                window_hwnd,
                &located.target_id,
                backend_node_id,
            )
            .to_string(),
        ),
    })
}

fn target_act_dom_id_selector(value: &str) -> Result<String, ErrorData> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act DOM element_id must be non-empty visible text",
        ));
    }
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!("[id=\"{escaped}\"]"))
}

fn target_act_has_any_locator(params: &TargetActParams) -> bool {
    params
        .element_id
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
        || target_act_has_dom_locator(params)
}

fn target_act_set_field_locator(params: &TargetActParams) -> Option<ActSetFieldTextLocator> {
    let role = trimmed_non_empty_string(params.role.as_deref());
    let name = trimmed_non_empty_string(params.name.as_deref());
    let automation_id = trimmed_non_empty_string(params.automation_id.as_deref());
    (role.is_some() || name.is_some() || automation_id.is_some()).then_some(
        ActSetFieldTextLocator {
            window_hwnd: None,
            role,
            name,
            name_substring: None,
            automation_id,
        },
    )
}

#[derive(Debug)]
enum TargetActSetFieldTarget {
    Browser {
        selector: Option<String>,
        element_id: Option<String>,
    },
    Native {
        element_id: Option<ElementId>,
        locator: Option<ActSetFieldTextLocator>,
    },
}

async fn target_act_set_field_with<T, F, Fut>(
    params: &TargetActParams,
    dispatch: F,
) -> Result<T, ErrorData>
where
    F: FnOnce(TargetActSetFieldTarget, String) -> Fut,
    Fut: std::future::Future<Output = Result<T, ErrorData>>,
{
    let text = params.text.clone().ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "set_field requires explicit text; use an empty string to clear",
        )
    })?;
    dispatch(target_act_set_field_target(params)?, text).await
}

#[cfg(windows)]
fn target_act_window_bounds_response(
    params: &TargetActParams,
    outcome: &synapse_a11y::WindowBoundsOutcome,
) -> (bool, &'static str, Value) {
    let size_satisfied = params.width.is_none_or(|w| w == outcome.actual.w)
        && params.height.is_none_or(|h| h == outcome.actual.h);
    let ok = size_satisfied
        && params.x.is_none_or(|x| x == outcome.actual.x)
        && params.y.is_none_or(|y| y == outcome.actual.y);
    (
        ok,
        if ok {
            TARGET_ACT_STATUS_OK
        } else {
            TARGET_ACT_STATUS_VERIFY_NEEDED
        },
        json!({
            "requested": { "x":params.x, "y":params.y, "width":params.width, "height":params.height },
            "actual": { "x":outcome.actual.x, "y":outcome.actual.y, "width":outcome.actual.w, "height":outcome.actual.h },
            "minimized":outcome.minimized, "size_satisfied":size_satisfied,
            "source_of_truth":"GetWindowRect readback after SetWindowPos"
        }),
    )
}

fn target_act_set_field_target(
    params: &TargetActParams,
) -> Result<TargetActSetFieldTarget, ErrorData> {
    let selector = trimmed_non_empty_string(params.selector.as_deref());
    let raw_element_id = trimmed_non_empty_string(params.element_id.as_deref());
    let native_locator = target_act_set_field_locator(params);
    let locator_count = usize::from(selector.is_some())
        + usize::from(raw_element_id.is_some())
        + usize::from(native_locator.is_some());
    if locator_count != 1 {
        let message = if locator_count == 0 {
            "target_act verb=set_field requires element_id, selector, or a native/UIA locator (role/name/automation_id)"
        } else {
            "target_act verb=set_field accepts exactly one of element_id, selector, or native/UIA locator (role/name/automation_id)"
        };
        return Err(mcp_error(error_codes::TOOL_PARAMS_INVALID, message));
    }
    if let Some(selector) = selector {
        return Ok(TargetActSetFieldTarget::Browser {
            selector: Some(selector),
            element_id: None,
        });
    }
    if let Some(raw_element_id) = raw_element_id {
        match ElementId::parse(&raw_element_id) {
            Ok(element_id) => {
                return Ok(TargetActSetFieldTarget::Native {
                    element_id: Some(element_id),
                    locator: None,
                });
            }
            Err(_)
                if synapse_browsers::BrowserRegistry::global()
                    .has_bridge_tab_prefix(&raw_element_id) =>
            {
                return Ok(TargetActSetFieldTarget::Browser {
                    selector: None,
                    element_id: Some(raw_element_id),
                });
            }
            Err(_) if target_act_click_element_id_can_be_dom_id(&raw_element_id) => {
                return Ok(TargetActSetFieldTarget::Browser {
                    selector: Some(target_act_dom_id_selector(&raw_element_id)?),
                    element_id: None,
                });
            }
            Err(error) => {
                return Err(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    format!("target_act verb=set_field element_id is invalid: {error}"),
                ));
            }
        }
    }
    Ok(TargetActSetFieldTarget::Native {
        element_id: None,
        locator: native_locator,
    })
}

impl TargetActParams {
    /// #2348: the option an `act select` names: `option`, else
    /// `option_label`, else `value` (the first that is not blank), or
    /// `option_index`.
    fn option_choice(&self) -> Result<synapse_a11y::OptionChoice, String> {
        let option = [
            self.option.as_deref(),
            self.option_label.as_deref(),
            self.value.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find(|text| !text.trim().is_empty());
        synapse_a11y::OptionChoice::new(option, self.option_index)
    }
}

fn target_act_has_dom_locator(params: &TargetActParams) -> bool {
    params
        .selector
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
        || params
            .role
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        || params
            .name
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        || params
            .value
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        || params
            .option
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn trimmed_non_empty_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// #1353: `verb=scroll` routes to the real `act_scroll` wheel primitive (visible
/// in break_glass), which the hidden-tool route guidance advertises. Wheel deltas
/// come in `args` as `[dx, dy]` wheel notches (e.g. `["0","-1"]` = one step down); an optional screen-space
/// `x`/`y` sets the screen point inside the target window. #317: with no
/// coordinates and a native window session target, the wheel goes to the
/// centre of that window in the background (it must be visible there and
/// claimed by this session); a tab target without a raw-CDP endpoint uses
/// a bridge wheel on that tab without activating it. A raw-endpoint tab or
/// no target keeps the existing cursor-over-the-focused-target path.
async fn target_act_scroll(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let mut scroll_params = target_act_scroll_params(params)?;
    let session_target = match super::context::mcp_session_id_from_request_context(request_context)?
    {
        Some(session_id) => service.session_target(Some(&session_id))?,
        None => None,
    };
    let route = target_act_scroll_route(scroll_params.at.is_some(), session_target.as_ref());
    #[cfg(windows)]
    if route == TargetActScrollRoute::BridgeTab
        && let Some(
            target @ SessionTarget::Cdp {
                window_hwnd,
                cdp_target_id,
            },
        ) = session_target.as_ref()
    {
        // A tab with a raw endpoint keeps today's native delegation.
        if synapse_a11y::endpoint_for_window(*window_hwnd).is_some() {
            return target_act_delegate_response(
                "act_scroll",
                service
                    .act_scroll(Parameters(scroll_params), request_context.clone())
                    .await,
            );
        }
        let session_id = target_act_session_id(request_context, "scroll")?;
        let request_details = json!({
            "session_id": &session_id,
            "verb": "scroll",
            "lane": "chrome_debugger_bridge.cdpInput",
            "real_trusted_input": true,
            "required_foreground": false,
        });
        if let Err(error) =
            service.ensure_target_claim_allows_session("target_act", &session_id, target)
        {
            service.audit_action_denied_with_details_for_session(
                "target_act",
                &error,
                &request_details,
                &session_id,
            );
            return Ok((
                "chrome_debugger_bridge.cdpInput",
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        }
        return target_act_bridge_cdp_input(
            service,
            "wheel",
            *window_hwnd,
            cdp_target_id,
            None,
            params,
            &request_details,
            &session_id,
        )
        .await;
    }
    if let TargetActScrollRoute::SessionWindow { hwnd } = route {
        match target_act_session_scroll_point(hwnd) {
            Ok(point) => scroll_params.at = Some(point),
            Err(error) => {
                return target_act_delegate_response("act_scroll", Err::<Json<Value>, _>(error));
            }
        }
    }
    target_act_delegate_response(
        "act_scroll",
        service
            .act_scroll(Parameters(scroll_params), request_context.clone())
            .await,
    )
}

fn target_act_scroll_params(params: &TargetActParams) -> Result<ActScrollParams, ErrorData> {
    let (dx, dy) = parse_target_act_scroll_args(&params.args)?;
    let at = match target_act_coordinate(params)? {
        Some(coordinate) => match coordinate.space {
            TargetActCoordinateSpace::Screen => Some(ActScrollPoint {
                x: coordinate.x,
                y: coordinate.y,
            }),
            other => {
                return Err(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    format!(
                        "target_act verb=scroll supports coordinate_space=screen for the at-point (got {other:?}); pass a screen point inside the target, or omit x/y with the cursor already over the focused target"
                    ),
                ));
            }
        },
        None => None,
    };
    Ok(ActScrollParams {
        dx,
        dy,
        at,
        target: None,
        smooth: false,
        verify_delta: false,
        verify_timeout_ms: default_verify_timeout_ms(),
    })
}

/// #317: where a coordinate-free `target_act verb=scroll` sends its wheel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetActScrollRoute {
    /// The caller passed x/y: scroll at that screen point (#219/#220).
    Point,
    /// No x/y and the session target is a native window: wheel at a visible
    /// point inside that window, without the foreground or the cursor.
    SessionWindow { hwnd: i64 },
    /// No x/y and no session target: the cursor-over-the-focused-target
    /// path, unchanged by #317.
    Unchanged,
    /// No x/y and the session target is a browser tab: check its endpoint
    /// before choosing the bridge or unchanged path (#583 / #317 row 4h).
    BridgeTab,
}

/// #317 / #583: an explicit screen point wins; otherwise a native window
/// gets its own point and a browser tab selects BridgeTab.
/// The caller checks the tab endpoint; this two-argument route stays pure.
const fn target_act_scroll_route(
    has_point: bool,
    target: Option<&SessionTarget>,
) -> TargetActScrollRoute {
    if has_point {
        return TargetActScrollRoute::Point;
    }
    match target {
        Some(SessionTarget::Window { hwnd }) => TargetActScrollRoute::SessionWindow { hwnd: *hwnd },
        Some(SessionTarget::Cdp { .. }) => TargetActScrollRoute::BridgeTab,
        None => TargetActScrollRoute::Unchanged,
    }
}

/// #317: the wheel point of a session-window scroll: the centre of `root`'s
/// window `bounds`, accepted only when `root_at` reports `root` itself under
/// it. A window with no area, a centre outside screen space, or a centre
/// over no window or another window (minimized, off-screen, covered) is
/// refused before any wheel message, so a covering window is never scrolled.
fn session_scroll_point(
    root: i64,
    bounds: Rect,
    root_at: impl Fn(Point) -> Option<i64>,
) -> Result<ActScrollPoint, ErrorData> {
    let refuse = |why: String| {
        mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "target_act verb=scroll refused: {why}; restore or uncover session target window 0x{root:x}, or pass x/y over a visible part of it; no wheel input was sent"
            ),
        )
    };
    if bounds.w <= 0 || bounds.h <= 0 {
        return Err(refuse(format!(
            "the window has no area ({}x{})",
            bounds.w, bounds.h
        )));
    }
    let centre = |origin: i32, size: i32| i32::try_from(i64::from(origin) + i64::from(size) / 2);
    let (Ok(x), Ok(y)) = (centre(bounds.x, bounds.w), centre(bounds.y, bounds.h)) else {
        return Err(refuse(
            "the window centre is outside screen space".to_owned(),
        ));
    };
    match root_at(Point { x, y }) {
        Some(hit) if hit == root => Ok(ActScrollPoint { x, y }),
        Some(hit) => Err(refuse(format!(
            "its centre ({x}, {y}) is covered by window 0x{hit:x}"
        ))),
        None => Err(refuse(format!(
            "its centre ({x}, {y}) is over no window (minimized or off-screen)"
        ))),
    }
}

/// #317 thin layer: the live root of the session target (a closed window is
/// `TARGET_WINDOW_NOT_FOUND`), its window bounds, and the root under the
/// chosen point.
fn target_act_session_scroll_point(hwnd: i64) -> Result<ActScrollPoint, ErrorData> {
    let root = crate::m2::live_target_root(hwnd, "target_act verb=scroll session target")?;
    let bounds = synapse_a11y::foreground_context(root)
        .map_err(|error| {
            mcp_error(
                error.code(),
                format!(
                    "target_act verb=scroll could not read the bounds of session target window 0x{root:x}: {error}"
                ),
            )
        })?
        .window_bounds;
    session_scroll_point(root, bounds, crate::m2::root_hwnd_at_point)
}

/// #219: `verb=scroll` args are `[dx, dy]` in wheel NOTCHES, the same unit as
/// `act_scroll` `dx`/`dy` (one notch = one Win32 wheel step; the 120 `WHEEL_DELTA`
/// multiplier is applied downstream). Positive `dy` scrolls UP and positive `dx`
/// scrolls RIGHT on every route, so this example scrolls down one step.
const TARGET_ACT_SCROLL_DOWN_ONE_STEP_ARGS: [&str; 2] = ["0", "-1"];

fn parse_target_act_scroll_args(args: &[String]) -> Result<(i32, i32), ErrorData> {
    let parse_delta = |raw: &str| -> Result<i32, ErrorData> {
        raw.trim().parse::<i32>().map_err(|_| {
            mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!("target_act verb=scroll args must be integer deltas [dx, dy]; got {raw:?}"),
            )
        })
    };
    let dx = args
        .first()
        .map(|s| parse_delta(s))
        .transpose()?
        .unwrap_or(0);
    let dy = args
        .get(1)
        .map(|s| parse_delta(s))
        .transpose()?
        .unwrap_or(0);
    if dx == 0 && dy == 0 {
        let [example_dx, example_dy] = TARGET_ACT_SCROLL_DOWN_ONE_STEP_ARGS;
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb=scroll requires wheel deltas in args as [dx, dy] (e.g. args=[\"{example_dx}\",\"{example_dy}\"] to scroll down)"
            ),
        ));
    }
    Ok((dx, dy))
}

fn target_act_coordinate(
    params: &TargetActParams,
) -> Result<Option<TargetActCoordinate>, ErrorData> {
    let top_level = match (params.x, params.y) {
        (Some(x), Some(y)) => Some((x, y)),
        (None, None) => None,
        _ => Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act coordinate fallback requires both x and y; one coordinate was missing",
        ))?,
    };
    let nested_position = params
        .coordinate_space
        .zip(params.position)
        .map(|(space, position)| (position.x, position.y, space));
    match (top_level, nested_position) {
        (Some(_), Some(_)) => Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act coordinate input is ambiguous: use either x/y or position with coordinate_space, not both",
        )),
        (Some((x, y)), None) => Ok(Some(TargetActCoordinate {
            x,
            y,
            space: params
                .coordinate_space
                .unwrap_or(TargetActCoordinateSpace::Screen),
        })),
        (None, Some((x, y, space))) => Ok(Some(TargetActCoordinate { x, y, space })),
        (None, None) => {
            if params.coordinate_space.is_some() {
                return Err(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    "target_act coordinate_space requires x/y or nested position {x,y}",
                ));
            }
            Ok(None)
        }
    }
}

fn target_act_coordinate_uses_nested_position(params: &TargetActParams) -> bool {
    params.coordinate_space.is_some()
        && params.position.is_some()
        && params.x.is_none()
        && params.y.is_none()
}

fn target_act_is_click_like_dom_action(action: &str) -> bool {
    matches!(action, "click" | "press" | "dblclick")
}

fn target_act_dom_click_count(action: &str, clicks: Option<u8>) -> Result<Option<u8>, ErrorData> {
    if target_act_is_click_like_dom_action(action) {
        target_act_click_count_for_action(action, clicks).map(Some)
    } else {
        Ok(None)
    }
}

fn target_act_click_count_for_action(action: &str, clicks: Option<u8>) -> Result<u8, ErrorData> {
    let default_clicks = if action == "dblclick" { 2 } else { 1 };
    let clicks = clicks.unwrap_or(default_clicks);
    if !(1..=3).contains(&clicks) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb={action} clicks must be in 1..=3, got {clicks}"),
        ));
    }
    if action == "dblclick" && clicks != 2 {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb=dblclick requires clicks/clickCount to be omitted or 2, got {clicks}"
            ),
        ));
    }
    Ok(clicks)
}

fn target_act_has_click_position_input(params: &TargetActParams) -> bool {
    params.position.is_some() || params.position_x.is_some() || params.position_y.is_some()
}

fn target_act_click_position(params: &TargetActParams) -> Result<Option<(i32, i32)>, ErrorData> {
    let flat = match (params.position_x, params.position_y) {
        (Some(x), Some(y)) => Some((x, y)),
        (None, None) => None,
        _ => {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                "target_act click position requires both position_x and position_y when using flat position fields",
            ));
        }
    };
    let nested = params.position.map(|position| (position.x, position.y));
    if let (Some(flat), Some(nested)) = (flat, nested) {
        if flat != nested {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "target_act click position is ambiguous: flat position {flat:?} conflicts with nested position {nested:?}"
                ),
            ));
        }
    }
    let position = nested.or(flat);
    if let Some((x, y)) = position {
        if x < 0 || y < 0 {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "target_act click position must be non-negative element-relative CSS pixels, got x={x} y={y}"
                ),
            ));
        }
    }
    Ok(position)
}

fn target_act_click_modifiers_bridge_value(
    modifiers: &[TargetActClickModifier],
) -> Result<Value, ErrorData> {
    serde_json::to_value(
        modifiers
            .iter()
            .map(|modifier| modifier.as_bridge_str())
            .collect::<Vec<_>>(),
    )
    .map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("target_act click modifiers encode failed: {error}"),
        )
    })
}

fn target_act_click_modifiers_act_value(
    modifiers: &[TargetActClickModifier],
) -> Result<Value, ErrorData> {
    serde_json::to_value(
        modifiers
            .iter()
            .map(|modifier| modifier.as_act_click_str())
            .collect::<Vec<_>>(),
    )
    .map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("target_act click modifiers encode failed: {error}"),
        )
    })
}

fn target_act_legacy_click_element_id(value: &str) -> Result<Option<ElementId>, ErrorData> {
    target_act_parse_observed_element_id(value, "click")
}

fn target_act_parse_observed_element_id(
    value: &str,
    verb: &str,
) -> Result<Option<ElementId>, ErrorData> {
    match ElementId::parse(value) {
        Ok(element_id) => Ok(Some(element_id)),
        Err(_) if target_act_click_element_id_can_be_dom_id(value) => Ok(None),
        Err(error) => Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb={verb} element_id is invalid: {error}"),
        )),
    }
}

fn target_act_click_element_id_can_be_dom_id(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || value.starts_with("0x") || value.starts_with("-0x") {
        return false;
    }
    if synapse_browsers::BrowserRegistry::global().has_bridge_tab_prefix(value) {
        return true;
    }
    !value.contains(':')
}

fn target_act_validate_dom_locator(
    action: &str,
    params: &TargetActParams,
) -> Result<(), ErrorData> {
    let has_element_id = params
        .element_id
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty());
    let has_selector = params
        .selector
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty());
    let has_semantic = params
        .role
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
        || params
            .name
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        || params
            .value
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty());
    if !(has_element_id || has_selector || has_semantic) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={action} requires element_id, selector, or a semantic locator (role/name/value)"
            ),
        ));
    }
    if action == "select" {
        target_act_validate_select_options(params)?;
    }
    if action == "select" && !target_act_has_select_option_spec(params) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=select requires option, value, option_label, option_index, or options[]",
        ));
    }
    if action == "dispatch_event"
        && !params
            .event_type
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=dispatch_event requires non-empty event_type",
        ));
    }
    // Accept event_init as either a JSON object or a JSON-encoded object string
    // (some MCP transports stringify nested object params); the in-page bridge's
    // normalizeEventInit parses the string back into an EventInit object (#1347).
    if action == "dispatch_event"
        && params
            .event_init
            .as_ref()
            .is_some_and(|value| !value.is_object() && !value.is_string())
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=dispatch_event event_init must be a JSON object or JSON-object string when supplied",
        ));
    }
    if target_act_is_click_like_dom_action(action) {
        let _ = target_act_click_count_for_action(action, params.clicks)?;
        let _ = target_act_click_position(params)?;
    } else if params.clicks.is_some()
        || params.button.is_some()
        || !params.modifiers.is_empty()
        || target_act_has_click_position_input(params)
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={action} does not accept click-only options clicks/clickCount, button, modifiers, or position"
            ),
        ));
    }
    Ok(())
}

fn target_act_has_select_option_spec(params: &TargetActParams) -> bool {
    params
        .option
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
        || params
            .value
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        || params
            .option_label
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        || params.option_index.is_some()
        || !params.options.is_empty()
}

fn target_act_validate_select_options(params: &TargetActParams) -> Result<(), ErrorData> {
    if let Some(index) = params.option_index
        && index < 0
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb=select option_index must be >= 0, got {index}"),
        ));
    }
    for (entry_index, option) in params.options.iter().enumerate() {
        let has_value = option
            .value
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty());
        let has_label = option
            .label
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty());
        let has_index = option.index.is_some();
        let present = usize::from(has_value) + usize::from(has_label) + usize::from(has_index);
        if present != 1 {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "target_act verb=select options[{entry_index}] must contain exactly one of value, label, or index"
                ),
            ));
        }
        if let Some(index) = option.index
            && index < 0
        {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "target_act verb=select options[{entry_index}].index must be >= 0, got {index}"
                ),
            ));
        }
    }
    Ok(())
}

fn target_act_dom_wait_timeout(value: Option<u64>) -> Result<u64, ErrorData> {
    let wait_timeout_ms = value.unwrap_or(default_verify_timeout_ms().into());
    if wait_timeout_ms == 0 || wait_timeout_ms > 30_000 {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act DOM wait_timeout_ms must be 1..=30000, got {wait_timeout_ms}"),
        ));
    }
    Ok(wait_timeout_ms)
}

fn act_operation_name(operation: ActOperation) -> &'static str {
    match operation {
        ActOperation::Invoke => "invoke",
        ActOperation::Foreground => "foreground",
        ActOperation::LeaseAcquire => "lease_acquire",
        ActOperation::LeaseStatus => "lease_status",
        ActOperation::LeaseRelease => "lease_release",
        ActOperation::RouteMemoryStatus => "route_memory_status",
        ActOperation::RouteMemoryClear => "route_memory_clear",
    }
}

/// Owner-away mode: a foreground action stopped because the owner used the
/// physical keyboard/mouse while it ran is reported as `USER_RETURNED`, not as
/// the generic release-interrupt code the backend raised.
fn owner_return_error(error: ErrorData, action_start_ns: u64) -> ErrorData {
    use super::user_active_gate::owner_presence::{last_owner_return_ns, stopped_by_owner_return};
    let code = error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str);
    if !stopped_by_owner_return(code, last_owner_return_ns(), action_start_ns) {
        return error;
    }
    mcp_error(
        error_codes::USER_RETURNED,
        format!(
            "the owner used the physical keyboard/mouse during the action; it was stopped and part of the input may already have been delivered (backend: {})",
            error.message
        ),
    )
}

fn act_command_audit_payload(params: &ActParams) -> Value {
    let owner_approved = params.owner_approved == Some(true);
    json!({
        "operation": act_operation_name(params.operation),
        "action_present": params.action.is_some(),
        "action_verb": params.action.as_ref().map(|action| action.verb.as_str()),
        "reason_present": params.reason.as_ref().is_some_and(|reason| !reason.trim().is_empty()),
        "ttl_ms": params.ttl_ms,
        // #258: an owner override of the user-activity gate is audited with
        // the owner's reason.
        "owner_approved": owner_approved,
        "owner_approved_reason": params.reason.as_deref().filter(|_| owner_approved),
    })
}

fn act_command_audit_before(operation: ActOperation) -> Value {
    json!({
        "source_of_truth": ACT_FACADE_SOURCE_OF_TRUTH,
        "operation": act_operation_name(operation),
        "lease": synapse_action::lease::status(),
    })
}

fn act_command_audit_success_after(response: &ActResponse) -> Value {
    json!({
        "source_of_truth": ACT_FACADE_SOURCE_OF_TRUTH,
        "operation": act_operation_name(response.operation),
        "lease": response.lease,
        "action_status": response.action.as_ref().map(|action| action.status.as_str()),
        "action_ok": response.action.as_ref().map(|action| action.ok),
        "profile_restored": response.foreground.as_ref().map(|foreground| foreground.profile_restored),
    })
}

fn act_command_audit_error_after(operation: ActOperation) -> Value {
    json!({
        "source_of_truth": ACT_FACADE_SOURCE_OF_TRUTH,
        "operation": act_operation_name(operation),
        "lease": synapse_action::lease::status(),
    })
}

fn validate_act_invoke_params(params: &ActParams) -> Result<(), ErrorData> {
    require_act_action(params, ActOperation::Invoke)?;
    if params.reason.is_some() {
        return Err(act_facade_error(
            ActOperation::Invoke,
            "act operation=invoke rejects reason; reason is only valid with operation=foreground",
            "remove reason or call act with operation=foreground",
            "reason",
        ));
    }
    if params.ttl_ms.is_some() {
        return Err(act_facade_error(
            ActOperation::Invoke,
            "act operation=invoke rejects ttl_ms; ttl_ms is only valid with operation=foreground",
            "remove ttl_ms or call act with operation=foreground",
            "ttl_ms",
        ));
    }
    reject_act_owner_approved(params, ActOperation::Invoke)
}

fn validate_act_foreground_params(params: &ActParams) -> Result<(), ErrorData> {
    require_act_action(params, ActOperation::Foreground)?;
    if let Some(ttl_ms) = params.ttl_ms {
        super::lease_tools::validate_lease_ttl_ms("act operation=foreground", ttl_ms)?;
    }
    if params
        .reason
        .as_deref()
        .is_none_or(|reason| reason.trim().is_empty())
    {
        return Err(act_facade_error(
            ActOperation::Foreground,
            "act operation=foreground requires a non-empty reason",
            "pass a non-empty reason explaining why the audited foreground lane is required",
            "reason",
        ));
    }
    Ok(())
}

fn validate_act_lease_acquire_params(params: &ActParams) -> Result<(), ErrorData> {
    reject_act_action(params, ActOperation::LeaseAcquire)?;
    reject_act_reason(params, ActOperation::LeaseAcquire)?;
    reject_act_owner_approved(params, ActOperation::LeaseAcquire)?;
    if let Some(ttl_ms) = params.ttl_ms {
        super::lease_tools::validate_lease_ttl_ms("act operation=lease_acquire", ttl_ms)?;
    }
    Ok(())
}

fn validate_act_lease_read_params(
    params: &ActParams,
    operation: ActOperation,
) -> Result<(), ErrorData> {
    reject_act_action(params, operation)?;
    reject_act_reason(params, operation)?;
    reject_act_ttl(params, operation)?;
    reject_act_owner_approved(params, operation)?;
    Ok(())
}

fn require_act_action(
    params: &ActParams,
    operation: ActOperation,
) -> Result<TargetActParams, ErrorData> {
    params.action.clone().ok_or_else(|| {
        act_facade_error(
            operation,
            format!(
                "act operation={} requires action",
                act_operation_name(operation)
            ),
            "pass a target_act action object for invoke/foreground operations",
            "action",
        )
    })
}

fn reject_act_action(params: &ActParams, operation: ActOperation) -> Result<(), ErrorData> {
    if params.action.is_some() {
        return Err(act_facade_error(
            operation,
            format!(
                "act operation={} rejects action; lease operations do not run target actions",
                act_operation_name(operation)
            ),
            "remove action or use operation=invoke/foreground",
            "action",
        ));
    }
    Ok(())
}

fn reject_act_reason(params: &ActParams, operation: ActOperation) -> Result<(), ErrorData> {
    if params.reason.is_some() {
        return Err(act_facade_error(
            operation,
            format!(
                "act operation={} rejects reason; lease audit uses the underlying lease command and profile set carries escalation reason",
                act_operation_name(operation)
            ),
            "remove reason; pass the profile transition reason to profile operation=set",
            "reason",
        ));
    }
    Ok(())
}

/// #258: `owner_approved` overrides the user-activity gate of the audited
/// foreground lane only; every other operation rejects it.
fn reject_act_owner_approved(params: &ActParams, operation: ActOperation) -> Result<(), ErrorData> {
    if params.owner_approved.is_some() {
        return Err(act_facade_error(
            operation,
            format!(
                "act operation={} rejects owner_approved; it is only valid with operation=foreground",
                act_operation_name(operation)
            ),
            "remove owner_approved, or call act operation=foreground with owner_approved=true and a non-empty reason",
            "owner_approved",
        ));
    }
    Ok(())
}

fn reject_act_ttl(params: &ActParams, operation: ActOperation) -> Result<(), ErrorData> {
    if params.ttl_ms.is_some() {
        return Err(act_facade_error(
            operation,
            format!(
                "act operation={} rejects ttl_ms; ttl_ms is only valid with foreground or lease_acquire",
                act_operation_name(operation)
            ),
            "remove ttl_ms or call operation=lease_acquire",
            "ttl_ms",
        ));
    }
    Ok(())
}

fn act_facade_error(
    operation: ActOperation,
    message: impl Into<String>,
    remediation: &'static str,
    source_id: &'static str,
) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        message.into(),
        Some(json!({
            "code": error_codes::TOOL_PARAMS_INVALID,
            "operation": act_operation_name(operation),
            "source_of_truth": ACT_FACADE_SOURCE_OF_TRUTH,
            "source_id": source_id,
            "remediation": remediation,
        })),
    )
}

/*
 * Legacy element-id click helpers below are kept for observed native/UIA/OCR
 * element ids. Browser DOM selector/name/role actions route through the normal
 * Chrome bridge instead.
 */

fn require_param(value: Option<String>, verb: &str, field: &str) -> Result<String, ErrorData> {
    value.filter(|value| !value.is_empty()).ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb={verb} requires a non-empty `{field}`"),
        )
    })
}

fn target_act_unknown_verb_error(verb: &str) -> ErrorData {
    mcp_error(
        error_codes::TOOL_PARAMS_INVALID,
        format!("target_act verb must be one of {TARGET_ACT_KNOWN_VERBS}; got {verb:?}"),
    )
}

fn target_act_focus_window_params(
    target: Option<&SessionTarget>,
) -> Result<ActFocusWindowParams, ErrorData> {
    let hwnd = match target {
        Some(SessionTarget::Window { hwnd }) => *hwnd,
        Some(SessionTarget::Cdp { window_hwnd, .. }) => *window_hwnd,
        None => {
            return Err(target_act_focus_window_missing_target_error());
        }
    };
    Ok(ActFocusWindowParams {
        hwnd: Some(hwnd),
        title_regex: None,
        pid: None,
        verify_timeout_ms: default_verify_timeout_ms(),
        stable_ms: DEFAULT_TARGET_ACT_FOCUS_STABLE_MS,
    })
}

fn target_act_focus_window_missing_target_error() -> ErrorData {
    mcp_error(
        error_codes::TARGET_NOT_SET,
        "target_act verb=focus_window requires this MCP session to have an agent_logical_foreground/foreground_lane target; call window_list then set_target for the exact HWND first",
    )
}

fn target_act_focus_window_preflight(
    service: &SynapseService,
    session_id: &str,
    target: &SessionTarget,
) -> Result<(), ErrorData> {
    if service
        .target_claim_for_session(session_id, target)?
        .is_none()
    {
        return Err(mcp_error(
            error_codes::TARGET_CLAIM_NOT_FOUND,
            "target_act verb=focus_window requires this MCP session to own a live target_claim for the session target before deliberate real-foreground activation",
        ));
    }
    let lease = synapse_action::lease::status();
    match lease.owner_session_id.as_deref() {
        Some(owner) if owner == session_id => Ok(()),
        Some(_owner) => Err(mcp_error(
            error_codes::ACTION_FOREGROUND_LEASE_BUSY,
            "target_act verb=focus_window requires this MCP session to own the foreground input lease; another live session owns it",
        )),
        None => Err(mcp_error(
            error_codes::ACTION_FOREGROUND_LEASE_NOT_HELD,
            "target_act verb=focus_window requires this MCP session to own the foreground input lease before deliberate real-foreground activation",
        )),
    }
}

fn target_act_read_delegated_tool(
    target: Option<&SessionTarget>,
) -> Result<&'static str, ErrorData> {
    match target {
        Some(SessionTarget::Cdp { .. }) => Ok("cdp_target_info"),
        Some(SessionTarget::Window { .. }) => Ok("observe"),
        None => Err(mcp_error(
            error_codes::TARGET_NOT_SET,
            "target_act verb=read requires this MCP session to have an agent_logical_foreground/foreground_lane target; refusing observe's legacy human OS foreground fallback",
        )),
    }
}

fn target_act_routing_description() -> String {
    "capability-preserving; delegated to the session-targeted primitive, inheriting action audit plus lane/lease/foreground guards and refusing implicit human OS foreground use before input".to_owned()
}

fn target_act_optional_session_target_window_hwnd(
    service: &SynapseService,
    request_context: &RequestContext<RoleServer>,
) -> Result<Option<i64>, ErrorData> {
    let Some(session_id) = super::context::mcp_session_id_from_request_context(request_context)?
    else {
        return Ok(None);
    };
    let target = service.session_target(Some(&session_id))?;
    Ok(target_act_target_window_hwnd(target.as_ref()))
}

const fn target_act_target_window_hwnd(target: Option<&SessionTarget>) -> Option<i64> {
    match target {
        Some(SessionTarget::Window { hwnd }) => Some(*hwnd),
        Some(SessionTarget::Cdp { window_hwnd, .. }) => Some(*window_hwnd),
        None => None,
    }
}

fn target_act_result<T: Serialize>(value: &T) -> Result<Value, ErrorData> {
    serde_json::to_value(value).map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("target_act failed to encode delegated tool result: {error}"),
        )
    })
}

fn target_act_delegate_response<T: Serialize>(
    delegated_tool: &'static str,
    result: Result<Json<T>, ErrorData>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    match result {
        Ok(response) => Ok((
            delegated_tool,
            true,
            TARGET_ACT_STATUS_OK,
            target_act_result(&response.0)?,
        )),
        Err(error) => {
            let status = target_act_error_status(&error);
            Ok((
                delegated_tool,
                false,
                status,
                target_act_error_result(delegated_tool, error),
            ))
        }
    }
}

/// Closing audit row for a target_act bridge/CDP action (#198). When
/// `secret_safe` is set the row is redacted; a redaction failure is itself
/// recorded as the closing error row and returned, so it never strands the
/// started row.
fn audit_action_result_maybe_redacted_for_session(
    service: &SynapseService,
    delegated_tool: &'static str,
    result: &Result<Value, ErrorData>,
    session_id: &str,
    secret_safe: bool,
) -> Result<(), ErrorData> {
    if !secret_safe {
        return service.audit_action_result_for_session("target_act", result, session_id);
    }
    match result {
        Ok(value) => match target_act_secret_safe_result(delegated_tool, value.clone()) {
            Ok(safe) => service.audit_action_redacted_result_for_session(
                "target_act",
                "ok",
                None,
                &json!({
                    "response": safe,
                    "secret_safe": true,
                    "redaction_policy": TARGET_ACT_SECRET_SAFE_REDACTION_POLICY,
                }),
                session_id,
            ),
            Err(redaction_error) => {
                audit_action_redacted_error_for_session(
                    service,
                    delegated_tool,
                    &redaction_error,
                    session_id,
                )?;
                Err(redaction_error)
            }
        },
        Err(error) => {
            audit_action_redacted_error_for_session(service, delegated_tool, error, session_id)
        }
    }
}

/// Redacted closing error row for a secret-safe target_act action.
fn audit_action_redacted_error_for_session(
    service: &SynapseService,
    delegated_tool: &'static str,
    error: &ErrorData,
    session_id: &str,
) -> Result<(), ErrorData> {
    let code = target_act_error_code(error).map(str::to_owned);
    let safe = target_act_secret_safe_error_result_ref(delegated_tool, error);
    service.audit_action_redacted_result_for_session(
        "target_act",
        "error",
        code.as_deref(),
        &json!({
            "error": safe.get("error").cloned().unwrap_or(Value::Null),
            "secret_safe": true,
            "redaction_policy": TARGET_ACT_SECRET_SAFE_REDACTION_POLICY,
        }),
        session_id,
    )
}

fn target_act_maybe_secret_safe_result(
    delegated_tool: &'static str,
    value: Value,
    secret_safe: bool,
) -> Result<Value, ErrorData> {
    if secret_safe {
        target_act_secret_safe_result(delegated_tool, value)
    } else {
        Ok(value)
    }
}

fn target_act_secret_safe_result(
    delegated_tool: &'static str,
    value: Value,
) -> Result<Value, ErrorData> {
    let mut redactions = Vec::new();
    let sanitized = target_act_secret_safe_sanitize_value(&value, "$", &mut redactions)?;
    let mut object = match sanitized {
        Value::Object(object) => object,
        other => {
            let mut object = Map::new();
            object.insert("value".to_owned(), other);
            object
        }
    };
    object.insert("secret_safe".to_owned(), Value::Bool(true));
    object.insert(
        "redaction_policy".to_owned(),
        Value::String(TARGET_ACT_SECRET_SAFE_REDACTION_POLICY.to_owned()),
    );
    object.insert(
        "delegated_tool".to_owned(),
        Value::String(delegated_tool.to_owned()),
    );
    object.insert(
        "redacted_fields".to_owned(),
        Value::Array(redactions.into_iter().map(Value::String).collect()),
    );
    object.insert(
        "source_of_truth".to_owned(),
        Value::String(
            "secret-safe structural browser action readback; page text/html/input/clipboard values suppressed before audit and MCP response"
                .to_owned(),
        ),
    );
    Ok(Value::Object(object))
}

fn target_act_maybe_secret_safe_error_result(
    delegated_tool: &'static str,
    error: ErrorData,
    secret_safe: bool,
) -> Value {
    if secret_safe {
        target_act_secret_safe_error_result_ref(delegated_tool, &error)
    } else {
        target_act_error_result(delegated_tool, error)
    }
}

fn target_act_secret_safe_error_result_ref(
    delegated_tool: &'static str,
    error: &ErrorData,
) -> Value {
    let code = target_act_error_code(error)
        .unwrap_or(error_codes::TOOL_INTERNAL_ERROR)
        .to_owned();
    let message = error.message.to_string();
    let mut redactions = vec!["$.error.message".to_owned()];
    let data = error
        .data
        .as_ref()
        .map(|value| {
            target_act_secret_safe_sanitize_value(value, "$.error.data", &mut redactions)
                .unwrap_or_else(|sanitize_error| {
                    json!({
                        "redacted": true,
                        "redaction_error": sanitize_error.message.to_string(),
                    })
                })
        })
        .unwrap_or(Value::Null);
    json!({
        "error": {
            "delegated_tool": delegated_tool,
            "code": code,
            "message_redacted": true,
            "message_len": message.chars().count(),
            "message_sha256": target_act_secret_safe_sha256(message.as_bytes()),
            "data": data,
            "redaction_policy": TARGET_ACT_SECRET_SAFE_REDACTION_POLICY,
            "redacted_fields": redactions,
        }
    })
}

fn target_act_secret_safe_sanitize_value(
    value: &Value,
    path: &str,
    redactions: &mut Vec<String>,
) -> Result<Value, ErrorData> {
    match value {
        Value::Object(fields) => {
            let mut out = Map::new();
            for (key, child) in fields {
                let child_path = format!("{path}.{}", target_act_secret_safe_path_segment(key));
                if target_act_secret_safe_redact_key(key) {
                    redactions.push(child_path);
                    out.insert(key.clone(), target_act_secret_safe_redacted_scalar(child)?);
                } else {
                    out.insert(
                        key.clone(),
                        target_act_secret_safe_sanitize_value(child, &child_path, redactions)?,
                    );
                }
            }
            Ok(Value::Object(out))
        }
        Value::Array(items) => items
            .iter()
            .enumerate()
            .map(|(index, child)| {
                target_act_secret_safe_sanitize_value(
                    child,
                    &format!("{path}[{index}]"),
                    redactions,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Ok(value.clone()),
    }
}

fn target_act_secret_safe_redact_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    if lower.ends_with("_len")
        || lower.ends_with("_length")
        || lower.ends_with("_sha256")
        || lower.ends_with("_hash")
        || lower == "sha256"
        || lower == "hash"
        || lower == "count"
        || lower.ends_with("_count")
    {
        return false;
    }
    let exact = [
        "text",
        "selected_text",
        "before_value",
        "after_value",
        "expected_value",
        "value",
        "label",
        "option",
        "option_label",
        "name",
        "name_attr",
        "accessible_name",
        "html",
        "body",
        "content",
        "data",
        "data_base64",
        "clipboard",
        "password",
        "secret",
        "token",
        "api_key",
        "apikey",
        "authorization",
        "bearer",
        "cookie",
        "error_detail",
        "message",
    ];
    if exact.iter().any(|candidate| lower == *candidate) {
        return true;
    }
    [
        "page_text",
        "in_page",
        "innerhtml",
        "outerhtml",
        "clipboard",
        "password",
        "secret",
        "token",
        "api_key",
        "apikey",
        "authorization",
        "bearer",
        "cookie",
        "selected_value",
        "selected_values",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || lower.ends_with("_text")
        || lower.ends_with("_html")
        || lower.ends_with("_value")
        || lower.ends_with("_label")
}

fn target_act_secret_safe_redacted_scalar(value: &Value) -> Result<Value, ErrorData> {
    let encoded = serde_json::to_vec(value).map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("target_act secret-safe redaction failed to encode scalar: {error}"),
        )
    })?;
    let original_type = match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    let char_len = value.as_str().map(|text| text.chars().count());
    Ok(json!({
        "redacted": true,
        "original_type": original_type,
        "char_len": char_len,
        "json_len_bytes": encoded.len(),
        "sha256": target_act_secret_safe_sha256(&encoded),
        "redaction_policy": TARGET_ACT_SECRET_SAFE_REDACTION_POLICY,
    }))
}

fn target_act_secret_safe_sha256(bytes: &[u8]) -> String {
    // Keyed (#166): the digest covers redacted page text / error messages, so
    // a plain SHA-256 would be brute-forceable for low-entropy content.
    format!(
        "hmac-sha256:{}",
        crate::content_digest::keyed_digest_hex(bytes)
    )
}

fn target_act_secret_safe_path_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

async fn target_act_key_press(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    if target_act_coordinate(params)?.is_some() || target_act_has_any_locator(params) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=key/press with key(s) targets the current focused control and does not accept x/y or element/DOM locators; focus/click first, or use insert_text/append_text",
        ));
    }
    let verb = params.verb.as_str();
    let keys = target_act_key_chord_keys(params, verb)?;
    let press_params = if params.force_foreground_key_delivery {
        target_act_foreground_press_params(keys, params.wait_timeout_ms, verb)?
    } else {
        target_act_press_params(keys, params.wait_timeout_ms, verb)?
    };
    let response = service
        .act_press(Parameters(press_params), request_context.clone())
        .await;
    if params.force_foreground_key_delivery {
        target_act_foreground_key_response(response)
    } else {
        target_act_delegate_response("act_press", response)
    }
}

async fn target_act_insert_or_append_text(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
    append: bool,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    if target_act_has_key_chord(params) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=insert_text/append_text does not accept key(s); use verb=key for raw chords",
        ));
    }
    let verb = if append { "append_text" } else { "insert_text" };
    if target_act_coordinate(params)?.is_some() && target_act_has_any_locator(params) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={verb} accepts either x/y coordinates or an element/DOM locator, not both"
            ),
        ));
    }
    let text = require_param(params.text.clone(), verb, "text")?;
    let type_params = target_act_type_params(text.clone(), params.wait_timeout_ms, None)?;
    // Reject globally oversized text before native field mutation, coordinate
    // focus, or caret movement. Foreground-only timing validation follows below
    // for routes that actually use SendInput (#50).
    crate::m2::validate_type_params(&type_params)?;
    if let Some(element_id) = target_act_native_text_element_id(params, verb)? {
        let verify_timeout_ms = target_act_verify_timeout(params.wait_timeout_ms, verb)?;
        return target_act_insert_or_append_text_native(
            service,
            element_id,
            text,
            append,
            verify_timeout_ms,
            request_context,
        )
        .await;
    }
    crate::m2::action_from_foreground_type_params(&type_params)?;
    let mut steps = Vec::new();

    if let Some(focus_step) =
        target_act_focus_for_text_insert(service, params, request_context, verb).await?
    {
        let (tool, ok, status, result) = focus_step;
        steps.push(json!({
            "step": "focus",
            "delegated_tool": tool,
            "ok": ok,
            "status": status,
            "result": result,
        }));
        if !ok {
            return Ok((
                "target_act.text_focus+act_type",
                false,
                status,
                json!({
                    "ok": false,
                    "mode": verb,
                    "failed_step": "focus",
                    "steps": steps,
                }),
            ));
        }
    }

    if append {
        let press_params =
            target_act_press_params(vec!["ctrl".to_owned(), "end".to_owned()], None, verb)?;
        let key_response = service
            .act_press(Parameters(press_params), request_context.clone())
            .await;
        let (tool, ok, status, result) = target_act_delegate_response("act_press", key_response)?;
        steps.push(json!({
            "step": "move_caret_to_end",
            "delegated_tool": tool,
            "ok": ok,
            "status": status,
            "result": result,
        }));
        if !ok {
            return Ok((
                "target_act.text_focus+act_press+act_type",
                false,
                status,
                json!({
                    "ok": false,
                    "mode": verb,
                    "failed_step": "move_caret_to_end",
                    "steps": steps,
                }),
            ));
        }
    }

    let type_response = service
        .act_type(Parameters(type_params), request_context.clone())
        .await;
    let (tool, ok, status, result) = target_act_delegate_response("act_type", type_response)?;
    steps.push(json!({
        "step": "type",
        "delegated_tool": tool,
        "ok": ok,
        "status": status,
        "result": result,
    }));

    Ok((
        if append {
            "target_act.text_focus+act_press+act_type"
        } else {
            "target_act.text_focus+act_type"
        },
        ok,
        status,
        json!({
            "ok": ok,
            "mode": verb,
            "steps": steps,
        }),
    ))
}

fn target_act_native_text_element_id(
    params: &TargetActParams,
    verb: &str,
) -> Result<Option<ElementId>, ErrorData> {
    let Some(raw_element_id) = params
        .element_id
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(element_id) = target_act_parse_observed_element_id(raw_element_id, verb)? else {
        return Ok(None);
    };
    if synapse_a11y::cdp_backend_from_element_id(&element_id).is_some() {
        return Ok(None);
    }
    if target_act_has_dom_locator(params) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={verb} accepts an observed native element_id or a DOM locator, not both"
            ),
        ));
    }
    Ok(Some(element_id))
}

async fn target_act_insert_or_append_text_native(
    service: &SynapseService,
    element_id: ElementId,
    text: String,
    append: bool,
    verify_timeout_ms: u32,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let verb = if append { "append_text" } else { "insert_text" };
    let delegated_tool = if append {
        "synapse_a11y.append_element_text"
    } else {
        "synapse_a11y.replace_element_text_selection"
    };
    let session_id = target_act_session_id(request_context, verb)?;
    let (element_hwnd, root_hwnd, target) = target_act_native_element_target(&element_id, verb)?;
    let request_details = json!({
        "session_id": &session_id,
        "verb": verb,
        "delegated_tool": delegated_tool,
        "element_id": element_id.to_string(),
        "element_hwnd": element_hwnd,
        "window_hwnd": root_hwnd,
        "text_len": text.chars().count(),
        "text_utf16_len": text.encode_utf16().count(),
        "append": append,
        "verify_timeout_ms": verify_timeout_ms,
        "required_foreground": false,
    });
    match service.session_target(Some(&session_id))? {
        Some(current) if current == target => {}
        Some(current) => {
            let error = mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                format!(
                    "target_act verb={verb} element belongs to HWND 0x{root_hwnd:x}, but this session target is {current:?}"
                ),
            );
            service.audit_action_denied_with_details_for_session(
                "target_act",
                &error,
                &request_details,
                &session_id,
            );
            return Ok((
                delegated_tool,
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        }
        None => {
            let error = mcp_error(
                error_codes::TARGET_NOT_SET,
                format!(
                    "target_act verb={verb} requires this MCP session to have an owned window target; call set_target for the exact HWND first"
                ),
            );
            service.audit_action_denied_with_details_for_session(
                "target_act",
                &error,
                &request_details,
                &session_id,
            );
            return Ok((
                delegated_tool,
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        }
    }
    if let Err(error) =
        service.ensure_target_claim_allows_session("target_act", &session_id, &target)
    {
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            delegated_tool,
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    }
    service.audit_action_started_with_details_for_session(
        "target_act",
        &request_details,
        &session_id,
    )?;
    let result = if append {
        synapse_a11y::append_element_text(&element_id, &text)
    } else {
        synapse_a11y::replace_element_text_selection(&element_id, &text)
    }
    .map_err(|error| {
        mcp_error(
            error.code(),
            format!("target_act native {verb} failed: {error}"),
        )
    });
    service.audit_action_result_for_session("target_act", &result, &session_id)?;
    match result {
        Ok(readback) => Ok((
            delegated_tool,
            true,
            TARGET_ACT_STATUS_OK,
            target_act_result(&readback)?,
        )),
        Err(error) => Ok((
            delegated_tool,
            false,
            target_act_error_status(&error),
            target_act_error_result(delegated_tool, error),
        )),
    }
}

async fn target_act_focus_for_text_insert(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
    verb: &str,
) -> Result<Option<(&'static str, bool, &'static str, Value)>, ErrorData> {
    if target_act_coordinate(params)?.is_some() {
        return target_act_coordinate_click(service, "click", params, request_context)
            .await
            .map(Some);
    }

    if let Some(raw_element_id) = params
        .element_id
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        if let Some(element_id) = target_act_legacy_click_element_id(raw_element_id)? {
            // #2406: the focus click is a click by element id: the element must
            // be inside this session's target (`ElementScope`); a refusal sends
            // nothing.
            if let ScopeVerdict::Refused { code, text } =
                target_act_element_scope(service, request_context, &element_id, verb)
                    .await?
                    .verdict(verb)
            {
                return Err(mcp_error(code, text));
            }
            let click_params = target_act_click_params(element_id, 1, None, &[])?;
            let response = service
                .act_click(Parameters(click_params), request_context.clone())
                .await;
            return target_act_delegate_response("act_click", response).map(Some);
        }
        return target_act_browser_dom_action(service, "click", params, request_context)
            .await
            .map(Some);
    }

    if target_act_has_dom_locator(params) {
        return target_act_browser_dom_action(service, "click", params, request_context)
            .await
            .map(Some);
    }

    tracing::info!(
        code = "TARGET_ACT_TEXT_INSERT_CURRENT_FOCUS",
        verb,
        "target_act text insertion will use the current focused control without a focus step"
    );
    Ok(None)
}

const TARGET_ACT_SET_SELECTION_JS: &str = r"(el, start, end) => {
    if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end < start) {
        throw new Error(`invalid selection range ${start}..${end}`);
    }
    const selectedText = () => {
        const selection = el.ownerDocument.defaultView.getSelection();
        return selection ? selection.toString() : '';
    };
    if (typeof el.setSelectionRange === 'function' && 'value' in el) {
        if (el.disabled || el.readOnly) {
            throw new Error('target value control is disabled or readonly');
        }
        const value = String(el.value ?? '');
        if (end > value.length) {
            throw new Error(`selection range ${start}..${end} exceeds value length ${value.length}`);
        }
        const beforeStart = el.selectionStart ?? 0;
        const beforeEnd = el.selectionEnd ?? beforeStart;
        el.focus();
        el.setSelectionRange(start, end);
        el.dispatchEvent(new Event('select', { bubbles: true }));
        return {
            method: 'dom_set_selection_range',
            text_len: value.length,
            requested_start: start,
            requested_end: end,
            before_start: beforeStart,
            before_end: beforeEnd,
            after_start: el.selectionStart ?? start,
            after_end: el.selectionEnd ?? end,
            is_password: String(el.tagName).toLowerCase() === 'input' && String(el.getAttribute('type') || '').toLowerCase() === 'password',
            selected_text: String(el.tagName).toLowerCase() === 'input' && String(el.getAttribute('type') || '').toLowerCase() === 'password' ? '[REDACTED]' : value.slice(start, end)
        };
    }
    if (!el.isContentEditable) {
        throw new Error('target is not an editable value control or contenteditable element');
    }
    const doc = el.ownerDocument;
    const walker = doc.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    const textNodes = [];
    let textLen = 0;
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        const len = node.nodeValue.length;
        textNodes.push({ node, start: textLen, end: textLen + len });
        textLen += len;
    }
    if (end > textLen) {
        throw new Error(`selection range ${start}..${end} exceeds textContent length ${textLen}`);
    }
    const boundary = (offset) => {
        for (const item of textNodes) {
            if (offset <= item.end) {
                return { node: item.node, offset: Math.max(0, offset - item.start) };
            }
        }
        return { node: el, offset: el.childNodes.length };
    };
    const selectionOffsets = () => {
        const selection = doc.defaultView.getSelection();
        if (!selection || selection.rangeCount === 0) {
            return { start: 0, end: 0 };
        }
        const range = selection.getRangeAt(0);
        const preStart = doc.createRange();
        preStart.selectNodeContents(el);
        preStart.setEnd(range.startContainer, range.startOffset);
        const preEnd = doc.createRange();
        preEnd.selectNodeContents(el);
        preEnd.setEnd(range.endContainer, range.endOffset);
        return { start: preStart.toString().length, end: preEnd.toString().length };
    };
    const before = selectionOffsets();
    const startBoundary = boundary(start);
    const endBoundary = boundary(end);
    const range = doc.createRange();
    range.setStart(startBoundary.node, startBoundary.offset);
    range.setEnd(endBoundary.node, endBoundary.offset);
    el.focus();
    const selection = doc.defaultView.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    doc.dispatchEvent(new Event('selectionchange', { bubbles: true }));
    const after = selectionOffsets();
    return {
        method: 'contenteditable_dom_range',
        text_len: textLen,
        requested_start: start,
        requested_end: end,
        before_start: before.start,
        before_end: before.end,
        after_start: after.start,
        after_end: after.end,
        selected_text: selectedText()
    };
}";

async fn target_act_set_selection(
    service: &SynapseService,
    params: &TargetActParams,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    if target_act_coordinate(params)?.is_some() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=set_selection does not accept x/y coordinates; pass an observed element_id",
        ));
    }
    if target_act_has_dom_locator(params) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=set_selection currently accepts an observed Synapse element_id only, not selector/role/name/value/option locators",
        ));
    }
    if target_act_has_key_chord(params) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=set_selection does not accept key(s); use verb=key for raw keyboard selection chords",
        ));
    }
    if params.text.as_ref().is_some_and(|value| !value.is_empty()) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=set_selection does not accept text; use insert_text or append_text after setting the selection",
        ));
    }
    let element_id = require_param(params.element_id.clone(), "set_selection", "element_id")?;
    let element_id = ElementId::parse(&element_id).map_err(|error| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb=set_selection element_id is invalid: {error}"),
        )
    })?;
    let (start, end) = target_act_selection_range(params)?;
    if synapse_a11y::cdp_backend_from_element_id(&element_id).is_some() {
        target_act_set_selection_web(service, element_id, start, end, request_context).await
    } else {
        target_act_set_selection_native(service, element_id, start, end, request_context).await
    }
}

async fn target_act_set_selection_web(
    service: &SynapseService,
    element_id: ElementId,
    start: u32,
    end: u32,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    #[cfg(windows)]
    {
        const DELEGATED_TOOL: &str = "synapse_a11y.cdp_evaluate_on_element";
        let session_id = target_act_session_id(request_context, "set_selection")?;
        let backend_node_id =
            synapse_a11y::cdp_backend_from_element_id(&element_id).ok_or_else(|| {
                mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    "target_act verb=set_selection element_id is not a CDP-backed web element",
                )
            })?;
        let element_target_id =
            synapse_a11y::cdp_target_from_element_id(&element_id).ok_or_else(|| {
                mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    "target_act verb=set_selection web element_id must include an embedded CDP target id; re-resolve it with find/observe against the owned tab",
                )
            })?;
        let element_hwnd = element_id
            .parts()
            .map_err(|error| {
                mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    format!("target_act verb=set_selection element_id is invalid: {error}"),
                )
            })?
            .hwnd;
        let request_details = json!({
            "session_id": &session_id,
            "verb": "set_selection",
            "element_id": element_id.to_string(),
            "element_hwnd": element_hwnd,
            "backend_node_id": backend_node_id,
            "element_cdp_target_id": &element_target_id,
            "selection_start": start,
            "selection_end": end,
            "delegated_tool": DELEGATED_TOOL,
            "required_foreground": false,
        });
        let Some(target) = service.session_target(Some(&session_id))? else {
            let error = mcp_error(
                error_codes::TARGET_NOT_SET,
                "target_act verb=set_selection requires this MCP session to own a CDP browser target; bind one with cdp_open_tab/set_target first",
            );
            service.audit_action_denied_with_details_for_session(
                "target_act",
                &error,
                &request_details,
                &session_id,
            );
            return Ok((
                DELEGATED_TOOL,
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        };
        let (window_hwnd, session_cdp_target_id) = match &target {
            SessionTarget::Cdp {
                window_hwnd,
                cdp_target_id,
            } => (*window_hwnd, cdp_target_id.clone()),
            SessionTarget::Window { .. } => {
                let error = mcp_error(
                    error_codes::ACTION_TARGET_INVALID,
                    "target_act verb=set_selection with an observed web element_id requires a session-owned CDP target, not a native/window target",
                );
                service.audit_action_denied_with_details_for_session(
                    "target_act",
                    &error,
                    &request_details,
                    &session_id,
                );
                return Ok((
                    DELEGATED_TOOL,
                    false,
                    target_act_error_status(&error),
                    target_act_error_result("target_act", error),
                ));
            }
        };
        if window_hwnd != element_hwnd {
            let error = mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                format!(
                    "target_act verb=set_selection element belongs to browser HWND 0x{element_hwnd:x}, but this session target is HWND 0x{window_hwnd:x}"
                ),
            );
            service.audit_action_denied_with_details_for_session(
                "target_act",
                &error,
                &request_details,
                &session_id,
            );
            return Ok((
                DELEGATED_TOOL,
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        }
        if let Err(error) =
            service.ensure_target_claim_allows_session("target_act", &session_id, &target)
        {
            service.audit_action_denied_with_details_for_session(
                "target_act",
                &error,
                &request_details,
                &session_id,
            );
            return Ok((
                DELEGATED_TOOL,
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        }
        let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) else {
            let error = mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                format!(
                    "target_act verb=set_selection requires a raw CDP debugging endpoint for window 0x{window_hwnd:x}"
                ),
            );
            service.audit_action_denied_with_details_for_session(
                "target_act",
                &error,
                &request_details,
                &session_id,
            );
            return Ok((
                DELEGATED_TOOL,
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        };
        if let Err(error) = target_act_validate_observed_cdp_element_target(
            &endpoint,
            window_hwnd,
            &session_cdp_target_id,
            &element_target_id,
            "set_selection",
        )
        .await
        {
            service.audit_action_denied_with_details_for_session(
                "target_act",
                &error,
                &request_details,
                &session_id,
            );
            return Ok((
                DELEGATED_TOOL,
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        }
        let request_details = json!({
            "session_id": &session_id,
            "verb": "set_selection",
            "element_id": element_id.to_string(),
            "window_hwnd": window_hwnd,
            "backend_node_id": backend_node_id,
            "session_cdp_target_id": &session_cdp_target_id,
            "element_cdp_target_id": &element_target_id,
            "dispatch_cdp_target_id": &element_target_id,
            "selection_start": start,
            "selection_end": end,
            "delegated_tool": DELEGATED_TOOL,
            "required_foreground": false,
        });
        service.audit_action_started_with_details_for_session(
            "target_act",
            &request_details,
            &session_id,
        )?;
        let result = synapse_a11y::cdp_evaluate_on_element(
            &endpoint,
            &element_target_id,
            backend_node_id,
            TARGET_ACT_SET_SELECTION_JS,
            &[json!(start), json!(end)],
            false,
            true,
        )
        .await
        .map_err(|error| {
            mcp_error(
                error.code(),
                format!("target_act web set_selection failed: {error}"),
            )
        });
        service.audit_action_result_for_session("target_act", &result, &session_id)?;
        match result {
            Ok(readback) => {
                let mut value = target_act_result(&readback)?;
                if let Some(object) = value.as_object_mut() {
                    object.insert("element_id".to_owned(), json!(element_id.to_string()));
                    object.insert("backend_node_id".to_owned(), json!(backend_node_id));
                    object.insert("session_target_id".to_owned(), json!(session_cdp_target_id));
                    object.insert("target_id".to_owned(), json!(element_target_id));
                    object.insert("window_hwnd".to_owned(), json!(window_hwnd));
                }
                Ok((DELEGATED_TOOL, true, TARGET_ACT_STATUS_OK, value))
            }
            Err(error) => Ok((
                DELEGATED_TOOL,
                false,
                target_act_error_status(&error),
                target_act_error_result(DELEGATED_TOOL, error),
            )),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (service, element_id, start, end, request_context);
        let error = mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            "target_act verb=set_selection observed CDP element_id support requires Windows CDP action support",
        );
        Ok((
            "synapse_a11y.cdp_evaluate_on_element",
            false,
            target_act_error_status(&error),
            target_act_error_result("synapse_a11y.cdp_evaluate_on_element", error),
        ))
    }
}

fn target_act_native_element_target(
    element_id: &ElementId,
    verb: &str,
) -> Result<(i64, i64, SessionTarget), ErrorData> {
    let root = ElementRoot::read(element_id, &format!("target_act verb={verb}"))?;
    Ok((
        root.element_hwnd(),
        root.root_hwnd(),
        SessionTarget::Window {
            hwnd: root.root_hwnd(),
        },
    ))
}

async fn target_act_set_selection_native(
    service: &SynapseService,
    element_id: ElementId,
    start: u32,
    end: u32,
    request_context: &RequestContext<RoleServer>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    let session_id = target_act_session_id(request_context, "set_selection")?;
    let (element_hwnd, root_hwnd, target) =
        target_act_native_element_target(&element_id, "set_selection")?;
    let request_details = json!({
        "session_id": &session_id,
        "verb": "set_selection",
        "element_id": element_id.to_string(),
        "element_hwnd": element_hwnd,
        "window_hwnd": root_hwnd,
        "selection_start": start,
        "selection_end": end,
        "required_foreground": false,
    });
    match service.session_target(Some(&session_id))? {
        Some(current) if current == target => {}
        Some(current) => {
            let error = mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                format!(
                    "target_act verb=set_selection element belongs to HWND 0x{root_hwnd:x}, but this session target is {current:?}"
                ),
            );
            service.audit_action_denied_with_details_for_session(
                "target_act",
                &error,
                &request_details,
                &session_id,
            );
            return Ok((
                "synapse_a11y.set_element_text_selection",
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        }
        None => {
            let error = mcp_error(
                error_codes::TARGET_NOT_SET,
                "target_act verb=set_selection requires this MCP session to have an owned window target; call set_target for the exact HWND first",
            );
            service.audit_action_denied_with_details_for_session(
                "target_act",
                &error,
                &request_details,
                &session_id,
            );
            return Ok((
                "synapse_a11y.set_element_text_selection",
                false,
                target_act_error_status(&error),
                target_act_error_result("target_act", error),
            ));
        }
    }
    if let Err(error) =
        service.ensure_target_claim_allows_session("target_act", &session_id, &target)
    {
        service.audit_action_denied_with_details_for_session(
            "target_act",
            &error,
            &request_details,
            &session_id,
        );
        return Ok((
            "synapse_a11y.set_element_text_selection",
            false,
            target_act_error_status(&error),
            target_act_error_result("target_act", error),
        ));
    }
    service.audit_action_started_with_details_for_session(
        "target_act",
        &request_details,
        &session_id,
    )?;
    let result =
        synapse_a11y::set_element_text_selection(&element_id, start, end).map_err(|error| {
            mcp_error(
                error.code(),
                format!("target_act native set_selection failed: {error}"),
            )
        });
    service.audit_action_result_for_session("target_act", &result, &session_id)?;
    match result {
        Ok(readback) => Ok((
            "synapse_a11y.set_element_text_selection",
            true,
            TARGET_ACT_STATUS_OK,
            target_act_result(&readback)?,
        )),
        Err(error) => Ok((
            "synapse_a11y.set_element_text_selection",
            false,
            target_act_error_status(&error),
            target_act_error_result("synapse_a11y.set_element_text_selection", error),
        )),
    }
}

fn target_act_click_params(
    element_id: ElementId,
    clicks: u8,
    button: Option<TargetActMouseButton>,
    modifiers: &[TargetActClickModifier],
) -> Result<ActClickParams, ErrorData> {
    let modifiers = target_act_click_modifiers_act_value(modifiers)?;
    serde_json::from_value(json!({
        "target": {
            "element_id": element_id.to_string()
        },
        "button": button.map(TargetActMouseButton::as_str).unwrap_or("left"),
        "clicks": clicks,
        "modifiers": modifiers,
        "verify_delta": true,
        "verify_timeout_ms": default_verify_timeout_ms()
    }))
    .map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("target_act failed to construct act_click params: {error}"),
        )
    })
}

fn target_act_click_point_params(
    point: Point,
    clicks: u8,
    button: Option<TargetActMouseButton>,
    modifiers: &[TargetActClickModifier],
    verify_target_window_hwnd: Option<i64>,
) -> Result<ActClickParams, ErrorData> {
    let modifiers = target_act_click_modifiers_act_value(modifiers)?;
    serde_json::from_value(json!({
        "target": {
            "x": point.x,
            "y": point.y
        },
        "button": button.map(TargetActMouseButton::as_str).unwrap_or("left"),
        "clicks": clicks,
        "modifiers": modifiers,
        "verify_delta": true,
        "verify_target_window_hwnd": verify_target_window_hwnd,
        "verify_timeout_ms": default_verify_timeout_ms()
    }))
    .map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("target_act failed to construct coordinate act_click params: {error}"),
        )
    })
}

fn target_act_type_params(
    text: String,
    wait_timeout_ms: Option<u64>,
    verify_target_window_hwnd: Option<i64>,
) -> Result<ActTypeParams, ErrorData> {
    let verify_timeout_ms = target_act_type_verify_timeout(wait_timeout_ms)?;
    serde_json::from_value(json!({
        "text": text,
        "verify_delta": true,
        "verify_target_window_hwnd": verify_target_window_hwnd,
        "verify_timeout_ms": verify_timeout_ms
    }))
    .map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("target_act failed to construct act_type params: {error}"),
        )
    })
}

fn target_act_press_params(
    keys: Vec<String>,
    wait_timeout_ms: Option<u64>,
    verb: &str,
) -> Result<ActPressParams, ErrorData> {
    let verify_timeout_ms = target_act_verify_timeout(wait_timeout_ms, verb)?;
    let params: ActPressParams = serde_json::from_value(json!({
        "keys": keys,
        "verify_delta": true,
        "verify_timeout_ms": verify_timeout_ms
    }))
    .map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("target_act failed to construct act_press params: {error}"),
        )
    })?;
    target_act_reject_printable_text_sequence(&params, verb)?;
    Ok(params)
}

fn target_act_foreground_press_params(
    keys: Vec<String>,
    wait_timeout_ms: Option<u64>,
    verb: &str,
) -> Result<ActPressParams, ErrorData> {
    let mut params = target_act_press_params(keys, wait_timeout_ms, verb)?;
    // Keep SendInput's software backend. The private marker selects the
    // foreground lane without making the BreakGlass profile itself alter a
    // caller's normal target-scoped routing, and without requesting the removed
    // hardware backend.
    // Native related-window readback can verify an observed ownership transition without
    // a broad UIA snapshot. Unchanged foreground stays delivered_unverified.
    params.verify_delta = false;
    params.force_foreground_key_delivery = true;
    Ok(params)
}

fn target_act_foreground_key_response(
    result: Result<Json<crate::m2::ActPressResponse>, ErrorData>,
) -> Result<(&'static str, bool, &'static str, Value), ErrorData> {
    match result {
        Ok(response) => {
            let verified = response.0.ok
                && response.0.postcondition.status == "observed_delta"
                && response.0.postcondition.observed_delta == Some(true);
            Ok((
                "act_press",
                verified,
                if verified {
                    TARGET_ACT_STATUS_OK
                } else {
                    TARGET_ACT_STATUS_VERIFY_NEEDED
                },
                target_act_result(&response.0)?,
            ))
        }
        Err(error) => {
            let status = target_act_error_status(&error);
            Ok((
                "act_press",
                false,
                status,
                target_act_error_result("act_press", error),
            ))
        }
    }
}

fn target_act_reject_printable_text_sequence(
    params: &ActPressParams,
    verb: &str,
) -> Result<(), ErrorData> {
    let labels = act_press_normalized_labels(params)?;
    // Count only: the labels are not echoed because errors are audited.
    let printable = labels
        .iter()
        .filter(|label| target_act_key_label_is_text_input(label))
        .count();
    if printable > 1 {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={verb} keys contain {printable} printable text-producing keys; key/press is a simultaneous chord route, not ordered text input. Use verb=type or verb=insert_text for ordered text so Synapse can preflight and verify the focused text Source of Truth before mutation.",
            ),
        ));
    }
    Ok(())
}

fn target_act_key_label_is_text_input(label: &str) -> bool {
    matches!(label, "`" | "space")
        || (label.len() == 1 && label.as_bytes()[0].is_ascii_alphanumeric())
}

fn target_act_type_verify_timeout(value: Option<u64>) -> Result<u32, ErrorData> {
    target_act_verify_timeout(value, "type")
}

fn target_act_verify_timeout(value: Option<u64>, verb: &str) -> Result<u32, ErrorData> {
    let wait_timeout_ms = value.unwrap_or_else(|| u64::from(default_verify_timeout_ms()));
    if !(50..=5000).contains(&wait_timeout_ms) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_act verb={verb} wait_timeout_ms must be 50..=5000, got {wait_timeout_ms}"
            ),
        ));
    }
    Ok(wait_timeout_ms as u32)
}

fn target_act_has_key_chord(params: &TargetActParams) -> bool {
    params
        .key
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty())
        || !params.keys.is_empty()
}

fn target_act_key_chord_keys(
    params: &TargetActParams,
    verb: &str,
) -> Result<Vec<String>, ErrorData> {
    let has_key = params
        .key
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty());
    let has_keys = !params.keys.is_empty();
    match (has_key, has_keys) {
        (true, true) => Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb={verb} accepts either key or keys, not both"),
        )),
        (true, false) => {
            target_act_parse_key_chord(params.key.as_deref().unwrap_or_default(), verb)
        }
        (false, true) => Ok(params.keys.clone()),
        (false, false) => Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb={verb} requires key or keys"),
        )),
    }
}

fn target_act_parse_key_chord(raw: &str, verb: &str) -> Result<Vec<String>, ErrorData> {
    let keys = raw
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if keys.is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb={verb} key chord must contain at least one key"),
        ));
    }
    Ok(keys)
}

fn target_act_selection_range(params: &TargetActParams) -> Result<(u32, u32), ErrorData> {
    let start = params.selection_start.ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=set_selection requires selection_start (alias: start)",
        )
    })?;
    let end = params.selection_end.ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act verb=set_selection requires selection_end (alias: end)",
        )
    })?;
    if end < start {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb=set_selection requires end >= start, got {start}..{end}"),
        ));
    }
    Ok((start, end))
}

fn target_act_window_coordinate_to_screen_point(
    hwnd: i64,
    coordinate: TargetActCoordinate,
) -> Result<Point, ErrorData> {
    if coordinate.space == TargetActCoordinateSpace::Viewport {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target_act native/window coordinate click does not support coordinate_space=viewport; use screen or window coordinates, or bind a Chrome CDP target for viewport coordinates",
        ));
    }
    let context = synapse_a11y::foreground_context(hwnd).map_err(|error| {
        mcp_error(
            error.code(),
            format!(
                "target_act coordinate click target window HWND 0x{hwnd:x} bounds readback failed: {error}"
            ),
        )
    })?;
    let bounds = context.window_bounds;
    let point = match coordinate.space {
        TargetActCoordinateSpace::Screen => Point {
            x: coordinate.x,
            y: coordinate.y,
        },
        TargetActCoordinateSpace::Window => Point {
            x: bounds.x.saturating_add(coordinate.x),
            y: bounds.y.saturating_add(coordinate.y),
        },
        TargetActCoordinateSpace::Viewport => unreachable!("viewport rejected above"),
    };
    if !target_act_rect_contains_point(bounds, point) {
        return Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "target_act coordinate click point ({}, {}) is outside target window 0x{hwnd:x} bounds ({}, {}, {}x{})",
                point.x, point.y, bounds.x, bounds.y, bounds.w, bounds.h
            ),
        ));
    }
    Ok(point)
}

/// #107: a claimed native coordinate click does not require OS foreground. Returns
/// the claimed root HWND; act_click picks the delivery route from live foreground
/// (background child-tree PostMessage, or fail-closed refusal) and never self-activates.
fn target_act_window_coordinate_target_root(hwnd: i64) -> Result<i64, ErrorData> {
    synapse_a11y::top_level_root_hwnd(hwnd).map_err(|error| {
        mcp_error(
            error.code(),
            format!(
                "target_act coordinate click target HWND 0x{hwnd:x} root readback failed: {error}"
            ),
        )
    })
}

fn target_act_rect_contains_point(rect: Rect, point: Point) -> bool {
    point.x >= rect.x
        && point.y >= rect.y
        && point.x < rect.x.saturating_add(rect.w)
        && point.y < rect.y.saturating_add(rect.h)
}

fn target_act_error_result(delegated_tool: &'static str, error: ErrorData) -> Value {
    let code = target_act_error_code(&error)
        .unwrap_or(error_codes::TOOL_INTERNAL_ERROR)
        .to_owned();
    let deferral = crate::m2::foreground_deferral::deferred_result_fields(&error);
    let mut result = json!({
        "error": {
            "delegated_tool": delegated_tool,
            "code": code,
            "message": error.message.to_string(),
            "data": error.data,
        }
    });
    if let (Some(object), Some(Value::Object(fields))) = (result.as_object_mut(), deferral) {
        object.extend(fields);
    }
    result
}

/// #9: the `target_act` response for a foreground call whose activation step was
/// deferred (retryable refusal) before any input was delivered.
fn target_act_deferred_response(verb: &str, error: ErrorData) -> TargetActResponse {
    TargetActResponse {
        verb: verb.to_owned(),
        ok: false,
        status: TARGET_ACT_STATUS_DEFERRED.to_owned(),
        delivery_state: DeliveryClaim::without_result(false, TARGET_ACT_STATUS_DEFERRED)
            .state()
            .to_owned(),
        delegated_tool: "act_focus_window".to_owned(),
        routing: target_act_routing_description(),
        result: target_act_error_result("act_focus_window", error),
        route_memory: None,
    }
}

/// Refusal text for a background WebView2 click with no reachable page CDP
/// endpoint. The target may or may not hold a session claim, so the text names
/// it only as the target HWND (#222).
pub(super) fn webview2_background_click_refusal_message(hwnd: i64, detail: Option<&str>) -> String {
    format!(
        "target_act background WebView2 click refused before delivery for target HWND 0x{hwnd:x}: no reachable page CDP endpoint. No foreground permission or activation is needed for a target-scoped CDP action. {}",
        detail.unwrap_or("Launch the app through Synapse process launch with cdp_debug=true")
    )
}

fn target_act_error_status(error: &ErrorData) -> &'static str {
    if target_act_error_has_delivered_tier(error) {
        return TARGET_ACT_STATUS_VERIFY_NEEDED;
    }
    if crate::m2::foreground_deferral::is_deferred(error) {
        return TARGET_ACT_STATUS_DEFERRED;
    }
    // #294: the per-batch delivery guard refuses a wrong-foreground batch
    // before it is sent; with dispatch proven not started, that is a refusal.
    if target_act_error_code(error) == Some(error_codes::ACTION_FOREGROUND_LOST)
        && target_act_error_dispatch_not_started(error)
    {
        return TARGET_ACT_STATUS_REFUSED;
    }
    match target_act_error_code(error) {
        Some(
            error_codes::ACTION_NO_OBSERVED_DELTA
            | error_codes::ACTION_FOREGROUND_LOST
            | error_codes::ACTION_POSTCONDITION_FAILED
            | error_codes::CHROME_DOM_ACTION_POSTCONDITION_FAILED
            | error_codes::ACTION_VERIFY_SURFACE_UNAVAILABLE,
        ) => TARGET_ACT_STATUS_VERIFY_NEEDED,
        Some(
            error_codes::ACTION_ELEMENT_NOT_RESOLVED
            | error_codes::ACTION_ELEMENT_PATTERN_UNSUPPORTED
            | error_codes::ACTION_ELEMENT_VALUE_READ_ONLY
            | error_codes::ACTION_FOREGROUND_LEASE_BUSY
            | error_codes::ACTION_FOREGROUND_LEASE_NOT_HELD
            | error_codes::ACTION_INPUT_TARGET_REQUIRED
            | error_codes::ACTION_TARGET_INVALID
            | error_codes::A11Y_ELEMENT_STALE
            | error_codes::CHROME_DOM_ACTION_UNSUPPORTED
            | error_codes::CHROME_DOM_ELEMENT_AMBIGUOUS
            | error_codes::CHROME_DOM_ELEMENT_NOT_ACTIONABLE
            | error_codes::CHROME_DOM_ELEMENT_NOT_FOUND
            | error_codes::CHROME_DOM_SELECTOR_INVALID
            | error_codes::FOREGROUND_ACTIVATION_REFUSED
            | error_codes::TARGET_CO_OWNED
            | error_codes::TARGET_CLAIM_NOT_FOUND
            | error_codes::TARGET_NOT_SET
            | error_codes::TARGET_WINDOW_NOT_FOUND
            | error_codes::TOOL_PROFILE_POLICY_DENIED
            | error_codes::TOOL_PARAMS_INVALID
            | error_codes::TRANSIENT_ELEMENT_EXPIRED
            // #258: the user-activity gate refuses before any lease or input.
            | error_codes::USER_ACTIVE
            | error_codes::USER_ACTIVITY_UNKNOWN
            | error_codes::USER_WORKSTATION_LOCKED,
        ) => TARGET_ACT_STATUS_REFUSED,
        _ => TARGET_ACT_STATUS_ERROR,
    }
}

fn target_act_error_code(error: &ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

fn target_act_error_dispatch_not_started(error: &ErrorData) -> bool {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("dispatch_not_started"))
        .and_then(Value::as_bool)
        == Some(true)
}

fn target_act_error_has_delivered_tier(error: &ErrorData) -> bool {
    let Some(data) = error.data.as_ref() else {
        return false;
    };
    // #189: unknown delivery is reported as delivered-unverified, never as
    // failed-before-delivery, so the caller re-observes instead of retrying.
    data.get("delivered").and_then(Value::as_bool) == Some(true)
        || data.get("possibly_delivered").and_then(Value::as_bool) == Some(true)
        || data
            .get("tier_attempts")
            .and_then(Value::as_array)
            .is_some_and(|attempts| {
                attempts.iter().any(|attempt| {
                    attempt.get("status").and_then(Value::as_str) == Some("delivered")
                })
            })
}

#[cfg(test)]
#[path = "tests/related_window_response.rs"]
mod related_window_response_tests;

#[cfg(test)]
#[path = "tests/target_act_scroll_args_219.rs"]
mod target_act_scroll_args_219;

#[cfg(test)]
#[path = "tests/scroll_delivery_state_220.rs"]
mod scroll_delivery_state_220_tests;

#[cfg(test)]
#[path = "tests/dispatch_status_189.rs"]
pub(crate) mod dispatch_status_189_tests;

#[cfg(test)]
#[path = "tests/delivery_state_294.rs"]
mod delivery_state_294_tests;

#[cfg(test)]
#[path = "tests/foreground_deferred_9.rs"]
mod foreground_deferred_9_tests;

#[cfg(test)]
#[path = "tests/press_capture_route_96.rs"]
mod press_capture_route_96_tests;

#[cfg(test)]
#[path = "tests/raw_cdp_click_dpr_121.rs"]
mod raw_cdp_click_dpr_121_tests;

#[cfg(all(test, windows))]
#[path = "tests/scroll_dispatch_192_219_220.rs"]
mod scroll_dispatch_192_219_220;

#[cfg(test)]
#[path = "tests/scroll_args_doc_219.rs"]
mod scroll_args_doc_219;

#[cfg(test)]
#[path = "tests/refusal_text_guard_222.rs"]
mod refusal_text_guard_222;

#[cfg(test)]
#[path = "tests/delivery_variants_220.rs"]
mod delivery_variants_220;

#[cfg(test)]
#[path = "tests/field_and_bounds_contract_247.rs"]
mod field_and_bounds_contract_247;

#[cfg(test)]
#[path = "tests/pixel_delivery_evidence_248.rs"]
mod pixel_delivery_evidence_248;

#[cfg(test)]
#[path = "tests/act_user_active_gate_258.rs"]
mod act_user_active_gate_258;

#[cfg(test)]
#[path = "tests/owner_approval_scope_258.rs"]
mod owner_approval_scope_258;

#[cfg(test)]
#[path = "background_router_native_route_table.rs"]
mod native_route_table;

/// What a native UIA pattern action did to the OS foreground (#300 row 300g).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeForegroundOutcome {
    Unchanged,
    TakenByTarget { restore_to: Option<i64> },
    MovedElsewhere { now: Option<i64> },
}

impl NativeForegroundOutcome {
    /// The window Synapse must put back in front: only when the target itself took the
    /// foreground from a real prior window. A human move is never undone.
    pub(crate) const fn restore_target(self) -> Option<i64> {
        match self {
            Self::TakenByTarget { restore_to } => restore_to,
            Self::Unchanged | Self::MovedElsewhere { .. } => None,
        }
    }

    /// The reply may say the foreground was untouched only when it never moved.
    pub(crate) const fn foreground_untouched(self) -> bool {
        matches!(self, Self::Unchanged)
    }

    /// Stable wire name for the `foreground_outcome` result field.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::TakenByTarget { .. } => "taken_by_target",
            Self::MovedElsewhere { .. } => "moved_elsewhere",
        }
    }
}

/// Classifies what a native UIA pattern action did to the OS foreground, from the
/// foreground ROOT window before and after the call and the target's root window.
pub(crate) const fn native_pattern_foreground_outcome(
    before_root: Option<i64>,
    after_root: Option<i64>,
    target_root: i64,
) -> NativeForegroundOutcome {
    let unchanged = match (before_root, after_root) {
        (Some(before), Some(after)) => before == after,
        (None, None) => true,
        _ => false,
    };
    if unchanged {
        return NativeForegroundOutcome::Unchanged;
    }
    match after_root {
        Some(after) if after == target_root => NativeForegroundOutcome::TakenByTarget {
            restore_to: before_root,
        },
        now => NativeForegroundOutcome::MovedElsewhere { now },
    }
}

/// #316: the error a background click reports for what it did to the OS foreground.
/// `restored` = None when no restore was attempted, Some(true/false) = restore result.
pub(crate) const fn background_click_foreground_error(
    outcome: NativeForegroundOutcome,
    restored: Option<bool>,
) -> Option<&'static str> {
    if outcome.restore_target().is_some() && !matches!(restored, Some(true)) {
        Some(error_codes::ACTION_FOREGROUND_LOST)
    } else {
        None
    }
}

/// #318: whether the background foreground guard (before/after read, settle,
/// restore) runs for an action reply. `reply_required_foreground` is
/// `Some(flag)` for a successful reply and `None` when the action itself failed.
pub(crate) const fn background_guard_applies(reply_required_foreground: Option<bool>) -> bool {
    !matches!(reply_required_foreground, Some(true))
}

/// #318: whether a failed read of the foreground BEFORE a background action
/// blocks the action. `code` is the read error's code.
pub(crate) const fn foreground_read_blocks_background_action(code: &str) -> bool {
    const NO_FOREGROUND: &[u8] = error_codes::A11Y_NO_FOREGROUND.as_bytes();
    !matches!(code.as_bytes(), NO_FOREGROUND)
}

/// #318c: env override for the background foreground guard's settle wait.
pub(crate) const BACKGROUND_FOREGROUND_SETTLE_ENV: &str = "SYNAPSE_BACKGROUND_FOREGROUND_SETTLE_MS";
/// #318c: default settle wait before the guard's "after" read of a posted action.
/// A window that reacts to a posted message by activating itself does so on its
/// own message loop, after the post returns: the 318c fixture polls every 50 ms,
/// so 150 ms covers one full poll plus 100 ms for its SetForegroundWindow.
pub(crate) const BACKGROUND_FOREGROUND_SETTLE_DEFAULT_MS: u64 = 150;
/// #318c: the settle wait is capped so a bad override cannot stall every action.
pub(crate) const BACKGROUND_FOREGROUND_SETTLE_MAX_MS: u64 = 1_000;
/// #318c: the backend tier whose delivery is asynchronous (a posted window message).
const POSTED_BACKEND_TIER: &str = "postmessage";

/// #318c: the configured settle wait from the raw env value: missing or not a
/// whole number = default, larger than the cap = the cap, 0 = disabled.
pub(crate) fn background_foreground_settle_config_ms(raw: Option<&str>) -> u64 {
    raw.and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(BACKGROUND_FOREGROUND_SETTLE_DEFAULT_MS)
        .min(BACKGROUND_FOREGROUND_SETTLE_MAX_MS)
}

/// #318c: how long the background guard waits before its "after" foreground read.
/// Only a posted action (`backend_tier_used` = postmessage) can make the target
/// take the foreground after the call returned; synchronous tiers and failed
/// actions (`None`) are read at once.
pub(crate) fn background_foreground_settle_ms(
    backend_tier: Option<&str>,
    configured_ms: u64,
) -> u64 {
    if backend_tier == Some(POSTED_BACKEND_TIER) {
        configured_ms.min(BACKGROUND_FOREGROUND_SETTLE_MAX_MS)
    } else {
        0
    }
}

/// #318c: the window the background guard watches for a scroll at a screen point.
/// A wheel posted at a point inside a claimed window (no element target) is a
/// background action on that window; the element-target route has its own guard
/// and a cursor-route scroll (no point) runs in the foreground under the lease.
pub(crate) const fn posted_scroll_guard_window(
    element_target: bool,
    at_point: bool,
    checked_window_hwnd: Option<i64>,
) -> Option<i64> {
    if !element_target && at_point {
        checked_window_hwnd
    } else {
        None
    }
}

#[cfg(test)]
#[path = "background_router_guard_table_318.rs"]
mod guard_table_318;

#[cfg(test)]
#[path = "background_router_guard_table_318c.rs"]
mod guard_table_318c;

#[cfg(test)]
#[path = "background_router_click_foreground_table_316.rs"]
mod click_foreground_table_316;

#[cfg(test)]
#[path = "background_router_scroll_session_table_317.rs"]
mod scroll_session_table_317;

#[cfg(test)]
#[path = "background_router_native_foreground_table.rs"]
mod native_foreground_table;
