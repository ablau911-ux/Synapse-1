//! Regression tests for #58: every advertised capability route must name a
//! callable public tool and, when it names an `operation=`, a value that tool's
//! real input schema accepts. Routes that fail this send agents into a
//! `TOOL_PARAMS_INVALID` on their next call.

use std::num::NonZeroUsize;

use serde_json::Value;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{m2::M2ServiceConfig, m3::M3ServiceConfig, m4::M4ServiceConfig};

fn service(dir: &std::path::Path) -> anyhow::Result<SynapseService> {
    SynapseService::try_with_m2_shutdown_reason_and_m3_config(
        CancellationToken::new(),
        "test",
        CancellationToken::new(),
        &M2ServiceConfig::default(),
        M3ServiceConfig::from_cli_parts(
            Some(dir.join("db")),
            Some(dir.to_path_buf()),
            false,
            "127.0.0.1:0".to_owned(),
            NonZeroUsize::new(4).ok_or_else(|| anyhow::anyhow!("nonzero"))?,
            false,
            true,
            None,
            false,
            None,
        ),
        M4ServiceConfig::default(),
    )
}

/// All string values reachable under a schema node (enum / const / oneOf).
fn enum_values(node: &Value, defs: &Value, out: &mut Vec<String>) {
    if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
        let name = reference.rsplit('/').next().unwrap_or_default();
        if let Some(target) = defs.get(name) {
            enum_values(target, defs, out);
        }
    }
    if let Some(list) = node.get("enum").and_then(Value::as_array) {
        out.extend(list.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    if let Some(value) = node.get("const").and_then(Value::as_str) {
        out.push(value.to_owned());
    }
    for key in ["oneOf", "anyOf"] {
        for branch in node
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            enum_values(branch, defs, out);
        }
    }
}

/// The `operation` values a tool accepts: `None` for a flat tool (no
/// `operation` property), otherwise the schema's enum.
fn accepted_operations(tool: &Tool) -> Option<Vec<String>> {
    let schema = Value::Object((*tool.input_schema).clone());
    let operation = schema.pointer("/properties/operation")?;
    let defs = schema.get("$defs").cloned().unwrap_or(Value::Null);
    let mut values = Vec::new();
    enum_values(operation, &defs, &mut values);
    Some(values)
}

fn route_errors(route: &str, tools: &[Tool]) -> Vec<String> {
    let mut parts = route.split_whitespace();
    let Some(name) = parts.next() else {
        return vec!["empty route".to_owned()];
    };
    if name.starts_with('<') {
        return Vec::new();
    }
    let Some(tool) = tools.iter().find(|tool| tool.name == name) else {
        return vec![format!("route '{route}' names unknown tool '{name}'")];
    };
    let requested = parts
        .find_map(|part| part.strip_prefix("operation="))
        .filter(|value| !value.starts_with('<'));
    match (requested, accepted_operations(tool)) {
        (Some(_), None) => vec![format!(
            "route '{route}': '{name}' is a flat tool with no operation parameter"
        )],
        (Some(value), Some(accepted)) if !accepted.iter().any(|a| a == value) => vec![format!(
            "route '{route}': operation '{value}' is not in {accepted:?}"
        )],
        _ => Vec::new(),
    }
}

#[test]
fn must_pass_every_hidden_tool_capability_route_is_callable() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service(dir.path())?;
    let public: Vec<Tool> = service
        .full_sanitized_tools()
        .into_iter()
        .filter(|tool| PUBLIC_TOOL_NAMES.contains(&tool.name.as_ref()))
        .collect();
    assert!(!public.is_empty());

    // With nothing visible, every hazardous/debugger-only route is emitted.
    let routes = hidden_tool_capability_routes(&[]);
    assert!(!routes.is_empty());
    let errors: Vec<String> = routes
        .iter()
        .flat_map(|route| route.preferred_tools.iter())
        .flat_map(|route| route_errors(route, &public))
        .collect();
    assert!(
        errors.is_empty(),
        "invalid advertised routes:\n{}",
        errors.join("\n")
    );
    Ok(())
}

#[test]
fn must_fail_route_validator_flags_the_known_bad_shapes() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service(dir.path())?;
    let public: Vec<Tool> = service
        .full_sanitized_tools()
        .into_iter()
        .filter(|tool| PUBLIC_TOOL_NAMES.contains(&tool.name.as_ref()))
        .collect();
    assert!(!route_errors("observe operation=current", &public).is_empty());
    assert!(!route_errors("browser_storage operation=read", &public).is_empty());
    assert!(!route_errors("no_such_tool", &public).is_empty());
    assert!(route_errors("browser_storage operation=get", &public).is_empty());
    assert!(route_errors("observe", &public).is_empty());
    assert!(route_errors("browser_debugger operation=<matching operation>", &public).is_empty());
    Ok(())
}
