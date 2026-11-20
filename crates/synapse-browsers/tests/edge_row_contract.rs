//! MY TDD contract for the Microsoft Edge row (`browsers/edge.json`).
//! Written from the correct contract before the row existed.
//! must_pass_*: the Edge row loads, validates, resolves `msedge.exe` windows
//! and keeps wire ids disjoint from Chrome; must_fail_*: WebView2, Edge
//! helper images and non-stable channels never resolve to the Edge bridge.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use synapse_browsers::{
    BridgeElementTarget, BridgeEndpoint, BridgeLane, BridgeTabTarget, BrowserExecutableLocator,
    BrowserFamily, BrowserRegistry, NoRegistry, OriginRule, ProfileLayout, RegistryHive,
    RegistryReader,
};

const EXTENSION_ID: &str = "leoocgnkjnplbfdbklajepahofecgfbk";
const EDGE_STABLE_EXE: &str = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
/// Real WebView2 runtime image on the configured host (read-only check,
/// 2026-10-15): the runtime is installed, the Edge browser is not.
const WEBVIEW2_EXE: &str = "C:\\Program Files (x86)\\Microsoft\\EdgeWebView\\Application\\153.0.4234.48\\msedgewebview2.exe";

fn rows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("browsers")
}

fn row_json(id: &str) -> String {
    std::fs::read_to_string(rows_dir().join(format!("{id}.json"))).unwrap()
}

/// Chrome (legacy default) plus Edge, exactly as shipped.
fn chrome_and_edge() -> BrowserRegistry {
    let chrome = row_json("chrome");
    let edge = row_json("edge");
    BrowserRegistry::from_json_sources(&[
        ("chrome.json", chrome.as_str()),
        ("edge.json", edge.as_str()),
    ])
    .unwrap()
}

#[test]
fn must_pass_edge_row_loads_and_validates() {
    let registry = chrome_and_edge();
    let edge = registry.by_id("edge").unwrap();
    edge.validate().unwrap();

    assert_eq!(edge.label, "Microsoft Edge");
    assert_eq!(edge.family, BrowserFamily::Chromium);
    assert_eq!(edge.profile_layout, ProfileLayout::Multi);
    assert_eq!(edge.process_names, vec!["msedge.exe".to_owned()]);
    assert_eq!(edge.app_paths_name.as_deref(), Some("msedge.exe"));
    assert_eq!(
        edge.user_data_root,
        "%LOCALAPPDATA%\\Microsoft\\Edge\\User Data"
    );
    assert_eq!(edge.extension.id, EXTENSION_ID);
    assert_eq!(edge.extension.origin_scheme, "chrome-extension");
    assert_eq!(edge.extension.extensions_page_url, "edge://extensions");
    assert_eq!(edge.extension.manifest_dir, "synapse-chrome-debugger");
    assert_eq!(edge.extension.deploy_subdir, "browser-extension\\edge");
    assert_eq!(
        edge.chromium_origin_rule(),
        OriginRule::Exact(format!("chrome-extension://{EXTENSION_ID}"))
    );
    assert!(edge.debugger_api_expected);
    assert!(edge.unsupported_capabilities.is_empty());
    assert!(edge.protected_extension_ids.is_empty());
    let ui = edge.install_ui.as_ref().unwrap();
    assert_eq!(ui.developer_mode_names, vec!["Developer mode".to_owned()]);
    assert_eq!(ui.load_unpacked_names, vec!["Load unpacked".to_owned()]);
    assert_eq!(edge.winget_id.as_deref(), Some("Microsoft.Edge"));
}

