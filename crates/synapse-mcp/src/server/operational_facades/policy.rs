use rmcp::{RoleServer, service::RequestContext};

use crate::server::{ErrorData, SynapseService};

use super::errors::facade_policy_error;
pub(super) fn session_or_stdio(
    request_context: &RequestContext<RoleServer>,
) -> Result<String, ErrorData> {
    Ok(
        crate::server::context::mcp_session_id_from_request_context(request_context)?
            .unwrap_or_else(|| "stdio".to_owned()),
    )
}

/// `remediation` is the `tool_profiles` remediation constant of the calling
/// facade (built from `maintenance_profile_remediation!`, naming that tool's
/// read-only operations), the same text the facade contract lists.
pub(in crate::server) fn require_maintenance_profile(
    service: &SynapseService,
    request_context: &RequestContext<RoleServer>,
    tool: &'static str,
    operation: &'static str,
    source_id: &str,
    source_of_truth: &'static str,
    remediation: &'static str,
) -> Result<(), ErrorData> {
    let session_id = crate::server::context::mcp_session_id_from_request_context(request_context)?;
    let snapshot = service.tool_profile_snapshot(session_id.as_deref())?;
    if snapshot.profile.allows_maintenance_mutation() {
        return Ok(());
    }
    Err(facade_policy_error(
        tool,
        operation,
        source_id,
        snapshot.profile,
        source_of_truth,
        remediation,
    ))
}
