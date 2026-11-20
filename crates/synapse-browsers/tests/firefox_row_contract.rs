//! MY TDD contract for the Firefox (gecko) browser row (`browsers/firefox.json`).
//! must_pass_*: the row loads beside the other rows, routes `firefox.exe`,
//! `firefox-tab:` ids and its endpoint to the firefox kind, and keeps every
//! WebExtension-capable command routable. must_fail_*: every command that
//! needs `chrome.debugger` (absent in Firefox) is refused per command with
//! `BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED`; a missing Firefox is not installed.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use synapse_browsers::{
    BridgeElementTarget, BridgeEndpoint, BridgeLane, BridgeTabTarget, BrowserBridgeError,
    BrowserExecutableLocator, BrowserFamily, BrowserRegistry, BrowserRow, NoRegistry,
    ProfileLayout, RegistryHive, RegistryReader,
};
use synapse_core::error_codes;

/// Commands whose shared-worker handler reaches `chrome.debugger` (CDP), plus
/// the Chrome-only `chrome.management` popup suppression. Derived from
/// `extensions/synapse-chrome-debugger/service_worker.js` handleCommand.
const CDP_ONLY_COMMANDS: [&str; 15] = [
    "evaluateScript",
    "initScript",
    "exposeBinding",
    "handleDialog",
    "fileUpload",
    "cdpInput",
    "typeActiveElement",
    "viewportEmulation",
    "deviceEmulation",
    "geolocationEmulation",
    "localeEmulation",
    "mediaEmulation",
    "networkConditions",
    "pagePdf",
    "externalPopupRiskSuppression",
];

/// Commands served by WebExtension APIs Firefox implements (tabs, scripting,
/// webNavigation, cookies, downloads, storage, captureVisibleTab, alarms).
const WEBEXTENSION_COMMANDS: [&str; 10] = [
    "listTabs",
    "openTab",
    "closeTab",
    "activateTab",
    "navigateTab",
    "pageContent",
    "pageScreenshot",
    "storageState",
    "downloads",
    "setFieldValue",
];

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn row_text(id: &str) -> String {
    std::fs::read_to_string(manifest_dir().join("browsers").join(format!("{id}.json"))).unwrap()
}

fn chrome_and_firefox() -> BrowserRegistry {
    let chrome = row_text("chrome");
    let firefox = row_text("firefox");
    BrowserRegistry::from_json_sources(&[
        ("chrome.json", chrome.as_str()),
        ("firefox.json", firefox.as_str()),
    ])
    .unwrap()
}

fn firefox(registry: &BrowserRegistry) -> &BrowserRow {
    registry.by_id("firefox").unwrap()
}

