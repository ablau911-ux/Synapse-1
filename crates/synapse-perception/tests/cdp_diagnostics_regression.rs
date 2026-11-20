//! Regression test for the CDP diagnostics contract that epic #682 depends on
//! (issue #691). This is *supporting evidence* per AGENTS.md D1 — NOT an FSV
//! harness, and deliberately not named `*_fsv`. Manual FSV against a real Chrome
//! remains the shipping gate; this guards the silent-fallthrough regression
//! (#683) from recurring at the `assemble()` boundary where `observe`/`find`
//! build their diagnostics.
//!
//! What it locks:
//! * Chromium foreground, no debug port →
//!   `diagnostics.cdp.status = A11Y_CDP_UNREACHABLE` with matching
//!   `reason_code`, and a named `web_path` (never a silent absent field).
//! * Chromium foreground, CDP attached → `diagnostics.cdp.status = ok` and DOM
//!   elements present in the observation.

use std::collections::BTreeMap;

use synapse_core::{
    AccessibleNode, CdpCapability, CdpDiagnostics, CdpStatus, ForegroundContext, Rect,
    SensorStatus, WebPerceptionPath, element_id, error_codes,
};
use synapse_perception::{ObservationInput, ObserveInclude, assemble};

fn chromium_foreground() -> ForegroundContext {
    ForegroundContext {
        hwnd: 0x2200,
        pid: 7777,
        process_name: "chrome.exe".to_owned(),
        process_path: "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe".to_owned(),
        window_title: "Apply to YC | Y Combinator - Google Chrome".to_owned(),
        window_bounds: Rect {
            x: 0,
            y: 0,
            w: 1600,
            h: 900,
        },
        monitor_index: 0,
        dpi_scale: 1.0,
        profile_id: Some("chrome".to_owned()),
        steam_appid: None,
        is_fullscreen: false,
        is_dwm_composed: true,
    }
}

/// The collapsed UIA-only tree a normally-launched Chrome yields: window → pane
/// → region, with zero document/link/button nodes. This is exactly the trap the
/// epic documents.
fn collapsed_uia_nodes() -> Vec<AccessibleNode> {
    ["Window", "Pane", "Region"]
        .iter()
        .enumerate()
        .map(|(index, role)| AccessibleNode {
            element_id: element_id(0x2200, &format!("0000220000{index:06x}")),
            parent: (index > 0).then(|| element_id(0x2200, "0000220000000000")),
            name: String::new(),
            role: (*role).to_owned(),
            automation_id: None,
            value: None,
            bbox: Rect {
                x: 0,
                y: 0,
                w: 1600,
                h: 900,
            },
            enabled: true,
            focused: index == 0,
            patterns: Vec::new(),
            children_count: u32::from(index < 2),
            depth: u32::try_from(index).unwrap_or(0),
            control_state: None,
        })
        .collect()
}

fn base_input() -> ObservationInput {
    let mut input = ObservationInput::new(chromium_foreground());
    input.elements = collapsed_uia_nodes();
    input.a11y_status = SensorStatus::Healthy;
    input.capture_status = SensorStatus::Healthy;
    input.sensor_latency_ms = BTreeMap::from([("a11y".to_owned(), 1.0)]);
    input
}

#[test]
fn must_fail_no_debug_port_surfaces_unreachable_status_and_named_web_path() {
    // `assemble` is not a pure pass-through for cdp: in the lean shape
    // (`diagnostics: false`) it drops cdp diagnostics unless the probe failed.
    // The #683 trap is exactly the lean, interactable-only observe of a
    // no-port Chrome: the collapsed UIA tree yields zero targetable elements,
    // so the failed CDP diagnosis is the only signal and must survive.
    let lean = ObserveInclude {
        diagnostics: false,
        interactable_only: true,
        ..ObserveInclude::default()
    };
    for (edge, include) in [("full", ObserveInclude::default()), ("lean", lean)] {
        let mut input = base_input();
        // Outcome the synchronous probe produces for a no-port Chrome.
        input.cdp = Some(CdpDiagnostics::unreachable(
            "chrome.exe",
            error_codes::A11Y_CDP_UNREACHABLE,
        ));
        input.web_path = Some(WebPerceptionPath::UiaOnly);

        println!(
            "readback=cdp_contract edge=no_debug_port/{edge} before=elements:{} cdp:{:?} web_path:{:?}",
            input.elements.len(),
            input.cdp.as_ref().map(|cdp| cdp.status),
            input.web_path,
        );

        let observation = assemble(include, input).expect("assemble succeeds");
        let cdp = observation.diagnostics.cdp.as_ref().unwrap_or_else(|| {
            panic!("{edge}: failed CDP must survive assemble — no silent fallthrough")
        });

        println!(
            "readback=cdp_contract edge=no_debug_port/{edge} after=cdp.status:{:?} reason:{:?} web_path:{:?} elements:{}",
            cdp.status,
            cdp.reason_code,
            observation.diagnostics.web_path,
            observation.elements.len()
        );

        assert_eq!(cdp.status, CdpStatus::Unreachable, "{edge}");
        assert_eq!(
            cdp.reason_code.as_deref(),
            Some(error_codes::A11Y_CDP_UNREACHABLE),
            "{edge}"
        );
        assert_eq!(
            observation.diagnostics.web_path,
            Some(WebPerceptionPath::UiaOnly),
            "{edge}: no-port Chromium must name its web_path, not leave it absent"
        );
        if edge == "lean" {
            assert!(
                observation.elements.is_empty(),
                "collapsed UIA tree has no targetable control: {:?}",
                observation.elements
            );
        } else {
            assert_eq!(observation.elements.len(), 3, "full shape keeps raw nodes");
        }
    }
}

