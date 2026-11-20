//! MY TDD contract for the declared browser registry (`browsers/*.json`).
//! must_pass_*: the data table loads and indexes rows; must_fail_*: a broken
//! table is refused at load time with the offending files named.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use synapse_browsers::{BrowserFamily, BrowserRegistry, BrowserRegistryError};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn chrome_json() -> String {
    std::fs::read_to_string(manifest_dir().join("browsers").join("chrome.json")).unwrap()
}

fn testium_json() -> String {
    std::fs::read_to_string(
        manifest_dir()
            .join("tests")
            .join("fixtures")
            .join("testium.json"),
    )
    .unwrap()
}

/// `testium.json` with one top-level string field replaced, for the
/// duplicate/invalid-row cases.
fn testium_with(field: &str, value: serde_json::Value) -> String {
    let mut row: serde_json::Value = serde_json::from_str(&testium_json()).unwrap();
    row[field] = value;
    row.to_string()
}

#[test]
fn must_pass_registry_loads_rows_from_dir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("chrome.json"), chrome_json()).unwrap();
    std::fs::write(dir.path().join("testium.json"), testium_json()).unwrap();
    std::fs::write(dir.path().join("README.txt"), "not a row").unwrap();

    let registry = BrowserRegistry::load_dir(dir.path()).unwrap();

    let ids: Vec<&str> = registry.rows().iter().map(|row| row.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["chrome", "testium"],
        "rows load in file-name order"
    );
    assert_eq!(
        registry.by_id("testium").unwrap().health_key,
        "testium_bridge"
    );
    assert_eq!(registry.legacy_default().unwrap().id.as_str(), "chrome");
    assert_eq!(
        registry.by_id("chrome").unwrap().family,
        BrowserFamily::Chromium
    );
    assert!(registry.by_id("edge").is_none());
}

#[test]
fn must_pass_bundled_rows_load_and_contain_the_legacy_default() {
    let registry = BrowserRegistry::load_dir(&manifest_dir().join("browsers")).unwrap();
    assert!(registry.legacy_default().is_some());
    let embedded = BrowserRegistry::embedded_baseline();
    assert_eq!(
        embedded.legacy_default().unwrap(),
        registry.legacy_default().unwrap(),
        "embedded fallback must be the same chrome.json the dir ships"
    );
}

#[test]
fn must_fail_registry_duplicate_id() {
    let duplicate = testium_with("id", serde_json::json!("chrome"));
    let error = BrowserRegistry::from_json_sources(&[
        ("chrome.json", chrome_json().as_str()),
        ("copy.json", duplicate.as_str()),
    ])
    .unwrap_err();
    assert!(
        matches!(&error, BrowserRegistryError::Duplicate { field: "id", .. }),
        "{error}"
    );
    let text = error.to_string();
    assert!(
        text.contains("chrome.json") && text.contains("copy.json"),
        "{text}"
    );
}

#[test]
fn must_fail_registry_duplicate_tab_prefix() {
    let duplicate = testium_with("tab_target_prefix", serde_json::json!("chrome-tab:"));
    let error = BrowserRegistry::from_json_sources(&[
        ("chrome.json", chrome_json().as_str()),
        ("testium.json", duplicate.as_str()),
    ])
    .unwrap_err();
    assert!(
        matches!(
            &error,
            BrowserRegistryError::Duplicate {
                field: "tab_target_prefix",
                ..
            }
        ),
        "{error}"
    );
    assert!(error.to_string().contains("testium.json"), "{error}");
}

