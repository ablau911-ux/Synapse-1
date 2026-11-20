//! MY TDD contract for row-driven executable discovery, the Chromium profile
//! inspector (multi + single layouts), the policy self-shield evaluation and
//! the gecko stub that later packages replace.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use synapse_browsers::chromium::{ChromiumProfileInspector, evaluate_self_policy_shield};
use synapse_browsers::gecko::GeckoProfileInspector;
use synapse_browsers::{
    BrowserExecutableLocator, BrowserProfileInspector, BrowserRegistry, NoRegistry, OriginRule,
    RegistryHive, RegistryReader, expand_env_vars, parse_code_stamp,
};

fn registry() -> BrowserRegistry {
    BrowserRegistry::embedded_baseline()
}

struct FakeRegistry(Option<String>);

impl RegistryReader for FakeRegistry {
    fn read_string(&self, hive: RegistryHive, subkey: &str, value_name: &str) -> Option<String> {
        (hive == RegistryHive::LocalMachine
            && subkey.ends_with("App Paths\\chrome.exe")
            && value_name.is_empty())
        .then(|| self.0.clone())
        .flatten()
    }
}

#[test]
fn must_pass_expand_env_vars_is_case_insensitive_and_fails_on_missing() {
    let env = |key: &str| match key.to_ascii_uppercase().as_str() {
        "LOCALAPPDATA" => Some("C:\\Users\\a\\AppData\\Local".to_owned()),
        "PROGRAMFILES(X86)" => Some("C:\\Program Files (x86)".to_owned()),
        _ => None,
    };
    assert_eq!(
        expand_env_vars("%LocalAppData%\\Google\\Chrome\\User Data", &env).as_deref(),
        Some("C:\\Users\\a\\AppData\\Local\\Google\\Chrome\\User Data")
    );
    assert_eq!(
        expand_env_vars("%ProgramFiles(x86)%\\x.exe", &env).as_deref(),
        Some("C:\\Program Files (x86)\\x.exe")
    );
    assert_eq!(expand_env_vars("%MISSING%\\x", &env), None);
    assert_eq!(expand_env_vars("%unterminated", &env), None);
}

#[test]
fn must_pass_locator_prefers_app_paths_then_candidates() {
    let registry = registry();
    let chrome = registry.legacy_default().unwrap();
    let env = |key: &str| match key {
        "ProgramFiles" => Some("C:\\PF".to_owned()),
        "LOCALAPPDATA" => Some("C:\\LAD".to_owned()),
        _ => None,
    };
    let app_paths = FakeRegistry(Some("D:\\Chrome\\chrome.exe".to_owned()));
    let all_exist = |_: &Path| true;
    let located = BrowserExecutableLocator::new(&app_paths, &env, &all_exist)
        .locate(chrome)
        .unwrap();
    assert_eq!(located.path, PathBuf::from("D:\\Chrome\\chrome.exe"));
    assert!(
        located.source_of_truth.contains("App Paths"),
        "{}",
        located.source_of_truth
    );

    let only_local = |path: &Path| path.starts_with("C:\\LAD");
    let located = BrowserExecutableLocator::new(&NoRegistry, &env, &only_local)
        .locate(chrome)
        .unwrap();
    assert_eq!(
        located.path,
        PathBuf::from("C:\\LAD\\Google\\Chrome\\Application\\chrome.exe")
    );
    assert!(located.source_of_truth.contains("exe_candidates"));

    let none_exist = |_: &Path| false;
    assert!(
        BrowserExecutableLocator::new(&NoRegistry, &env, &none_exist)
            .locate(chrome)
            .is_none()
    );
}

fn write_prefs(profile_dir: &Path, extension_id: &str, extension_path: &Path) {
    std::fs::create_dir_all(profile_dir).unwrap();
    let prefs = serde_json::json!({
        "extensions": {"settings": {extension_id: {"path": extension_path.to_string_lossy()}}}
    });
    std::fs::write(profile_dir.join("Preferences"), prefs.to_string()).unwrap();
}