/// Derived (not passed-through) outcome of `assemble` for an attached CDP
/// session in the lean interactable-only shape: the healthy cdp block is
/// dropped (only a failed diagnosis is kept lean), the web path is still
/// named, and the interactable filter keeps exactly the DOM button while the
/// anonymous structural UIA nodes are removed.
#[test]
fn must_pass_attached_cdp_lean_observe_keeps_only_dom_control_and_drops_ok_cdp() {
    let mut input = base_input();
    input.elements.push(AccessibleNode {
        element_id: element_id(0x2200, "00002201cdp00001"),
        parent: Some(element_id(0x2200, "0000220000000000")),
        name: "Apply".to_owned(),
        role: "button".to_owned(),
        automation_id: Some("cdp:backendNodeId=42".to_owned()),
        value: None,
        bbox: Rect {
            x: 720,
            y: 480,
            w: 120,
            h: 40,
        },
        enabled: true,
        focused: false,
        patterns: Vec::new(),
        children_count: 0,
        depth: 2,
        control_state: None,
    });
    input.cdp = Some(CdpDiagnostics {
        process_name: "chrome.exe".to_owned(),
        status: CdpStatus::Ok,
        endpoint: Some("http://127.0.0.1:9222".to_owned()),
        checked_ports: vec![9222],
        checked_endpoints: vec!["http://127.0.0.1:9222".to_owned()],
        reason_code: None,
        detail: None,
        capabilities: vec![CdpCapability::AccessibilityFullAxTree],
        attached_node_count: Some(1),
        selected_target_id: Some("target-1".to_owned()),
        selected_session_id: Some("session-1".to_owned()),
        target_selection_reason: Some("url_hint".to_owned()),
        target_candidate_count: Some(1),
        frame_tree_frame_count: Some(1),
        attached_frame_target_count: Some(0),
        blocked_frame_targets: Vec::new(),
        frame_snapshot_errors: Vec::new(),
    });
    input.web_path = Some(WebPerceptionPath::Cdp);
    let include = ObserveInclude {
        diagnostics: false,
        interactable_only: true,
        ..ObserveInclude::default()
    };

    let full = assemble(ObserveInclude::default(), input.clone()).expect("assemble succeeds");
    assert_eq!(
        full.diagnostics.cdp.as_ref().map(|cdp| cdp.status),
        Some(CdpStatus::Ok),
        "requested diagnostics keep the healthy cdp block"
    );
    assert_eq!(
        full.elements.len(),
        4,
        "full shape keeps 3 UIA nodes + DOM button"
    );

    let observation = assemble(include, input).expect("assemble succeeds");
    let names: Vec<(&str, &str)> = observation
        .elements
        .iter()
        .map(|node| (node.role.as_str(), node.name.as_str()))
        .collect();
    println!(
        "readback=cdp_contract edge=attached_lean after=cdp:{:?} web_path:{:?} elements:{names:?}",
        observation.diagnostics.cdp.as_ref().map(|cdp| cdp.status),
        observation.diagnostics.web_path
    );

    assert_eq!(names, vec![("button", "Apply")]);
    assert!(
        observation.diagnostics.cdp.is_none(),
        "healthy cdp diagnostics must be dropped when diagnostics were not requested"
    );
    assert_eq!(
        observation.diagnostics.web_path,
        Some(WebPerceptionPath::Cdp)
    );
}