#[test]
fn must_fail_registry_duplicate_health_key() {
    let duplicate = testium_with("health_key", serde_json::json!("chrome_bridge"));
    let error = BrowserRegistry::from_json_sources(&[
        ("chrome.json", chrome_json().as_str()),
        ("testium.json", duplicate.as_str()),
    ])
    .unwrap_err();
    assert!(
        matches!(
            &error,
            BrowserRegistryError::Duplicate {
                field: "health_key",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn must_fail_registry_duplicate_registration_token_file() {
    let duplicate = testium_with(
        "registration_token_file",
        serde_json::json!("chrome-bridge-token.txt"),
    );
    let error = BrowserRegistry::from_json_sources(&[
        ("chrome.json", chrome_json().as_str()),
        ("testium.json", duplicate.as_str()),
    ])
    .unwrap_err();
    assert!(
        matches!(
            &error,
            BrowserRegistryError::Duplicate {
                field: "registration_token_file",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn must_fail_row_unknown_field() {
    let unknown = testium_with("surprise", serde_json::json!(true));
    let error =
        BrowserRegistry::from_json_sources(&[("testium.json", unknown.as_str())]).unwrap_err();
    assert!(
        matches!(&error, BrowserRegistryError::RowInvalid { .. }),
        "{error}"
    );
    let text = error.to_string();
    assert!(
        text.contains("testium.json") && text.contains("surprise"),
        "{text}"
    );
}

#[test]
fn must_fail_row_unknown_family() {
    let unknown = testium_with("family", serde_json::json!("webkit"));
    let error =
        BrowserRegistry::from_json_sources(&[("testium.json", unknown.as_str())]).unwrap_err();
    assert!(
        matches!(&error, BrowserRegistryError::RowInvalid { .. }),
        "{error}"
    );
}

#[test]
fn must_fail_row_invalid_kind_id_or_prefix_shape() {
    for (field, value) in [
        ("id", serde_json::json!("Edge Browser")),
        ("tab_target_prefix", serde_json::json!("testium:")),
        ("health_key", serde_json::json!("testium bridge")),
        (
            "registration_token_file",
            serde_json::json!("..\\escape.txt"),
        ),
        ("process_names", serde_json::json!([])),
    ] {
        let broken = testium_with(field, value);
        let error = BrowserRegistry::from_json_sources(&[("testium.json", broken.as_str())])
            .expect_err(field);
        assert!(
            matches!(&error, BrowserRegistryError::RowInvalid { .. }),
            "{field}: {error}"
        );
    }
}

#[test]
fn must_fail_gecko_row_that_expects_the_debugger_api() {
    let mut row: serde_json::Value = serde_json::from_str(&testium_json()).unwrap();
    row["family"] = serde_json::json!("gecko");
    row["profile_layout"] = serde_json::json!("gecko");
    row["extension"]["origin_scheme"] = serde_json::json!("moz-extension");
    row["debugger_api_expected"] = serde_json::json!(true);
    let error = BrowserRegistry::from_json_sources(&[("testium.json", row.to_string().as_str())])
        .unwrap_err();
    assert!(
        error.to_string().contains("debugger_api_expected"),
        "{error}"
    );
}

#[test]
fn must_pass_is_chromium_family_parity() {
    let registry = BrowserRegistry::embedded_baseline();
    for name in [
        "chrome.exe",
        "chromium.exe",
        "msedge.exe",
        "brave.exe",
        "vivaldi.exe",
        "opera.exe",
        "chrome",
        "chromium",
        "msedge",
        "brave",
        "vivaldi",
        "opera",
        "CHROME.EXE",
        "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    ] {
        assert!(
            registry.is_chromium_process(name),
            "{name} must stay chromium"
        );
    }
    for name in ["firefox.exe", "msedgewebview2.exe", "notepad.exe", ""] {
        assert!(
            !registry.is_chromium_process(name),
            "{name} must not be chromium"
        );
    }
}

#[test]
fn must_pass_by_process_resolves_case_insensitive_and_fails_closed_on_ambiguity() {
    let gx = testium_with("id", serde_json::json!("testium_gx"));
    let mut gx: serde_json::Value = serde_json::from_str(&gx).unwrap();
    gx["tab_target_prefix"] = serde_json::json!("testiumgx-tab:");
    gx["endpoint_base"] = serde_json::json!("synapse-bridge://testium_gx");
    gx["health_key"] = serde_json::json!("testium_gx_bridge");
    gx["registration_token_file"] = serde_json::json!("browser-bridge-token-testium_gx.txt");
    gx["exe_path_markers"] = serde_json::json!(["\\Testium GX\\"]);
    let mut plain: serde_json::Value = serde_json::from_str(&testium_json()).unwrap();
    plain["exe_path_markers"] = serde_json::json!(["\\Testium\\"]);
    let registry = BrowserRegistry::from_json_sources(&[
        ("chrome.json", chrome_json().as_str()),
        ("testium.json", plain.to_string().as_str()),
        ("testium_gx.json", gx.to_string().as_str()),
    ])
    .unwrap();

    let chrome = registry.by_process("CHROME.EXE", None).unwrap().unwrap();
    assert_eq!(chrome.id.as_str(), "chrome");
    assert!(registry.by_process("notepad.exe", None).unwrap().is_none());
    assert!(
        registry.by_process("msedge.exe", None).unwrap().is_none(),
        "a legacy chromium name without a row has no bridge kind"
    );
    let gx_row = registry
        .by_process(
            "testium.exe",
            Some("C:\\Users\\a\\AppData\\Local\\Programs\\Testium GX\\testium.exe"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(gx_row.id.as_str(), "testium_gx");
    let ambiguous = registry.by_process("testium.exe", None).unwrap_err();
    assert_eq!(
        ambiguous.code,
        synapse_core::error_codes::BROWSER_BRIDGE_KIND_UNRESOLVED
    );
}
