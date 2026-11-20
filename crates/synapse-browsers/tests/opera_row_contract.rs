//! MY TDD contract for the Opera and Opera GX rows (`browsers/opera.json`,
//! `browsers/opera_gx.json`). Written from the correct contract before the
//! rows existed.
//!
//! Both browsers run as `opera.exe`, so a window is told apart ONLY by its
//! executable path (`exe_path_markers`), never by the image name alone.
//! must_pass_*: both rows load with disjoint wire ids and each install path
//! resolves to its own row; must_fail_*: an `opera.exe` whose path matches
//! neither row resolves to neither, a bare `opera.exe` is refused as
//! ambiguous, and one variant's install never stands in for the other.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use synapse_browsers::chromium::ChromiumProfileInspector;
use synapse_browsers::{
    BridgeEndpoint, BridgeLane, BridgeTabTarget, BrowserExecutableLocator, BrowserFamily,
    BrowserRegistry, BrowserRow, NoRegistry, OriginRule, ProfileLayout, RegistryHive,
    RegistryReader,
};

const EXTENSION_ID: &str = "leoocgnkjnplbfdbklajepahofecgfbk";
const OPERA_USER_EXE: &str = "C:\\Users\\a\\AppData\\Local\\Programs\\Opera\\opera.exe";
const OPERA_VERSIONED_EXE: &str =
    "C:\\Users\\a\\AppData\\Local\\Programs\\Opera\\137.0.6036.39\\opera.exe";
const OPERA_MACHINE_EXE: &str = "C:\\Program Files\\Opera\\137.0.6036.39\\opera.exe";
const GX_USER_EXE: &str = "C:\\Users\\a\\AppData\\Local\\Programs\\Opera GX\\opera.exe";
const GX_VERSIONED_EXE: &str =
    "C:\\Users\\a\\AppData\\Local\\Programs\\Opera GX\\136.0.6008.111\\opera.exe";
const GX_MACHINE_EXE: &str = "C:\\Program Files (x86)\\Opera GX\\136.0.6008.111\\opera.exe";

fn rows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("browsers")
}

fn row_json(id: &str) -> String {
    std::fs::read_to_string(rows_dir().join(format!("{id}.json"))).unwrap()
}

/// Chrome (legacy default) plus both Opera rows, exactly as shipped.
fn chrome_and_operas() -> BrowserRegistry {
    let chrome = row_json("chrome");
    let opera = row_json("opera");
    let gx = row_json("opera_gx");
    BrowserRegistry::from_json_sources(&[
        ("chrome.json", chrome.as_str()),
        ("opera.json", opera.as_str()),
        ("opera_gx.json", gx.as_str()),
    ])
    .unwrap()
}

fn resolved_id(registry: &BrowserRegistry, path: &str) -> Option<String> {
    registry
        .by_process("opera.exe", Some(path))
        .unwrap()
        .map(|row| row.id.as_str().to_owned())
}