#[test]
fn must_pass_chromium_inspector_reads_active_multi_profile() {
    let registry = registry();
    let chrome = registry.legacy_default().unwrap();
    let root = tempfile::tempdir().unwrap();
    let extension_dir = root.path().join("ext");
    std::fs::create_dir_all(&extension_dir).unwrap();
    std::fs::write(
        extension_dir.join("service_worker.js"),
        "const BRIDGE_CODE_STAMP = \"abc123\";\n",
    )
    .unwrap();
    write_prefs(
        &root.path().join("Profile 1"),
        &chrome.extension.id,
        &extension_dir,
    );
    std::fs::create_dir_all(root.path().join("Default")).unwrap();
    std::fs::write(
        root.path().join("Local State"),
        r#"{"profile":{"last_used":"Profile 1"}}"#,
    )
    .unwrap();

    let state = ChromiumProfileInspector::scan_root(chrome, root.path());
    assert!(state.scanned && state.installed, "{state:?}");
    assert_eq!(state.active_profile.as_deref(), Some("Profile 1"));
    assert_eq!(
        state.extension_path.as_deref(),
        Some(extension_dir.as_path())
    );
    assert_eq!(state.code_stamp.as_deref(), Some("abc123"));
    assert_eq!(
        state.service_worker_sha256.as_ref().map(String::len),
        Some(64)
    );
}

#[test]
fn must_pass_chromium_inspector_single_layout_reads_root_preferences() {
    let registry = registry();
    let mut row = registry.legacy_default().unwrap().clone();
    row.profile_layout = synapse_browsers::ProfileLayout::Single;
    let root = tempfile::tempdir().unwrap();
    let extension_dir = root.path().join("ext");
    std::fs::create_dir_all(&extension_dir).unwrap();
    std::fs::write(extension_dir.join("service_worker.js"), "// no stamp\n").unwrap();
    write_prefs(root.path(), &row.extension.id, &extension_dir);

    let state = ChromiumProfileInspector::scan_root(&row, root.path());
    assert!(state.installed, "{state:?}");
    assert_eq!(state.code_stamp, None);
}

#[test]
fn must_fail_chromium_inspector_missing_root_reports_not_scanned() {
    let registry = registry();
    let chrome = registry.legacy_default().unwrap();
    let state =
        ChromiumProfileInspector::scan_root(chrome, Path::new("Z:\\definitely\\absent\\root"));
    assert!(!state.scanned && !state.installed);
    assert_eq!(state.reason, "user_data_root_unreadable");
}

#[test]
fn must_pass_chromium_origin_rule_and_gecko_stub() {
    let registry = registry();
    let chrome = registry.legacy_default().unwrap();
    let rule = ChromiumProfileInspector.expected_origin(chrome);
    assert!(rule.accepts("chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk"));
    assert!(rule.accepts("chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/"));
    assert!(!rule.accepts("chrome-extension://aaaabbbbccccddddeeeeffffgggghhhh"));
    assert!(!rule.accepts("moz-extension://leoocgnkjnplbfdbklajepahofecgfbk"));

    let stub = GeckoProfileInspector.install_state(chrome);
    assert!(!stub.scanned);
    assert!(matches!(
        GeckoProfileInspector.expected_origin(chrome),
        OriginRule::Unknown { .. }
    ));
    assert!(
        !OriginRule::Unknown {
            reason: "x".to_owned()
        }
        .accepts("moz-extension://x")
    );
}

#[test]
fn must_pass_policy_shield_evaluation_matches_legacy_reasons() {
    let id = "leoocgnkjnplbfdbklajepahofecgfbk";
    let marker = "marker";
    assert_eq!(
        evaluate_self_policy_shield(None, id, marker).reason,
        "value_missing_or_unreadable"
    );
    assert_eq!(
        evaluate_self_policy_shield(Some("  "), id, marker).reason,
        "value_empty"
    );
    assert_eq!(
        evaluate_self_policy_shield(Some("{"), id, marker).reason,
        "parse_error"
    );
    assert_eq!(
        evaluate_self_policy_shield(Some("{}"), id, marker).reason,
        "self_entry_missing"
    );
    let present = format!(
        r#"{{"{id}":{{"blocked_permissions":["nativeMessaging"],"blocked_install_message":"{marker}"}}}}"#
    );
    let evaluation = evaluate_self_policy_shield(Some(&present), id, marker);
    assert!(evaluation.present);
    assert_eq!(evaluation.reason, "native_messaging_self_shield_active");
    let incomplete = format!(r#"{{"{id}":{{"blocked_permissions":["debugger"]}}}}"#);
    let evaluation = evaluate_self_policy_shield(Some(&incomplete), id, marker);
    assert!(!evaluation.present);
    assert_eq!(evaluation.reason, "self_entry_incomplete");
    assert_eq!(evaluation.blocked_permissions, vec!["debugger".to_owned()]);
}

#[test]
fn must_pass_code_stamp_parse_contract() {
    assert_eq!(
        parse_code_stamp("x\n  const BRIDGE_CODE_STAMP = \"s1\";\n").as_deref(),
        Some("s1")
    );
    assert_eq!(parse_code_stamp("const BRIDGE_CODE_STAMP = \"\";"), None);
    assert_eq!(parse_code_stamp("const OTHER = \"s1\";"), None);
}
