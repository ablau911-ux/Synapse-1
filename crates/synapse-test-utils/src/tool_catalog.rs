//! The `tools/list` view a strict MCP client keeps, and the checks it applies
//! around every `tools/call` (D1 client-parity, #548/#549).

use std::collections::HashMap;

use anyhow::{Context, bail};
use serde_json::Value;

use crate::json_schema::{SchemaValidator, strict_client_schema_issues};

/// Tools from the latest `tools/list`, keyed by name.
#[derive(Debug, Default)]
pub struct ToolCatalog {
    tools: HashMap<String, Value>,
}

impl ToolCatalog {
    /// Builds the catalog from a `tools/list` result. Fails the way a strict
    /// client fails the whole listing: a tool without an object `inputSchema`
    /// rooted at `"type": "object"`, or any schema carrying a bare boolean
    /// subschema, rejects every tool.
    pub fn from_tools_list(result: &Value) -> anyhow::Result<Self> {
        let listed = result
            .get("tools")
            .and_then(Value::as_array)
            .with_context(|| format!("tools/list result has no tools array: {result}"))?;
        let mut tools = HashMap::new();
        let mut issues = Vec::new();
        for tool in listed {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .with_context(|| format!("tools/list entry without a name: {tool}"))?;
            match tool.get("inputSchema") {
                Some(schema @ Value::Object(map))
                    if map.get("type").and_then(Value::as_str) == Some("object") =>
                {
                    issues.extend(
                        strict_client_schema_issues(schema)
                            .into_iter()
                            .map(|issue| format!("{name}.inputSchema {issue}")),
                    );
                }
                other => issues.push(format!(
                    "{name}.inputSchema must be an object schema with type \"object\", got {other:?}"
                )),
            }
            if let Some(output) = tool.get("outputSchema") {
                issues.extend(
                    strict_client_schema_issues(output)
                        .into_iter()
                        .map(|issue| format!("{name}.outputSchema {issue}")),
                );
            }
            tools.insert(name.to_owned(), tool.clone());
        }
        if !issues.is_empty() {
            bail!(
                "strict MCP client would reject tools/list: {}",
                issues.join("; ")
            );
        }
        Ok(Self { tools })
    }

    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Validates call arguments against the tool's `inputSchema` before send.
    pub fn check_arguments(&self, name: &str, args: &Value) -> anyhow::Result<()> {
        let tool = self.tool(name)?;
        let schema = tool.get("inputSchema").unwrap_or(&Value::Null);
        SchemaValidator::new(schema).validate(args).map_err(|error| {
            anyhow::anyhow!(
                "CLIENT_SCHEMA_INVALID: strict client refuses tools/call `{name}` arguments {args}: {error}"
            )
        })
    }

    /// Validates a successful result's `structuredContent` against the tool's
    /// `outputSchema`, as the official MCP client SDKs do.
    pub fn check_structured_output(&self, name: &str, result: &Value) -> anyhow::Result<()> {
        let tool = self.tool(name)?;
        let Some(schema) = tool.get("outputSchema") else {
            return Ok(());
        };
        let structured = result.get("structuredContent").with_context(|| {
            format!(
                "CLIENT_OUTPUT_INVALID: tool `{name}` declares outputSchema but result has no structuredContent: {result}"
            )
        })?;
        SchemaValidator::new(schema)
            .validate(structured)
            .map_err(|error| {
                anyhow::anyhow!(
                    "CLIENT_OUTPUT_INVALID: tool `{name}` structuredContent violates outputSchema: {error}"
                )
            })
    }

    fn tool(&self, name: &str) -> anyhow::Result<&Value> {
        self.tools.get(name).with_context(|| {
            format!("CLIENT_TOOL_UNLISTED: tool `{name}` is absent from tools/list; a strict client cannot call it")
        })
    }
}

/// `true` when a `tools/call` result is a tool-level failure (`isError: true`).
#[must_use]
pub fn is_tool_error(result: &Value) -> bool {
    result.get("isError").and_then(Value::as_bool) == Some(true)
}
