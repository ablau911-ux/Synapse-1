//! Browser route resolution for normal-bridge commands (multi-browser bridge).
//!
//! Every bridge command payload carries `hwnd` and usually `targetIdHint` /
//! `elementId`, so the bridge resolves the browser kind from the command
//! itself and no public tool schema or call site has to change:
//!
//! 1. the window's process image (and path, for rows sharing an image) names a
//!    declared row -> that kind; a target id of another kind is refused;
//! 2. else a bridge target/element id prefix (`edge-tab:`) names the kind;
//! 3. else a window that is positively NOT a bridge browser routes to the only
//!    active host's kind, and is refused when several kinds are active;
//! 4. else (no window, unreadable process) -> the legacy default kind, which
//!    is exactly the pre-multi-browser behaviour.
//!
//! Unknown `<kind>-tab:` prefixes and ambiguous rows fail closed with
//! `BROWSER_BRIDGE_KIND_UNRESOLVED`; nothing is ever routed by guess.

use serde_json::Value;
use synapse_browsers::{BridgeTabTarget, BrowserBridgeError, BrowserKindId, BrowserRegistry};

/// Payload keys that carry a bridge tab or element id.
const TARGET_KEYS: [&str; 2] = ["targetIdHint", "elementId"];
const HWND_KEY: &str = "hwnd";

/// Process identity of a window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WindowProcess {
    pub(super) name: String,
    pub(super) path: Option<String>,
}

/// Window -> process lookup (live on Windows; injectable in tests).
pub(super) trait WindowProcessProbe {
    fn process_of(&self, hwnd: i64) -> Option<WindowProcess>;
}

/// Live probe through `synapse_a11y::foreground_context`.
pub(super) struct LiveWindowProcessProbe;

impl WindowProcessProbe for LiveWindowProcessProbe {
    #[cfg(windows)]
    fn process_of(&self, hwnd: i64) -> Option<WindowProcess> {
        let context = synapse_a11y::foreground_context(hwnd).ok()?;
        let path = (!context.process_path.trim().is_empty()).then_some(context.process_path);
        // `foreground_context` reports `pid-<n>` when the image is unreadable.
        if path.is_none() && context.process_name.starts_with("pid-") {
            return None;
        }
        Some(WindowProcess {
            name: context.process_name,
            path,
        })
    }

    #[cfg(not(windows))]
    fn process_of(&self, _hwnd: i64) -> Option<WindowProcess> {
        None
    }
}

/// A resolved route and the rule that chose it (readback for logs/details).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BrowserRoute {
    pub(super) kind: BrowserKindId,
    pub(super) source: &'static str,
}

impl BrowserRoute {
    fn new(kind: BrowserKindId, source: &'static str) -> Self {
        Self { kind, source }
    }
}

enum WindowKind {
    Known(BrowserKindId),
    NotABridgeBrowser(String),
    Unknown,
}

/// Resolves the browser kind for one command payload.
///
/// # Errors
/// `BROWSER_BRIDGE_KIND_UNRESOLVED` (unknown prefix, ambiguous rows, window
/// and target of different kinds, non-browser window with several active
/// kinds) or `TOOL_PARAMS_INVALID` (malformed bridge id).
pub(super) fn resolve_command_route(
    registry: &BrowserRegistry,
    probe: &dyn WindowProcessProbe,
    active_kinds: &[BrowserKindId],
    params: &Value,
) -> Result<BrowserRoute, BrowserBridgeError> {
    let target_kind = target_kind_from_params(registry, params)?;
    let window_kind = window_kind_from_params(registry, probe, params)?;
    match (target_kind, window_kind) {
        (Some(target), WindowKind::Known(window)) if target != window => {
            Err(BrowserBridgeError::kind_unresolved(format!(
                "bridge target belongs to browser_kind={target} but the window process belongs to browser_kind={window}; refusing a cross-browser command"
            )))
        }
        (_, WindowKind::Known(window)) => Ok(BrowserRoute::new(window, "window_process")),
        (Some(target), _) => Ok(BrowserRoute::new(target, "target_prefix")),
        (None, WindowKind::NotABridgeBrowser(process)) => {
            route_for_non_browser_window(registry, active_kinds, &process)
        }
        (None, WindowKind::Unknown) => legacy_route(registry, "legacy_default_no_window_kind"),
    }
}

fn route_for_non_browser_window(
    registry: &BrowserRegistry,
    active_kinds: &[BrowserKindId],
    process: &str,
) -> Result<BrowserRoute, BrowserBridgeError> {
    match active_kinds {
        [] => legacy_route(registry, "legacy_default_no_active_host"),
        [only] => Ok(BrowserRoute::new(
            only.clone(),
            "single_active_host_for_non_browser_window",
        )),
        several => Err(BrowserBridgeError::kind_unresolved(format!(
            "window process {process:?} is not a declared bridge browser and {} browser kinds have active hosts ({}); pass the browser window_hwnd or a <kind>-tab: target",
            several.len(),
            several
                .iter()
                .map(BrowserKindId::as_str)
                .collect::<Vec<_>>()
                .join(",")
        ))),
    }
}

fn legacy_route(
    registry: &BrowserRegistry,
    source: &'static str,
) -> Result<BrowserRoute, BrowserBridgeError> {
    registry
        .legacy_default()
        .map(|row| BrowserRoute::new(row.id.clone(), source))
        .ok_or_else(|| {
            BrowserBridgeError::kind_unresolved(format!(
                "no window/target names a browser and the legacy default row is missing from the registry origin={}",
                registry.origin()
            ))
        })
}

fn target_kind_from_params(
    registry: &BrowserRegistry,
    params: &Value,
) -> Result<Option<BrowserKindId>, BrowserBridgeError> {
    let mut found: Option<BrowserKindId> = None;
    for key in TARGET_KEYS {
        let Some(raw) = params.get(key).and_then(Value::as_str) else {
            continue;
        };
        let Some(kind) = kind_of_bridge_id(registry, raw)? else {
            continue;
        };
        if let Some(previous) = &found
            && *previous != kind
        {
            return Err(BrowserBridgeError::kind_unresolved(format!(
                "command names bridge ids of two browser kinds ({previous} and {kind})"
            )));
        }
        found = Some(kind);
    }
    Ok(found)
}

/// Kind of a tab id or element id by its declared prefix (the extension still
/// validates the rest, as before); `Ok(None)` for non-bridge ids.
///
/// # Errors
/// `BROWSER_BRIDGE_KIND_UNRESOLVED` for an undeclared `<kind>-tab:` prefix.
pub(super) fn kind_of_bridge_id(
    registry: &BrowserRegistry,
    raw: &str,
) -> Result<Option<BrowserKindId>, BrowserBridgeError> {
    if let Some(row) = registry.by_tab_target(raw) {
        return Ok(Some(row.id.clone()));
    }
    BridgeTabTarget::parse(registry, raw).map(|_| None)
}

fn window_kind_from_params(
    registry: &BrowserRegistry,
    probe: &dyn WindowProcessProbe,
    params: &Value,
) -> Result<WindowKind, BrowserBridgeError> {
    let Some(hwnd) = params
        .get(HWND_KEY)
        .and_then(Value::as_i64)
        .filter(|hwnd| *hwnd != 0)
    else {
        return Ok(WindowKind::Unknown);
    };
    let Some(process) = probe.process_of(hwnd) else {
        return Ok(WindowKind::Unknown);
    };
    Ok(
        match registry.by_process(&process.name, process.path.as_deref())? {
            Some(row) => WindowKind::Known(row.id.clone()),
            None => WindowKind::NotABridgeBrowser(process.name),
        },
    )
}