#[test]
fn must_pass_edge_wire_ids_unique_vs_chrome() {
    let registry = chrome_and_edge();
    let chrome = registry.legacy_default().unwrap();
    let edge = registry.by_id("edge").unwrap();

    assert_eq!(edge.tab_target_prefix, "edge-tab:");
    assert_eq!(edge.health_key, "edge_bridge");
    assert_eq!(
        edge.registration_token_file,
        "browser-bridge-token-edge.txt"
    );
    assert_eq!(edge.endpoint_base, "synapse-bridge://edge");
    assert_ne!(edge.tab_target_prefix, chrome.tab_target_prefix);
    assert_ne!(edge.health_key, chrome.health_key);
    assert_ne!(edge.registration_token_file, chrome.registration_token_file);
    assert_ne!(edge.endpoint_base, chrome.endpoint_base);

    assert_eq!(
        edge.endpoint(BridgeLane::Tabs),
        "synapse-bridge://edge/chrome.tabs"
    );
    let parsed = BridgeEndpoint::parse(&registry, "synapse-bridge://edge/chrome.debugger").unwrap();
    assert_eq!(parsed.kind().as_str(), "edge");
    assert_eq!(parsed.lane(), BridgeLane::Debugger);
    assert_eq!(
        parsed.to_wire(&registry).as_deref(),
        Some("synapse-bridge://edge/chrome.debugger")
    );
    // The Edge worker reports the same extension id as Chrome; its readback
    // must still name the Edge endpoint, never the legacy chrome wire form.
    assert_eq!(
        registry.bridge_endpoint_for_reported_kind(
            Some("edge"),
            Some(EXTENSION_ID),
            BridgeLane::Tabs
        ),
        "synapse-bridge://edge/chrome.tabs"
    );
    // A persisted legacy chrome endpoint stays chrome after Edge is added.
    let legacy = format!("chrome-extension://{EXTENSION_ID}/chrome.tabs");
    assert_eq!(
        registry.endpoint_kind(&legacy).unwrap().id.as_str(),
        "chrome"
    );
    assert_eq!(
        registry.by_health_key("edge_bridge").unwrap().id.as_str(),
        "edge"
    );
}

#[test]
fn must_pass_edge_row_coexists_with_every_shipped_row() {
    // Uniqueness (id, tab prefix, health key, endpoint base, token file) is
    // enforced by the loader across every row in the shipped dir.
    let registry = BrowserRegistry::load_dir(&rows_dir()).unwrap();
    assert!(registry.by_id("edge").is_some());
    assert_eq!(registry.legacy_default().unwrap().id.as_str(), "chrome");
}

#[test]
fn must_pass_edge_resolves_from_msedge_exe_case_insensitive() {
    let registry = chrome_and_edge();
    for (name, path) in [
        ("msedge.exe", None),
        ("MSEDGE.EXE", None),
        ("msedge", None),
        (EDGE_STABLE_EXE, None),
        ("msedge.exe", Some(EDGE_STABLE_EXE)),
        (
            "MSEdge.exe",
            Some("c:/program files (x86)/microsoft/edge/application/msedge.exe"),
        ),
    ] {
        let row = registry.by_process(name, path).unwrap().unwrap();
        assert_eq!(row.id.as_str(), "edge", "{name} {path:?}");
    }
    assert_eq!(
        registry
            .by_process("chrome.exe", None)
            .unwrap()
            .unwrap()
            .id
            .as_str(),
        "chrome"
    );
}

#[test]
fn must_fail_edge_rejects_msedgewebview2_process() {
    let registry = chrome_and_edge();
    let edge = registry.by_id("edge").unwrap();
    for (name, path) in [
        ("msedgewebview2.exe", None),
        ("msedgewebview2.exe", Some(WEBVIEW2_EXE)),
        ("MSEDGEWEBVIEW2.EXE", Some(WEBVIEW2_EXE)),
        ("msedgewebview2", None),
        (WEBVIEW2_EXE, None),
    ] {
        assert!(
            !edge.matches_process_name(name),
            "{name} must not match the edge row"
        );
        assert!(
            registry.by_process(name, path).unwrap().is_none(),
            "{name} {path:?}: WebView2 is not the Edge browser and stays on its raw-CDP path"
        );
    }
    // Adding the Edge row must not pull WebView2 into the Chromium set.
    assert!(!registry.is_chromium_process("msedgewebview2.exe"));
    assert!(!registry.is_chromium_process(WEBVIEW2_EXE));
    assert!(registry.is_chromium_process("msedge.exe"));
}

