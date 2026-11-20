//! MCP tools for the multi-agent input lease (epic #719, issue #733).
//!
//! The lease is the coordination primitive that lets many agents share the
//! single real foreground/cursor without interleaving: an agent acquires it
//! before a leased-foreground action and releases it after. Background tiers
//! (CDP/UIA/PostMessage) never touch it. These tools expose the lease over MCP
//! so an agent (or operator) can explicitly acquire/release/inspect it, and the
//! lease state is also surfaced under `health.subsystems.action`.
//!
//! The lease is keyed by `Mcp-Session-Id`, so every lease tool requires a
//! session. In the shared-daemon HTTP deployment each agent terminal has its
//! own session id; a missing session id is a fail-loud `TOOL_PARAMS_INVALID`.

use super::{
    ErrorData, Json, Parameters, SynapseService, empty_input_schema, mcp_error,
    session_registry::{SessionRegistryRead, unix_time_ms_now},
    session_tools::validate_session_id,
    tool, tool_router,
};
use rmcp::{RoleServer, model::ErrorCode, service::RequestContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use synapse_action::{LeaseOutcome, LeaseStatus, lease};
use synapse_core::error_codes;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlLeaseAcquireParams {
    /// Lease lifetime in milliseconds. Must be in [100, 30000]. The lease is
    /// renewed on every leased action and on a repeat acquire by the holder, so
    /// a short TTL is the safety floor against a crashed holder, not a hard cap
    /// on how long real work can take.
    #[serde(default = "default_lease_ttl_ms")]
    #[schemars(default = "default_lease_ttl_ms", range(min = 100, max = 30000))]
    pub ttl_ms: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlLeaseHandoffParams {
    /// Live MCP session id that should receive the foreground input lease.
    pub to_session: String,
    /// Fresh lease lifetime in milliseconds for the recipient. Must be in
    /// [100, 30000] like `control_lease_acquire`.
    #[serde(default = "default_lease_ttl_ms")]
    #[schemars(default = "default_lease_ttl_ms", range(min = 100, max = 30000))]
    pub ttl_ms: u64,
}

const fn default_lease_ttl_ms() -> u64 {
    synapse_action::DEFAULT_LEASE_TTL_MS
}

/// Flattened lease snapshot returned by every lease tool. `LeaseStatus` lives in
/// `synapse-action` (no `schemars` dep there), so its fields are flattened here
/// rather than embedded, keeping the action crate schema-free.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControlLeaseResponse {
    /// One of: `acquired`, `renewed`, `released`, `status`.
    pub outcome: String,
    /// For release replies, whether the lease was already unheld.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub already_released: Option<bool>,
    /// Whether the lease is currently held by anyone.
    pub held: bool,
    /// The session id of the current holder, if any.
    pub owner_session_id: Option<String>,
    /// The session id that issued this tool call.
    pub this_session_id: String,
    /// Whether the calling session is the current holder.
    pub is_owner: bool,
    pub acquired_at_ms_ago: Option<u64>,
    pub renewed_at_ms_ago: Option<u64>,
    pub ttl_ms: Option<u64>,
    pub expires_in_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DashboardControlLeaseForceReleaseResponse {
    pub requested_owner_session_id: String,
    pub confirmed: bool,
    pub released: bool,
    pub before: ControlLeaseResponse,
    pub after: ControlLeaseResponse,
    pub persisted_row_deleted: bool,
    pub source_of_truth: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DashboardControlLeaseHandoffResponse {
    pub from_session_id: String,
    pub to_session_id: String,
    pub ttl_ms: u64,
    pub before_from: ControlLeaseResponse,
    pub before_to: ControlLeaseResponse,
    pub response: ControlLeaseResponse,
    pub after_from: ControlLeaseResponse,
    pub after_to: ControlLeaseResponse,
    pub source_of_truth: &'static str,
}

impl ControlLeaseResponse {
    fn from_status(outcome: &str, this_session_id: String, status: &LeaseStatus) -> Self {
        let is_owner = status.owner_session_id.as_deref() == Some(this_session_id.as_str());
        Self {
            outcome: outcome.to_owned(),
            already_released: None,
            held: status.held,
            owner_session_id: status.owner_session_id.clone(),
            this_session_id,
            is_owner,
            acquired_at_ms_ago: status.acquired_at_ms_ago,
            renewed_at_ms_ago: status.renewed_at_ms_ago,
            ttl_ms: status.ttl_ms,
            expires_in_ms: status.expires_in_ms,
        }
    }

    /// Whether this release reply freed a lease the caller held (#2325): an
    /// `already_released` success is not a release by this call.
    const fn released_by_this_call(&self) -> bool {
        matches!(self.already_released, Some(false))
    }
}

pub(crate) const DASHBOARD_LEASE_SOURCE_OF_TRUTH: &str =
    "synapse_action::lease + CF_SESSIONS MCP session lease rows + CF_AGENT_EVENTS + CF_ACTION_LOG";
pub(super) const LEASE_TTL_SOURCE_OF_TRUTH: &str =
    "request ttl_ms before synapse_action::lease mutation";

#[tool_router(router = lease_tool_router, vis = "pub(super)")]
impl SynapseService {
    #[tool(
        description = "Acquire (or renew) the process-global input lease for this MCP session. The lease serializes real foreground/cursor/keyboard/clipboard actions across agents; background tiers (CDP/UIA/PostMessage) never need it. Refuse-not-block: if another live session holds it, returns ACTION_FOREGROUND_LEASE_BUSY with the current holder and a retry hint instead of waiting."
    )]
    pub async fn control_lease_acquire(
        &self,
        params: Parameters<ControlLeaseAcquireParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ControlLeaseResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "control_lease_acquire",
            "tool.invocation kind=control_lease_acquire"
        );
        let session_id = require_lease_session_id(&request_context)?;
        validate_lease_ttl_ms("control_lease_acquire", params.0.ttl_ms)?;
        let mut promotion = super::act_foreground_slot::lock_lease_promotion(&session_id).await;
        let admission = super::act_foreground_slot::admit_session_operation(
            &session_id,
            "control_lease_acquire",
        );
        let transition_gate = self.lease_transition_gate();
        let _transition = transition_gate.lock().await;
        // The promotion gate prevents fence claims/unwind between the owner
        // check, audited acquire, and promotion; the transition gate excludes
        // concurrent lease mutations throughout that same transaction.
        let hold = match admission {
            Ok(hold) => Some(hold),
            Err(_)
                if promotion.can_join()
                    && lease::status().owner_session_id.as_deref() == Some(session_id.as_str()) =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        let result = self.control_lease_acquire_locked(params.0, &session_id);
        if result.is_ok() && hold.is_none() {
            promotion.promote();
        }
        result
    }

    /// Acquire/renew without the in-flight foreground-escalation guard; used
    /// by the escalation itself.
    pub(super) async fn control_lease_acquire_inner(
        &self,
        params: Parameters<ControlLeaseAcquireParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ControlLeaseResponse>, ErrorData> {
        let params = params.0;
        validate_lease_ttl_ms("control_lease_acquire", params.ttl_ms)?;
        let session_id = require_lease_session_id(&request_context)?;
        let transition_gate = self.lease_transition_gate();
        let _transition = transition_gate.lock().await;
        self.control_lease_acquire_locked(params, &session_id)
    }

    fn control_lease_acquire_locked(
        &self,
        params: ControlLeaseAcquireParams,
        session_id: &str,
    ) -> Result<Json<ControlLeaseResponse>, ErrorData> {
        self.restore_session_lease_if_needed(session_id)?;
        let mut command = lease_acquire_audit(session_id, params.ttl_ms);
        self.command_audit_intent(command.clone())?;
        let (response, converted_from_auto) =
            acquire_lease_for_session_tracked(session_id, params.ttl_ms)
                .map_err(|error| self.audit_lease_acquire_failure(command.clone(), error, None))?;
        self.persist_acquired_lease(session_id, &response, converted_from_auto)
            .map_err(|(error, released)| {
                self.audit_lease_acquire_failure(command.clone(), error, Some(released))
            })?;
        command.outcome = "ok";
        command.after = json!({
            "source_of_truth": "synapse_action::lease",
            "response": &response,
            "caller": lease_status_for_session(session_id),
        });
        if let Err(error) = self.command_audit_final(command) {
            if converted_from_auto {
                let _restored = lease::restore_auto_if_owner(session_id);
            } else if response.outcome == "acquired" {
                let _released = self.rollback_unacknowledged_lease(session_id);
            }
            return Err(error);
        }
        Ok(Json(response))
    }

    fn persist_acquired_lease(
        &self,
        session_id: &str,
        response: &ControlLeaseResponse,
        converted_from_auto: bool,
    ) -> Result<(), (ErrorData, bool)> {
        let status = lease::status();
        let persist_result = if response.outcome == "acquired" {
            self.persist_session_lease_with_acquisition_event(session_id, &status)
        } else {
            self.persist_session_lease(session_id, &status)
        };
        if let Err(error) = persist_result {
            let restored_to_action =
                converted_from_auto && lease::restore_auto_if_owner(session_id);
            let released = response.outcome == "acquired"
                && !converted_from_auto
                && lease::release_if_owner(session_id);
            tracing::error!(
                code = error_codes::TOOL_INTERNAL_ERROR,
                session_id,
                released_after_persist_failure = released,
                restored_to_action,
                error = ?error,
                "input lease durability write failed; rolled back only this acquisition"
            );
            return Err((error, released));
        }
        Ok(())
    }

    fn audit_lease_acquire_failure(
        &self,
        mut command: super::command_audit::CommandAuditInput,
        error: ErrorData,
        released: Option<bool>,
    ) -> ErrorData {
        command.outcome = "error";
        command.after = json!({
            "source_of_truth": "synapse_action::lease",
            "caller": command.actor_session_id.as_deref().map(lease_status_for_session),
        });
        if let Some(released) = released {
            command.after["released_after_persist_failure"] = json!(released);
        }
        let command = command.with_error(
            super::command_audit::command_audit_error_from_error_data(&error),
        );
        match self.command_audit_final(command) {
            Ok(_) => error,
            Err(audit_error) => audit_error,
        }
    }

    #[tool(
        description = "Release the input lease held by this MCP session. An unheld or expired lease returns success with already_released=true. If another session holds the lease, returns ACTION_FOREGROUND_LEASE_NOT_HELD with the holder.",
        input_schema = empty_input_schema()
    )]
    pub async fn control_lease_release(
        &self,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ControlLeaseResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "control_lease_release",
            "tool.invocation kind=control_lease_release"
        );
        let session_id = require_lease_session_id(&request_context)?;
        let _hold = super::act_foreground_slot::admit_session_operation(
            &session_id,
            "control_lease_release",
        )?;
        let mut _physically_released = false;
        self.control_lease_release_inner(request_context, &mut _physically_released)
            .await
    }

    /// Release without the in-flight foreground-escalation guard; used by the
    /// escalation's unwind. `physically_released` is set once the in-memory
    /// lease release commits, so a caller can tell a later reconcile/audit
    /// failure apart from a release that never happened.
    pub(super) async fn control_lease_release_inner(
        &self,
        request_context: RequestContext<RoleServer>,
        physically_released: &mut bool,
    ) -> Result<Json<ControlLeaseResponse>, ErrorData> {
        let session_id = require_lease_session_id(&request_context)?;
        let transition_gate = self.lease_transition_gate();
        let _transition = transition_gate.lock().await;
        self.restore_session_lease_if_needed(&session_id)?;
        let command_payload = json!({});
        let command_before = json!({
            "source_of_truth": "synapse_action::lease",
            "caller": lease_status_for_session(&session_id),
        });
        self.command_audit_intent(super::command_audit::CommandAuditInput::mcp(
            "control_lease_release",
            "lease_release",
            Some(session_id.clone()),
            Some(session_id.clone()),
            command_payload.clone(),
            command_before.clone(),
            Value::Null,
            "pending",
        ))?;
        let response = match release_lease_for_session(&session_id) {
            Ok(response) => {
                *physically_released = response.released_by_this_call();
                response
            }
            Err(error) => {
                self.command_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "control_lease_release",
                        "lease_release",
                        Some(session_id.clone()),
                        Some(session_id.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": "synapse_action::lease",
                            "caller": lease_status_for_session(&session_id),
                        }),
                        "error",
                    )
                    .with_error(
                        super::command_audit::command_audit_error_from_error_data(&error),
                    ),
                )?;
                return Err(error);
            }
        };
        if let Err(error) =
            reconcile_lease_release_after_mutation(self, &session_id, "explicit_release")
        {
            self.command_audit_final(
                super::command_audit::CommandAuditInput::mcp(
                    "control_lease_release",
                    "lease_release",
                    Some(session_id.clone()),
                    Some(session_id.clone()),
                    command_payload,
                    command_before,
                    json!({
                        "source_of_truth": "synapse_action::lease",
                        "response": &response,
                        "caller": lease_status_for_session(&session_id),
                    }),
                    "error",
                )
                .with_error(
                    super::command_audit::command_audit_error_from_error_data(&error),
                ),
            )?;
            return Err(error);
        }
        self.command_audit_final(super::command_audit::CommandAuditInput::mcp(
            "control_lease_release",
            "lease_release",
            Some(session_id.clone()),
            Some(session_id.clone()),
            command_payload,
            command_before,
            json!({
                "source_of_truth": "synapse_action::lease",
                "response": &response,
                "caller": lease_status_for_session(&session_id),
            }),
            "ok",
        ))?;
        Ok(Json(response))
    }

    #[tool(
        description = "Atomically hand off the input lease from this MCP session to a named live peer without releasing it into a race. The caller must be the current holder; the recipient must be a live registered MCP session. Stale, closed, unknown, malformed, or self recipients fail closed."
    )]
    pub async fn control_lease_handoff(
        &self,
        params: Parameters<ControlLeaseHandoffParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ControlLeaseResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "control_lease_handoff",
            "tool.invocation kind=control_lease_handoff"
        );
        let params = params.0;
        validate_lease_ttl_ms("control_lease_handoff", params.ttl_ms)?;
        let session_id = require_lease_session_id(&request_context)?;
        let _hold = super::act_foreground_slot::admit_session_operation(
            &session_id,
            "control_lease_handoff",
        )?;
        // A handoff to a session whose escalation is in flight would land a
        // lease that escalation then misreads as pre-existing.
        let _hold_recipient = super::act_foreground_slot::admit_session_operation(
            &params.to_session,
            "control_lease_handoff",
        )?;
        let transition_gate = self.lease_transition_gate();
        let _transition = transition_gate.lock().await;
        self.restore_session_lease_if_needed(&session_id)?;
        let to_session = params.to_session;
        self.ensure_handoff_recipient_live(&session_id, &to_session)?;
        let command_payload = json!({
            "to_session": &to_session,
            "ttl_ms": params.ttl_ms,
        });
        let command_before = json!({
            "source_of_truth": "synapse_action::lease",
            "from": lease_status_for_session(&session_id),
            "to": lease_status_for_session(&to_session),
        });
        self.command_audit_intent(super::command_audit::CommandAuditInput::mcp(
            "control_lease_handoff",
            "lease_handoff",
            Some(session_id.clone()),
            Some(to_session.clone()),
            command_payload.clone(),
            command_before.clone(),
            Value::Null,
            "pending",
        ))?;
        let handoff = match handoff_lease_for_session(&session_id, &to_session, params.ttl_ms) {
            Ok(handoff) => handoff,
            Err(error) => {
                self.command_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "control_lease_handoff",
                        "lease_handoff",
                        Some(session_id.clone()),
                        Some(to_session.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": "synapse_action::lease",
                            "from": lease_status_for_session(&session_id),
                            "to": lease_status_for_session(&to_session),
                        }),
                        "error",
                    )
                    .with_error(
                        super::command_audit::command_audit_error_from_error_data(&error),
                    ),
                )?;
                return Err(error);
            }
        };
        let response =
            ControlLeaseResponse::from_status("handed_off", session_id.clone(), &handoff.current);
        let persist_readback = match self.persist_session_lease_handoff_with_events(
            &session_id,
            &to_session,
            &handoff.current,
        ) {
            Ok(readback) => readback,
            Err(error) => {
                self.rollback_handoff_after_persist_failure(
                    &session_id,
                    &to_session,
                    &handoff.prior,
                    &error,
                );
                self.command_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "control_lease_handoff",
                        "lease_handoff",
                        Some(session_id.clone()),
                        Some(to_session.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": "synapse_action::lease",
                            "from": lease_status_for_session(&session_id),
                            "to": lease_status_for_session(&to_session),
                        }),
                        "error",
                    )
                    .with_error(
                        super::command_audit::command_audit_error_from_error_data(&error),
                    ),
                )?;
                return Err(error);
            }
        };
        tracing::info!(
            code = "INPUT_LEASE_HANDOFF_COMMITTED",
            from_session_id = session_id,
            to_session_id = to_session,
            from_row_existed_before = persist_readback.from_row_existed_before,
            from_row_exists_after = persist_readback.from_row_exists_after,
            from_row_deleted = persist_readback.from_row_deleted,
            to_row_exists_after = persist_readback.to_row_exists_after,
            to_row_session_id = ?persist_readback.to_row_session_id,
            "readback=input_lease edge=handoff_committed"
        );
        self.command_audit_final(super::command_audit::CommandAuditInput::mcp(
            "control_lease_handoff",
            "lease_handoff",
            Some(session_id.clone()),
            Some(to_session.clone()),
            command_payload,
            command_before,
            json!({
                "source_of_truth": "synapse_action::lease",
                "response": &response,
                "from": lease_status_for_session(&session_id),
                "to": lease_status_for_session(&to_session),
            }),
            "ok",
        ))?;
        Ok(Json(response))
    }

    #[tool(
        description = "Read the current input lease state (holder, age, TTL, expiry). Never blocks; safe to poll. Reports whether the calling session is the holder.",
        input_schema = empty_input_schema()
    )]
    pub async fn control_lease_status(
        &self,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ControlLeaseResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "control_lease_status",
            "tool.invocation kind=control_lease_status"
        );
        let session_id = require_lease_session_id(&request_context)?;
        let transition_gate = self.lease_transition_gate();
        let _transition = transition_gate.lock().await;
        self.restore_session_lease_if_needed(&session_id)?;
        Ok(Json(lease_status_for_session(&session_id)))
    }
}