/// The capability list the shared worker's Chromium adapter advertises (the
/// global set the daemon requires), read from the adapter source.
fn chromium_adapter_capabilities() -> Vec<String> {
    let source = std::fs::read_to_string(
        manifest_dir()
            .join("..")
            .join("..")
            .join("extensions")
            .join("synapse-chrome-debugger")
            .join("adapters")
            .join("chromium.js"),
    )
    .unwrap();
    let start = source.find("capabilities: Object.freeze([").unwrap();
    let body = &source[start..];
    let end = body.find("])").unwrap();
    body[..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

#[test]
fn must_pass_firefox_row_loads_with_unique_ids() {
    let registry = chrome_and_firefox();
    let row = firefox(&registry);
    assert_eq!(row.id.as_str(), "firefox");
    assert_eq!(row.tab_target_prefix, "firefox-tab:");
    assert_eq!(row.health_key, "firefox_bridge");
    assert_eq!(row.endpoint_base, "synapse-bridge://firefox");
    assert_eq!(
        row.registration_token_file,
        "browser-bridge-token-firefox.txt"
    );
    assert_eq!(row.extension.id, "synapse-bridge@synapse.local");
    assert_eq!(row.extension.manifest_dir, "synapse-firefox-bridge");
    assert_eq!(row.extension.deploy_subdir, "browser-extension\\firefox");
    assert_eq!(registry.legacy_default().unwrap().id.as_str(), "chrome");

    let testium = std::fs::read_to_string(
        manifest_dir()
            .join("tests")
            .join("fixtures")
            .join("testium.json"),
    )
    .unwrap();
    let firefox_text = row_text("firefox");
    let chrome_text = row_text("chrome");
    BrowserRegistry::from_json_sources(&[
        ("chrome.json", chrome_text.as_str()),
        ("firefox.json", firefox_text.as_str()),
        ("testium.json", testium.as_str()),
    ])
    .expect("firefox wire ids must not collide with chrome or the testium fixture");
}

#[test]
fn must_pass_firefox_row_family_gecko_with_debugger_lanes_unsupported() {
    let registry = chrome_and_firefox();
    let row = firefox(&registry);
    assert_eq!(row.family, BrowserFamily::Gecko);
    assert_eq!(row.profile_layout, ProfileLayout::Gecko);
    assert_eq!(row.extension.origin_scheme, "moz-extension");
    assert!(!row.debugger_api_expected);
    assert_eq!(row.process_names, vec!["firefox.exe".to_owned()]);
    assert!(row.legacy_chromium_process_names.is_empty());
    assert!(row.cdp_product_tokens.is_empty());
    assert!(row.native_messaging_cleanup_roots.is_empty());
    assert!(row.install_ui.is_none());
    assert_eq!(row.user_data_root, "%APPDATA%\\Mozilla\\Firefox");
    assert_eq!(row.app_paths_name.as_deref(), Some("firefox.exe"));
    let policy = row.policy.as_ref().unwrap();
    assert_eq!(policy.root, "Software\\Policies\\Mozilla\\Firefox");
    assert_eq!(policy.extension_settings_value, "ExtensionSettings");

    let mut declared = row.unsupported_capabilities.clone();
    declared.sort();
    let mut expected = CDP_ONLY_COMMANDS.map(str::to_owned).to_vec();
    expected.sort();
    assert_eq!(declared, expected);
    assert!(!registry.is_chromium_process("firefox.exe"));
}

#[test]
fn must_pass_firefox_unsupported_list_names_real_capabilities_only() {
    let global = chromium_adapter_capabilities();
    assert_eq!(global.len(), 51, "chromium adapter list moved; resync");
    let registry = chrome_and_firefox();
    let row = firefox(&registry);
    for capability in &row.unsupported_capabilities {
        assert!(
            global.contains(capability),
            "unsupported capability {capability:?} is not a real command kind (typo would silently allow it)"
        );
    }
    let global_refs: Vec<&str> = global.iter().map(String::as_str).collect();
    let required = row.required_capabilities(&global_refs);
    assert_eq!(
        required.len(),
        global.len() - CDP_ONLY_COMMANDS.len(),
        "firefox requires exactly the global set minus the CDP-only lanes"
    );
    for command in CDP_ONLY_COMMANDS {
        assert!(
            !required.contains(&command),
            "{command} must not be required"
        );
    }
}

#[test]
fn must_fail_firefox_cdp_only_commands_refuse_with_capability_unsupported() {
    let registry = chrome_and_firefox();
    let row = firefox(&registry);
    let chrome = registry.legacy_default().unwrap();
    for command in CDP_ONLY_COMMANDS {
        assert!(!row.supports_capability(command), "{command}");
        assert!(
            chrome.supports_capability(command),
            "chrome keeps {command}"
        );
        let error = BrowserBridgeError::capability_unsupported(row.id.as_str(), command);
        assert_eq!(
            error.code,
            error_codes::BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED
        );
        assert!(error.detail.contains("browser_kind=firefox"));
        assert!(error.detail.contains(&format!("capability={command}")));
        assert!(
            error
                .detail
                .contains("source_of_truth=browsers/firefox.json")
        );
    }
}

#[test]
fn must_pass_firefox_webextension_commands_stay_routable() {
    let registry = chrome_and_firefox();
    let row = firefox(&registry);
    for command in WEBEXTENSION_COMMANDS {
        assert!(row.supports_capability(command), "{command}");
    }
}

#[test]
fn must_pass_firefox_tab_target_roundtrip() {
    let registry = chrome_and_firefox();
    let row = firefox(&registry);
    let target = BridgeTabTarget::new(row, 42);
    assert_eq!(target.wire(), "firefox-tab:42");
    let parsed = BridgeTabTarget::parse(&registry, " firefox-tab:42 ")
        .unwrap()
        .unwrap();
    assert_eq!(parsed.kind().as_str(), "firefox");
    assert_eq!(parsed.tab_id(), 42);
    assert_eq!(
        registry.by_tab_target("firefox-tab:7").unwrap().id.as_str(),
        "firefox"
    );
    let element = BridgeElementTarget::parse(&registry, "firefox-tab:3:frame:0:path:0.1.s.0")
        .unwrap()
        .unwrap();
    assert_eq!(element.tab().kind().as_str(), "firefox");
    assert_eq!(
        registry
            .tab_target_of_element_id("firefox-tab:3:frame:0:path:1")
            .as_deref(),
        Some("firefox-tab:3")
    );
    let chrome = BridgeTabTarget::parse(&registry, "chrome-tab:42")
        .unwrap()
        .unwrap();
    assert_eq!(
        chrome.kind().as_str(),
        "chrome",
        "chrome ids are unaffected"
    );
}

#[test]
fn must_fail_firefox_tab_target_malformed_id() {
    let registry = chrome_and_firefox();
    for raw in ["firefox-tab:", "firefox-tab:x1", "firefox-tab:-1"] {
        let error = BridgeTabTarget::parse(&registry, raw).unwrap_err();
        assert_eq!(error.code, error_codes::TOOL_PARAMS_INVALID, "{raw}");
    }
}

#[test]
fn must_pass_firefox_process_and_endpoint_route_to_firefox_kind() {
    let registry = chrome_and_firefox();
    for name in [
        "firefox.exe",
        "FIREFOX.EXE",
        "C:\\Program Files\\Mozilla Firefox\\firefox.exe",
    ] {
        assert_eq!(
            registry
                .by_process(name, None)
                .unwrap()
                .unwrap()
                .id
                .as_str(),
            "firefox",
            "{name}"
        );
    }
    assert!(registry.by_process("notepad.exe", None).unwrap().is_none());
    let row = firefox(&registry);
    let wire = row.endpoint(BridgeLane::Tabs);
    assert_eq!(wire, "synapse-bridge://firefox/chrome.tabs");
    let endpoint = BridgeEndpoint::parse(&registry, &wire).unwrap();
    assert_eq!(endpoint.kind().as_str(), "firefox");
    assert_eq!(
        registry.endpoint_kind(&wire).unwrap().id.as_str(),
        "firefox"
    );
    assert!(registry.is_bridge_tab_lane_endpoint(&wire));
    assert_eq!(
        registry.bridge_endpoint_for_reported_target(
            "firefox-tab:5",
            Some("synapse-bridge@synapse.local"),
            BridgeLane::Tabs
        ),
        "synapse-bridge://firefox/chrome.tabs",
        "the gecko endpoint never embeds the reported extension id"
    );
}

#[test]
fn must_pass_firefox_title_suffix_strip() {
    let registry = chrome_and_firefox();
    let row = firefox(&registry);
    assert_eq!(
        row.strip_window_title_suffix("Inbox \u{2014} Mozilla Firefox"),
        "Inbox"
    );
    assert_eq!(
        row.strip_window_title_suffix("Inbox - Mozilla Firefox"),
        "Inbox"
    );
    assert_eq!(row.strip_window_title_suffix("Inbox"), "Inbox");
}

struct FirefoxAppPaths(Option<String>);

impl RegistryReader for FirefoxAppPaths {
    fn read_string(&self, hive: RegistryHive, subkey: &str, value_name: &str) -> Option<String> {
        (hive == RegistryHive::LocalMachine
            && subkey.ends_with("App Paths\\firefox.exe")
            && value_name.is_empty())
        .then(|| self.0.clone())
        .flatten()
    }
}

#[test]
fn must_pass_firefox_locator_prefers_app_paths() {
    let registry = chrome_and_firefox();
    let row = firefox(&registry);
    let env = |key: &str| (key == "ProgramFiles").then(|| "C:\\PF".to_owned());
    let all_exist = |_: &Path| true;
    let app_paths = FirefoxAppPaths(Some(
        "\"C:\\Program Files\\Mozilla Firefox\\firefox.exe\"".to_owned(),
    ));
    let located = BrowserExecutableLocator::new(&app_paths, &env, &all_exist)
        .locate(row)
        .unwrap();
    assert_eq!(
        located.path,
        PathBuf::from("C:\\Program Files\\Mozilla Firefox\\firefox.exe")
    );
    assert!(located.source_of_truth.contains("browser_kind=firefox"));

    let candidate = BrowserExecutableLocator::new(&NoRegistry, &env, &all_exist)
        .locate(row)
        .unwrap();
    assert_eq!(
        candidate.path,
        PathBuf::from("C:\\PF\\Mozilla Firefox\\firefox.exe")
    );
}

#[test]
fn must_fail_firefox_not_installed_locates_nothing() {
    let registry = chrome_and_firefox();
    let row = firefox(&registry);
    let env = |key: &str| match key {
        "ProgramFiles" => Some("C:\\PF".to_owned()),
        "ProgramFiles(x86)" => Some("C:\\PF86".to_owned()),
        _ => None,
    };
    let nothing_exists = |_: &Path| false;
    assert!(
        BrowserExecutableLocator::new(&FirefoxAppPaths(None), &env, &nothing_exists)
            .locate(row)
            .is_none(),
        "no executable is the not_installed health path, never an error"
    );
    let stale_app_paths = FirefoxAppPaths(Some("D:\\gone\\firefox.exe".to_owned()));
    assert!(
        BrowserExecutableLocator::new(&stale_app_paths, &env, &nothing_exists)
            .locate(row)
            .is_none()
    );
}
