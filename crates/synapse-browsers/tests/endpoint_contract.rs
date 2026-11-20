//! MY TDD contract for bridge endpoints. Chrome keeps its legacy
//! `chrome-extension://<id>/<lane>` wire form (persisted owner rows stay
//! valid); other kinds use their row's `endpoint_base`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use synapse_browsers::{BridgeEndpoint, BridgeLane, BrowserKindId, BrowserRegistry};

fn two_kind_registry() -> BrowserRegistry {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let chrome = std::fs::read_to_string(root.join("browsers").join("chrome.json")).unwrap();
    let testium =
        std::fs::read_to_string(root.join("tests").join("fixtures").join("testium.json")).unwrap();
    BrowserRegistry::from_json_sources(&[
        ("chrome.json", chrome.as_str()),
        ("testium.json", testium.as_str()),
    ])
    .unwrap()
}

#[test]
fn must_pass_endpoint_roundtrip_per_kind() {
    let registry = two_kind_registry();
    for (kind, lane, wire) in [
        (
            "chrome",
            BridgeLane::Tabs,
            "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/chrome.tabs",
        ),
        (
            "chrome",
            BridgeLane::Debugger,
            "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/chrome.debugger",
        ),
        (
            "testium",
            BridgeLane::Tabs,
            "synapse-bridge://testium/chrome.tabs",
        ),
    ] {
        let endpoint = BridgeEndpoint::new(BrowserKindId::new(kind).unwrap(), lane);
        assert_eq!(endpoint.to_wire(&registry).as_deref(), Some(wire));
        assert_eq!(BridgeEndpoint::parse(&registry, wire), Some(endpoint));
    }
}

#[test]
fn must_pass_bridge_tab_lane_endpoint_replaces_both_legacy_helpers() {
    let registry = two_kind_registry();
    assert!(registry.is_bridge_tab_lane_endpoint(
        "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/chrome.tabs"
    ));
    assert!(registry.is_bridge_tab_lane_endpoint("synapse-bridge://testium/chrome.debugger"));
    // Legacy compat: the old helpers accepted any chrome-extension id.
    assert!(registry.is_bridge_tab_lane_endpoint(
        "chrome-extension://aaaabbbbccccddddeeeeffffgggghhhh/chrome.tabs"
    ));
    for raw in [
        "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/chrome.webNavigation",
        "http://127.0.0.1:9222",
        "ws://127.0.0.1:9222/devtools/browser/x",
        "synapse-bridge://unknown/chrome.tabs",
        "",
    ] {
        assert!(!registry.is_bridge_tab_lane_endpoint(raw), "{raw}");
    }
}

#[test]
fn must_pass_endpoint_kind_resolves_owner_rows() {
    let registry = two_kind_registry();
    assert_eq!(
        registry
            .endpoint_kind("synapse-bridge://testium/chrome.tabs")
            .map(|row| row.id.as_str()),
        Some("testium")
    );
    assert_eq!(
        registry
            .endpoint_kind(
                "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/chrome.webNavigation"
            )
            .map(|row| row.id.as_str()),
        Some("chrome")
    );
    assert!(registry.endpoint_kind("http://127.0.0.1:9222").is_none());
}

#[test]
fn must_pass_endpoint_for_target_keeps_legacy_reported_id_readback() {
    let registry = two_kind_registry();
    assert_eq!(
        registry.bridge_endpoint_for_target("chrome-tab:4", BridgeLane::WebNavigation),
        "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/chrome.webNavigation"
    );
    assert_eq!(
        registry.bridge_endpoint_for_target("testium-tab:4", BridgeLane::Tabs),
        "synapse-bridge://testium/chrome.tabs"
    );
    // Chrome's endpoint is keyed by the reported extension id (old readback).
    assert_eq!(
        registry.bridge_endpoint_for_reported_target(
            "chrome-tab:4",
            Some("aaaabbbbccccddddeeeeffffgggghhhh"),
            BridgeLane::Tabs
        ),
        "chrome-extension://aaaabbbbccccddddeeeeffffgggghhhh/chrome.tabs"
    );
    // Other kinds keep their row endpoint whatever id the worker reports.
    assert_eq!(
        registry.bridge_endpoint_for_reported_target(
            "testium-tab:4",
            Some("leoocgnkjnplbfdbklajepahofecgfbk"),
            BridgeLane::Tabs
        ),
        "synapse-bridge://testium/chrome.tabs"
    );
    // Non-bridge or empty input falls back to the legacy row.
    assert_eq!(
        registry.bridge_endpoint_for_reported_target("", Some(" "), BridgeLane::Tabs),
        "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/chrome.tabs"
    );
}