fn lease_acquire_audit(session_id: &str, ttl_ms: u64) -> super::command_audit::CommandAuditInput {
    super::command_audit::CommandAuditInput::mcp(
        "control_lease_acquire",
        "lease_acquire",
        Some(session_id.to_owned()),
        Some(session_id.to_owned()),
        json!({ "ttl_ms": ttl_ms }),
        json!({
            "source_of_truth": "synapse_action::lease",
            "caller": lease_status_for_session(session_id),
        }),
        Value::Null,
        "pending",
    )
}

pub(super) fn validate_lease_ttl_ms(tool: &'static str, ttl_ms: u64) -> Result<(), ErrorData> {
    if (synapse_action::MIN_LEASE_TTL_MS..=synapse_action::MAX_LEASE_TTL_MS).contains(&ttl_ms) {
        return Ok(());
    }
    tracing::warn!(
        code = error_codes::TOOL_PARAMS_INVALID,
        detail_code = "LEASE_TTL_OUT_OF_RANGE",
        tool,
        ttl_ms,
        min_ttl_ms = synapse_action::MIN_LEASE_TTL_MS,
        max_ttl_ms = synapse_action::MAX_LEASE_TTL_MS,
        source_of_truth = LEASE_TTL_SOURCE_OF_TRUTH,
        "lease ttl_ms rejected before lease mutation"
    );
    Err(ErrorData::new(
        ErrorCode(-32099),
        format!(
            "{tool} ttl_ms must be between {} and {}; got {ttl_ms}",
            synapse_action::MIN_LEASE_TTL_MS,
            synapse_action::MAX_LEASE_TTL_MS
        ),
        Some(json!({
            "code": error_codes::TOOL_PARAMS_INVALID,
            "detail_code": "LEASE_TTL_OUT_OF_RANGE",
            "tool": tool,
            "source_id": "ttl_ms",
            "source_of_truth": LEASE_TTL_SOURCE_OF_TRUTH,
            "min_ttl_ms": synapse_action::MIN_LEASE_TTL_MS,
            "max_ttl_ms": synapse_action::MAX_LEASE_TTL_MS,
            "ttl_ms": ttl_ms,
            "remediation": "pass ttl_ms in the advertised lease range or omit it for the default",
        })),
    ))
}

