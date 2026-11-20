//! MY TDD contract for the Vivaldi browser row (`browsers/vivaldi.json`).
//! must_pass_*: the row loads beside chrome with its own wire ids, resolves a
//! per-user or system install, strips the ` - Vivaldi` title suffix and
//! round-trips `vivaldi-tab:` targets. must_fail_*: Vivaldi's own UI is a
//! built-in component extension (`mpognobbkildjkofajifpdfhcoklimli`); popup
//! suppression must never disable it, and a wire-id collision with chrome is
//! refused at load time.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use synapse_browsers::{
    BridgeLane, BridgeTabTarget, BrowserExecutableLocator, BrowserFamily, BrowserRegistry,
    BrowserRegistryError, NoRegistry, OriginRule, ProfileLayout, RegistryHive, RegistryReader,
};

/// Vivaldi's browser UI runs as this built-in component extension.
const VIVALDI_UI_EXTENSION_ID: &str = "mpognobbkildjkofajifpdfhcoklimli";
/// The shared Synapse bridge extension id (fixed by the manifest `key`).
const BRIDGE_EXTENSION_ID: &str = "leoocgnkjnplbfdbklajepahofecgfbk";

fn rows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("browsers")
}

fn row_json(id: &str) -> String {
    std::fs::read_to_string(rows_dir().join(format!("{id}.json"))).unwrap()
}

fn chrome_and_vivaldi() -> BrowserRegistry {
    let chrome = row_json("chrome");
    let vivaldi = row_json("vivaldi");
    BrowserRegistry::from_json_sources(&[
        ("chrome.json", chrome.as_str()),
        ("vivaldi.json", vivaldi.as_str()),
    ])
    .unwrap()
}

fn is_chromium_extension_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte))
}

#[test]
fn must_pass_vivaldi_row_loads_and_validates() {
    let registry = chrome_and_vivaldi();
    let vivaldi = registry.by_id("vivaldi").unwrap();

    assert_eq!(vivaldi.label, "Vivaldi");
    assert_eq!(vivaldi.family, BrowserFamily::Chromium);
    assert_eq!(vivaldi.profile_layout, ProfileLayout::Multi);
    assert_eq!(vivaldi.process_names, vec!["vivaldi.exe".to_owned()]);
    assert!(vivaldi.exe_path_markers.is_empty(), "vivaldi.exe is unique");
    assert_eq!(vivaldi.app_paths_name.as_deref(), Some("vivaldi.exe"));
    assert_eq!(vivaldi.user_data_root, "%LOCALAPPDATA%\\Vivaldi\\User Data");
    assert_eq!(vivaldi.tab_target_prefix, "vivaldi-tab:");
    assert_eq!(vivaldi.endpoint_base, "synapse-bridge://vivaldi");
    assert_eq!(
        vivaldi.endpoint(BridgeLane::Tabs),
        "synapse-bridge://vivaldi/chrome.tabs"
    );
    assert_eq!(vivaldi.health_key, "vivaldi_bridge");
    assert_eq!(
        vivaldi.registration_token_file,
        "browser-bridge-token-vivaldi.txt"
    );
    assert_eq!(vivaldi.extension.id, BRIDGE_EXTENSION_ID);
    assert_eq!(vivaldi.extension.origin_scheme, "chrome-extension");
    assert_eq!(
        vivaldi.extension.extensions_page_url,
        "vivaldi://extensions"
    );
    assert_eq!(vivaldi.extension.manifest_dir, "synapse-chrome-debugger");
    assert_eq!(
        vivaldi.extension.deploy_subdir,
        "browser-extension\\vivaldi"
    );
    assert_eq!(
        vivaldi.chromium_origin_rule(),
        OriginRule::Exact(format!("chrome-extension://{BRIDGE_EXTENSION_ID}"))
    );
    assert!(
        vivaldi.policy.is_none(),
        "policy root unverified: no shield"
    );
    assert!(vivaldi.legacy_chromium_process_names.is_empty());
    assert!(vivaldi.unsupported_capabilities.is_empty());
    assert!(vivaldi.debugger_api_expected);
    let install_ui = vivaldi.install_ui.as_ref().unwrap();
    assert_eq!(install_ui.developer_mode_names, vec!["Developer mode"]);
    assert_eq!(install_ui.load_unpacked_names, vec!["Load unpacked"]);
    assert_eq!(vivaldi.winget_id.as_deref(), Some("Vivaldi.Vivaldi"));
    assert!(!vivaldi.id.is_legacy_default());
    assert_eq!(registry.legacy_default().unwrap().id.as_str(), "chrome");
}