#[test]
fn must_pass_opera_and_gx_rows_load_distinct() {
    let registry = chrome_and_operas();
    let opera = registry.by_id("opera").unwrap();
    let gx = registry.by_id("opera_gx").unwrap();

    for row in [opera, gx] {
        assert_eq!(row.family, BrowserFamily::Chromium);
        assert_eq!(row.process_names, vec!["opera.exe".to_owned()]);
        assert!(!row.exe_path_markers.is_empty(), "{} needs markers", row.id);
        assert_eq!(row.app_paths_name, None, "shared opera.exe App Paths key");
        assert_eq!(row.extension.id, EXTENSION_ID);
        assert_eq!(row.extension.extensions_page_url, "opera://extensions");
        assert_eq!(
            row.chromium_origin_rule(),
            OriginRule::Exact(format!("chrome-extension://{EXTENSION_ID}"))
        );
        assert!(row.policy.is_none(), "Opera reads no Chrome policy root");
        assert!(row.native_messaging_cleanup_roots.is_empty());
        assert!(row.debugger_api_expected, "Opera documents chrome.debugger");
        assert!(row.unsupported_capabilities.is_empty());
        assert_eq!(row.strip_window_title_suffix("Inbox - Opera"), "Inbox");
    }
    assert_eq!(opera.tab_target_prefix, "opera-tab:");
    assert_eq!(gx.tab_target_prefix, "operagx-tab:");
    assert_eq!(opera.health_key, "opera_bridge");
    assert_eq!(gx.health_key, "opera_gx_bridge");
    assert_eq!(
        opera.registration_token_file,
        "browser-bridge-token-opera.txt"
    );
    assert_eq!(
        gx.registration_token_file,
        "browser-bridge-token-opera_gx.txt"
    );
    assert_eq!(opera.endpoint_base, "synapse-bridge://opera");
    assert_eq!(gx.endpoint_base, "synapse-bridge://opera_gx");
    assert_eq!(opera.extension.deploy_subdir, "browser-extension\\opera");
    assert_eq!(gx.extension.deploy_subdir, "browser-extension\\opera_gx");
    assert_eq!(opera.winget_id.as_deref(), Some("Opera.Opera"));
    assert_eq!(gx.winget_id.as_deref(), Some("Opera.OperaGX"));
}

#[test]
fn must_pass_opera_rows_keep_their_own_user_data_layouts() {
    let registry = chrome_and_operas();
    let opera = registry.by_id("opera").unwrap();
    let gx = registry.by_id("opera_gx").unwrap();
    assert_eq!(
        opera.user_data_root,
        "%APPDATA%\\Opera Software\\Opera Stable"
    );
    assert_eq!(opera.profile_layout, ProfileLayout::Multi);
    assert_eq!(
        gx.user_data_root,
        "%APPDATA%\\Opera Software\\Opera GX Stable"
    );
    assert_eq!(gx.profile_layout, ProfileLayout::Single);
}

#[test]
fn must_pass_bundled_rows_dir_loads_with_both_opera_rows() {
    let registry = BrowserRegistry::load_dir(&rows_dir()).unwrap();
    assert!(registry.legacy_default().is_some());
    assert!(registry.by_id("opera").is_some());
    assert!(registry.by_id("opera_gx").is_some());
}

#[test]
fn must_pass_opera_exe_resolves_to_its_row_by_install_path() {
    let registry = chrome_and_operas();
    for path in [
        OPERA_USER_EXE,
        OPERA_VERSIONED_EXE,
        OPERA_MACHINE_EXE,
        "c:/users/a/appdata/local/programs/opera/opera.exe",
    ] {
        assert_eq!(
            resolved_id(&registry, path).as_deref(),
            Some("opera"),
            "{path}"
        );
    }
    for path in [
        GX_USER_EXE,
        GX_VERSIONED_EXE,
        GX_MACHINE_EXE,
        "C:\\PROGRAM FILES\\OPERA GX\\OPERA.EXE",
    ] {
        assert_eq!(
            resolved_id(&registry, path).as_deref(),
            Some("opera_gx"),
            "{path}"
        );
    }
    let upper = registry.by_process("OPERA.EXE", Some(GX_USER_EXE)).unwrap();
    assert_eq!(upper.unwrap().id.as_str(), "opera_gx");
}

#[test]
fn must_fail_unknown_path_opera_exe_resolves_to_neither() {
    let registry = chrome_and_operas();
    for path in [
        "C:\\Tools\\Portable\\opera.exe",
        "C:\\Users\\a\\AppData\\Local\\Programs\\Opera beta\\opera.exe",
        "C:\\Users\\a\\AppData\\Local\\Programs\\Opera developer\\opera.exe",
        "C:\\Users\\a\\AppData\\Local\\Programs\\Opera Air\\opera.exe",
        "D:\\Opera GX Backup\\opera.exe",
        "",
    ] {
        assert_eq!(resolved_id(&registry, path), None, "{path:?}");
    }
}