/// Explicit acquire. The flag is `true` when the call took over a lease that
/// the same session's foreground action had auto-acquired (#205): the action's
/// guard will no longer release it, and the response reports a fresh
/// `acquired` so the caller journals a `LeaseAcquired` for the new explicit
/// hold instead of an unmatched row.
fn acquire_lease_for_session_tracked(
    session_id: &str,
    ttl_ms: u64,
) -> Result<(ControlLeaseResponse, bool), ErrorData> {
    let ttl = lease::ttl_from_ms(ttl_ms);
    let (outcome, converted_from_auto) = lease::try_acquire_explicit(session_id, ttl);
    match outcome {
        LeaseOutcome::Acquired(status) => {
            tracing::info!(
                code = "INPUT_LEASE_ACQUIRED",
                session_id = %session_id,
                ttl_ms = status.ttl_ms,
                "readback=input_lease outcome=acquired"
            );
            Ok((
                ControlLeaseResponse::from_status("acquired", session_id.to_owned(), &status),
                converted_from_auto,
            ))
        }
        LeaseOutcome::Renewed(status) if converted_from_auto => {
            tracing::info!(
                code = "INPUT_LEASE_ACQUIRED",
                session_id = %session_id,
                ttl_ms = status.ttl_ms,
                "readback=input_lease outcome=acquired takeover=action_auto_lease"
            );
            Ok((
                ControlLeaseResponse::from_status("acquired", session_id.to_owned(), &status),
                true,
            ))
        }
        LeaseOutcome::Renewed(status) => {
            tracing::info!(
                code = "INPUT_LEASE_RENEWED",
                session_id = %session_id,
                ttl_ms = status.ttl_ms,
                "readback=input_lease outcome=renewed"
            );
            Ok((
                ControlLeaseResponse::from_status("renewed", session_id.to_owned(), &status),
                false,
            ))
        }
        LeaseOutcome::Busy {
            holder,
            retry_after_ms,
        } => {
            tracing::warn!(
                code = error_codes::ACTION_FOREGROUND_LEASE_BUSY,
                session_id = %session_id,
                holder = ?holder.owner_session_id,
                retry_after_ms,
                "readback=input_lease outcome=busy"
            );
            Err(lease_busy_error(session_id, &holder, retry_after_ms))
        }
        LeaseOutcome::CleanupPending {
            expired,
            retry_after_ms,
        } => {
            tracing::warn!(
                code = error_codes::ACTION_FOREGROUND_LEASE_BUSY,
                session_id = %session_id,
                expired_owner = ?expired.owner_session_id,
                retry_after_ms,
                "readback=input_lease outcome=cleanup_pending"
            );
            Err(lease_cleanup_pending_error(
                session_id,
                &expired,
                retry_after_ms,
            ))
        }
    }
}