#[test]
fn must_pass_vivaldi_wire_ids_differ_from_chrome() {
    let registry = chrome_and_vivaldi();
    let chrome = registry.by_id("chrome").unwrap();
    let vivaldi = registry.by_id("vivaldi").unwrap();
    assert_ne!(vivaldi.tab_target_prefix, chrome.tab_target_prefix);
    assert_ne!(vivaldi.health_key, chrome.health_key);
    assert_ne!(
        vivaldi.registration_token_file,
        chrome.registration_token_file
    );
    assert_ne!(vivaldi.endpoint_base, chrome.endpoint_base);
    assert_eq!(
        registry
            .by_health_key("vivaldi_bridge")
            .unwrap()
            .id
            .as_str(),
        "vivaldi"
    );
    assert_eq!(
        registry
            .endpoint_kind("synapse-bridge://vivaldi/chrome.tabs")
            .unwrap()
            .id
            .as_str(),
        "vivaldi"
    );
    assert!(registry.is_bridge_tab_lane_endpoint("synapse-bridge://vivaldi/chrome.debugger"));
}

#[test]
fn must_fail_vivaldi_wire_id_collision_with_chrome_is_refused() {
    let chrome = row_json("chrome");
    for (field, value) in [
        ("tab_target_prefix", "chrome-tab:"),
        ("health_key", "chrome_bridge"),
        ("registration_token_file", "chrome-bridge-token.txt"),
    ] {
        let mut row: serde_json::Value = serde_json::from_str(&row_json("vivaldi")).unwrap();
        row[field] = serde_json::json!(value);
        let collided = row.to_string();
        let error = BrowserRegistry::from_json_sources(&[
            ("chrome.json", chrome.as_str()),
            ("vivaldi.json", collided.as_str()),
        ])
        .unwrap_err();
        assert!(
            matches!(error, BrowserRegistryError::Duplicate { field: f, .. } if f == field),
            "{field}: {error}"
        );
    }
}

#[test]
fn must_fail_vivaldi_ui_extension_never_suppressed() {
    let registry = chrome_and_vivaldi();
    let vivaldi = registry.by_id("vivaldi").unwrap();

    // The suppression filters (service worker PROTECTED_EXTENSION_IDS and the
    // daemon profile scan) skip exactly the ids in this list.
    assert!(
        vivaldi
            .protected_extension_ids
            .iter()
            .any(|id| id == VIVALDI_UI_EXTENSION_ID),
        "Vivaldi UI extension must be protected: {:?}",
        vivaldi.protected_extension_ids
    );
    assert!(
        vivaldi
            .protected_extension_ids
            .iter()
            .map(String::as_str)
            .all(is_chromium_extension_id),
        "protected ids must be 32-char a-p Chromium ids: {:?}",
        vivaldi.protected_extension_ids
    );
    assert!(
        !vivaldi
            .protected_extension_ids
            .iter()
            .any(|id| id == &vivaldi.extension.id),
        "the bridge itself is not a protected browser component"
    );
    // No ExtensionSettings policy shield is written for Vivaldi, so no policy
    // entry can block the UI extension either.
    assert!(vivaldi.policy.is_none());
    // The protection is per row: chrome has no Vivaldi UI to protect.
    assert!(
        !registry
            .by_id("chrome")
            .unwrap()
            .protected_extension_ids
            .iter()
            .any(|id| id == VIVALDI_UI_EXTENSION_ID)
    );
}

