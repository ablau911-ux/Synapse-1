//! Advisory target ownership claims for multi-agent coordination (#797).
//!
//! Claims are daemon-local read-model state keyed by target identity. They do
//! not replace per-tool validation or capability checks; they make same-target
//! mutation conflicts explicit before another session silently clobbers a
//! claimed window/tab.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use rmcp::{RoleServer, model::ErrorCode, service::RequestContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use synapse_action::{
    ActionError, InputDeliveryTarget, delivery_target::require_stored_input_target,
};
use synapse_core::{error_codes, types::StoredInputTarget};

use super::{
    ErrorData, Json, Parameters, SessionTarget, SynapseService, TargetWire, mcp_error,
    session_registry::unix_time_ms_now, tool, tool_router,
};

#[path = "target_claim_adopt_proof.rs"]
pub(super) mod adopt_proof;
#[path = "tab_lock_idle.rs"]
pub(super) mod tab_lock_idle;

#[cfg(test)]
#[path = "adoption_pause_support.rs"]
mod adoption_pause_support;
#[cfg(test)]
use adoption_pause_support::{adoption_handoff_pause_for_test, pause_adoption_handoff_for_test};

pub(crate) const DEFAULT_TARGET_CLAIM_TTL_MS: u64 = 120_000;
const MIN_TARGET_CLAIM_TTL_MS: u64 = 1_000;
const MAX_TARGET_CLAIM_TTL_MS: u64 = 600_000;
/// Upper bound on live claims one session may hold at once (#207).
pub(crate) const MAX_TARGET_CLAIMS_PER_SESSION: usize = 64;

pub(crate) type SharedTargetClaims = Arc<Mutex<TargetClaimRegistry>>;

#[derive(Debug, Default)]
pub(crate) struct TargetClaimRegistry {
    claims: BTreeMap<String, TargetClaimEntry>,
    next_generation: u64,
    adoption_reservations: BTreeSet<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct TargetClaimEntry {
    target_key: String,
    target: SessionTarget,
    owner_session_id: String,
    claimed_at_unix_ms: u64,
    renewed_at_unix_ms: u64,
    ttl_ms: u64,
    expires_at_unix_ms: u64,
    generation: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetClaimTargetParam {
    Window {
        window_hwnd: i64,
    },
    Cdp {
        window_hwnd: i64,
        cdp_target_id: String,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetClaimParams {
    #[serde(default)]
    #[schemars(default)]
    pub target: Option<TargetClaimTargetParam>,
    #[serde(default = "default_target_claim_ttl_ms")]
    #[schemars(
        default = "default_target_claim_ttl_ms",
        range(min = 1000, max = 600000)
    )]
    pub ttl_ms: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetClaimAdoptParams {
    /// Existing owner session id read from target_claim_status/session_list.
    pub owner_session_id: String,
    #[serde(default)]
    #[schemars(default)]
    pub target: Option<TargetClaimTargetParam>,
    #[serde(default = "default_target_claim_ttl_ms")]
    #[schemars(
        default = "default_target_claim_ttl_ms",
        range(min = 1000, max = 600000)
    )]
    pub ttl_ms: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetReleaseParams {
    #[serde(default)]
    #[schemars(default)]
    pub target: Option<TargetClaimTargetParam>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetClaimStatusParams {
    #[serde(default)]
    #[schemars(default)]
    pub target: Option<TargetClaimTargetParam>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetClaimRead {
    pub target_key: String,
    pub target: TargetWire,
    pub owner_session_id: String,
    pub claimed_at_unix_ms: u64,
    pub renewed_at_unix_ms: u64,
    pub ttl_ms: u64,
    pub expires_at_unix_ms: u64,
    pub expires_in_ms: u64,
    pub generation: u64,
    pub source_of_truth: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetClaimResponse {
    pub session_id: String,
    pub outcome: String,
    pub claim: TargetClaimRead,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetClaimAdoptResponse {
    pub session_id: String,
    pub adopted_from_session_id: String,
    pub outcome: String,
    pub prior_claim: TargetClaimRead,
    pub claim: TargetClaimRead,
    pub owner_in_flight_before: Vec<crate::daemon_lifecycle::InFlightToolCallRead>,
    pub owner_teardown_report: crate::server::session_lifecycle::SessionTeardownReport,
    pub source_of_truth: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetReleaseResponse {
    pub session_id: String,
    pub target_key: String,
    pub target: TargetWire,
    pub released: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_claim: Option<TargetClaimRead>,
    pub source_of_truth: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetClaimStatusResponse {
    pub session_id: String,
    pub now_unix_ms: u64,
    pub claim_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_claim: Option<TargetClaimRead>,
    /// Whether the status target's window still exists (#206). `None` when no
    /// target was supplied or active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_live: Option<bool>,
    pub claims: Vec<TargetClaimRead>,
    pub source_of_truth: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DashboardTargetClaimPruneResponse {
    pub before_count: usize,
    pub after_count: usize,
    pub pruned_count: usize,
    pub before_claims: Vec<TargetClaimRead>,
    pub after: TargetClaimStatusResponse,
    pub source_of_truth: String,
}

#[derive(Clone, Debug, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetClaimCleanupReport {
    pub owned_before: usize,
    pub released: usize,
    pub target_keys: Vec<String>,
    pub failed: bool,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct TargetClaimConflict {
    entry: TargetClaimEntry,
    requester_session_id: String,
    tool: &'static str,
}

#[tool_router(router = target_claim_tool_router, vis = "pub(super)")]
impl SynapseService {
    #[tool(
        description = "Claim this MCP session's active target, or an explicit window/CDP target, as an advisory ownership lease. A live claim causes other sessions' mutating actions against the same target to fail closed with TARGET_CO_OWNED while read-only observe remains allowed."
    )]
    pub async fn target_claim(
        &self,
        params: Parameters<TargetClaimParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<TargetClaimResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "target_claim",
            "tool.invocation kind=target_claim"
        );
        let session_id = require_claim_session_id(&request_context)?;
        validate_target_claim_ttl(params.0.ttl_ms)?;
        let target = self.resolve_target_claim_target(&session_id, params.0.target)?;
        self.validate_registered_claim_target(&target)?;
        let now = unix_time_ms_now();
        let live_sessions = self.live_target_claim_sessions(now, Some(&session_id))?;
        let target_key = target_key(&target);
        let target_wire = target_wire(&target);
        let command_payload = json!({
            "target_key": &target_key,
            "target": &target_wire,
            "ttl_ms": params.0.ttl_ms,
        });
        // #207: snapshot under the lock, then run the WAL-synced audit writes
        // with the claims mutex released.
        let claims_before = snapshot_claims(&self.target_claims, now, &live_sessions)?;
        let command_before = json!({
            "source_of_truth": source_of_truth(),
            "claims": bounded_claims_view(&claims_before, &target_key),
        });
        self.claims_audit_intent(
            super::command_audit::CommandAuditInput::mcp(
                "target_claim",
                "target_claim",
                Some(session_id.clone()),
                Some(session_id.clone()),
                command_payload.clone(),
                command_before.clone(),
                Value::Null,
                "pending",
            )
            .with_target(json!({
                "target_key": &target_key,
                "target": &target_wire,
            })),
        )?;
        let attempt = attempt_claim(
            &self.target_claims,
            &session_id,
            target,
            params.0.ttl_ms,
            now,
            &live_sessions,
        )?;
        let claims_after = bounded_claims_view(&attempt.claims_after, &target_key);
        match attempt.result {
            Ok((entry, outcome)) => {
                let response = TargetClaimResponse {
                    session_id: session_id.clone(),
                    outcome: outcome.to_owned(),
                    claim: entry.read(now),
                };
                self.claims_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "target_claim",
                        "target_claim",
                        Some(session_id.clone()),
                        Some(session_id.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": source_of_truth(),
                            "response": &response,
                            "claims": claims_after,
                        }),
                        "ok",
                    )
                    .with_target(json!({
                        "target_key": &target_key,
                        "target": &target_wire,
                    })),
                )?;
                tracing::info!(
                    code = "TARGET_CLAIM_SET",
                    session_id = %session_id,
                    target_key = %entry.target_key,
                    outcome,
                    expires_at_unix_ms = entry.expires_at_unix_ms,
                    "readback=target_claim outcome={outcome}"
                );
                Ok(Json(response))
            }
            Err(refusal) => {
                let error = refusal.error("target_claim", &session_id, "target_claim");
                self.claims_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "target_claim",
                        "target_claim",
                        Some(session_id.clone()),
                        Some(session_id.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": source_of_truth(),
                            "claims": claims_after,
                        }),
                        "error",
                    )
                    .with_target(json!({
                        "target_key": &target_key,
                        "target": &target_wire,
                    }))
                    .with_error(
                        super::command_audit::command_audit_error_from_error_data(&error),
                    ),
                )?;
                Err(error)
            }
        }
    }