fn release_lease_for_session(session_id: &str) -> Result<ControlLeaseResponse, ErrorData> {
    let (status, already_released) =
        lease::release(session_id).map_err(|error| lease_not_held_error(session_id, &error))?;
    if !already_released {
        tracing::info!(
            code = "INPUT_LEASE_RELEASED",
            session_id = %session_id,
            "readback=input_lease outcome=released"
        );
    }
    let mut response =
        ControlLeaseResponse::from_status("released", session_id.to_owned(), &status);
    response.already_released = Some(already_released);
    Ok(response)
}

fn handoff_lease_for_session(
    from_session_id: &str,
    to_session_id: &str,
    ttl_ms: u64,
) -> Result<synapse_action::LeaseHandoff, ErrorData> {
    match lease::handoff(from_session_id, to_session_id, lease::ttl_from_ms(ttl_ms)) {
        Ok(handoff) => {
            tracing::info!(
                code = "INPUT_LEASE_HANDED_OFF",
                from_session_id,
                to_session_id,
                prior = ?handoff.prior,
                current = ?handoff.current,
                "readback=input_lease outcome=handed_off"
            );
            Ok(handoff)
        }
        Err(error) => Err(lease_not_held_error(from_session_id, &error)),
    }
}

fn lease_status_for_session(session_id: &str) -> ControlLeaseResponse {
    let status = lease::status();
    ControlLeaseResponse::from_status("status", session_id.to_owned(), &status)
}