struct FakeAppPaths(&'static str);

impl RegistryReader for FakeAppPaths {
    fn read_string(&self, hive: RegistryHive, subkey: &str, value_name: &str) -> Option<String> {
        (hive == RegistryHive::CurrentUser
            && subkey.ends_with("App Paths\\vivaldi.exe")
            && value_name.is_empty())
        .then(|| self.0.to_owned())
    }
}

fn fake_env(key: &str) -> Option<String> {
    match key {
        "LOCALAPPDATA" => Some("C:\\LAD".to_owned()),
        "ProgramFiles" => Some("C:\\PF".to_owned()),
        _ => None,
    }
}

#[test]
fn must_pass_vivaldi_resolves_per_user_install() {
    let registry = chrome_and_vivaldi();
    let vivaldi = registry.by_id("vivaldi").unwrap();
    let per_user = PathBuf::from("C:\\LAD\\Vivaldi\\Application\\vivaldi.exe");
    let system = PathBuf::from("C:\\PF\\Vivaldi\\Application\\vivaldi.exe");

    let only_per_user = |path: &Path| path == per_user;
    let located = BrowserExecutableLocator::new(&NoRegistry, &fake_env, &only_per_user)
        .locate(vivaldi)
        .unwrap();
    assert_eq!(located.path, per_user);
    assert!(located.source_of_truth.contains("exe_candidates"));
    assert!(located.source_of_truth.contains("browsers/vivaldi.json"));

    let only_system = |path: &Path| path == system;
    let located = BrowserExecutableLocator::new(&NoRegistry, &fake_env, &only_system)
        .locate(vivaldi)
        .unwrap();
    assert_eq!(located.path, system);

    let registered = "D:\\Portable\\Vivaldi\\Application\\vivaldi.exe";
    let all_exist = |_: &Path| true;
    let located = BrowserExecutableLocator::new(&FakeAppPaths(registered), &fake_env, &all_exist)
        .locate(vivaldi)
        .unwrap();
    assert_eq!(located.path, PathBuf::from(registered));
    assert!(located.source_of_truth.contains("browser_kind=vivaldi"));
}

#[test]
fn must_pass_vivaldi_not_installed_locates_nothing() {
    // `None` here is the input of the daemon's `not_installed` health branch
    // (browser_bridge_health.rs), which never reports `error`.
    let registry = chrome_and_vivaldi();
    let vivaldi = registry.by_id("vivaldi").unwrap();
    let nothing = |_: &Path| false;
    let locator = BrowserExecutableLocator::new(&NoRegistry, &fake_env, &nothing);
    assert!(locator.locate(vivaldi).is_none());
    assert_eq!(
        locator.first_candidate(vivaldi),
        Some(PathBuf::from("C:\\LAD\\Vivaldi\\Application\\vivaldi.exe")),
        "diagnostics name the per-user default first"
    );
    let unset = |_: &str| -> Option<String> { None };
    assert!(
        BrowserExecutableLocator::new(&NoRegistry, &unset, &|_: &Path| true)
            .locate(vivaldi)
            .is_none(),
        "unset %VAR% never yields a half-expanded path"
    );
}

#[test]
fn must_pass_vivaldi_title_suffix_stripped() {
    let registry = chrome_and_vivaldi();
    let vivaldi = registry.by_id("vivaldi").unwrap();
    assert_eq!(vivaldi.window_title_suffixes, vec![" - Vivaldi".to_owned()]);
    assert_eq!(
        vivaldi.strip_window_title_suffix("Inbox - Vivaldi"),
        "Inbox"
    );
    assert_eq!(
        vivaldi.strip_window_title_suffix("a - Vivaldi - Vivaldi"),
        "a - Vivaldi"
    );
    assert_eq!(vivaldi.strip_window_title_suffix("Vivaldi"), "Vivaldi");
    assert_eq!(
        vivaldi.strip_window_title_suffix("Inbox - Google Chrome"),
        "Inbox - Google Chrome"
    );
}

#[test]
fn must_pass_vivaldi_tab_target_roundtrip() {
    let registry = chrome_and_vivaldi();
    let vivaldi = registry.by_id("vivaldi").unwrap();
    let target = BridgeTabTarget::parse(&registry, "vivaldi-tab:42")
        .unwrap()
        .unwrap();
    assert_eq!(target.kind().as_str(), "vivaldi");
    assert_eq!(target.tab_id(), 42);
    assert_eq!(BridgeTabTarget::new(vivaldi, 42), target);
    let chrome = BridgeTabTarget::parse(&registry, "chrome-tab:42")
        .unwrap()
        .unwrap();
    assert_eq!(chrome.kind().as_str(), "chrome", "chrome ids unaffected");
    assert_eq!(
        registry.tab_target_of_element_id("vivaldi-tab:3:frame:0:path:0.1.s.0"),
        Some("vivaldi-tab:3".to_owned())
    );
    assert!(BridgeTabTarget::parse(&registry, "vivaldi-tab:x").is_err());
}

#[test]
fn must_pass_vivaldi_window_resolves_to_its_kind() {
    let registry = chrome_and_vivaldi();
    for (name, path) in [
        ("vivaldi.exe", None),
        (
            "VIVALDI.EXE",
            Some("C:\\Users\\a\\AppData\\Local\\Vivaldi\\Application\\vivaldi.exe"),
        ),
        ("vivaldi", None),
    ] {
        let row = registry.by_process(name, path).unwrap().unwrap();
        assert_eq!(row.id.as_str(), "vivaldi", "{name}");
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
    assert!(registry.is_chromium_process("vivaldi.exe"));
}