    #[tool(
        description = "Explicitly recover a target claim from an older same-agent MCP session whose agent process is provably gone. The caller must provide the current owner_session_id from target_claim_status/session_list; adoption fails closed unless both sessions have the same client identity, the caller is newer, the old owner has no in-flight tool call or input lease, and every process recorded for the owner is no longer running (a shared client name or idleness is never proof). On success the old session is terminated through session lifecycle cleanup before the caller receives the target claim."
    )]
    pub async fn target_claim_adopt(
        &self,
        params: Parameters<TargetClaimAdoptParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<TargetClaimAdoptResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "target_claim_adopt",
            "tool.invocation kind=target_claim_adopt"
        );
        let session_id = require_claim_session_id(&request_context)?;
        self.target_claim_adopt_for_session(session_id, params.0)
            .await
    }

    /// `target_claim_adopt` for an already-resolved requester MCP session id.
    /// The tool entry point only resolves the session id from the request
    /// context and delegates here (test seam: no `RequestContext` needed).
    pub(crate) async fn target_claim_adopt_for_session(
        &self,
        session_id: String,
        params: TargetClaimAdoptParams,
    ) -> Result<Json<TargetClaimAdoptResponse>, ErrorData> {
        let TargetClaimAdoptParams {
            owner_session_id,
            target,
            ttl_ms,
        } = params;
        super::session_tools::validate_session_id(&owner_session_id)?;
        if owner_session_id == session_id {
            return Err(adopt_refused_error(
                &session_id,
                &owner_session_id,
                "target_claim_adopt requires a different owner_session_id",
                None,
                None,
            ));
        }
        validate_target_claim_ttl(ttl_ms)?;
        let target = self.resolve_target_claim_target(&session_id, target)?;
        self.validate_registered_claim_target(&target)?;
        let now = unix_time_ms_now();
        let live_sessions = self.live_target_claim_sessions(now, Some(&session_id))?;
        let prior_entry = {
            let mut guard = self.lock_target_claims()?;
            guard.prune_inactive(now, &live_sessions);
            guard.get(&target_key(&target))
        }
        .ok_or_else(|| target_claim_not_found_error("target_claim_adopt", &session_id, &target))?;
        if prior_entry.owner_session_id != owner_session_id {
            return Err(adopt_refused_error(
                &session_id,
                &owner_session_id,
                "target_claim_adopt owner_session_id does not match the live target claim owner",
                None,
                None,
            ));
        }
        let prior_claim = prior_entry.read(now);
        let (requester, owner, owner_in_flight_before) =
            self.ensure_same_agent_adoption_allowed(&session_id, &owner_session_id, now)?;
        // #201: adoption terminates the owner's session, processes and tabs, so
        // an idle-but-alive peer must be refused; require independent proof.
        let owner_gone_proof = adopt_proof::prove_owner_gone(&owner, &crate::m4::process_exists)
            .map_err(|reason| {
                adopt_refused_error(
                    &session_id,
                    &owner_session_id,
                    &reason,
                    Some(&requester),
                    Some(&owner),
                )
            })?;
        let command_payload = json!({
            "owner_session_id": &owner_session_id,
            "target_key": &prior_claim.target_key,
            "target": &prior_claim.target,
            "ttl_ms": ttl_ms,
        });
        let command_before = json!({
            "source_of_truth": source_of_truth(),
            "prior_claim": &prior_claim,
            "owner_in_flight_before": owner_in_flight_before,
            "owner_gone_proof": &owner_gone_proof,
        });
        self.claims_audit_intent(
            super::command_audit::CommandAuditInput::mcp(
                "target_claim_adopt",
                "target_claim_adopt",
                Some(session_id.clone()),
                Some(owner_session_id.clone()),
                command_payload.clone(),
                command_before.clone(),
                Value::Null,
                "pending",
            )
            .with_target(json!({
                "target_key": &prior_claim.target_key,
                "target": &prior_claim.target,
            })),
        )?;

        // The intent audit write above took time: abort without tearing anything
        // down if the claim moved since it was read (#201), or if the requester
        // could not take the claim afterwards anyway (#207).
        let reservation = AdoptionReservation::reserve(self, &prior_entry, &session_id, &target)?;
        if let Err(reason) = reservation {
            let error = adopt_refused_error(
                &session_id,
                &owner_session_id,
                reason,
                Some(&requester),
                Some(&owner),
            );
            self.claims_audit_final(
                super::command_audit::CommandAuditInput::mcp(
                    "target_claim_adopt",
                    "target_claim_adopt",
                    Some(session_id.clone()),
                    Some(owner_session_id.clone()),
                    command_payload,
                    command_before,
                    json!({ "source_of_truth": source_of_truth() }),
                    "error",
                )
                .with_target(json!({
                    "target_key": &prior_claim.target_key,
                    "target": &prior_claim.target,
                }))
                .with_error(
                    super::command_audit::command_audit_error_from_error_data(&error),
                ),
            )?;
            return Err(error);
        }
        let mut reservation = reservation.map_err(|reason| {
            adopt_refused_error(
                &session_id,
                &owner_session_id,
                reason,
                Some(&requester),
                Some(&owner),
            )
        })?;

        let lifecycle = self.session_lifecycle_state()?;
        let owner_teardown_report = match lifecycle
            .teardown_session(&owner_session_id, "target_claim_adopt")
            .await
        {
            Ok(report) => report,
            Err(error) => {
                self.claims_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "target_claim_adopt",
                        "target_claim_adopt",
                        Some(session_id.clone()),
                        Some(owner_session_id.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": source_of_truth(),
                            "claims": self.claims_audit_view_now(&session_id, &prior_claim.target_key)?,
                        }),
                        "error",
                    )
                    .with_target(json!({
                        "target_key": &prior_claim.target_key,
                        "target": &prior_claim.target,
                    }))
                    .with_error(
                        super::command_audit::command_audit_error_from_error_data(&error),
                    ),
                )?;
                return Err(error);
            }
        };

        #[cfg(test)]
        pause_adoption_handoff_for_test(self, &owner_session_id).await;
        let now_after = unix_time_ms_now();
        let live_sessions_after = self.live_target_claim_sessions(now_after, Some(&session_id))?;
        let attempt = attempt_claim(
            &self.target_claims,
            &session_id,
            target,
            ttl_ms,
            now_after,
            &live_sessions_after,
        )?;
        let claims_after = bounded_claims_view(&attempt.claims_after, &prior_claim.target_key);
        reservation.committed = attempt.result.is_ok();
        let (entry, _claim_outcome) = match attempt.result {
            Ok(result) => result,
            Err(refusal) => {
                let error = refusal.error(
                    "target_claim_adopt",
                    &session_id,
                    "adopt_after_owner_teardown",
                );
                self.claims_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "target_claim_adopt",
                        "target_claim_adopt",
                        Some(session_id.clone()),
                        Some(owner_session_id.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": source_of_truth(),
                            "owner_teardown_report": &owner_teardown_report,
                            "claims": claims_after,
                        }),
                        "error",
                    )
                    .with_target(json!({
                        "target_key": &prior_claim.target_key,
                        "target": &prior_claim.target,
                    }))
                    .with_error(
                        super::command_audit::command_audit_error_from_error_data(&error),
                    ),
                )?;
                return Err(error);
            }
        };
        let response = TargetClaimAdoptResponse {
            session_id: session_id.clone(),
            adopted_from_session_id: owner_session_id.clone(),
            outcome: "adopted".to_owned(),
            prior_claim,
            claim: entry.read(now_after),
            owner_in_flight_before,
            owner_teardown_report,
            source_of_truth: source_of_truth(),
        };
        self.claims_audit_final(
            super::command_audit::CommandAuditInput::mcp(
                "target_claim_adopt",
                "target_claim_adopt",
                Some(session_id.clone()),
                Some(owner_session_id.clone()),
                command_payload,
                command_before,
                json!({
                    "source_of_truth": source_of_truth(),
                    "response": &response,
                    "claims": claims_after,
                }),
                "ok",
            )
            .with_target(json!({
                "target_key": &response.claim.target_key,
                "target": &response.claim.target,
            })),
        )?;
        tracing::info!(
            code = "TARGET_CLAIM_ADOPTED",
            session_id = %session_id,
            adopted_from_session_id = %owner_session_id,
            target_key = %entry.target_key,
            generation = entry.generation,
            prior_generation = prior_entry.generation,
            "readback=target_claim_adopt outcome=adopted"
        );
        Ok(Json(response))
    }

    #[tool(
        description = "Release this MCP session's advisory target ownership claim for the active target, or an explicit window/CDP target. A session cannot release another session's claim."
    )]
    pub async fn target_release(
        &self,
        params: Parameters<TargetReleaseParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<TargetReleaseResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "target_release",
            "tool.invocation kind=target_release"
        );
        let session_id = require_claim_session_id(&request_context)?;
        let target = self.resolve_target_claim_target(&session_id, params.0.target)?;
        validate_claim_target_shape(&target)?;
        let target_key = target_key(&target);
        let now = unix_time_ms_now();
        let live_sessions = self.live_target_claim_sessions(now, Some(&session_id))?;
        let target_wire = target_wire(&target);
        let command_payload = json!({
            "target_key": &target_key,
            "target": &target_wire,
        });
        let claims_before = snapshot_claims(&self.target_claims, now, &live_sessions)?;
        let command_before = json!({
            "source_of_truth": source_of_truth(),
            "claims": bounded_claims_view(&claims_before, &target_key),
        });
        self.claims_audit_intent(
            super::command_audit::CommandAuditInput::mcp(
                "target_release",
                "target_release",
                Some(session_id.clone()),
                Some(session_id.clone()),
                command_payload.clone(),
                command_before.clone(),
                Value::Null,
                "pending",
            )
            .with_target(json!({
                "target_key": &target_key,
                "target": &target_wire,
            })),
        )?;
        let attempt = attempt_release(
            &self.target_claims,
            &session_id,
            &target_key,
            now,
            &live_sessions,
        )?;
        let claims_after = bounded_claims_view(&attempt.claims_after, &target_key);
        match attempt.result {
            Ok(released) => {
                let released_claim = released.as_ref().map(|entry| entry.read(now));
                let response = TargetReleaseResponse {
                    session_id: session_id.clone(),
                    target_key: target_key.clone(),
                    target: target_wire.clone(),
                    released: released.is_some(),
                    released_claim,
                    source_of_truth: source_of_truth(),
                };
                self.claims_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "target_release",
                        "target_release",
                        Some(session_id.clone()),
                        Some(session_id.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": source_of_truth(),
                            "response": &response,
                            "claims": claims_after,
                        }),
                        "ok",
                    )
                    .with_target(json!({
                        "target_key": &target_key,
                        "target": &target_wire,
                    })),
                )?;
                tracing::info!(
                    code = "TARGET_CLAIM_RELEASED",
                    session_id = %session_id,
                    target_key = %target_key,
                    released = released.is_some(),
                    "readback=target_release"
                );
                Ok(Json(response))
            }
            Err(conflict) => {
                let error =
                    conflict_error("target_release", &session_id, &conflict, "target_release");
                self.claims_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "target_release",
                        "target_release",
                        Some(session_id.clone()),
                        Some(session_id.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": source_of_truth(),
                            "claims": claims_after,
                        }),
                        "error",
                    )
                    .with_target(json!({
                        "target_key": &target_key,
                        "target": &target_wire,
                    }))
                    .with_error(
                        super::command_audit::command_audit_error_from_error_data(&error),
                    ),
                )?;
                Err(error)
            }
        }
    }

    #[tool(
        description = "Read the live advisory target ownership claims. With target omitted, returns all live claims; with target supplied or active, also returns the claim for that target."
    )]
    pub async fn target_claim_status(
        &self,
        params: Parameters<TargetClaimStatusParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<TargetClaimStatusResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "target_claim_status",
            "tool.invocation kind=target_claim_status"
        );
        let session_id = require_claim_session_id(&request_context)?;
        let target = match params.0.target {
            Some(target) => Some(target_param_to_session_target(target)?),
            None => self.session_target(Some(&session_id))?,
        }
        .map(canonical_claim_target);
        // #206: a gone window must not hide its (stale) claim from the agent
        // that needs to see and release it; report liveness instead of erroring.
        let target_live = target.as_ref().map(status_target_liveness).transpose()?;
        let now = unix_time_ms_now();
        let live_sessions = self.live_target_claim_sessions(now, Some(&session_id))?;
        let mut guard = self.lock_target_claims()?;
        guard.prune_inactive(now, &live_sessions);
        let target_key = target.as_ref().map(target_key);
        let claims = guard.reads(now);
        let target_claim = target_key
            .as_ref()
            .and_then(|key| claims.iter().find(|claim| &claim.target_key == key))
            .cloned();
        Ok(Json(TargetClaimStatusResponse {
            session_id,
            now_unix_ms: now,
            claim_count: claims.len(),
            target_key,
            target_claim,
            target_live,
            claims,
            source_of_truth: source_of_truth(),
        }))
    }
}

