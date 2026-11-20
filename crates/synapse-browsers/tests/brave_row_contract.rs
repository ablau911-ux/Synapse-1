//! MY TDD contract for the Brave browser row (`browsers/brave.json`).
//! must_pass_*: the row loads, owns unique wire ids, resolves both install
//! scopes and keeps Brave's own component extension out of popup suppression.
//! must_fail_*: brave.exe never resolves to the chrome kind (and chrome.exe
//! never to brave), the scan never reads Google Chrome's user data, and a
//! Brave row that reuses a chrome wire id is refused at load time.
//!
//! Field sources (verified 2026-10-15, Brave not installed on the dev PC):
//! - brave.exe: brave-core chromium_src/chrome/common/chrome_constants.cc
//!   (kBrowserProcessExecutableName) and chromium_src/chrome/installer/util/
//!   util_constants.h (kChromeExe = L"brave.exe", the App Paths key name).
//! - BraveSoftware\Brave-Browser: brave-core chromium_src/chrome/install_static/
//!   chromium_install_modes.h (kCompanyPathName, kProductPathName); Chromium
//!   install_static user_data_dir.cc appends "\User Data" under LOCALAPPDATA.
//! - " - Brave": brave-core app/brave_strings.grd IDS_BROWSER_WINDOW_TITLE_FORMAT.
//! - Software\Policies\BraveSoftware\Brave: github.com/brave/brave-browser/issues/25394.
//! - NativeMessagingHosts: Chromium chrome/browser/extensions/api/messaging/
//!   launch_context_win.cc reads only SOFTWARE\Google\Chrome (and Chromium).
//! - CDP Browser "Chrome/<ver>": Chromium components/version_info/
//!   version_info_with_user_agent.cc (Brave overrides only the version values).
//! - mnojpmjdmbbfmejpflffifhffcmidifd: brave-core chromium_src/extensions/
//!   common/constants.h `brave_extension_id` (component extension).
//! - Brave.Brave: microsoft/winget-pkgs manifests/b/Brave/Brave (user + machine).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use synapse_browsers::chromium::ChromiumProfileInspector;
use synapse_browsers::{
    BridgeElementTarget, BridgeLane, BridgeTabTarget, BrowserExecutableLocator, BrowserFamily,
    BrowserRegistry, BrowserRegistryError, BrowserRow, NoRegistry, OriginRule, ProfileLayout,
    RegistryHive, RegistryReader, expand_env_vars,
};

const BRIDGE_EXTENSION_ID: &str = "leoocgnkjnplbfdbklajepahofecgfbk";
const BRAVE_COMPONENT_EXTENSION_ID: &str = "mnojpmjdmbbfmejpflffifhffcmidifd";
const SYSTEM_EXE: &str = "C:\\PF\\BraveSoftware\\Brave-Browser\\Application\\brave.exe";
const SYSTEM_X86_EXE: &str = "C:\\PF86\\BraveSoftware\\Brave-Browser\\Application\\brave.exe";
const PER_USER_EXE: &str = "C:\\LAD\\BraveSoftware\\Brave-Browser\\Application\\brave.exe";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn row_json(id: &str) -> String {
    std::fs::read_to_string(manifest_dir().join("browsers").join(format!("{id}.json"))).unwrap()
}

/// `brave.json` with one top-level field replaced.
fn brave_with(field: &str, value: serde_json::Value) -> String {
    let mut row: serde_json::Value = serde_json::from_str(&row_json("brave")).unwrap();
    row[field] = value;
    row.to_string()
}

fn chrome_and_brave() -> BrowserRegistry {
    BrowserRegistry::from_json_sources(&[
        ("chrome.json", row_json("chrome").as_str()),
        ("brave.json", row_json("brave").as_str()),
    ])
    .unwrap()
}

fn brave(registry: &BrowserRegistry) -> &BrowserRow {
    registry.by_id("brave").unwrap()
}

fn fake_env(key: &str) -> Option<String> {
    match key.to_ascii_uppercase().as_str() {
        "PROGRAMFILES" => Some("C:\\PF".to_owned()),
        "PROGRAMFILES(X86)" => Some("C:\\PF86".to_owned()),
        "LOCALAPPDATA" => Some("C:\\LAD".to_owned()),
        _ => None,
    }
}

