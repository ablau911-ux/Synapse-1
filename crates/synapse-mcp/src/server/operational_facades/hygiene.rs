use std::sync::{Arc, Mutex};

use rmcp::{RoleServer, service::RequestContext};
use synapse_reflex::ReflexRuntime;

use crate::m3::hygiene::HygieneScanTextParams;
use crate::server::{ErrorData, Json, Parameters, SynapseService};

use super::{
    HYGIENE_SOT, HYGIENE_TOOL,
    errors::{facade_delegate_error, missing_spec},
    policy::{require_maintenance_profile, require_maintenance_profile_for_session},
    response::hygiene_response,
    types::{HygieneOperation, HygieneParams, HygieneResponse},
    validation::validate_hygiene_params,
};
pub(super) async fn handle(
    service: &SynapseService,
    params: Parameters<HygieneParams>,
    request_context: RequestContext<RoleServer>,
) -> Result<Json<HygieneResponse>, ErrorData> {
    validate_hygiene_params(&params.0)?;
    let operation = params.0.operation;
    tracing::info!(
        code = "MCP_TOOL_INVOCATION",
        kind = HYGIENE_TOOL,
        operation = operation.as_str(),
        "tool.invocation kind=hygiene"
    );
    let runtime = service.reflex_runtime().map_err(|error| {
        facade_delegate_error(
            HYGIENE_TOOL,
            operation.as_str(),
            "reflex_runtime",
            HYGIENE_SOT,
            error,
            "repair storage/reflex initialization and retry the hygiene operation",
        )
    })?;
    match operation {
        HygieneOperation::ScanText => {
            let spec = params
                .0
                .scan_text
                .ok_or_else(|| missing_spec(HYGIENE_TOOL, "scan_text"))?;
            // The MCP session is only resolved for a persisting scan, which
            // needs the maintenance-profile gate.
            let session_id = if spec.persist {
                crate::server::context::mcp_session_id_from_request_context(&request_context)?
            } else {
                None
            };
            scan_text_for_session(service, &runtime, &spec, session_id.as_deref())
        }
        HygieneOperation::ScanStorage => {
            let spec = params
                .0
                .scan_storage
                .ok_or_else(|| missing_spec(HYGIENE_TOOL, "scan_storage"))?;
            require_maintenance_profile(
                service,
                &request_context,
                HYGIENE_TOOL,
                operation.as_str(),
                "storage_scan",
                HYGIENE_SOT,
            )?;
            service.require_m3_permissions(
                HYGIENE_TOOL,
                &crate::m3::hygiene::required_permissions_scan_storage(&spec),
            )?;
            let response = crate::m3::hygiene::scan_storage(&runtime, &spec).map_err(|error| {
                facade_delegate_error(
                    HYGIENE_TOOL,
                    operation.as_str(),
                    "storage_scan",
                    HYGIENE_SOT,
                    error,
                    "fix source_cfs/cursor and inspect CF_KV hygiene flag rows",
                )
            })?;
            Ok(Json(hygiene_response(
                operation,
                format!(
                    "scanned_rows={} flags_written={}",
                    response.scanned_rows, response.flags_written
                ),
                |out| out.scan_storage = Some(response),
            )))
        }
        HygieneOperation::Flags => {
            let spec = params.0.flags.unwrap_or_default();
            service.require_m3_permissions(
                HYGIENE_TOOL,
                &crate::m3::hygiene::required_permissions_flags(&spec),
            )?;
            let response = crate::m3::hygiene::query_flags(&runtime, &spec).map_err(|error| {
                facade_delegate_error(
                    HYGIENE_TOOL,
                    operation.as_str(),
                    spec.source_key_hex.as_deref().unwrap_or("flag_prefix"),
                    HYGIENE_SOT,
                    error,
                    "inspect CF_KV hygiene/flag/v1 rows and cursor format",
                )
            })?;
            Ok(Json(hygiene_response(
                operation,
                format!(
                    "flags={} scanned_rows={}",
                    response.flags.len(),
                    response.scanned_rows
                ),
                |out| out.flags = Some(response),
            )))
        }
        HygieneOperation::Report => {
            let spec = params.0.report.unwrap_or_default();
            service.require_m3_permissions(
                HYGIENE_TOOL,
                &crate::m3::hygiene::required_permissions_report(&spec),
            )?;
            let response = crate::m3::hygiene::report(&runtime, &spec).map_err(|error| {
                facade_delegate_error(
                    HYGIENE_TOOL,
                    operation.as_str(),
                    spec.source_key_hex.as_deref().unwrap_or("report"),
                    HYGIENE_SOT,
                    error,
                    "inspect hygiene report joins and CF_KV flag/taint rows",
                )
            })?;
            Ok(Json(hygiene_response(
                operation,
                format!(
                    "flags_total={} impacted_routines={}",
                    response.summary.flags_total, response.summary.impacted_routine_count
                ),
                |out| out.report = Some(response),
            )))
        }
    }
}

/// `hygiene operation=scan_text` for an already-resolved MCP session id
/// (`None` = stdio / no session header). A persisting scan requires the
/// maintenance profile for that session before the permission check and scan.
pub(super) fn scan_text_for_session(
    service: &SynapseService,
    runtime: &Arc<Mutex<ReflexRuntime>>,
    spec: &HygieneScanTextParams,
    session_id: Option<&str>,
) -> Result<Json<HygieneResponse>, ErrorData> {
    let operation = HygieneOperation::ScanText;
    if spec.persist {
        require_maintenance_profile_for_session(
            service,
            session_id,
            HYGIENE_TOOL,
            operation.as_str(),
            spec.source_cf.as_deref().unwrap_or("source_cf_missing"),
            HYGIENE_SOT,
        )?;
    }
    service.require_m3_permissions(
        HYGIENE_TOOL,
        &crate::m3::hygiene::required_permissions_scan_text(spec),
    )?;
    let response = crate::m3::hygiene::scan_text_tool(runtime, spec).map_err(|error| {
        facade_delegate_error(
            HYGIENE_TOOL,
            operation.as_str(),
            spec.source_key_hex.as_deref().unwrap_or("text_only"),
            HYGIENE_SOT,
            error,
            "fix text/source row identity and inspect hygiene flags before retrying",
        )
    })?;
    Ok(Json(hygiene_response(
        operation,
        format!(
            "matches={} flags_written={}",
            response.matches.len(),
            response.flags_written
        ),
        |out| out.scan_text = Some(response),
    )))
}