impl SynapseService {
    pub(crate) fn lock_target_claims(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, TargetClaimRegistry>, ErrorData> {
        lock_claims(&self.target_claims)
    }

    /// Writes a target-claim intent audit row. The claims mutex must be free
    /// during the WAL-synced write (#207); `audit_claims_lock_probe` observes it.
    fn claims_audit_intent(
        &self,
        input: super::command_audit::CommandAuditInput,
    ) -> Result<(), ErrorData> {
        audit_claims_lock_probe::observe(&self.target_claims);
        self.command_audit_intent(input)?;
        Ok(())
    }

    /// Writes a target-claim final audit row; see [`Self::claims_audit_intent`].
    fn claims_audit_final(
        &self,
        input: super::command_audit::CommandAuditInput,
    ) -> Result<(), ErrorData> {
        audit_claims_lock_probe::observe(&self.target_claims);
        self.command_audit_final(input)?;
        Ok(())
    }

    fn claims_audit_view_now(
        &self,
        session_id: &str,
        target_key: &str,
    ) -> Result<Value, ErrorData> {
        let now = unix_time_ms_now();
        let live_sessions = self.live_target_claim_sessions(now, Some(session_id))?;
        let claims = snapshot_claims(&self.target_claims, now, &live_sessions)?;
        Ok(bounded_claims_view(&claims, target_key))
    }

    pub(crate) fn target_claim_reads_by_owner(
        &self,
    ) -> Result<BTreeMap<String, Vec<TargetClaimRead>>, ErrorData> {
        let now = unix_time_ms_now();
        let live_sessions = self.live_target_claim_sessions(now, None)?;
        let mut guard = self.lock_target_claims()?;
        guard.prune_inactive(now, &live_sessions);
        let mut by_owner: BTreeMap<String, Vec<TargetClaimRead>> = BTreeMap::new();
        for claim in guard.reads(now) {
            by_owner
                .entry(claim.owner_session_id.clone())
                .or_default()
                .push(claim);
        }
        Ok(by_owner)
    }

    pub(crate) fn target_claim_status_snapshot(
        &self,
    ) -> Result<TargetClaimStatusResponse, ErrorData> {
        let now = unix_time_ms_now();
        let live_sessions = self.live_target_claim_sessions(now, None)?;
        let mut guard = self.lock_target_claims()?;
        guard.prune_inactive(now, &live_sessions);
        let claims = guard.reads(now);
        Ok(TargetClaimStatusResponse {
            session_id: "dashboard".to_owned(),
            now_unix_ms: now,
            claim_count: claims.len(),
            target_key: None,
            target_claim: None,
            target_live: None,
            claims,
            source_of_truth: source_of_truth(),
        })
    }

    fn validate_registered_claim_target(&self, target: &SessionTarget) -> Result<(), ErrorData> {
        validate_claim_target(target)?;
        if let SessionTarget::Cdp {
            window_hwnd,
            cdp_target_id,
        } = target
        {
            let owners = self.cdp_target_owners_ref().lock().map_err(|_| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "CDP owner registry lock poisoned",
                )
            })?;
            if !owners.values().any(|owner| {
                owner.cdp_target_id == *cdp_target_id
                    && window_root_hwnd(owner.window_hwnd) == window_root_hwnd(*window_hwnd)
            }) {
                return Err(mcp_error(
                    error_codes::TARGET_WINDOW_NOT_FOUND,
                    "CDP claim target is not registered for this window",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn dashboard_target_claim_prune(
        &self,
    ) -> Result<DashboardTargetClaimPruneResponse, ErrorData> {
        let now = unix_time_ms_now();
        let live_sessions = self.live_target_claim_sessions(now, None)?;
        let before_claims = {
            let guard = self.lock_target_claims()?;
            guard.reads(now)
        };
        let command_payload = json!({
            "live_session_count": live_sessions.len(),
        });
        let command_before = json!({
            "source_of_truth": source_of_truth(),
            "claim_count": before_claims.len(),
        });
        self.claims_audit_intent(
            super::command_audit::CommandAuditInput::mcp(
                "target_claim_status",
                "target_claim_prune",
                None,
                None,
                command_payload.clone(),
                command_before.clone(),
                Value::Null,
                "pending",
            )
            .with_channel("dashboard"),
        )?;
        let after_claims = {
            let mut guard = self.lock_target_claims()?;
            guard.prune_inactive(now, &live_sessions);
            guard.reads(now)
        };
        let after_keys = after_claims
            .iter()
            .map(|claim| claim.target_key.clone())
            .collect::<BTreeSet<_>>();
        let pruned_count = before_claims
            .iter()
            .filter(|claim| !after_keys.contains(&claim.target_key))
            .count();
        let after = TargetClaimStatusResponse {
            session_id: "dashboard".to_owned(),
            now_unix_ms: now,
            claim_count: after_claims.len(),
            target_key: None,
            target_claim: None,
            target_live: None,
            claims: after_claims,
            source_of_truth: source_of_truth(),
        };
        let response = DashboardTargetClaimPruneResponse {
            before_count: before_claims.len(),
            after_count: after.claim_count,
            pruned_count,
            before_claims,
            after,
            source_of_truth: source_of_truth(),
        };
        self.claims_audit_final(
            super::command_audit::CommandAuditInput::mcp(
                "target_claim_status",
                "target_claim_prune",
                None,
                None,
                command_payload,
                command_before,
                json!({
                    "source_of_truth": source_of_truth(),
                    "before_count": response.before_count,
                    "after_count": response.after_count,
                    "pruned_count": response.pruned_count,
                }),
                "ok",
            )
            .with_channel("dashboard"),
        )?;
        Ok(response)
    }

    pub(crate) fn ensure_target_claim_allows_action(
        &self,
        tool: &'static str,
        explicit_target: Option<SessionTarget>,
        request_context: &RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let Some(session_id) =
            super::context::mcp_session_id_from_request_context(request_context)?
        else {
            return Ok(());
        };
        let target = match explicit_target {
            Some(target) => Some(target),
            None => self.session_target(Some(&session_id))?,
        };
        let target = match target {
            Some(target) => target,
            None => current_foreground_session_target()?,
        };
        self.ensure_target_claim_allows_session(tool, &session_id, &target)
    }

    pub(crate) fn ensure_target_claim_allows_session(
        &self,
        tool: &'static str,
        session_id: &str,
        target: &SessionTarget,
    ) -> Result<(), ErrorData> {
        super::agent_scope::ensure_access(session_id, target)?;
        let now = unix_time_ms_now();
        let live_sessions = self.live_target_claim_sessions(now, Some(session_id))?;
        let mut guard = self.lock_target_claims()?;
        guard.prune_inactive(now, &live_sessions);
        if let Some(entry) = guard.conflict(session_id, target) {
            return Err(conflict_error(
                tool,
                session_id,
                &TargetClaimConflict {
                    entry,
                    requester_session_id: session_id.to_owned(),
                    tool,
                },
                "mutating_action_claim_check",
            ));
        }
        Ok(())
    }

    pub(crate) fn target_claim_for_session(
        &self,
        session_id: &str,
        target: &SessionTarget,
    ) -> Result<Option<TargetClaimRead>, ErrorData> {
        let now = unix_time_ms_now();
        let live_sessions = self.live_target_claim_sessions(now, Some(session_id))?;
        let mut guard = self.lock_target_claims()?;
        guard.prune_inactive(now, &live_sessions);
        let claim = guard
            .get(&target_key(&canonical_claim_target(target.clone())))
            .filter(|entry| entry.owner_session_id == session_id)
            .map(|entry| entry.read(now));
        Ok(claim)
    }

    fn resolve_target_claim_target(
        &self,
        session_id: &str,
        target: Option<TargetClaimTargetParam>,
    ) -> Result<SessionTarget, ErrorData> {
        let target = match target {
            Some(target) => target_param_to_session_target(target)?,
            None => self.session_target(Some(session_id))?.ok_or_else(|| {
                mcp_error(
                    error_codes::TARGET_NOT_SET,
                    "target_claim requires an explicit target or this session's active target",
                )
            })?,
        };
        Ok(canonical_claim_target(target))
    }

    fn live_target_claim_sessions(
        &self,
        now_unix_ms: u64,
        include_session_id: Option<&str>,
    ) -> Result<BTreeSet<String>, ErrorData> {
        let mut live = BTreeSet::new();
        if let Some(session_id) = include_session_id {
            live.insert(session_id.to_owned());
        }
        let guard = self.session_registry_ref().lock().map_err(|_error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "session registry lock poisoned while reading target claim owner liveness",
            )
        })?;
        for read in guard.reads(now_unix_ms) {
            if read.lifecycle == "live" {
                live.insert(read.session_id);
            }
        }
        Ok(live)
    }

    pub(crate) fn ensure_same_agent_adoption_allowed(
        &self,
        requester_session_id: &str,
        owner_session_id: &str,
        now_unix_ms: u64,
    ) -> Result<
        (
            super::session_registry::SessionRegistryRead,
            super::session_registry::SessionRegistryRead,
            Vec<crate::daemon_lifecycle::InFlightToolCallRead>,
        ),
        ErrorData,
    > {
        let (requester, owner) = {
            let guard = self.session_registry_ref().lock().map_err(|_error| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "session registry lock poisoned while validating target claim adoption",
                )
            })?;
            let reads = guard.reads(now_unix_ms);
            let requester = reads
                .iter()
                .find(|read| read.session_id == requester_session_id)
                .cloned()
                .ok_or_else(|| {
                    adopt_refused_error(
                        requester_session_id,
                        owner_session_id,
                        "requesting session is missing from session registry",
                        None,
                        None,
                    )
                })?;
            let owner = reads
                .iter()
                .find(|read| read.session_id == owner_session_id)
                .cloned()
                .ok_or_else(|| {
                    adopt_refused_error(
                        requester_session_id,
                        owner_session_id,
                        "owner session is missing from session registry",
                        Some(&requester),
                        None,
                    )
                })?;
            (requester, owner)
        };
        if requester.lifecycle != "live" || owner.lifecycle != "live" {
            return Err(adopt_refused_error(
                requester_session_id,
                owner_session_id,
                "target_claim_adopt requires both sessions to be live before explicit takeover",
                Some(&requester),
                Some(&owner),
            ));
        }
        if requester.agent_kind == "unknown" || requester.agent_kind != owner.agent_kind {
            return Err(adopt_refused_error(
                requester_session_id,
                owner_session_id,
                "target_claim_adopt requires matching known agent_kind",
                Some(&requester),
                Some(&owner),
            ));
        }
        if requester.client_name.is_none() || requester.client_name != owner.client_name {
            return Err(adopt_refused_error(
                requester_session_id,
                owner_session_id,
                "target_claim_adopt requires matching client_name",
                Some(&requester),
                Some(&owner),
            ));
        }
        if requester.started_at_unix_ms <= owner.started_at_unix_ms {
            return Err(adopt_refused_error(
                requester_session_id,
                owner_session_id,
                "target_claim_adopt requires the adopting session to be newer than the owner",
                Some(&requester),
                Some(&owner),
            ));
        }
        let owner_in_flight =
            match crate::daemon_lifecycle::in_flight_tool_calls_for_session(owner_session_id) {
                Ok(calls) => calls,
                #[cfg(test)]
                Err(error) if error.to_string().contains("ledger is not configured") => Vec::new(),
                Err(error) => {
                    return Err(mcp_error(
                        error_codes::TOOL_INTERNAL_ERROR,
                        format!("read daemon lifecycle in-flight tool calls: {error:#}"),
                    ));
                }
            };
        if !owner_in_flight.is_empty() {
            return Err(owner_active_error(
                requester_session_id,
                owner_session_id,
                owner_in_flight,
                "owner session has in-flight tool calls",
            ));
        }
        let lease_status = synapse_action::lease::status();
        if lease_status.owner_session_id.as_deref() == Some(owner_session_id) {
            return Err(owner_active_error(
                requester_session_id,
                owner_session_id,
                Vec::new(),
                "owner session holds the foreground input lease",
            ));
        }
        Ok((requester, owner, owner_in_flight))
    }
}

