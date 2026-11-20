//! Typed, session-owned controls for isolated agent seats.
//!
//! The MCP caller never supplies an owner. The owner is derived from the
//! transport-issued MCP session id, then passed independently to the shared
//! supervisor for every operation.

use rmcp::{ErrorData, RoleServer, model::ErrorCode, service::RequestContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use synapse_core::error_codes;
use synapse_seats::{
    ActionReceipt, BackendKind, CaptureReceipt, OwnerId, SeatAction, SeatConfig, SeatError,
    SeatGeneration, SeatId, SeatView, TeardownStatus,
};

use super::SynapseService;

pub(crate) const SEAT_SOURCE_OF_TRUTH: &str =
    "SeatSupervisor owned-seat registry + isolated backend readback";

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeatCreateParams {
    pub seat: SeatConfig,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeatIdParams {
    pub seat_id: SeatId,
    pub generation: SeatGeneration,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeatActParams {
    pub seat_id: SeatId,
    pub generation: SeatGeneration,
    pub action: SeatAction,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeatListReadback {
    pub browser_backend_available: bool,
    pub browser_backend_detail: String,
    pub seats: Vec<SeatView>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeatOperationResponse {
    pub source_of_truth: &'static str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<SeatView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seats: Option<SeatListReadback>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_receipt: Option<ActionReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_receipt: Option<CaptureReceipt>,
}

pub(crate) fn trusted_seat_owner(
    request_context: &RequestContext<RoleServer>,
    operation: &'static str,
) -> Result<OwnerId, ErrorData> {
    let session_id = super::context::mcp_session_id_from_request_context(request_context)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            seat_operation_error(
                operation,
                error_codes::HTTP_SESSION_INVALID,
                "an MCP session identity is required for seat ownership",
                "establish an MCP session through the same transport, then retry",
            )
        })?;
    Ok(OwnerId::new(session_id))
}

pub(crate) async fn create(
    service: &SynapseService,
    owner: OwnerId,
    params: SeatCreateParams,
) -> Result<SeatOperationResponse, ErrorData> {
    if params.seat.backend_kind() == BackendKind::Browser {
        let (available, detail) = service.browser_seat_backend_status();
        if !available {
            return Err(seat_operation_error(
                "seat_create",
                "SEAT_BACKEND_UNAVAILABLE",
                detail,
                "configure a daemon-owned Chromium executable and retry browser seat creation",
            ));
        }
    }
    let seat = service
        .seat_supervisor_ref()
        .create(owner, params.seat)
        .await
        .map_err(|error| map_seat_error("seat_create", error))?;
    Ok(SeatOperationResponse {
        source_of_truth: SEAT_SOURCE_OF_TRUTH,
        seat: Some(seat),
        seats: None,
        action_receipt: None,
        capture_receipt: None,
    })
}

pub(crate) async fn list(
    service: &SynapseService,
    owner: OwnerId,
) -> Result<SeatOperationResponse, ErrorData> {
    let seats = service.seat_supervisor_ref().list(&owner).await;
    let (browser_backend_available, browser_backend_detail) = service.browser_seat_backend_status();
    Ok(SeatOperationResponse {
        source_of_truth: SEAT_SOURCE_OF_TRUTH,
        seat: None,
        seats: Some(SeatListReadback {
            browser_backend_available,
            browser_backend_detail: browser_backend_detail.to_owned(),
            seats,
        }),
        action_receipt: None,
        capture_receipt: None,
    })
}

pub(crate) async fn get(
    service: &SynapseService,
    owner: OwnerId,
    seat_id: SeatId,
) -> Result<SeatOperationResponse, ErrorData> {
    let seat = service
        .seat_supervisor_ref()
        .get(&owner, seat_id)
        .await
        .map_err(|error| map_seat_error("seat_get", error))?;
    Ok(SeatOperationResponse {
        source_of_truth: SEAT_SOURCE_OF_TRUTH,
        seat: Some(seat),
        seats: None,
        action_receipt: None,
        capture_receipt: None,
    })
}

pub(crate) async fn act(
    service: &SynapseService,
    owner: OwnerId,
    params: SeatActParams,
) -> Result<SeatOperationResponse, ErrorData> {
    let action_receipt = service
        .seat_supervisor_ref()
        .act(&owner, params.seat_id, params.generation, params.action)
        .await
        .map_err(|error| map_seat_error("seat_act", error))?;
    Ok(SeatOperationResponse {
        source_of_truth: SEAT_SOURCE_OF_TRUTH,
        seat: None,
        seats: None,
        action_receipt: Some(action_receipt),
        capture_receipt: None,
    })
}

pub(crate) async fn capture(
    service: &SynapseService,
    owner: OwnerId,
    params: SeatIdParams,
) -> Result<SeatOperationResponse, ErrorData> {
    let capture_receipt = service
        .seat_supervisor_ref()
        .capture(&owner, params.seat_id, params.generation)
        .await
        .map_err(|error| map_seat_error("seat_capture", error))?;
    Ok(SeatOperationResponse {
        source_of_truth: SEAT_SOURCE_OF_TRUTH,
        seat: None,
        seats: None,
        action_receipt: None,
        capture_receipt: Some(capture_receipt),
    })
}

pub(crate) async fn pause(
    service: &SynapseService,
    owner: OwnerId,
    params: SeatIdParams,
) -> Result<SeatOperationResponse, ErrorData> {
    let seat = service
        .seat_supervisor_ref()
        .pause(&owner, params.seat_id, params.generation)
        .await
        .map_err(|error| map_seat_error("seat_pause", error))?;
    Ok(seat_view_response(seat))
}

pub(crate) async fn resume(
    service: &SynapseService,
    owner: OwnerId,
    params: SeatIdParams,
) -> Result<SeatOperationResponse, ErrorData> {
    let seat = service
        .seat_supervisor_ref()
        .resume(&owner, params.seat_id, params.generation)
        .await
        .map_err(|error| map_seat_error("seat_resume", error))?;
    Ok(seat_view_response(seat))
}

pub(crate) async fn close(
    service: &SynapseService,
    owner: OwnerId,
    params: SeatIdParams,
) -> Result<SeatOperationResponse, ErrorData> {
    let seat = service
        .seat_supervisor_ref()
        .close(&owner, params.seat_id, params.generation)
        .await
        .map_err(|error| map_seat_error("seat_close", error))?;
    Ok(seat_view_response(seat))
}

fn seat_view_response(seat: SeatView) -> SeatOperationResponse {
    SeatOperationResponse {
        source_of_truth: SEAT_SOURCE_OF_TRUTH,
        seat: Some(seat),
        seats: None,
        action_receipt: None,
        capture_receipt: None,
    }
}

pub(crate) fn seat_operation_error(
    operation: &'static str,
    code: &'static str,
    detail: impl Into<String>,
    remediation: &'static str,
) -> ErrorData {
    let detail = detail.into();
    ErrorData::new(
        ErrorCode(-32099),
        format!("seat operation={operation} failed: {detail}"),
        Some(json!({
            "code": code,
            "operation": operation,
            "source_of_truth": SEAT_SOURCE_OF_TRUTH,
            "remediation": remediation,
        })),
    )
}

fn map_seat_error(operation: &'static str, error: SeatError) -> ErrorData {
    let (code, remediation) = match &error {
        SeatError::InvalidConfig(_) | SeatError::InvalidAction(_) => (
            error_codes::TOOL_PARAMS_INVALID,
            "correct the typed seat configuration or action and retry",
        ),
        SeatError::NotFound(_) => (
            "SEAT_NOT_FOUND",
            "call session operation=seat_list to read owned seats",
        ),
        SeatError::NotOwner(_) => (
            "SEAT_OWNER_FORBIDDEN",
            "use the MCP session that created this seat; owner cannot be supplied by callers",
        ),
        SeatError::StaleGeneration { .. } => (
            "SEAT_STALE_GENERATION",
            "call session operation=seat_get and retry with its current generation",
        ),
        SeatError::Paused(_) => (
            "SEAT_PAUSED",
            "resume the owned seat and use the returned generation",
        ),
        SeatError::Closed(_) => (
            "SEAT_CLOSED",
            "create a new seat; closed seats cannot be reused",
        ),
        SeatError::Failed(_, _) => (
            "SEAT_FAILED",
            "inspect the returned failure and create a new seat after repair",
        ),
        SeatError::CapacityExhausted => (
            "SEAT_CAPACITY_EXHAUSTED",
            "close an owned seat or wait for capacity",
        ),
        SeatError::QueueFull(_) => (
            "SEAT_QUEUE_FULL",
            "wait for the in-flight operation before retrying",
        ),
        SeatError::Timeout(_) => (
            "SEAT_TIMEOUT",
            "inspect the seat state before a deliberate retry",
        ),
        SeatError::StartupTimeout {
            teardown: TeardownStatus::Confirmed,
            ..
        } => (
            "SEAT_STARTUP_TIMEOUT",
            "no seat exists and the partial backend's exit and profile removal were read back; check the backend, then retry seat_create",
        ),
        SeatError::StartupTimeout {
            teardown: TeardownStatus::Pending(_),
            ..
        } => (
            "SEAT_STARTUP_CLEANUP_PENDING",
            "no seat exists but teardown is not confirmed; inspect the process and path named in the message before retrying",
        ),
        SeatError::BackendUnavailable(_) => (
            "SEAT_BACKEND_UNAVAILABLE",
            "configure the daemon-owned backend before creating this seat",
        ),
        SeatError::Backend(_)
        | SeatError::FrameTooLarge
        | SeatError::BackendAlreadyRegistered(_) => (
            "SEAT_BACKEND_ERROR",
            "inspect seat state and backend configuration before retrying",
        ),
    };
    seat_operation_error(operation, code, error.to_string(), remediation)
}

#[cfg(test)]
#[path = "seat_tools_startup_timeout_73.rs"]
mod startup_timeout_73_tests;