fn reconcile_lease_release_after_mutation(
    service: &SynapseService,
    session_id: &str,
    reason_code: &str,
) -> Result<(), ErrorData> {
    let readback = super::session_continuity::reconcile_persisted_session_lease_row(
        &service.m3_state_handle(),
        session_id,
        reason_code,
    )
    .map_err(|error| mcp_error(error_codes::STORAGE_CORRUPTED, error))?;
    // An absent row is not corruption (#94): auto-acquired foreground leases are
    // never persisted/journaled, and a prior closer (orphan sweep, hotkey K2)
    // already wrote the single balancing event and deleted the row. With no
    // open journaled acquisition there is nothing to balance.
    if !readback.release_event_written {
        tracing::info!(
            code = "MCP_SESSION_LEASE_RELEASE_NO_PERSISTED_ROW",
            session_id,
            reason_code,
            "no persisted input lease lifecycle row to close"
        );
    }
    Ok(())
}

impl SynapseService {
    fn ensure_handoff_recipient_live(
        &self,
        from_session_id: &str,
        to_session_id: &str,
    ) -> Result<(), ErrorData> {
        validate_session_id(to_session_id)?;
        if from_session_id == to_session_id {
            return Err(ErrorData::new(
                ErrorCode(-32099),
                "control_lease_handoff requires a different recipient session",
                Some(json!({
                    "code": error_codes::TOOL_PARAMS_INVALID,
                    "from_session_id": from_session_id,
                    "to_session_id": to_session_id,
                })),
            ));
        }
        let now_unix_ms = unix_time_ms_now();
        let recipient = {
            let guard = self.session_registry_ref().lock().map_err(|_error| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "session registry lock poisoned while validating lease handoff recipient",
                )
            })?;
            guard
                .reads(now_unix_ms)
                .into_iter()
                .find(|entry| entry.session_id == to_session_id)
        };
        match recipient.as_ref() {
            Some(read) if read.lifecycle == "live" => Ok(()),
            _ => Err(recipient_unknown_error(
                from_session_id,
                to_session_id,
                recipient.as_ref(),
            )),
        }
    }

    fn rollback_handoff_after_persist_failure(
        &self,
        from_session_id: &str,
        to_session_id: &str,
        prior: &LeaseStatus,
        error: &ErrorData,
    ) {
        let rollback_ttl_ms = prior
            .expires_in_ms
            .or(prior.ttl_ms)
            .unwrap_or(synapse_action::DEFAULT_LEASE_TTL_MS);
        match lease::handoff(
            to_session_id,
            from_session_id,
            lease::ttl_from_ms(rollback_ttl_ms),
        ) {
            Ok(rollback) => {
                // Restore the durable row only for a holder that had one. An
                // action-acquired lease is never persisted or journaled; a row
                // created here would have no `LeaseAcquired` and later be closed
                // by the orphan sweep with an unmatched `LeaseReleased` (#205).
                let holder_had_row = self.session_lease_row_exists(from_session_id);
                let row_restored = matches!(holder_had_row, Ok(true));
                let rollback_persist: Result<(), ErrorData> = match holder_had_row {
                    Ok(true) => self
                        .persist_session_lease_handoff(
                            to_session_id,
                            from_session_id,
                            &rollback.current,
                        )
                        .map(|_readback| ()),
                    Ok(false) => Ok(()),
                    Err(error) => Err(error),
                };
                tracing::error!(
                    code = error_codes::TOOL_INTERNAL_ERROR,
                    from_session_id,
                    to_session_id,
                    rollback_persisted = rollback_persist.is_ok(),
                    row_restored,
                    rollback_persist_error = ?rollback_persist.as_ref().err(),
                    original_error = ?error,
                    rollback = ?rollback,
                    "input lease handoff failed durability write; rolled memory back to prior holder"
                );
            }
            Err(rollback_error) => {
                let recipient_released = lease::release_if_owner(to_session_id);
                tracing::error!(
                    code = error_codes::TOOL_INTERNAL_ERROR,
                    from_session_id,
                    to_session_id,
                    recipient_released,
                    rollback_error = ?rollback_error,
                    original_error = ?error,
                    "input lease handoff failed durability write and memory rollback could not restore prior holder"
                );
            }
        }
    }

    pub(crate) async fn dashboard_control_lease_force_release(
        &self,
        owner_session_id: String,
        confirmed: bool,
    ) -> Result<DashboardControlLeaseForceReleaseResponse, ErrorData> {
        validate_session_id(&owner_session_id)?;
        // Not even an operator override may pull the lease out from under an
        // in-flight escalation: its unwind owns that lease until it finishes.
        let _hold = super::act_foreground_slot::admit_session_operation(
            &owner_session_id,
            "dashboard_control_lease_force_release",
        )?;
        let transition_gate = self.lease_transition_gate();
        let _transition = transition_gate.lock().await;
        let before = lease_status_for_session(&owner_session_id);
        let command_payload = json!({
            "owner_session_id": &owner_session_id,
            "confirmed": confirmed,
        });
        let command_before = json!({
            "source_of_truth": DASHBOARD_LEASE_SOURCE_OF_TRUTH,
            "owner": &before,
        });
        self.command_audit_intent(
            super::command_audit::CommandAuditInput::mcp(
                "control_lease_force_release",
                "lease_force_release",
                None,
                Some(owner_session_id.clone()),
                command_payload.clone(),
                command_before.clone(),
                Value::Null,
                "pending",
            )
            .with_channel("dashboard"),
        )?;
        if !confirmed {
            let error = mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                "dashboard lease force-release requires confirmation",
            );
            self.command_audit_final(
                super::command_audit::CommandAuditInput::mcp(
                    "control_lease_force_release",
                    "lease_force_release",
                    None,
                    Some(owner_session_id.clone()),
                    command_payload,
                    command_before,
                    json!({
                        "source_of_truth": DASHBOARD_LEASE_SOURCE_OF_TRUTH,
                        "owner": lease_status_for_session(&owner_session_id),
                    }),
                    "error",
                )
                .with_channel("dashboard")
                .with_error(
                    super::command_audit::command_audit_error_from_error_data(&error),
                ),
            )?;
            return Err(error);
        }
        let released_prior =
            lease::force_clear_if_owner(&owner_session_id, "dashboard_force_release");
        let released = released_prior.is_some();
        let persisted_row_deleted = if released {
            if let Err(error) = reconcile_lease_release_after_mutation(
                self,
                &owner_session_id,
                "dashboard_force_release",
            ) {
                self.command_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "control_lease_force_release",
                        "lease_force_release",
                        None,
                        Some(owner_session_id.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": DASHBOARD_LEASE_SOURCE_OF_TRUTH,
                            "released": true,
                            "owner": lease_status_for_session(&owner_session_id),
                        }),
                        "error",
                    )
                    .with_channel("dashboard")
                    .with_error(
                        super::command_audit::command_audit_error_from_error_data(&error),
                    ),
                )?;
                return Err(error);
            }
            true
        } else {
            match super::session_continuity::reconcile_persisted_session_lease_row(
                &self.m3_state_handle(),
                &owner_session_id,
                "dashboard_force_release",
            ) {
                Ok(readback) => readback.row_deleted,
                Err(error) => {
                    let error = mcp_error(error_codes::STORAGE_CORRUPTED, error);
                    self.command_audit_final(
                        super::command_audit::CommandAuditInput::mcp(
                            "control_lease_force_release",
                            "lease_force_release",
                            None,
                            Some(owner_session_id.clone()),
                            command_payload,
                            command_before,
                            json!({
                                "source_of_truth": DASHBOARD_LEASE_SOURCE_OF_TRUTH,
                                "released": false,
                                "owner": lease_status_for_session(&owner_session_id),
                            }),
                            "error",
                        )
                        .with_channel("dashboard")
                        .with_error(
                            super::command_audit::command_audit_error_from_error_data(&error),
                        ),
                    )?;
                    return Err(error);
                }
            }
        };
        let response = DashboardControlLeaseForceReleaseResponse {
            requested_owner_session_id: owner_session_id.clone(),
            confirmed,
            released,
            before,
            after: lease_status_for_session(&owner_session_id),
            persisted_row_deleted,
            source_of_truth: DASHBOARD_LEASE_SOURCE_OF_TRUTH,
        };
        self.command_audit_final(
            super::command_audit::CommandAuditInput::mcp(
                "control_lease_force_release",
                "lease_force_release",
                None,
                Some(owner_session_id),
                command_payload,
                command_before,
                json!({
                    "source_of_truth": DASHBOARD_LEASE_SOURCE_OF_TRUTH,
                    "response": &response,
                }),
                "ok",
            )
            .with_channel("dashboard"),
        )?;
        Ok(response)
    }

    pub(crate) async fn dashboard_control_lease_handoff(
        &self,
        from_session_id: String,
        to_session_id: String,
        ttl_ms: u64,
    ) -> Result<DashboardControlLeaseHandoffResponse, ErrorData> {
        validate_session_id(&from_session_id)?;
        let _hold_from = super::act_foreground_slot::admit_session_operation(
            &from_session_id,
            "dashboard_control_lease_handoff",
        )?;
        let _hold_to = super::act_foreground_slot::admit_session_operation(
            &to_session_id,
            "dashboard_control_lease_handoff",
        )?;
        let transition_gate = self.lease_transition_gate();
        let _transition = transition_gate.lock().await;
        self.ensure_handoff_recipient_live(&from_session_id, &to_session_id)?;
        self.restore_session_lease_if_needed(&from_session_id)?;
        let before_from = lease_status_for_session(&from_session_id);
        let before_to = lease_status_for_session(&to_session_id);
        let command_payload = json!({
            "from_session_id": &from_session_id,
            "to_session_id": &to_session_id,
            "ttl_ms": ttl_ms,
        });
        let command_before = json!({
            "source_of_truth": DASHBOARD_LEASE_SOURCE_OF_TRUTH,
            "from": &before_from,
            "to": &before_to,
        });
        self.command_audit_intent(
            super::command_audit::CommandAuditInput::mcp(
                "control_lease_handoff",
                "lease_handoff",
                Some(from_session_id.clone()),
                Some(to_session_id.clone()),
                command_payload.clone(),
                command_before.clone(),
                Value::Null,
                "pending",
            )
            .with_channel("dashboard"),
        )?;
        let handoff = match handoff_lease_for_session(&from_session_id, &to_session_id, ttl_ms) {
            Ok(handoff) => handoff,
            Err(error) => {
                self.command_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "control_lease_handoff",
                        "lease_handoff",
                        Some(from_session_id.clone()),
                        Some(to_session_id.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": DASHBOARD_LEASE_SOURCE_OF_TRUTH,
                            "from": lease_status_for_session(&from_session_id),
                            "to": lease_status_for_session(&to_session_id),
                        }),
                        "error",
                    )
                    .with_channel("dashboard")
                    .with_error(
                        super::command_audit::command_audit_error_from_error_data(&error),
                    ),
                )?;
                return Err(error);
            }
        };
        let response = ControlLeaseResponse::from_status(
            "handed_off",
            from_session_id.clone(),
            &handoff.current,
        );
        let persist_readback = match self.persist_session_lease_handoff_with_events(
            &from_session_id,
            &to_session_id,
            &handoff.current,
        ) {
            Ok(readback) => readback,
            Err(error) => {
                self.rollback_handoff_after_persist_failure(
                    &from_session_id,
                    &to_session_id,
                    &handoff.prior,
                    &error,
                );
                self.command_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "control_lease_handoff",
                        "lease_handoff",
                        Some(from_session_id.clone()),
                        Some(to_session_id.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": DASHBOARD_LEASE_SOURCE_OF_TRUTH,
                            "from": lease_status_for_session(&from_session_id),
                            "to": lease_status_for_session(&to_session_id),
                        }),
                        "error",
                    )
                    .with_channel("dashboard")
                    .with_error(
                        super::command_audit::command_audit_error_from_error_data(&error),
                    ),
                )?;
                return Err(error);
            }
        };
        tracing::info!(
            code = "DASHBOARD_INPUT_LEASE_HANDOFF_COMMITTED",
            from_session_id,
            to_session_id,
            from_row_existed_before = persist_readback.from_row_existed_before,
            from_row_exists_after = persist_readback.from_row_exists_after,
            to_row_exists_after = persist_readback.to_row_exists_after,
            "readback=input_lease edge=dashboard_handoff_committed"
        );
        let dashboard_response = DashboardControlLeaseHandoffResponse {
            from_session_id: from_session_id.clone(),
            to_session_id: to_session_id.clone(),
            ttl_ms,
            before_from,
            before_to,
            response,
            after_from: lease_status_for_session(&from_session_id),
            after_to: lease_status_for_session(&to_session_id),
            source_of_truth: DASHBOARD_LEASE_SOURCE_OF_TRUTH,
        };
        self.command_audit_final(
            super::command_audit::CommandAuditInput::mcp(
                "control_lease_handoff",
                "lease_handoff",
                Some(from_session_id),
                Some(to_session_id),
                command_payload,
                command_before,
                json!({
                    "source_of_truth": DASHBOARD_LEASE_SOURCE_OF_TRUTH,
                    "response": &dashboard_response,
                }),
                "ok",
            )
            .with_channel("dashboard"),
        )?;
        Ok(dashboard_response)
    }
}