/// Outcome of one claim attempt plus the post-attempt claim snapshot, both
/// owned so the caller can write audit rows with the claims mutex released.
struct ClaimAttempt {
    result: Result<(TargetClaimEntry, &'static str), ClaimRefusal>,
    claims_after: Vec<TargetClaimRead>,
}

/// Why a claim attempt was refused; every refusal is written through the
/// caller's final audit row (#207).
#[derive(Debug)]
enum ClaimRefusal {
    /// A live peer holds an overlapping claim.
    Conflict(TargetClaimConflict),
    /// The session already holds [`MAX_TARGET_CLAIMS_PER_SESSION`] claims.
    LimitReached,
}

impl ClaimRefusal {
    fn error(&self, tool: &'static str, session_id: &str, operation: &'static str) -> ErrorData {
        match self {
            Self::Conflict(conflict) => conflict_error(tool, session_id, conflict, operation),
            Self::LimitReached => mcp_error(
                error_codes::TARGET_CLAIM_LIMIT_REACHED,
                format!(
                    "{tool} refused: session {session_id:?} already holds {MAX_TARGET_CLAIMS_PER_SESSION} target claims; release one first"
                ),
            ),
        }
    }
}

struct ReleaseAttempt {
    result: Result<Option<TargetClaimEntry>, TargetClaimConflict>,
    claims_after: Vec<TargetClaimRead>,
}

fn lock_claims(
    claims: &SharedTargetClaims,
) -> Result<std::sync::MutexGuard<'_, TargetClaimRegistry>, ErrorData> {
    claims.lock().map_err(|_error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            "target claim registry lock poisoned",
        )
    })
}

