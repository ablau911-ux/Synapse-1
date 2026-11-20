//! MY TDD contract for the Firefox WebExtension package source
//! (`extensions/synapse-firefox-bridge`): the manifest matches the firefox
//! row, requests no debugger/management/nativeMessaging permission, keeps a
//! persistent background page that aliases the namespace before the shared
//! worker runs, and never carries the bridge registration secret (the package
//! is uploaded to AMO for signing).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use serde_json::Value;
use synapse_browsers::BrowserRegistry;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn package_dir() -> PathBuf {
    repo_root()
        .join("extensions")
        .join("synapse-firefox-bridge")
}

fn manifest() -> Value {
    serde_json::from_str(&std::fs::read_to_string(package_dir().join("manifest.json")).unwrap())
        .unwrap()
}

fn firefox_extension_id() -> String {
    let text = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("browsers")
            .join("firefox.json"),
    )
    .unwrap();
    let registry = BrowserRegistry::from_json_sources(&[("firefox.json", text.as_str())]).unwrap();
    registry.by_id("firefox").unwrap().extension.id.clone()
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

/// Any run of exactly 64 lowercase hex chars: the registration token shape.
fn contains_token_shaped_secret(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut run = 0usize;
    for (index, byte) in bytes.iter().enumerate() {
        if byte.is_ascii_digit() || (b'a'..=b'f').contains(byte) {
            run += 1;
        } else {
            run = 0;
        }
        let next_is_hex = bytes
            .get(index + 1)
            .is_some_and(|next| next.is_ascii_digit() || (b'a'..=b'f').contains(next));
        if run == 64 && !next_is_hex {
            return true;
        }
    }
    false
}

#[test]
fn must_pass_firefox_manifest_matches_row() {
    let manifest = manifest();
    assert_eq!(
        manifest["browser_specific_settings"]["gecko"]["id"],
        Value::String(firefox_extension_id())
    );
    assert_eq!(
        manifest["manifest_version"], 2,
        "MV2: persistent page + promise browser.*"
    );
    assert_eq!(manifest["background"]["page"], "background.html");
    assert_eq!(manifest["background"]["persistent"], true);
    assert!(manifest["background"].get("service_worker").is_none());
    assert!(
        manifest.get("key").is_none(),
        "Chromium key has no meaning in Firefox"
    );
    assert!(
        manifest.get("web_accessible_resources").is_none(),
        "the private bridge exposes no packaged resource"
    );
}

#[test]
fn must_pass_firefox_manifest_permissions_are_webextension_only() {
    let manifest = manifest();
    let permissions: Vec<&str> = manifest["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    for required in [
        "alarms",
        "cookies",
        "downloads",
        "scripting",
        "storage",
        "tabs",
        "webNavigation",
        "webRequest",
        "<all_urls>",
        "http://127.0.0.1:7700/*",
    ] {
        assert!(permissions.contains(&required), "missing {required}");
    }
    for forbidden in ["debugger", "management", "nativeMessaging"] {
        assert!(
            !permissions.contains(&forbidden),
            "must not request {forbidden}"
        );
    }
}

#[test]
fn must_pass_background_page_aliases_namespace_before_worker() {
    let html = std::fs::read_to_string(package_dir().join("background.html")).unwrap();
    let shim = html.find("src=\"gecko_background.js\"").unwrap();
    let worker = html.find("src=\"service_worker.js\"").unwrap();
    assert!(shim < worker, "the alias module must evaluate first");
    assert_eq!(html.matches("type=\"module\"").count(), 2);
    let shim_source = std::fs::read_to_string(package_dir().join("gecko_background.js")).unwrap();
    assert!(shim_source.contains("installGeckoNamespace(globalThis)"));
}

#[test]
fn must_fail_firefox_package_contains_no_secret() {
    for file in files_under(&package_dir()) {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        assert_ne!(name, "bridge-auth.json", "{}", file.display());
        assert_ne!(
            name,
            "bridge-config.json",
            "installer generates it: {}",
            file.display()
        );
        let text = String::from_utf8_lossy(&std::fs::read(&file).unwrap()).into_owned();
        assert!(
            !contains_token_shaped_secret(&text),
            "token-shaped secret in {}",
            file.display()
        );
    }
    let adapter = std::fs::read_to_string(
        repo_root()
            .join("extensions")
            .join("synapse-chrome-debugger")
            .join("adapters")
            .join("gecko.js"),
    )
    .unwrap();
    assert!(!adapter.contains("bridge-auth.json"));
    assert!(!contains_token_shaped_secret(&adapter));
}

#[test]
fn must_fail_secret_scanner_detects_planted_token() {
    let token = "0123456789abcdef".repeat(4);
    assert!(contains_token_shaped_secret(&format!(
        "{{\"t\":\"{token}\"}}"
    )));
    assert!(
        !contains_token_shaped_secret(&format!("{token}0")),
        "65 hex is a hash, not the token"
    );
    assert!(!contains_token_shaped_secret(&token[..63]));
}