/// Resolves the calling session id, failing loud when absent. The lease is
/// per-session, so an unidentified caller cannot meaningfully own it.
fn require_lease_session_id(
    request_context: &RequestContext<RoleServer>,
) -> Result<String, ErrorData> {
    super::context::mcp_session_id_from_request_context(request_context)?.ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "input lease tools require an MCP session id (run the daemon in HTTP mode so each agent has its own Mcp-Session-Id)",
        )
    })
}

fn lease_busy_error(session_id: &str, holder: &LeaseStatus, retry_after_ms: u64) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "input lease is held by session {:?}; retry after {retry_after_ms}ms",
            holder.owner_session_id
        ),
        Some(json!({
            "code": error_codes::ACTION_FOREGROUND_LEASE_BUSY,
            "requesting_session_id": session_id,
            "holder_session_id": holder.owner_session_id,
            "retry_after_ms": retry_after_ms,
            "holder": holder,
        })),
    )
}

fn lease_cleanup_pending_error(
    session_id: &str,
    expired: &LeaseStatus,
    retry_after_ms: u64,
) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "input lease expired for session {:?}; held-input cleanup is pending, retry after {retry_after_ms}ms",
            expired.owner_session_id
        ),
        Some(json!({
            "code": error_codes::ACTION_FOREGROUND_LEASE_BUSY,
            "requesting_session_id": session_id,
            "holder_session_id": expired.owner_session_id,
            "retry_after_ms": retry_after_ms,
            "expired": expired,
            "cleanup_pending": true,
        })),
    )
}