/// Prune and copy out the claims; the guard is dropped before returning (#207).
fn snapshot_claims(
    claims: &SharedTargetClaims,
    now_unix_ms: u64,
    live_sessions: &BTreeSet<String>,
) -> Result<Vec<TargetClaimRead>, ErrorData> {
    let mut guard = lock_claims(claims)?;
    guard.prune_inactive(now_unix_ms, live_sessions);
    Ok(guard.reads(now_unix_ms))
}

/// Claim under the lock and return owned results; the guard is dropped before
/// returning so audit I/O never runs while other sessions' action checks wait.
fn attempt_claim(
    claims: &SharedTargetClaims,
    session_id: &str,
    target: SessionTarget,
    ttl_ms: u64,
    now_unix_ms: u64,
    live_sessions: &BTreeSet<String>,
) -> Result<ClaimAttempt, ErrorData> {
    let mut guard = lock_claims(claims)?;
    guard.prune_inactive(now_unix_ms, live_sessions);
    // #207: claims are bounded per session, so a client cannot grow the audit
    // trail and every action's conflict scan without limit.
    let result = if guard.has_claim_capacity(session_id, &target) {
        guard
            .claim(session_id, target, ttl_ms, now_unix_ms, live_sessions)
            .map_err(ClaimRefusal::Conflict)
    } else {
        Err(ClaimRefusal::LimitReached)
    };
    let claims_after = guard.reads(now_unix_ms);
    Ok(ClaimAttempt {
        result,
        claims_after,
    })
}

fn attempt_release(
    claims: &SharedTargetClaims,
    session_id: &str,
    target_key: &str,
    now_unix_ms: u64,
    live_sessions: &BTreeSet<String>,
) -> Result<ReleaseAttempt, ErrorData> {
    let mut guard = lock_claims(claims)?;
    guard.prune_inactive(now_unix_ms, live_sessions);
    let result = guard.release(session_id, target_key);
    let claims_after = guard.reads(now_unix_ms);
    Ok(ReleaseAttempt {
        result,
        claims_after,
    })
}

/// Audit-row view of the claim registry: the total count plus the one claim for
/// the target in question. Rows no longer embed every claim, so they do not
/// grow with the number of claims (#207).
fn bounded_claims_view(claims: &[TargetClaimRead], target_key: &str) -> Value {
    json!({
        "claim_count": claims.len(),
        "target_claim": claims.iter().find(|claim| claim.target_key == target_key),
    })
}

pub(crate) fn cleanup_claims_for_session(
    claims: &SharedTargetClaims,
    session_id: &str,
) -> TargetClaimCleanupReport {
    match claims.lock() {
        Ok(mut claims) => {
            let owned_before = claims
                .claims
                .values()
                .filter(|entry| entry.owner_session_id == session_id)
                .count();
            let released = claims.release_owner(session_id);
            TargetClaimCleanupReport {
                owned_before,
                released: released.len(),
                target_keys: released
                    .iter()
                    .map(|entry| entry.target_key.clone())
                    .collect(),
                failed: false,
                error_message: None,
            }
        }
        Err(_error) => TargetClaimCleanupReport {
            failed: true,
            error_message: Some("target claim registry lock poisoned".to_owned()),
            ..TargetClaimCleanupReport::default()
        },
    }
}

/// Owns the target and one requester slot across destructive, awaited cleanup.
struct AdoptionReservation {
    claims: SharedTargetClaims,
    prior: TargetClaimEntry,
    generation: u64,
    committed: bool,
}

impl AdoptionReservation {
    fn reserve(
        service: &SynapseService,
        prior: &TargetClaimEntry,
        session_id: &str,
        target: &SessionTarget,
    ) -> Result<Result<Self, &'static str>, ErrorData> {
        let mut registry = service.lock_target_claims()?;
        if let Err(reason) = registry.adopt_gate(prior, session_id, target) {
            return Ok(Err(reason));
        }
        if registry.adoption_reservations.contains(&prior.target_key) {
            return Ok(Err("target already reserved for adoption"));
        }
        let mut reserved = prior.clone();
        session_id.clone_into(&mut reserved.owner_session_id);
        reserved.generation = registry.allocate_generation();
        registry
            .claims
            .insert(prior.target_key.clone(), reserved.clone());
        registry
            .adoption_reservations
            .insert(prior.target_key.clone());
        Ok(Ok(Self {
            claims: Arc::clone(&service.target_claims),
            prior: prior.clone(),
            generation: reserved.generation,
            committed: false,
        }))
    }
}

