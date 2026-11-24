use rmcp::model::ErrorCode;
use serde_json::json;
use synapse_core::error_codes;

use crate::server::ErrorData;

use super::{
    HYGIENE_TOOL, MODEL_TOOL, SETUP_TOOL, STORAGE_TOOL,
    types::{
        HygieneParams, ModelParams, SetupParams, StorageParams, TelemetryOperation, TelemetryParams,
    },
};
pub(super) fn validate_storage_params(params: &StorageParams) -> Result<(), ErrorData> {
    validate_exact_spec(
        STORAGE_TOOL,
        params.operation.as_str(),
        &[
            ("inspect", params.inspect.is_some()),
            ("summary", params.summary.is_some()),
            ("gc_once", params.gc_once.is_some()),
        ],
        &["inspect", "summary"],
    )
}

pub(super) fn validate_model_params(params: &ModelParams) -> Result<(), ErrorData> {
    validate_exact_spec(
        MODEL_TOOL,
        params.operation.as_str(),
        &[
            ("list", params.list.is_some()),
            ("status", params.status.is_some()),
            ("probe", params.probe.is_some()),
            ("register", params.register.is_some()),
            ("update", params.update.is_some()),
            ("remove", params.remove.is_some()),
        ],
        &["list", "status"],
    )
}

pub(super) fn validate_hygiene_params(params: &HygieneParams) -> Result<(), ErrorData> {
    validate_exact_spec(
        HYGIENE_TOOL,
        params.operation.as_str(),
        &[
            ("scan_text", params.scan_text.is_some()),
            ("scan_storage", params.scan_storage.is_some()),
            ("flags", params.flags.is_some()),
            ("report", params.report.is_some()),
        ],
        &["flags", "report"],
    )
}

pub(super) fn validate_setup_params(params: &SetupParams) -> Result<(), ErrorData> {
    validate_exact_spec(
        SETUP_TOOL,
        params.operation.as_str(),
        &[
            ("status", params.status.is_some()),
            ("doctor", params.doctor.is_some()),
            ("repair", params.repair.is_some()),
        ],
        &["status", "doctor"],
    )
}

pub(super) fn validate_telemetry_params(params: &TelemetryParams) -> Result<(), ErrorData> {
    match params.operation {
        TelemetryOperation::Status => {
            let _status_payload_present = params.status.is_some();
            Ok(())
        }
    }
}

/// Accepts the payload whose key matches `operation` and no other payload.
/// Operations listed in `payload_optional` take defaults for every field, so
/// their payload may be omitted.
fn validate_exact_spec(
    tool: &'static str,
    operation: &'static str,
    specs: &[(&'static str, bool)],
    payload_optional: &[&'static str],
) -> Result<(), ErrorData> {
    let matching_present = payload_optional.contains(&operation)
        || specs.iter().any(|(name, present)| *name == operation && *present);
    let extra = specs
        .iter()
        .filter_map(|(name, present)| (*present && *name != operation).then_some(*name))
        .collect::<Vec<_>>();
    if matching_present && extra.is_empty() {
        return Ok(());
    }
    Err(ErrorData::new(
        ErrorCode(-32099),
        format!("{tool} operation={operation} requires exactly one matching operation payload"),
        Some(json!({
            "code": error_codes::TOOL_PARAMS_INVALID,
            "tool": tool,
            "operation": operation,
            "source_of_truth": "MCP request parameters",
            "source_id": operation,
            "matching_payload_present": matching_present,
            "extra_payloads": extra,
            "remediation": "pass exactly one payload object whose key matches operation",
        })),
    ))
}