fn recipient_unknown_error(
    from_session_id: &str,
    to_session_id: &str,
    recipient: Option<&SessionRegistryRead>,
) -> ErrorData {
    ErrorData::new(
        ErrorCode(-32099),
        format!("lease handoff recipient session {to_session_id:?} is not live"),
        Some(json!({
            "code": error_codes::RECIPIENT_UNKNOWN,
            "from_session_id": from_session_id,
            "to_session_id": to_session_id,
            "recipient": recipient,
            "resolution": "start or reconnect the recipient agent so it registers a live MCP session, then retry handoff",
        })),
    )
}

fn lease_not_held_error(session_id: &str, error: &synapse_action::LeaseError) -> ErrorData {
    let holder = match error {
        synapse_action::LeaseError::NotHeld { holder, .. }
        | synapse_action::LeaseError::CleanupBusy { holder } => holder.clone(),
    };
    ErrorData::new(
        ErrorCode(-32099),
        error.to_string(),
        Some(json!({
            "code": error.code(),
            "requesting_session_id": session_id,
            "holder_session_id": holder,
        })),
    )
}

impl SynapseService {
    /// Releases a lease whose acquisition could not be acknowledged to the
    /// caller, reconciling the durable session lease row like an explicit
    /// release. Returns whether this call released the lease.
    pub(super) fn rollback_unacknowledged_lease(&self, session_id: &str) -> bool {
        let released = release_lease_for_session(session_id)
            .is_ok_and(|response| response.released_by_this_call());
        let reconciled =
            reconcile_lease_release_after_mutation(self, session_id, "unacknowledged_acquire")
                .is_ok();
        tracing::warn!(
            code = "LEASE_ACQUIRE_ROLLED_BACK",
            session_id,
            released,
            reconciled,
            "input lease acquisition was rolled back because its final audit failed"
        );
        released
    }
}

#[cfg(test)]
#[path = "tests/lease_release_rowless_94.rs"]
mod lease_release_rowless_94_tests;

#[cfg(test)]
#[path = "tests/lease_source_of_truth_226.rs"]
mod lease_source_of_truth_226_tests;

#[cfg(test)]
#[path = "tests/lease_explicit_over_auto_205.rs"]
mod lease_explicit_over_auto_205_tests;

#[cfg(test)]
#[path = "tests/lease_tools_test_support.rs"]
mod lease_tools_test_support;
