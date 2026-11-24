//! Envelope for a failure returned by a delegated implementation tool.
//!
//! Tool envelopes preserve the delegate's `code` and `remediation`; the
//! facade's own remediation is kept as `facade_remediation` and is the
//! fallback (#276, upstream #1911). Operation envelopes retain their original
//! facade-only remediation field.
//!
//! Two envelope shapes exist on the public surface and both are kept:
//! * pass-through ([`delegate_error`], [`operation_delegate_error`]): the
//!   delegate's JSON-RPC code and message are returned unchanged and its data
//!   becomes `cause`;
//! * wrapping ([`wrapped_delegate_error`]): a facade-level `-32099` error whose
//!   message names the facade operation.

use rmcp::model::ErrorCode;
use serde_json::{Map, Value, json};
use synapse_core::error_codes;

use crate::m1::error_code;
use crate::server::ErrorData;

/// The delegate's own remediation, when it carried one. Empty or
/// whitespace-only counts as absent so it cannot displace the facade's text.
/// A structured remediation object (e.g. the permission layer's
/// `{runtime_opt_in, startup_opt_in, ...}`) is flattened to `key: value; ...`
/// so the envelope field stays a string.
fn remediation_from(error: &ErrorData) -> Option<String> {
    let remediation = error.data.as_ref()?.get("remediation")?;
    let text = match remediation {
        Value::String(text) => text.trim().to_owned(),
        Value::Object(fields) => fields
            .iter()
            .filter_map(|(key, value)| {
                let value = value.as_str()?.trim();
                (!value.is_empty()).then(|| format!("{key}: {value}"))
            })
            .collect::<Vec<_>>()
            .join("; "),
        _ => String::new(),
    };
    (!text.is_empty()).then_some(text)
}

/// The delegate's canonical error code (`data.code`), or `TOOL_INTERNAL_ERROR`.
pub(crate) fn delegate_code(error: &ErrorData) -> String {
    error_code(error)
        .unwrap_or(error_codes::TOOL_INTERNAL_ERROR)
        .to_owned()
}

/// Pass-through core: `identity` holds the facade's own fields (tool,
/// operation, source of truth, ...); the delegate supplies code, message and
/// `cause`, and its remediation wins over the facade's.
fn pass_through(
    mut identity: Map<String, Value>,
    error: ErrorData,
    remediation: &'static str,
) -> ErrorData {
    let delegate_remediation = remediation_from(&error);
    identity.insert("code".to_owned(), json!(delegate_code(&error)));
    identity.insert(
        "remediation".to_owned(),
        json!(delegate_remediation.as_deref().unwrap_or(remediation)),
    );
    identity.insert("facade_remediation".to_owned(), json!(remediation));
    identity.insert(
        "cause".to_owned(),
        error.data.clone().unwrap_or(Value::Null),
    );
    ErrorData::new(
        error.code,
        error.message.to_string(),
        Some(Value::Object(identity)),
    )
}

fn identity_fields(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(fields) => fields,
        _ => Map::new(),
    }
}

/// Pass-through envelope for facades that report their tool name.
pub(crate) fn delegate_error(
    tool: &'static str,
    operation: &'static str,
    source_of_truth: &'static str,
    source_id: impl Into<String>,
    error: ErrorData,
    remediation: &'static str,
) -> ErrorData {
    pass_through(
        identity_fields(json!({
            "tool": tool,
            "operation": operation,
            "source_of_truth": source_of_truth,
            "source_id": source_id.into(),
        })),
        error,
        remediation,
    )
}

/// Pass-through envelope for single-tool facades (shell, process, browser):
/// no `tool` field; `readback_source_of_truth` is added when the facade has one.
/// Same #276 rule as every other facade: the delegate's remediation wins and
/// the facade's text is kept as `facade_remediation`.
pub(crate) fn operation_delegate_error(
    operation: &'static str,
    source_of_truth: &'static str,
    readback_source_of_truth: Option<&'static str>,
    source_id: impl Into<String>,
    error: ErrorData,
    remediation: &'static str,
) -> ErrorData {
    let mut identity = identity_fields(json!({
        "operation": operation,
        "source_of_truth": source_of_truth,
        "source_id": source_id.into(),
    }));
    if let Some(readback) = readback_source_of_truth {
        identity.insert("readback_source_of_truth".to_owned(), json!(readback));
    }
    pass_through(identity, error, remediation)
}

/// Wrapping envelope: a facade-level `-32099` error whose message names the
/// facade operation and whose `cause` keeps the delegate's message and data.
pub(crate) fn wrapped_delegate_error(
    tool: &'static str,
    operation: &'static str,
    source_id: &str,
    source_of_truth: &'static str,
    error: ErrorData,
    remediation: &'static str,
) -> ErrorData {
    let delegate_remediation = remediation_from(&error);
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "{tool} operation={operation} failed for {source_id}: {}",
            error.message
        ),
        Some(json!({
            "code": delegate_code(&error),
            "tool": tool,
            "operation": operation,
            "source_id": source_id,
            "source_of_truth": source_of_truth,
            "remediation": delegate_remediation.as_deref().unwrap_or(remediation),
            "facade_remediation": remediation,
            "cause": {
                "message": error.message.to_string(),
                "data": error.data,
            },
        })),
    )
}
