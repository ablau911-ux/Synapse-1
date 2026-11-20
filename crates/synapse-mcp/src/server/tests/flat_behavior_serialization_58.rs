//! #58: behavior labels must never advertise nonexistent operation parameters.
use super::*;

fn wire() -> Value {
    serde_json::to_value(facade_contract_snapshots(FACADE_TOOL_CONTRACTS)).unwrap()
}

#[test]
fn must_pass_flat_facades_serialize_exact_behavior_labels_58() {
    let rows = wire();
    for (name, behavior) in [
        ("health", "status"),
        ("observe", "current"),
        ("find", "elements"),
        ("read_text", "text"),
    ] {
        let row = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["tool_name"] == name)
            .unwrap();
        assert_eq!(row["operations"].as_array().unwrap().len(), 1);
        assert_eq!(row["operations"][0]["behavior"], behavior);
        assert_eq!(row["operations"][0].get("operation"), None);
    }
}

#[test]
fn must_fail_flat_labels_cannot_leak_into_operation_facades_58() {
    let before = wire();
    let rows = before.as_array().unwrap();
    let routine = rows.iter().find(|r| r["tool_name"] == "routine").unwrap();
    let mine = routine["operations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["operation"] == "mine")
        .unwrap();
    assert_eq!(mine.get("behavior"), None);
    assert_eq!(mine["operation"], "mine");
    for row in rows
        .iter()
        .filter(|r| r["operation_enum"] == FLAT_FACADE_OPERATION_ENUM)
    {
        for operation in row["operations"].as_array().unwrap() {
            assert_eq!(operation.get("operation"), None);
            assert!(
                operation["behavior"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
            );
        }
    }
    assert_eq!(wire(), before, "serialization must not mutate registry");
}
