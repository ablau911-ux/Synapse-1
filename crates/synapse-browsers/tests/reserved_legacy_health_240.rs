//! #240: an absent legacy row still owns its health namespace.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use serde_json::{Value, json};
use synapse_browsers::{BrowserRegistry, BrowserRegistryError};

fn edge(health: &str) -> String {
    let mut row: Value = serde_json::from_str(include_str!("../browsers/chrome.json")).unwrap();
    row["id"] = json!("edge");
    row["tab_target_prefix"] = json!("edge-tab:");
    row["endpoint_base"] = json!("chrome-extension://edge");
    row["registration_token_file"] = json!("edge-token.txt");
    row["health_key"] = json!(health);
    row.to_string()
}

#[test]
fn must_pass_legacy_owner_and_distinct_nonlegacy_health_keys_240() {
    let edge = edge("edge_bridge");
    let rows = BrowserRegistry::from_json_sources(&[
        ("chrome.json", include_str!("../browsers/chrome.json")),
        ("edge.json", &edge),
    ])
    .unwrap();
    assert_eq!(rows.rows().len(), 2);
    assert_eq!(
        rows.by_health_key("chrome_bridge").unwrap().id.as_str(),
        "chrome"
    );
    assert_eq!(
        rows.by_health_key("edge_bridge").unwrap().id.as_str(),
        "edge"
    );
}

#[test]
fn must_fail_reserved_legacy_key_without_loaded_legacy_row_240() {
    let valid = edge("edge_bridge");
    let baseline = BrowserRegistry::from_json_sources(&[("edge.json", &valid)]).unwrap();
    assert_eq!(baseline.rows().len(), 1);
    assert!(baseline.by_health_key("chrome_bridge").is_none());
    let invalid = edge("chrome_bridge");
    assert_eq!(
        BrowserRegistry::from_json_sources(&[("edge.json", &invalid)]).unwrap_err(),
        BrowserRegistryError::Duplicate {
            field: "health_key",
            value: "chrome_bridge".into(),
            first: "embedded:browsers/chrome.json".into(),
            second: "edge.json".into()
        }
    );
    // The embedded owner is stable; the offender must name the actual source.
    assert_eq!(
        BrowserRegistry::from_json_sources(&[("renamed-edge.json", &invalid)]).unwrap_err(),
        BrowserRegistryError::Duplicate {
            field: "health_key",
            value: "chrome_bridge".into(),
            first: "embedded:browsers/chrome.json".into(),
            second: "renamed-edge.json".into()
        }
    );
    assert_eq!(
        baseline.by_health_key("edge_bridge").unwrap().id.as_str(),
        "edge"
    );
    assert!(baseline.by_health_key("chrome_bridge").is_none());
    assert_eq!(
        BrowserRegistry::from_json_sources(&[("edge.json", &valid)])
            .unwrap()
            .rows(),
        baseline.rows(),
        "refusal must not reserve keys or poison subsequent loads"
    );
}
