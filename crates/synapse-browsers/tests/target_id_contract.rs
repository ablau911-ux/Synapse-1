//! MY TDD contract for bridge tab / element target ids. The chrome wire form
//! (`chrome-tab:<id>`) never changes; every other kind gets its own prefix and
//! an unknown `<kind>-tab:` prefix fails closed instead of guessing a browser.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use synapse_browsers::{BridgeElementTarget, BridgeTabTarget, BrowserRegistry};
use synapse_core::error_codes;

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
fn must_pass_tab_target_roundtrip() {
    let registry = two_kind_registry();
    for (raw, kind, tab_id) in [
        ("chrome-tab:42", "chrome", 42_u64),
        ("testium-tab:7", "testium", 7),
        ("  chrome-tab:0  ", "chrome", 0),
    ] {
        let target = BridgeTabTarget::parse(&registry, raw).unwrap().unwrap();
        assert_eq!(target.kind().as_str(), kind, "{raw}");
        assert_eq!(target.tab_id(), tab_id, "{raw}");
        assert_eq!(target.wire(), raw.trim(), "{raw}");
        let again = BridgeTabTarget::parse(&registry, target.wire())
            .unwrap()
            .unwrap();
        assert_eq!(again, target);
    }
    assert!(registry.is_bridge_tab_target("chrome-tab:5"));
    assert!(!registry.is_bridge_tab_target("chrome-tab:"));
    assert!(!registry.is_bridge_tab_target("chrome-tab:5a"));
}

#[test]
fn must_pass_non_bridge_ids_are_not_bridge_targets() {
    let registry = two_kind_registry();
    for raw in ["8D5C2D0B9E5A4C1B8F0E3A2B1C0D9E8F", "page-1", "", "chrome:1"] {
        assert!(
            BridgeTabTarget::parse(&registry, raw).unwrap().is_none(),
            "{raw:?} is not a bridge id"
        );
    }
}

#[test]
fn must_fail_tab_target_unknown_prefix() {
    let registry = two_kind_registry();
    let error = BridgeTabTarget::parse(&registry, "foo-tab:1").unwrap_err();
    assert_eq!(error.code, error_codes::BROWSER_BRIDGE_KIND_UNRESOLVED);
    assert!(error.detail.contains("foo-tab:1"), "{}", error.detail);
}

#[test]
fn must_fail_tab_target_malformed_tab_id() {
    let registry = two_kind_registry();
    for raw in [
        "chrome-tab:",
        "chrome-tab:x1",
        "testium-tab:-3",
        "chrome-tab:99999999999999999999999",
    ] {
        let error = BridgeTabTarget::parse(&registry, raw).unwrap_err();
        assert_eq!(error.code, error_codes::TOOL_PARAMS_INVALID, "{raw}");
    }
}

#[test]
fn must_pass_element_target_with_shadow_hop() {
    let registry = two_kind_registry();
    let element = BridgeElementTarget::parse(&registry, "chrome-tab:3:frame:0:path:0.1.s.0")
        .unwrap()
        .unwrap();
    assert_eq!(element.tab().wire(), "chrome-tab:3");
    assert_eq!(element.frame_id(), 0);
    assert_eq!(element.path(), "0.1.s.0");

    let other = BridgeElementTarget::parse(&registry, "testium-tab:9:frame:2:path:4")
        .unwrap()
        .unwrap();
    assert_eq!(other.tab().kind().as_str(), "testium");
    assert_eq!(
        registry.tab_target_of_element_id("testium-tab:9:frame:2:path:4"),
        Some("testium-tab:9".to_owned())
    );
}

#[test]
fn must_fail_element_target_invalid_components() {
    let registry = two_kind_registry();
    for raw in [
        "chrome-tab:3",
        "chrome-tab:3:frame:0",
        "chrome-tab:3:frame:x:path:0",
        "chrome-tab:3:frame:0:path:",
        "chrome-tab:3:frame:0:path:0..1",
        "chrome-tab:3:frame:0:path:0.t.1",
    ] {
        let error = BridgeElementTarget::parse(&registry, raw).unwrap_err();
        assert_eq!(error.code, error_codes::TOOL_PARAMS_INVALID, "{raw}");
    }
    assert!(
        BridgeElementTarget::parse(&registry, "0x1234")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        BridgeElementTarget::parse(&registry, "zzz-tab:1:frame:0:path:0")
            .unwrap_err()
            .code,
        error_codes::BROWSER_BRIDGE_KIND_UNRESOLVED
    );
}