/// App Paths reader that knows only `brave.exe`, in one hive.
struct BraveAppPaths(RegistryHive, &'static str);

impl RegistryReader for BraveAppPaths {
    fn read_string(&self, hive: RegistryHive, subkey: &str, value_name: &str) -> Option<String> {
        (hive == self.0 && subkey.ends_with("App Paths\\brave.exe") && value_name.is_empty())
            .then(|| self.1.to_owned())
    }
}

fn write_installed_profile(profile_dir: &Path, extension_id: &str) {
    let extension_dir = profile_dir.join("ext");
    std::fs::create_dir_all(&extension_dir).unwrap();
    std::fs::write(
        extension_dir.join("service_worker.js"),
        "const BRIDGE_CODE_STAMP = \"brave1\";\n",
    )
    .unwrap();
    let prefs = serde_json::json!({
        "extensions": {"settings": {extension_id: {"path": extension_dir.to_string_lossy()}}}
    });
    std::fs::write(profile_dir.join("Preferences"), prefs.to_string()).unwrap();
}

#[test]
fn must_pass_brave_row_loads_and_validates() {
    let shipped = BrowserRegistry::load_dir(&manifest_dir().join("browsers")).unwrap();
    let row = brave(&shipped);
    row.validate().unwrap();

    assert_eq!(row.family, BrowserFamily::Chromium);
    assert_eq!(row.profile_layout, ProfileLayout::Multi);
    assert_eq!(row.process_names, vec!["brave.exe".to_owned()]);
    assert_eq!(row.app_paths_name.as_deref(), Some("brave.exe"));
    assert_eq!(
        row.user_data_root,
        "%LOCALAPPDATA%\\BraveSoftware\\Brave-Browser\\User Data"
    );
    assert_eq!(row.window_title_suffixes, vec![" - Brave".to_owned()]);
    assert_eq!(row.extension.id, BRIDGE_EXTENSION_ID);
    assert_eq!(row.extension.origin_scheme, "chrome-extension");
    assert_eq!(row.extension.extensions_page_url, "brave://extensions");
    assert_eq!(row.extension.manifest_dir, "synapse-chrome-debugger");
    assert_eq!(row.extension.deploy_subdir, "browser-extension\\brave");
    assert_eq!(row.winget_id.as_deref(), Some("Brave.Brave"));
    assert!(row.debugger_api_expected);
    assert!(row.unsupported_capabilities.is_empty());
    assert_eq!(
        row.chromium_origin_rule(),
        OriginRule::Exact(format!("chrome-extension://{BRIDGE_EXTENSION_ID}"))
    );
}

#[test]
fn must_pass_brave_wire_ids_are_its_own() {
    let registry = chrome_and_brave();
    let row = brave(&registry);
    let chrome = registry.legacy_default().unwrap();

    assert_eq!(row.tab_target_prefix, "brave-tab:");
    assert_eq!(row.health_key, "brave_bridge");
    assert_eq!(
        row.registration_token_file,
        "browser-bridge-token-brave.txt"
    );
    assert_eq!(row.endpoint_base, "synapse-bridge://brave");
    assert_ne!(row.tab_target_prefix, chrome.tab_target_prefix);
    assert_ne!(row.health_key, chrome.health_key);
    assert_ne!(row.registration_token_file, chrome.registration_token_file);
    assert_ne!(row.endpoint_base, chrome.endpoint_base);

    let tabs = row.endpoint(BridgeLane::Tabs);
    assert_eq!(registry.endpoint_kind(&tabs).unwrap().id.as_str(), "brave");
    assert!(registry.is_bridge_tab_lane_endpoint(&tabs));
    assert_eq!(
        registry.by_health_key("brave_bridge").unwrap().id.as_str(),
        "brave"
    );
}

#[test]
fn must_fail_brave_row_reusing_a_chrome_wire_id_is_refused() {
    let chrome = registry_chrome_row();
    for (field, value) in [
        ("tab_target_prefix", chrome.tab_target_prefix.clone()),
        ("health_key", chrome.health_key.clone()),
        (
            "registration_token_file",
            chrome.registration_token_file.clone(),
        ),
        ("endpoint_base", chrome.endpoint_base.clone()),
    ] {
        let clashing = brave_with(field, serde_json::json!(value));
        let error = BrowserRegistry::from_json_sources(&[
            ("chrome.json", row_json("chrome").as_str()),
            ("brave.json", clashing.as_str()),
        ])
        .unwrap_err();
        assert!(
            matches!(&error, BrowserRegistryError::Duplicate { field: f, .. } if *f == field),
            "{field}: {error}"
        );
        assert!(error.to_string().contains("brave.json"), "{error}");
    }
}

fn registry_chrome_row() -> BrowserRow {
    chrome_and_brave().legacy_default().unwrap().clone()
}

#[test]
fn must_pass_brave_resolves_from_brave_exe_case_insensitive() {
    let registry = chrome_and_brave();
    for (name, path) in [
        ("brave.exe", Some(SYSTEM_EXE)),
        ("BRAVE.EXE", None),
        ("brave", None),
        (PER_USER_EXE, Some(PER_USER_EXE)),
    ] {
        let row = registry.by_process(name, path).unwrap().unwrap();
        assert_eq!(row.id.as_str(), "brave", "{name}");
    }
}

#[test]
fn must_fail_brave_exe_never_resolves_to_chrome_and_chrome_exe_never_to_brave() {
    let registry = chrome_and_brave();
    let row = brave(&registry);
    let chrome = registry.legacy_default().unwrap();
    assert!(!chrome.matches_process_name("brave.exe"));
    assert!(!row.matches_process_name("chrome.exe"));
    assert!(!row.matches_process_name("msedge.exe"));

    for (name, path) in [("brave.exe", Some(SYSTEM_EXE)), ("brave.exe", None)] {
        let resolved = registry.by_process(name, path).unwrap().unwrap();
        assert_ne!(resolved.id.as_str(), "chrome", "{name} {path:?}");
    }
    let chrome_path = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
    for path in [Some(chrome_path), None] {
        let resolved = registry.by_process("chrome.exe", path).unwrap().unwrap();
        assert_eq!(resolved.id.as_str(), "chrome", "{path:?}");
    }
    assert!(
        registry
            .by_process("braveupdate.exe", None)
            .unwrap()
            .is_none()
    );
}

#[test]
fn must_pass_brave_resolves_per_user_and_system_install_paths() {
    let registry = chrome_and_brave();
    let row = brave(&registry);
    let env = |key: &str| fake_env(key);

    for (exists, expected) in [
        (SYSTEM_EXE, SYSTEM_EXE),
        (SYSTEM_X86_EXE, SYSTEM_X86_EXE),
        (PER_USER_EXE, PER_USER_EXE),
    ] {
        let only = |path: &Path| path == Path::new(exists);
        let located = BrowserExecutableLocator::new(&NoRegistry, &env, &only)
            .locate(row)
            .unwrap();
        assert_eq!(located.path, PathBuf::from(expected));
        assert!(
            located
                .source_of_truth
                .contains("browsers/brave.json exe_candidates"),
            "{}",
            located.source_of_truth
        );
    }

    let all_exist = |_: &Path| true;
    for hive in [RegistryHive::LocalMachine, RegistryHive::CurrentUser] {
        let app_paths = BraveAppPaths(hive, "\"D:\\Brave\\brave.exe\"");
        let located = BrowserExecutableLocator::new(&app_paths, &env, &all_exist)
            .locate(row)
            .unwrap();
        assert_eq!(located.path, PathBuf::from("D:\\Brave\\brave.exe"));
        assert!(located.source_of_truth.contains("App Paths\\brave.exe"));
        assert!(located.source_of_truth.contains("browser_kind=brave"));
    }
}

#[test]
fn must_pass_brave_not_installed_resolves_to_clean_absence() {
    // The daemon maps "no located executable" to health status
    // `not_installed`, which never degrades overall health
    // (synapse-mcp browser_bridge_health::browser_row_health).
    let registry = chrome_and_brave();
    let row = brave(&registry);
    let env = |key: &str| fake_env(key);
    let none_exist = |_: &Path| false;
    let locator = BrowserExecutableLocator::new(&NoRegistry, &env, &none_exist);
    assert!(locator.locate(row).is_none());
    assert_eq!(
        locator.first_candidate(row),
        Some(PathBuf::from(SYSTEM_EXE))
    );

    let missing = Path::new("Z:\\absent\\BraveSoftware\\Brave-Browser\\User Data");
    let state = ChromiumProfileInspector::scan_root(row, missing);
    assert!(!state.scanned && !state.installed, "{state:?}");
    assert_eq!(state.reason, "user_data_root_unreadable");
    assert!(state.detail(row).contains("browser_kind=brave"));
}

#[test]
fn must_fail_brave_scan_never_reads_google_chrome_user_data() {
    let registry = chrome_and_brave();
    let row = brave(&registry);
    let local_app_data = tempfile::tempdir().unwrap();
    let lad = local_app_data.path().to_string_lossy().into_owned();
    let env = |key: &str| {
        key.eq_ignore_ascii_case("LOCALAPPDATA")
            .then(|| lad.clone())
    };

    let chrome_root = local_app_data.path().join("Google\\Chrome\\User Data");
    write_installed_profile(&chrome_root.join("Default"), BRIDGE_EXTENSION_ID);

    let brave_root = PathBuf::from(expand_env_vars(&row.user_data_root, &env).unwrap());
    assert_eq!(
        brave_root,
        local_app_data
            .path()
            .join("BraveSoftware\\Brave-Browser\\User Data")
    );
    assert!(!brave_root.starts_with(&chrome_root));

    let state = ChromiumProfileInspector::scan_root(row, &brave_root);
    assert!(
        !state.installed,
        "chrome's install row leaked into brave: {state:?}"
    );

    write_installed_profile(&brave_root.join("Default"), BRIDGE_EXTENSION_ID);
    let state = ChromiumProfileInspector::scan_root(row, &brave_root);
    assert!(state.scanned && state.installed, "{state:?}");
    assert_eq!(state.user_data_root.as_deref(), Some(brave_root.as_path()));
    assert!(state.extension_path.unwrap().starts_with(&brave_root));
    assert_eq!(state.code_stamp.as_deref(), Some("brave1"));
}

#[test]
fn must_pass_brave_cdp_version_chrome_token_accepted_for_brave_exe() {
    let registry =
        BrowserRegistry::from_json_sources(&[("brave.json", row_json("brave").as_str())]).unwrap();
    assert_eq!(
        brave(&registry).cdp_product_tokens,
        vec!["Chrome".to_owned()]
    );
    assert!(registry.cdp_product_is_chromium("Chrome/155.0.7727.56"));
    assert!(registry.is_chromium_process("brave.exe"));
    assert!(!registry.cdp_product_is_chromium("Firefox/158.0"));
}

#[test]
fn must_pass_brave_tab_target_roundtrip() {
    let registry = chrome_and_brave();
    let target = BridgeTabTarget::parse(&registry, "brave-tab:42")
        .unwrap()
        .unwrap();
    assert_eq!(target.kind().as_str(), "brave");
    assert_eq!(target.tab_id(), 42);
    assert_eq!(target.wire(), "brave-tab:42");
    assert_eq!(BridgeTabTarget::new(brave(&registry), 42), target);

    let element = BridgeElementTarget::parse(&registry, "brave-tab:3:frame:0:path:0.1.s.0")
        .unwrap()
        .unwrap();
    assert_eq!(element.tab().kind().as_str(), "brave");
    assert_eq!(element.path(), "0.1.s.0");

    let chrome = BridgeTabTarget::parse(&registry, "chrome-tab:42")
        .unwrap()
        .unwrap();
    assert_eq!(chrome.kind().as_str(), "chrome");
}

#[test]
fn must_pass_brave_title_suffix_and_policy_root_are_brave_not_chrome() {
    let registry = chrome_and_brave();
    let row = brave(&registry);
    assert_eq!(row.strip_window_title_suffix("Inbox - Brave"), "Inbox");
    assert_eq!(
        row.strip_window_title_suffix("Inbox - Google Chrome"),
        "Inbox - Google Chrome"
    );

    let policy = row.policy.as_ref().unwrap();
    assert_eq!(policy.root, "Software\\Policies\\BraveSoftware\\Brave");
    assert_eq!(policy.extension_settings_value, "ExtensionSettings");
    let chrome_policy = registry.legacy_default().unwrap().policy.as_ref().unwrap();
    assert_ne!(policy.root, chrome_policy.root);
}

#[test]
fn must_pass_brave_native_messaging_cleanup_covers_the_keys_brave_reads() {
    // Brave reads native hosts from the Google Chrome key (Chromium
    // launch_context_win.cc); there is no BraveSoftware NativeMessagingHosts key.
    let registry = chrome_and_brave();
    let row = brave(&registry);
    assert_eq!(
        row.native_messaging_cleanup_roots,
        vec![
            "Software\\Google\\Chrome\\NativeMessagingHosts".to_owned(),
            "Software\\Wow6432Node\\Google\\Chrome\\NativeMessagingHosts".to_owned(),
        ]
    );
    assert!(
        row.native_messaging_cleanup_roots
            .iter()
            .all(|root| !root.contains("BraveSoftware"))
    );
}

#[test]
fn must_pass_brave_component_extension_is_protected_from_suppression() {
    let registry = chrome_and_brave();
    let row = brave(&registry);
    assert!(
        row.protected_extension_ids
            .iter()
            .any(|id| id == BRAVE_COMPONENT_EXTENSION_ID)
    );
    assert!(
        row.protected_extension_ids
            .iter()
            .all(|id| id != BRIDGE_EXTENSION_ID),
        "the bridge extension itself is never on the protected list"
    );
    for id in &row.protected_extension_ids {
        assert!(
            id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte)),
            "{id} is not a Chromium extension id"
        );
    }
}