impl Drop for AdoptionReservation {
    fn drop(&mut self) {
        let mut registry = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        registry
            .adoption_reservations
            .remove(&self.prior.target_key);
        if !self.committed
            && registry
                .claims
                .get(&self.prior.target_key)
                .is_some_and(|entry| entry.generation == self.generation)
        {
            registry
                .claims
                .insert(self.prior.target_key.clone(), self.prior.clone());
        }
    }
}

/// Checks the binding against the current exact window claim and OS foreground.
pub(crate) fn resolve_stored_input_target(
    claims: &SharedTargetClaims,
    automation_id: &str,
    target: Option<StoredInputTarget>,
    owner: Option<&str>,
) -> Result<InputDeliveryTarget, ActionError> {
    let claimed_foreground = match (target, owner) {
        (Some(target), Some(owner)) => {
            let window = window_session_target(target.root_hwnd);
            let key = target_key(&window);
            let now = unix_time_ms_now();
            let claimed = claims.lock().is_ok_and(|guard| {
                !guard.adoption_reservations.contains(&key)
                    && guard.claims.get(&key).is_some_and(|entry| {
                        entry.owner_session_id == owner && entry.expires_at_unix_ms > now
                    })
            });
            if claimed && super::agent_scope::ensure_access(owner, &window).is_ok() {
                synapse_a11y::foreground_root_identity()
                    .ok()
                    .map(|identity| StoredInputTarget {
                        root_hwnd: identity.root_hwnd,
                        pid: identity.pid,
                    })
            } else {
                None
            }
        }
        _ => None,
    };
    require_stored_input_target(automation_id, target, claimed_foreground)
        .map(InputDeliveryTarget::from_stored)
}

impl TargetClaimRegistry {
    pub(crate) fn claim(
        &mut self,
        session_id: &str,
        target: SessionTarget,
        ttl_ms: u64,
        now_unix_ms: u64,
        live_sessions: &BTreeSet<String>,
    ) -> Result<(TargetClaimEntry, &'static str), TargetClaimConflict> {
        self.claim_with_roots(
            session_id,
            target,
            ttl_ms,
            now_unix_ms,
            live_sessions,
            &window_root_hwnd,
        )
    }

    /// Whether `session_id` may claim `target`: renewing a claim it already
    /// holds always may; a new claim only below [`MAX_TARGET_CLAIMS_PER_SESSION`].
    pub(crate) fn has_claim_capacity(&self, session_id: &str, target: &SessionTarget) -> bool {
        let key = target_key(target);
        if self
            .claims
            .get(&key)
            .is_some_and(|entry| entry.owner_session_id == session_id)
        {
            return true;
        }
        self.claims
            .values()
            .filter(|entry| entry.owner_session_id == session_id)
            .count()
            < MAX_TARGET_CLAIMS_PER_SESSION
    }

    /// [`Self::claim`] with an injectable top-level-root resolver so the
    /// hierarchy rules (#202) are testable without real windows.
    fn claim_with_roots(
        &mut self,
        session_id: &str,
        target: SessionTarget,
        ttl_ms: u64,
        now_unix_ms: u64,
        live_sessions: &BTreeSet<String>,
        root_of: &dyn Fn(i64) -> i64,
    ) -> Result<(TargetClaimEntry, &'static str), TargetClaimConflict> {
        let target_key = target_key(&target);
        if let Some(existing) = self.claims.values().find(|existing| {
            (existing.owner_session_id != session_id
                || (self.adoption_reservations.contains(&existing.target_key)
                    && existing.target_key != target_key))
                && (live_sessions.contains(&existing.owner_session_id)
                    || self.adoption_reservations.contains(&existing.target_key))
                && targets_overlap(&existing.target, &target, root_of)
        }) {
            return Err(TargetClaimConflict {
                entry: existing.clone(),
                requester_session_id: session_id.to_owned(),
                tool: "target_claim",
            });
        }
        let existing = self.claims.get(&target_key).cloned();
        let same_owner = existing
            .as_ref()
            .is_some_and(|entry| entry.owner_session_id == session_id);
        let outcome = if same_owner { "renewed" } else { "claimed" };
        let generation = if same_owner {
            existing.as_ref().map_or(0, |entry| entry.generation)
        } else {
            self.allocate_generation()
        };
        let claimed_at_unix_ms = if same_owner {
            existing
                .as_ref()
                .map_or(now_unix_ms, |entry| entry.claimed_at_unix_ms)
        } else {
            now_unix_ms
        };
        let entry = TargetClaimEntry {
            target_key: target_key.clone(),
            target,
            owner_session_id: session_id.to_owned(),
            claimed_at_unix_ms,
            renewed_at_unix_ms: now_unix_ms,
            ttl_ms,
            expires_at_unix_ms: now_unix_ms.saturating_add(ttl_ms),
            generation,
        };
        self.claims.insert(target_key, entry.clone());
        Ok((entry, outcome))
    }

    pub(crate) fn release(
        &mut self,
        session_id: &str,
        target_key: &str,
    ) -> Result<Option<TargetClaimEntry>, TargetClaimConflict> {
        let Some(existing) = self.claims.get(target_key).cloned() else {
            return Ok(None);
        };
        if existing.owner_session_id != session_id
            || self.adoption_reservations.contains(target_key)
        {
            return Err(TargetClaimConflict {
                entry: existing,
                requester_session_id: session_id.to_owned(),
                tool: "target_release",
            });
        }
        Ok(self.claims.remove(target_key))
    }

    fn release_owner(&mut self, session_id: &str) -> Vec<TargetClaimEntry> {
        let target_keys = self
            .claims
            .iter()
            .filter_map(|(key, entry)| {
                (entry.owner_session_id == session_id && !self.adoption_reservations.contains(key))
                    .then(|| key.clone())
            })
            .collect::<Vec<_>>();
        target_keys
            .into_iter()
            .filter_map(|key| self.claims.remove(&key))
            .collect()
    }

    fn conflict(&self, session_id: &str, target: &SessionTarget) -> Option<TargetClaimEntry> {
        self.conflict_with_roots(session_id, target, &window_root_hwnd)
    }

    /// First claim held by another session whose target overlaps `target` in
    /// the window hierarchy (#202). Exact-key matching alone let a window claim
    /// be bypassed through a child control, or a tab claim through its window.
    fn conflict_with_roots(
        &self,
        session_id: &str,
        target: &SessionTarget,
        root_of: &dyn Fn(i64) -> i64,
    ) -> Option<TargetClaimEntry> {
        self.claims
            .values()
            .find(|entry| {
                entry.owner_session_id != session_id
                    && targets_overlap(&entry.target, target, root_of)
            })
            .cloned()
    }