#[test]
fn must_fail_bare_opera_exe_without_path_is_ambiguous() {
    let registry = chrome_and_operas();
    let error = registry.by_process("opera.exe", None).unwrap_err();
    assert_eq!(
        error.code,
        synapse_core::error_codes::BROWSER_BRIDGE_KIND_UNRESOLVED
    );
    assert!(
        error.detail.contains("opera") && error.detail.contains("opera_gx"),
        "{}",
        error.detail
    );
}

#[test]
fn must_fail_non_browser_images_in_opera_dirs_never_resolve() {
    let registry = chrome_and_operas();
    for (image, path) in [
        (
            "launcher.exe",
            "C:\\Users\\a\\AppData\\Local\\Programs\\Opera\\launcher.exe",
        ),
        ("opera_crashreporter.exe", GX_VERSIONED_EXE),
    ] {
        assert!(
            registry.by_process(image, Some(path)).unwrap().is_none(),
            "{image}"
        );
    }
    let chrome = registry
        .by_process("chrome.exe", Some(OPERA_USER_EXE))
        .unwrap()
        .unwrap();
    assert_eq!(
        chrome.id.as_str(),
        "chrome",
        "markers never steal chrome.exe"
    );
}

#[test]
fn must_pass_opera_tab_targets_and_endpoints_never_cross() {
    let registry = chrome_and_operas();
    let opera_tab = BridgeTabTarget::parse(&registry, "opera-tab:7")
        .unwrap()
        .unwrap();
    let gx_tab = BridgeTabTarget::parse(&registry, "operagx-tab:7")
        .unwrap()
        .unwrap();
    assert_eq!(opera_tab.kind().as_str(), "opera");
    assert_eq!(gx_tab.kind().as_str(), "opera_gx");
    assert_eq!(gx_tab.tab_id(), 7);

    for id in ["opera", "opera_gx"] {
        let row = registry.by_id(id).unwrap();
        let wire = row.endpoint(BridgeLane::Tabs);
        let parsed = BridgeEndpoint::parse(&registry, &wire).unwrap();
        assert_eq!(parsed.kind().as_str(), id, "{wire}");
        assert_eq!(registry.endpoint_kind(&wire).unwrap().id.as_str(), id);
    }
    assert_eq!(
        registry.by_tab_target("chrome-tab:7").unwrap().id.as_str(),
        "chrome"
    );
}

/// App Paths answers `opera.exe` with whichever variant installed last.
struct SharedOperaAppPaths;

impl RegistryReader for SharedOperaAppPaths {
    fn read_string(&self, _hive: RegistryHive, subkey: &str, _value: &str) -> Option<String> {
        subkey
            .to_ascii_lowercase()
            .ends_with("app paths\\opera.exe")
            .then(|| GX_USER_EXE.to_owned())
    }
}

fn host_env(key: &str) -> Option<String> {
    match key.to_ascii_uppercase().as_str() {
        "LOCALAPPDATA" => Some("C:\\Users\\a\\AppData\\Local".to_owned()),
        "APPDATA" => Some("C:\\Users\\a\\AppData\\Roaming".to_owned()),
        "PROGRAMFILES" => Some("C:\\Program Files".to_owned()),
        "PROGRAMFILES(X86)" => Some("C:\\Program Files (x86)".to_owned()),
        _ => None,
    }
}

fn locate(row: &BrowserRow, registry: &dyn RegistryReader, exists: &str) -> Option<PathBuf> {
    let is_file = |path: &Path| path == Path::new(exists);
    BrowserExecutableLocator::new(registry, &host_env, &is_file)
        .locate(row)
        .map(|located| located.path)
}

#[test]
fn must_pass_not_installed_opera_rows_locate_nothing() {
    let registry = chrome_and_operas();
    for id in ["opera", "opera_gx"] {
        let row = registry.by_id(id).unwrap();
        assert_eq!(locate(row, &NoRegistry, "Z:\\nothing.exe"), None, "{id}");
    }
}

