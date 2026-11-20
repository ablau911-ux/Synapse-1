//! Test-only entry points for `lease_tools` regression tests (#205).

use rmcp::ErrorData;

use super::{ControlLeaseResponse, acquire_lease_for_session_tracked};

/// Acquire/renew the lease for `session_id` without an MCP `RequestContext`.
pub(super) fn acquire_lease_for_session(
    session_id: &str,
    ttl_ms: u64,
) -> Result<ControlLeaseResponse, ErrorData> {
    acquire_lease_for_session_tracked(session_id, ttl_ms).map(|(response, _converted)| response)
}