#[test]
fn must_fail_edge_rejects_helper_images_and_other_channels() {
    let registry = chrome_and_edge();
    // Helper images that ship beside msedge.exe / msedgewebview2.exe.
    for name in [
        "msedge_proxy.exe",
        "identity_helper.exe",
        "elevation_service.exe",
        "notification_helper.exe",
        "",
    ] {
        assert!(
            registry.by_process(name, None).unwrap().is_none(),
            "{name:?}"
        );
    }
    // msedge.exe outside the stable `\Microsoft\Edge\Application\` dir
    // (Beta/Dev/Canary keep separate user data, WebView2 runtime dir) is
    // refused instead of being scanned against the stable profile.
    for path in [
        "C:\\Program Files (x86)\\Microsoft\\Edge Beta\\Application\\msedge.exe",
        "C:\\Program Files (x86)\\Microsoft\\Edge Dev\\Application\\msedge.exe",
        "C:\\Users\\a\\AppData\\Local\\Microsoft\\Edge SxS\\Application\\msedge.exe",
        "C:\\Program Files (x86)\\Microsoft\\EdgeWebView\\Application\\153.0.4234.48\\msedge.exe",
    ] {
        assert!(
            registry
                .by_process("msedge.exe", Some(path))
                .unwrap()
                .is_none(),
            "{path}"
        );
    }
}

#[test]
fn must_pass_edge_title_suffix_with_zero_width_space_stripped() {
    let registry = chrome_and_edge();
    let edge = registry.by_id("edge").unwrap();
    assert!(
        edge.window_title_suffixes
            .iter()
            .any(|suffix| suffix == " - Microsoft\u{200b} Edge")
    );
    assert_eq!(
        edge.strip_window_title_suffix("Inbox - Microsoft\u{200b} Edge"),
        "Inbox"
    );
    assert_eq!(
        edge.strip_window_title_suffix("Inbox - Microsoft Edge"),
        "Inbox"
    );
    assert_eq!(
        edge.strip_window_title_suffix("Inbox - Personal - Microsoft\u{200b} Edge"),
        "Inbox - Personal"
    );
    assert_eq!(
        edge.strip_window_title_suffix("Inbox - Google Chrome"),
        "Inbox - Google Chrome",
        "the edge row never strips another browser's suffix"
    );
    assert_eq!(edge.strip_window_title_suffix(""), "");
}

#[test]
fn must_pass_edge_tab_target_roundtrip() {
    let registry = chrome_and_edge();
    let edge = registry.by_id("edge").unwrap();

    let target = BridgeTabTarget::new(edge, 42);
    assert_eq!(target.wire(), "edge-tab:42");
    let parsed = BridgeTabTarget::parse(&registry, "edge-tab:42")
        .unwrap()
        .unwrap();
    assert_eq!(parsed, target);
    assert_eq!(parsed.kind().as_str(), "edge");
    assert_eq!(
        registry.by_tab_target("edge-tab:7").unwrap().id.as_str(),
        "edge"
    );
    assert_eq!(
        BridgeTabTarget::parse(&registry, "chrome-tab:42")
            .unwrap()
            .unwrap()
            .kind()
            .as_str(),
        "chrome"
    );

    let element = BridgeElementTarget::parse(&registry, "edge-tab:3:frame:0:path:0.1.s.0")
        .unwrap()
        .unwrap();
    assert_eq!(element.tab().kind().as_str(), "edge");
    assert_eq!(element.tab().tab_id(), 3);
    assert_eq!(element.path(), "0.1.s.0");
}

#[test]
fn must_fail_edge_tab_target_malformed() {
    let registry = chrome_and_edge();
    for raw in ["edge-tab:", "edge-tab:abc", "edge-tab:-1", "edge-tab:1.5"] {
        let error = BridgeTabTarget::parse(&registry, raw).unwrap_err();
        assert_eq!(
            error.code,
            synapse_core::error_codes::TOOL_PARAMS_INVALID,
            "{raw}"
        );
    }
    let error = BridgeTabTarget::parse(&registry, "msedge-tab:1").unwrap_err();
    assert_eq!(
        error.code,
        synapse_core::error_codes::BROWSER_BRIDGE_KIND_UNRESOLVED
    );
}