#[test]
fn must_fail_one_opera_variant_never_locates_as_the_other() {
    let registry = chrome_and_operas();
    let opera = registry.by_id("opera").unwrap();
    let gx = registry.by_id("opera_gx").unwrap();
    assert_eq!(
        locate(opera, &SharedOperaAppPaths, GX_USER_EXE),
        None,
        "GX install (and its App Paths opera.exe) must leave Opera not_installed"
    );
    assert_eq!(
        locate(gx, &SharedOperaAppPaths, GX_USER_EXE),
        Some(PathBuf::from(GX_USER_EXE))
    );
    assert_eq!(locate(gx, &NoRegistry, OPERA_USER_EXE), None);
    assert_eq!(
        locate(opera, &NoRegistry, OPERA_USER_EXE),
        Some(PathBuf::from(OPERA_USER_EXE))
    );
}

#[test]
fn must_pass_opera_capabilities_equal_the_global_set() {
    let registry = chrome_and_operas();
    let global = [
        "listTabs",
        "debuggerAttach",
        "evaluateScript",
        "pageScreenshot",
    ];
    for id in ["opera", "opera_gx"] {
        let row = registry.by_id(id).unwrap();
        assert_eq!(row.required_capabilities(&global), global.to_vec(), "{id}");
    }
}

fn write_prefs(dir: &Path, extension_path: Option<&Path>) {
    std::fs::create_dir_all(dir).unwrap();
    let settings = extension_path.map_or_else(
        || serde_json::json!({}),
        |path| serde_json::json!({ EXTENSION_ID: { "path": path.to_string_lossy() } }),
    );
    let prefs = serde_json::json!({ "extensions": { "settings": settings } });
    std::fs::write(dir.join("Preferences"), prefs.to_string()).unwrap();
}

#[test]
fn must_pass_opera_gx_single_layout_reads_root_preferences() {
    let registry = chrome_and_operas();
    let gx = registry.by_id("opera_gx").unwrap();
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("Opera GX Stable");
    let extension_dir = base.path().join("ext");
    std::fs::create_dir_all(&extension_dir).unwrap();
    write_prefs(&root, Some(&extension_dir));

    let state = ChromiumProfileInspector::scan_root(gx, &root);
    assert!(state.scanned && state.installed, "{state:?}");
    assert_eq!(state.active_profile.as_deref(), Some("Opera GX Stable"));
    assert_eq!(
        state.extension_path.as_deref(),
        Some(extension_dir.as_path())
    );
}

#[test]
fn must_pass_opera_multi_layout_reads_default_profile() {
    let registry = chrome_and_operas();
    let opera = registry.by_id("opera").unwrap();
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("Opera Stable");
    let extension_dir = base.path().join("ext");
    std::fs::create_dir_all(&extension_dir).unwrap();
    write_prefs(&root.join("Default"), Some(&extension_dir));
    std::fs::write(
        root.join("Local State"),
        r#"{"profile":{"last_used":"Default"}}"#,
    )
    .unwrap();

    let state = ChromiumProfileInspector::scan_root(opera, &root);
    assert!(state.installed, "{state:?}");
    assert_eq!(state.active_profile.as_deref(), Some("Default"));
}

#[test]
fn must_fail_opera_stale_root_preferences_from_migration_are_ignored() {
    let registry = chrome_and_operas();
    let opera = registry.by_id("opera").unwrap();
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("Opera Stable");
    let stale_dir = base.path().join("stale-ext");
    std::fs::create_dir_all(&stale_dir).unwrap();
    write_prefs(&root, Some(&stale_dir));
    write_prefs(&root.join("Default"), None);
    std::fs::write(
        root.join("Local State"),
        r#"{"profile":{"last_used":"Default"}}"#,
    )
    .unwrap();

    let state = ChromiumProfileInspector::scan_root(opera, &root);
    assert!(state.scanned, "{state:?}");
    assert!(
        !state.installed,
        "a pre-migration root Preferences duplicate must not count as installed: {state:?}"
    );
    assert_eq!(state.extension_path, None);
}