    /// Checks run under the claims lock right before an adoption tears the
    /// owner down: the claim must not have moved (#201), and the requester must
    /// be able to take it afterwards (#207).
    fn adopt_gate(
        &self,
        prior: &TargetClaimEntry,
        session_id: &str,
        target: &SessionTarget,
    ) -> Result<(), &'static str> {
        if !self.claim_unmoved(prior) {
            return Err(
                "target claim changed after it was read; adoption aborted before owner teardown",
            );
        }
        if !self.has_claim_capacity(session_id, target) {
            return Err(
                "requesting session already holds the maximum number of target claims; adoption aborted before owner teardown",
            );
        }
        Ok(())
    }

    /// Whether the claim for `prior`'s target is still held by the same owner
    /// at the same generation (#201).
    fn claim_unmoved(&self, prior: &TargetClaimEntry) -> bool {
        self.claims.get(&prior.target_key).is_some_and(|current| {
            current.owner_session_id == prior.owner_session_id
                && current.generation == prior.generation
        })
    }

    fn get(&self, target_key: &str) -> Option<TargetClaimEntry> {
        self.claims.get(target_key).cloned()
    }

    pub(crate) fn reads(&self, now_unix_ms: u64) -> Vec<TargetClaimRead> {
        self.claims
            .values()
            .map(|entry| entry.read(now_unix_ms))
            .collect()
    }

    fn prune_inactive(&mut self, now_unix_ms: u64, live_sessions: &BTreeSet<String>) {
        self.claims.retain(|_key, entry| {
            self.adoption_reservations.contains(&entry.target_key)
                || (entry.expires_at_unix_ms > now_unix_ms
                    && live_sessions.contains(&entry.owner_session_id))
        });
    }

    fn allocate_generation(&mut self) -> u64 {
        self.next_generation = self.next_generation.saturating_add(1);
        self.next_generation
    }
}

impl TargetClaimEntry {
    fn read(&self, now_unix_ms: u64) -> TargetClaimRead {
        TargetClaimRead {
            target_key: self.target_key.clone(),
            target: target_wire(&self.target),
            owner_session_id: self.owner_session_id.clone(),
            claimed_at_unix_ms: self.claimed_at_unix_ms,
            renewed_at_unix_ms: self.renewed_at_unix_ms,
            ttl_ms: self.ttl_ms,
            expires_at_unix_ms: self.expires_at_unix_ms,
            expires_in_ms: self.expires_at_unix_ms.saturating_sub(now_unix_ms),
            generation: self.generation,
            source_of_truth: source_of_truth(),
        }
    }
}

/// Top-level (GA_ROOT) window of `hwnd`; a window that is gone keeps its own
/// handle so stale claims stay addressable.
fn window_root_hwnd(hwnd: i64) -> i64 {
    synapse_a11y::top_level_root_hwnd(hwnd).unwrap_or(hwnd)
}

const fn claim_window_hwnd(target: &SessionTarget) -> i64 {
    match target {
        SessionTarget::Window { hwnd }
        | SessionTarget::Cdp {
            window_hwnd: hwnd, ..
        } => *hwnd,
    }
}

/// Claim identity is the top-level root window (and CDP tab id), so a child
/// control HWND and its root claim the same thing (#202).
pub(crate) fn canonical_claim_target(target: SessionTarget) -> SessionTarget {
    canonical_claim_target_with(target, &window_root_hwnd)
}

fn canonical_claim_target_with(
    target: SessionTarget,
    root_of: &dyn Fn(i64) -> i64,
) -> SessionTarget {
    match target {
        SessionTarget::Window { hwnd } => SessionTarget::Window {
            hwnd: root_of(hwnd),
        },
        SessionTarget::Cdp {
            window_hwnd,
            cdp_target_id,
        } => SessionTarget::Cdp {
            window_hwnd: root_of(window_hwnd),
            cdp_target_id,
        },
    }
}

/// Whether mutating `requested` can disturb what `claimed` protects. Targets
/// in different top-level windows never overlap. Inside one window a window
/// target overlaps everything (OS input and window-level verbs reach every
/// tab), while two CDP targets overlap only on the same tab id.
fn targets_overlap(
    claimed: &SessionTarget,
    requested: &SessionTarget,
    root_of: &dyn Fn(i64) -> i64,
) -> bool {
    if root_of(claim_window_hwnd(claimed)) != root_of(claim_window_hwnd(requested)) {
        return false;
    }
    match (claimed, requested) {
        (
            SessionTarget::Cdp {
                cdp_target_id: claimed_id,
                ..
            },
            SessionTarget::Cdp {
                cdp_target_id: requested_id,
                ..
            },
            // CDP TargetIDs are hex; element ids carry them uppercased (#202).
        ) => claimed_id.eq_ignore_ascii_case(requested_id),
        _ => true,
    }
}

pub(crate) fn target_key(target: &SessionTarget) -> String {
    match target {
        SessionTarget::Window { hwnd } => format!("window:0x{hwnd:x}"),
        SessionTarget::Cdp {
            window_hwnd,
            cdp_target_id,
        } => format!("cdp:0x{window_hwnd:x}:{cdp_target_id}"),
    }
}

pub(crate) fn target_wire(target: &SessionTarget) -> TargetWire {
    match target {
        SessionTarget::Window { hwnd } => TargetWire::Window { window_hwnd: *hwnd },
        SessionTarget::Cdp {
            window_hwnd,
            cdp_target_id,
        } => TargetWire::Cdp {
            window_hwnd: *window_hwnd,
            cdp_target_id: cdp_target_id.clone(),
        },
    }
}

pub(crate) fn window_session_target(hwnd: i64) -> SessionTarget {
    SessionTarget::Window { hwnd }
}

pub(super) fn current_foreground_session_target() -> Result<SessionTarget, ErrorData> {
    let foreground = synapse_a11y::current_foreground_context().map_err(|error| {
        mcp_error(
            error_codes::TARGET_WINDOW_NOT_FOUND,
            format!("target claim foreground read failed before mutating action: {error}"),
        )
    })?;
    validate_claim_target(&SessionTarget::Window {
        hwnd: foreground.hwnd,
    })?;
    Ok(SessionTarget::Window {
        hwnd: foreground.hwnd,
    })
}

fn target_param_to_session_target(
    target: TargetClaimTargetParam,
) -> Result<SessionTarget, ErrorData> {
    match target {
        TargetClaimTargetParam::Window { window_hwnd } => {
            Ok(SessionTarget::Window { hwnd: window_hwnd })
        }
        TargetClaimTargetParam::Cdp {
            window_hwnd,
            cdp_target_id,
        } => {
            validate_cdp_target_id(&cdp_target_id)?;
            Ok(SessionTarget::Cdp {
                window_hwnd,
                cdp_target_id,
            })
        }
    }
}

fn validate_claim_target(target: &SessionTarget) -> Result<(), ErrorData> {
    validate_claim_target_shape(target)?;
    match target {
        SessionTarget::Window { hwnd }
        | SessionTarget::Cdp {
            window_hwnd: hwnd, ..
        } => {
            synapse_a11y::foreground_context(*hwnd).map_err(|error| {
                mcp_error(
                    error_codes::TARGET_WINDOW_NOT_FOUND,
                    format!("target claim window_hwnd 0x{hwnd:x} is not live: {error}"),
                )
            })?;
        }
    }
    Ok(())
}

/// Status-time target check (#206): shape-only validation plus a liveness
/// readback. A closed window is a valid thing to ask about (its claim may still
/// be held), so only a structurally invalid target is an error.
fn status_target_liveness(target: &SessionTarget) -> Result<bool, ErrorData> {
    validate_claim_target_shape(target)?;
    let (SessionTarget::Window { hwnd }
    | SessionTarget::Cdp {
        window_hwnd: hwnd, ..
    }) = target;
    Ok(synapse_a11y::foreground_context(*hwnd).is_ok())
}

fn validate_claim_target_shape(target: &SessionTarget) -> Result<(), ErrorData> {
    match target {
        SessionTarget::Window { hwnd } => {
            if *hwnd == 0 {
                return Err(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    "target claim window_hwnd must be non-zero",
                ));
            }
        }
        SessionTarget::Cdp {
            window_hwnd,
            cdp_target_id,
        } => {
            if *window_hwnd == 0 {
                return Err(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    "target claim window_hwnd must be non-zero",
                ));
            }
            validate_cdp_target_id(cdp_target_id)?;
        }
    }
    Ok(())
}

fn validate_cdp_target_id(cdp_target_id: &str) -> Result<(), ErrorData> {
    if cdp_target_id.trim().is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target claim cdp_target_id must not be empty",
        ));
    }
    if cdp_target_id.chars().count() > 256 {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target claim cdp_target_id must be at most 256 Unicode scalar values",
        ));
    }
    if !cdp_target_id.chars().all(|ch| ('!'..='~').contains(&ch)) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "target claim cdp_target_id must contain only visible ASCII characters",
        ));
    }
    Ok(())
}