#[test]
fn must_pass_edge_policy_root_is_edge_not_chrome() {
    let registry = chrome_and_edge();
    let edge = registry.by_id("edge").unwrap();
    let chrome = registry.legacy_default().unwrap();

    let policy = edge.policy.as_ref().unwrap();
    assert_eq!(policy.root, "Software\\Policies\\Microsoft\\Edge");
    assert_eq!(policy.extension_settings_value, "ExtensionSettings");
    assert_ne!(policy.root, chrome.policy.as_ref().unwrap().root);

    assert_eq!(
        edge.native_messaging_cleanup_roots,
        vec![
            "Software\\Microsoft\\Edge\\NativeMessagingHosts".to_owned(),
            "Software\\Wow6432Node\\Microsoft\\Edge\\NativeMessagingHosts".to_owned(),
        ]
    );
    assert!(
        edge.native_messaging_cleanup_roots
            .iter()
            .all(|root| !root.contains("Google")),
        "chrome keys are cleaned by the chrome row only"
    );
}

#[test]
fn must_pass_edge_cdp_product_token_identifies_edge() {
    let registry = chrome_and_edge();
    let edge = registry.by_id("edge").unwrap();
    assert_eq!(edge.cdp_product_tokens, vec!["Edg/".to_owned()]);
    assert!(registry.cdp_product_is_chromium("Edg/155.0.4283.45"));
    assert!(!registry.cdp_product_is_chromium("Firefox/158.0"));
}

struct EdgeAppPaths(Option<&'static str>);

impl RegistryReader for EdgeAppPaths {
    fn read_string(&self, hive: RegistryHive, subkey: &str, value_name: &str) -> Option<String> {
        (hive == RegistryHive::LocalMachine
            && subkey.ends_with("App Paths\\msedge.exe")
            && value_name.is_empty())
        .then(|| self.0.map(str::to_owned))
        .flatten()
    }
}

fn host_env(key: &str) -> Option<String> {
    match key {
        "ProgramFiles(x86)" => Some("C:\\Program Files (x86)".to_owned()),
        "LOCALAPPDATA" => Some("C:\\Users\\a\\AppData\\Local".to_owned()),
        _ => None,
    }
}

#[test]
fn must_pass_edge_locator_prefers_app_paths_then_program_files_x86() {
    let registry = chrome_and_edge();
    let edge = registry.by_id("edge").unwrap();
    let all_exist = |_: &Path| true;

    let app_paths = EdgeAppPaths(Some("D:\\Edge\\msedge.exe"));
    let located = BrowserExecutableLocator::new(&app_paths, &host_env, &all_exist)
        .locate(edge)
        .unwrap();
    assert_eq!(located.path, PathBuf::from("D:\\Edge\\msedge.exe"));
    assert!(located.source_of_truth.contains("App Paths\\msedge.exe"));
    assert!(located.source_of_truth.contains("browser_kind=edge"));

    let located = BrowserExecutableLocator::new(&NoRegistry, &host_env, &all_exist)
        .locate(edge)
        .unwrap();
    assert_eq!(located.path, PathBuf::from(EDGE_STABLE_EXE));
    assert!(located.source_of_truth.contains("browsers/edge.json"));
}

#[test]
fn must_pass_not_installed_edge_locates_nothing() {
    // `None` is exactly what the daemon health row maps to
    // `status=not_installed` (never `error`, never degrades overall health).
    let registry = chrome_and_edge();
    let edge = registry.by_id("edge").unwrap();
    let none_exist = |_: &Path| false;

    // Configured host shape: EdgeUpdate + WebView2 present, msedge.exe absent,
    // no App Paths row.
    let locator = BrowserExecutableLocator::new(&NoRegistry, &host_env, &none_exist);
    assert!(locator.locate(edge).is_none());
    assert_eq!(
        locator.first_candidate(edge),
        Some(PathBuf::from(EDGE_STABLE_EXE))
    );
    // A stale App Paths value pointing at a missing file is not an install.
    let stale = EdgeAppPaths(Some(EDGE_STABLE_EXE));
    assert!(
        BrowserExecutableLocator::new(&stale, &host_env, &none_exist)
            .locate(edge)
            .is_none()
    );
    // Unset %ProgramFiles(x86)% never yields a half-expanded path.
    let empty_env = |_: &str| -> Option<String> { None };
    let locator = BrowserExecutableLocator::new(&NoRegistry, &empty_env, &none_exist);
    assert!(locator.locate(edge).is_none());
    assert_eq!(locator.first_candidate(edge), None);
}
