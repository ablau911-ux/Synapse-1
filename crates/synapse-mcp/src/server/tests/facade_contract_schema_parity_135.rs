//! #135 raid: the facade-contract/live-schema parity gate
//! (`facade_contract_operations_match_live_schema`, one of the eight #135 tests) was deleted
//! with the inline test blocks in 8081a84c and never replaced. These tests restore it:
//! FACADE_TOOL_CONTRACTS must list exactly the operation enum each built tool serializes
//! (the same sanitized schema a strict MCP client parses), and flat tools must carry
//! FLAT_FACADE_OPERATION_ENUM. A drift detector that stops detecting drift fails too.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use synapse_core::error_codes;

use super::{
    FACADE_TOOL_CONTRACTS, FLAT_FACADE_OPERATION_ENUM, FacadeOperationContractSpec,
    FacadeToolContractSpec, facade_contract, op,
};
use crate::server::{SynapseService, schema_sanitize::sanitize_tools};

/// Every `enum`/`const` string reachable from an `operation` schema node
/// (inline enum, `$ref` into `$defs`, `oneOf`/`anyOf`/`allOf` variants).
fn collect_enum_consts(root: &Value, node: &Value, out: &mut BTreeSet<String>) {
    if let Some(values) = node.get("enum").and_then(Value::as_array) {
        out.extend(values.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    if let Some(text) = node.get("const").and_then(Value::as_str) {
        out.insert(text.to_owned());
    }
    for combinator in ["oneOf", "anyOf", "allOf"] {
        for variant in node
            .get(combinator)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            collect_enum_consts(root, variant, out);
        }
    }
    let def = node
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix("#/$defs/"))
        .and_then(|name| root.get("$defs").and_then(|defs| defs.get(name)));
    if let Some(def) = def {
        collect_enum_consts(root, def, out);
    }
}

fn schema_operation_names(schema: &Value) -> Option<BTreeSet<String>> {
    let operation = schema.get("properties")?.get("operation")?;
    let mut out = BTreeSet::new();
    collect_enum_consts(schema, operation, &mut out);
    Some(out)
}

fn built_schemas() -> BTreeMap<String, Value> {
    sanitize_tools(SynapseService::tool_router().list_all())
        .iter()
        .map(|tool| {
            (
                tool.name.as_ref().to_owned(),
                Value::Object((*tool.input_schema).clone()),
            )
        })
        .collect()
}

/// One failure line per contract that disagrees with the built schema.
fn drift(contracts: &[FacadeToolContractSpec], schemas: &BTreeMap<String, Value>) -> Vec<String> {
    let mut failures = Vec::new();
    for contract in contracts {
        let Some(schema) = schemas.get(contract.tool_name) else {
            failures.push(format!(
                "{}: tool missing from built surface",
                contract.tool_name
            ));
            continue;
        };
        let declared: BTreeSet<String> = contract
            .operations
            .iter()
            .map(|operation| operation.operation.to_owned())
            .collect();
        let flat = contract.operation_enum == FLAT_FACADE_OPERATION_ENUM;
        match (schema_operation_names(schema), flat) {
            (None, true) => {}
            (None, false) => failures.push(format!(
                "{}: no operation property but operation_enum={:?}",
                contract.tool_name, contract.operation_enum
            )),
            (Some(live), true) => failures.push(format!(
                "{}: marked flat but schema exposes {live:?}",
                contract.tool_name
            )),
            (Some(live), false) if live != declared => failures.push(format!(
                "{}: operation drift schema={live:?} contract={declared:?}",
                contract.tool_name
            )),
            (Some(_), false) => {}
        }
    }
    failures
}

const WRONG_STORAGE_OPS: &[FacadeOperationContractSpec] = &[
    op(
        "read",
        false,
        true,
        "wrong",
        None,
        error_codes::ACTION_TARGET_INVALID,
        "wrong",
    ),
    op(
        "write",
        true,
        true,
        "wrong",
        Some("wrong"),
        error_codes::TOOL_PARAMS_INVALID,
        "wrong",
    ),
];
const FLAT_HEALTH_OPS: &[FacadeOperationContractSpec] = &[op(
    "status",
    false,
    false,
    "wrong",
    None,
    error_codes::TOOL_INTERNAL_ERROR,
    "wrong",
)];

#[test]
fn must_pass_facade_contract_operations_match_live_schema_135() {
    let schemas = built_schemas();
    assert!(
        schemas.len() >= FACADE_TOOL_CONTRACTS.len(),
        "built surface ({}) smaller than the contract list ({})",
        schemas.len(),
        FACADE_TOOL_CONTRACTS.len()
    );
    let failures = drift(FACADE_TOOL_CONTRACTS, &schemas);
    assert!(
        failures.is_empty(),
        "FACADE_TOOL_CONTRACTS drifted from the built tool schema:\n{}",
        failures.join("\n")
    );
}

#[test]
fn must_fail_parity_gate_detects_wrong_ops_and_a_fabricated_flat_enum_135() {
    let schemas = built_schemas();
    let wrong_ops = [facade_contract(
        "browser_storage",
        "BrowserStorageOperation",
        "wrong",
        WRONG_STORAGE_OPS,
    )];
    let failures = drift(&wrong_ops, &schemas);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].starts_with("browser_storage: operation drift")
            && failures[0].contains("save_state"),
        "{failures:?}"
    );

    let fabricated = [facade_contract(
        "health",
        "HealthOperation",
        "wrong",
        FLAT_HEALTH_OPS,
    )];
    let failures = drift(&fabricated, &schemas);
    assert_eq!(
        failures,
        vec!["health: no operation property but operation_enum=\"HealthOperation\"".to_owned()]
    );

    let correct_flat = [facade_contract(
        "health",
        FLAT_FACADE_OPERATION_ENUM,
        "ok",
        FLAT_HEALTH_OPS,
    )];
    assert_eq!(drift(&correct_flat, &schemas), Vec::<String>::new());
}
