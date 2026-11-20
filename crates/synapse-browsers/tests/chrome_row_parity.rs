//! MY TDD parity contract: the shipped `browsers/chrome.json` row reproduces
//! every legacy Chrome wire id byte-for-byte, so persisted session rows, the
//! health key read by `synapse-setup.ps1`, the token file and the policy root
//! stay identical after the multi-browser refactor.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use synapse_browsers::{BridgeLane, BrowserFamily, BrowserRegistry, OriginRule, ProfileLayout};

const LEGACY_EXTENSION_ID: &str = "leoocgnkjnplbfdbklajepahofecgfbk";
const LEGACY_EXTENSION_ORIGIN: &str = "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk";
const LEGACY_TABS_ENDPOINT: &str =
    "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/chrome.tabs";
const LEGACY_WEB_NAVIGATION_ENDPOINT: &str =
    "chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/chrome.webNavigation";

#[test]
fn must_pass_chrome_row_preserves_legacy_wire_ids() {
    let registry = BrowserRegistry::embedded_baseline();
    let chrome = registry.legacy_default().unwrap();

    assert_eq!(chrome.id.as_str(), "chrome");
    assert_eq!(chrome.family, BrowserFamily::Chromium);
    assert_eq!(chrome.profile_layout, ProfileLayout::Multi);
    assert_eq!(chrome.tab_target_prefix, "chrome-tab:");
    assert_eq!(chrome.tab_target_wire("42"), "chrome-tab:42");
    assert_eq!(chrome.endpoint(BridgeLane::Tabs), LEGACY_TABS_ENDPOINT);
    assert_eq!(
        chrome.endpoint(BridgeLane::WebNavigation),
        LEGACY_WEB_NAVIGATION_ENDPOINT
    );
    assert_eq!(chrome.health_key, "chrome_bridge");
    assert_eq!(chrome.registration_token_file, "chrome-bridge-token.txt");
    assert_eq!(chrome.extension.id, LEGACY_EXTENSION_ID);
    assert_eq!(chrome.extension_origin(), LEGACY_EXTENSION_ORIGIN);
    assert_eq!(
        chrome.chromium_origin_rule(),
        OriginRule::Exact(LEGACY_EXTENSION_ORIGIN.to_owned())
    );
    let policy = chrome.policy.as_ref().unwrap();
    assert_eq!(policy.root, "Software\\Policies\\Google\\Chrome");
    assert_eq!(policy.extension_settings_value, "ExtensionSettings");
    assert_eq!(
        chrome.user_data_root,
        "%LOCALAPPDATA%\\Google\\Chrome\\User Data"
    );
    assert_eq!(chrome.extension.extensions_page_url, "chrome://extensions");
    assert_eq!(chrome.extension.deploy_subdir, "chrome-extension");
    assert_eq!(chrome.process_names, vec!["chrome.exe".to_owned()]);
    assert!(chrome.unsupported_capabilities.is_empty());
    assert!(chrome.debugger_api_expected);
    assert_eq!(
        chrome.strip_window_title_suffix("Inbox - Google Chrome"),
        "Inbox"
    );
}

#[test]
fn must_pass_chrome_row_keeps_seat_candidates_and_cdp_tokens() {
    let registry = BrowserRegistry::embedded_baseline();
    let chrome = registry.legacy_default().unwrap();
    assert_eq!(
        chrome.exe_candidates,
        vec![
            "%ProgramFiles%\\Google\\Chrome\\Application\\chrome.exe".to_owned(),
            "%LOCALAPPDATA%\\Google\\Chrome\\Application\\chrome.exe".to_owned(),
        ]
    );
    for product in [
        "Chrome/155.0.8059.40",
        "HeadlessChrome/155.0",
        "Chromium/120",
        "Edg/120",
    ] {
        assert!(registry.cdp_product_is_chromium(product), "{product}");
    }
    assert!(!registry.cdp_product_is_chromium("Firefox/158.0"));
}