fn validate_target_claim_ttl(ttl_ms: u64) -> Result<(), ErrorData> {
    if !(MIN_TARGET_CLAIM_TTL_MS..=MAX_TARGET_CLAIM_TTL_MS).contains(&ttl_ms) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "target_claim ttl_ms must be in {MIN_TARGET_CLAIM_TTL_MS}..={MAX_TARGET_CLAIM_TTL_MS}, got {ttl_ms}"
            ),
        ));
    }
    Ok(())
}

fn conflict_error(
    tool: &'static str,
    requester_session_id: &str,
    conflict: &TargetClaimConflict,
    operation: &'static str,
) -> ErrorData {
    let now = unix_time_ms_now();
    let holder = conflict.entry.owner_session_id.clone();
    let read = conflict.entry.read(now);
    tracing::warn!(
        code = error_codes::TARGET_CO_OWNED,
        tool,
        operation,
        requester_session_id,
        holder_session_id = %holder,
        target_key = %conflict.entry.target_key,
        conflict_tool = conflict.tool,
        conflict_requester_session_id = %conflict.requester_session_id,
        "target claim conflict"
    );
    ErrorData::new(
        ErrorCode(-32099),
        format!(
            "{tool} refused target {} because it is claimed by live MCP session {holder}",
            conflict.entry.target_key
        ),
        Some(json!({
            "code": error_codes::TARGET_CO_OWNED,
            "tool": tool,
            "operation": operation,
            "source_of_truth": source_of_truth(),
            "target_key": conflict.entry.target_key,
            "target": read.target,
            "requester_session_id": requester_session_id,
            "holder_session_id": holder,
            "claim": read,
            "read_only_observe_allowed": true,
            "mutation_allowed": false,
            "resolution": "release the claim, wait for claim expiry, use a different target, or call target_claim_adopt only when the owner's agent process is provably gone",
        })),
    )
}

fn target_claim_not_found_error(
    tool: &'static str,
    requester_session_id: &str,
    target: &SessionTarget,
) -> ErrorData {
    let key = target_key(target);
    tracing::warn!(
        code = error_codes::TARGET_CLAIM_NOT_FOUND,
        tool,
        requester_session_id,
        target_key = %key,
        "target claim adoption found no live claim for target"
    );
    ErrorData::new(
        ErrorCode(-32099),
        format!("{tool} found no live claim for target {key}"),
        Some(json!({
            "code": error_codes::TARGET_CLAIM_NOT_FOUND,
            "tool": tool,
            "requester_session_id": requester_session_id,
            "target_key": key,
            "target": target_wire(target),
            "source_of_truth": source_of_truth(),
            "resolution": "read target_claim_status/session_list and call target_claim for unowned or expired targets",
        })),
    )
}

fn adopt_refused_error(
    requester_session_id: &str,
    owner_session_id: &str,
    reason: &str,
    requester: Option<&super::session_registry::SessionRegistryRead>,
    owner: Option<&super::session_registry::SessionRegistryRead>,
) -> ErrorData {
    tracing::warn!(
        code = error_codes::TARGET_CLAIM_ADOPT_REFUSED,
        requester_session_id,
        owner_session_id,
        reason,
        "target claim adoption refused"
    );
    ErrorData::new(
        ErrorCode(-32099),
        format!("target_claim_adopt refused: {reason}"),
        Some(json!({
            "code": error_codes::TARGET_CLAIM_ADOPT_REFUSED,
            "tool": "target_claim_adopt",
            "requester_session_id": requester_session_id,
            "owner_session_id": owner_session_id,
            "reason": reason,
            "requester": requester,
            "owner": owner,
            "source_of_truth": "session registry + daemon target claim registry",
            "mutation_allowed": false,
        })),
    )
}

fn owner_active_error(
    requester_session_id: &str,
    owner_session_id: &str,
    owner_in_flight: Vec<crate::daemon_lifecycle::InFlightToolCallRead>,
    reason: &str,
) -> ErrorData {
    let lease_status = synapse_action::lease::status();
    tracing::warn!(
        code = error_codes::TARGET_CLAIM_OWNER_ACTIVE,
        requester_session_id,
        owner_session_id,
        reason,
        in_flight_count = owner_in_flight.len(),
        lease_owner_session_id = ?lease_status.owner_session_id,
        "target claim adoption refused because owner is active"
    );
    ErrorData::new(
        ErrorCode(-32099),
        format!("target_claim_adopt refused: {reason}"),
        Some(json!({
            "code": error_codes::TARGET_CLAIM_OWNER_ACTIVE,
            "tool": "target_claim_adopt",
            "requester_session_id": requester_session_id,
            "owner_session_id": owner_session_id,
            "reason": reason,
            "owner_in_flight": owner_in_flight,
            "lease": lease_status,
            "source_of_truth": "daemon lifecycle in-flight ledger + input lease registry",
            "mutation_allowed": false,
        })),
    )
}

fn require_claim_session_id(
    request_context: &RequestContext<RoleServer>,
) -> Result<String, ErrorData> {
    super::context::mcp_session_id_from_request_context(request_context)?.ok_or_else(|| {
        mcp_error(
            error_codes::HTTP_SESSION_INVALID,
            "target claim tools require an MCP session id",
        )
    })
}

const fn default_target_claim_ttl_ms() -> u64 {
    DEFAULT_TARGET_CLAIM_TTL_MS
}

fn source_of_truth() -> String {
    "daemon target claim registry".to_owned()
}

/// Test seam (#207): records, for every target-claim audit write, whether the
/// claims mutex was free at write time. Recording is opt-in per thread
/// (`start`/`take`) and compiles to a no-op outside `cfg(test)`.
pub(crate) mod audit_claims_lock_probe {
    use super::SharedTargetClaims;

    #[cfg(test)]
    thread_local! {
        static OBSERVATIONS: std::cell::RefCell<Option<Vec<bool>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Called right before each audit row write with the claims registry.
    pub(super) fn observe(claims: &SharedTargetClaims) {
        #[cfg(test)]
        OBSERVATIONS.with(|cell| {
            if let Some(observations) = cell.borrow_mut().as_mut() {
                observations.push(claims.try_lock().is_ok());
            }
        });
        #[cfg(not(test))]
        let _ = claims;
    }

    /// Starts recording on the current thread, clearing earlier observations.
    #[cfg(test)]
    pub(crate) fn start() {
        OBSERVATIONS.with(|cell| *cell.borrow_mut() = Some(Vec::new()));
    }

    /// Stops recording and returns one `true` (mutex free) / `false` (mutex
    /// held) per audit write since `start`.
    #[cfg(test)]
    pub(crate) fn take() -> Vec<bool> {
        OBSERVATIONS.with(|cell| cell.borrow_mut().take().unwrap_or_default())
    }
}

#[cfg(test)]
#[path = "tests/target_claims_status_gone_window_206.rs"]
mod target_claims_status_gone_window_206;

#[cfg(test)]
#[path = "tests/target_claims_hierarchy_202.rs"]
mod target_claims_hierarchy_202;

#[cfg(test)]
#[path = "tests/target_claims_lock_scope_207.rs"]
mod target_claims_lock_scope_207;

#[cfg(test)]
#[path = "tests/target_claim_adopt_201.rs"]
mod target_claim_adopt_201;

#[cfg(test)]
#[path = "tests/adoption_handoff_race_201.rs"]
mod adoption_handoff_race_201;
#[cfg(test)]
#[path = "tests/claim_prune_rows_207.rs"]
mod claim_prune_rows_207;
#[cfg(test)]
#[path = "tests/claim_status_handler_206.rs"]
mod claim_status_handler_206;

#[cfg(all(test, windows))]
#[path = "tests/synthetic_cdp_claim_207.rs"]
mod synthetic_cdp_claim_207;
