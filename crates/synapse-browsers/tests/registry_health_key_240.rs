//! FU1 #240: distinct browser rows cannot overwrite one health subsystem key.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use serde_json::{Value, json};
use synapse_browsers::{BrowserRegistry, BrowserRegistryError};

fn row(id: &str, health: &str) -> String {
    let mut value: Value = serde_json::from_str(include_str!("../browsers/chrome.json")).unwrap();
    value["id"] = json!(id);
    value["tab_target_prefix"] = json!(format!("{id}-tab:"));
    value["health_key"] = json!(health);
    value["endpoint_base"] = json!(format!("chrome-extension://{id}"));
    value["registration_token_file"] = json!(format!("{id}-token.txt"));
    value.to_string()
}

#[test]
fn must_pass_distinct_health_keys_preserve_both_rows_240() {
    let a = row("edge", "edge_bridge");
    let b = row("brave", "brave_bridge");
    let registry = BrowserRegistry::from_json_sources(&[("a.json", &a), ("b.json", &b)]).unwrap();
    assert_eq!(registry.rows().len(), 2);
    assert_eq!(
        registry.by_health_key("edge_bridge").unwrap().id.as_str(),
        "edge"
    );
    assert_eq!(
        registry.by_health_key("brave_bridge").unwrap().id.as_str(),
        "brave"
    );
}

#[test]
fn must_fail_duplicate_health_key_refuses_entire_registry_240() {
    let a = row("edge", "shared_bridge");
    let b = row("brave", "shared_bridge");
    let baseline = BrowserRegistry::from_json_sources(&[("a.json", &a)]).unwrap();
    assert_eq!(baseline.rows().len(), 1);
    let error = BrowserRegistry::from_json_sources(&[("a.json", &a), ("b.json", &b)]).unwrap_err();
    assert_eq!(
        error,
        BrowserRegistryError::Duplicate {
            field: "health_key",
            value: "shared_bridge".to_owned(),
            first: "a.json".to_owned(),
            second: "b.json".to_owned(),
        }
    );
    assert_eq!(baseline.rows().len(), 1);
    assert_eq!(
        baseline.by_health_key("shared_bridge").unwrap().id.as_str(),
        "edge"
    );
    assert_eq!(
        BrowserRegistry::from_json_sources(&[("a.json", &a)])
            .unwrap()
            .rows(),
        baseline.rows()
    );
}
