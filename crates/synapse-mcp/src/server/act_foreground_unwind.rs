//! Fail-safe unwind for `act operation=foreground` (#53).
//!
//! Every exit from the foreground escalation — refused escalation, an
//! escalation whose profile row was written before a later failure, a failed
//! action, or a successful one — runs the same unwind: restore the session's
//! prior tool profile (falling back to least-privilege `normal_agent`, through
//! the audited path first and a direct fail-closed row write last) and release
//! a lease this call newly acquired. The readback reports the physical
//! CF_SESSIONS profile and lease owner afterwards.

use rmcp::{RoleServer, model::ErrorCode, service::RequestContext};
use serde_json::json;
use synapse_core::error_codes;

use super::tool_profiles::{ToolProfileKind, ToolProfileSetParams, bounded_profile_reason};
use super::{ErrorData, SynapseService};

/// What an act_foreground call changed and must undo.
pub(crate) struct ForegroundUnwind<'a> {
    pub session_id: &'a str,
    pub prior_profile: ToolProfileKind,
    pub reason: &'a str,
    pub acquired: bool,
}

/// Physical state after the unwind.
pub(crate) struct ForegroundUnwindOutcome {
    pub profile_restored: bool,
    pub restored_profile: String,
    /// True only when this call committed the release of its own lease.
    pub released_lease: bool,
    /// The session left neither an escalated profile nor a lease this call
    /// acquired behind.
    pub complete: bool,
}

impl ForegroundUnwindOutcome {
    /// Error returned instead of the action result when cleanup failed.
    pub(crate) fn incomplete_error(&self, unwind: &ForegroundUnwind<'_>) -> ErrorData {
        ErrorData::new(
            ErrorCode(-32099),
            format!(
                "act operation=foreground could not fully unwind its escalation: session left in profile {} (prior {}), newly acquired lease released={}",
                self.restored_profile,
                unwind.prior_profile.as_str(),
                self.released_lease
            ),
            Some(json!({
            "code": error_codes::TOOL_INTERNAL_ERROR,
            "detail_code": "ACT_FOREGROUND_UNWIND_INCOMPLETE",
            "session_id": unwind.session_id,
            "prior_profile": unwind.prior_profile.as_str(),
            "restored_profile": self.restored_profile,
            "acquired_lease": unwind.acquired,
            "released_lease": self.released_lease,
            "source_of_truth": "CF_SESSIONS tool profile row + synapse_action::lease owner",
            })),
        )
    }
}

impl SynapseService {
    /// Restores the prior profile, then releases a newly acquired lease.
    pub(crate) async fn unwind_act_foreground(
        &self,
        unwind: &ForegroundUnwind<'_>,
        request_context: &RequestContext<RoleServer>,
    ) -> ForegroundUnwindOutcome {
        let mut promotion =
            super::act_foreground_slot::lock_lease_promotion(unwind.session_id).await;
        let promoted = promotion.start_unwind();
        // The profile is restored first: restoring an advanced prior profile
        // needs the lease this call still holds.
        self.restore_foreground_profile(unwind, request_context)
            .await;
        let released_lease = if unwind.acquired && !promoted {
            self.release_foreground_lease(unwind.session_id, request_context)
                .await
        } else {
            false
        };
        let current = self.current_profile(unwind.session_id);
        let lease_left_behind = unwind.acquired && !promoted && lease_owned_by(unwind.session_id);
        let profile_safe =
            current.is_some_and(|p| p == unwind.prior_profile || p == ToolProfileKind::NormalAgent);
        ForegroundUnwindOutcome {
            profile_restored: current == Some(unwind.prior_profile),
            restored_profile: current.map_or_else(
                || "unknown".to_owned(),
                |profile| profile.as_str().to_owned(),
            ),
            released_lease,
            complete: profile_safe && !lease_left_behind,
        }
    }

    async fn restore_foreground_profile(
        &self,
        unwind: &ForegroundUnwind<'_>,
        request_context: &RequestContext<RoleServer>,
    ) {
        self.restore_foreground_profile_with(unwind, |profile, reason| async move {
            self.set_profile_for_unwind(unwind.session_id, profile, &reason, request_context)
                .await
        })
        .await;
    }

    /// [`Self::restore_foreground_profile`] with an injected profile writer
    /// that returns true when the requested profile landed. Test seam: the
    /// restore and least-privilege fallback decisions without a
    /// `RequestContext`.
    pub(crate) async fn restore_foreground_profile_with<F, Fut>(
        &self,
        unwind: &ForegroundUnwind<'_>,
        set_profile: F,
    ) where
        F: Fn(ToolProfileKind, String) -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        if self.current_profile(unwind.session_id) == Some(unwind.prior_profile) {
            return;
        }
        let restore_reason = bounded_profile_reason("restore after act_foreground", unwind.reason);
        if set_profile(unwind.prior_profile, restore_reason).await {
            return;
        }
        // The prior profile could not be restored (e.g. an advanced profile
        // whose lease proof expired). Never leave the session escalated: drop
        // to the least-privileged profile, which needs no lease.
        let fallback_reason = bounded_profile_reason(
            "least-privilege fallback after act_foreground",
            unwind.reason,
        );
        let fell_back = set_profile(ToolProfileKind::NormalAgent, fallback_reason.clone()).await
            || self.force_least_privilege_profile_logged(unwind.session_id, fallback_reason);
        tracing::warn!(
            code = "ACT_FOREGROUND_PROFILE_RESTORE_FAILED",
            session_id = unwind.session_id,
            prior_profile = unwind.prior_profile.as_str(),
            fell_back_to_normal_agent = fell_back,
            "act_foreground could not restore the prior tool profile"
        );
    }

    fn force_least_privilege_profile_logged(&self, session_id: &str, reason: String) -> bool {
        match self.force_least_privilege_profile(session_id, reason) {
            Ok(_row) => self.current_profile(session_id) == Some(ToolProfileKind::NormalAgent),
            Err(error) => {
                tracing::error!(
                    code = "ACT_FOREGROUND_FAIL_CLOSED_PROFILE_WRITE_FAILED",
                    session_id,
                    error = %error.message,
                    "act_foreground could not write the least-privilege profile row"
                );
                false
            }
        }
    }

    async fn set_profile_for_unwind(
        &self,
        session_id: &str,
        profile: ToolProfileKind,
        reason: &str,
        request_context: &RequestContext<RoleServer>,
    ) -> bool {
        let confirm_break_glass = profile.requires_explicit_confirm();
        let result = self
            .tool_profile_set_for_foreground(
                ToolProfileSetParams {
                    profile,
                    reason: Some(reason.to_owned()),
                    confirm_break_glass,
                },
                request_context,
            )
            .await;
        self.unwind_profile_landed(session_id, profile, result.is_ok())
    }

    /// Whether an unwind profile write landed. A write can land before a later
    /// readback/audit step fails, so trust the physical row rather than the
    /// call result. Test seam: `RequestContext`-free half of
    /// `set_profile_for_unwind`, for writers injected into
    /// [`Self::restore_foreground_profile_with`].
    pub(crate) fn unwind_profile_landed(
        &self,
        session_id: &str,
        profile: ToolProfileKind,
        write_ok: bool,
    ) -> bool {
        write_ok || self.current_profile(session_id) == Some(profile)
    }

    /// Releases this call's lease; true only when a release committed here.
    async fn release_foreground_lease(
        &self,
        session_id: &str,
        request_context: &RequestContext<RoleServer>,
    ) -> bool {
        let mut physically_released = false;
        let result = self
            .control_lease_release_inner(request_context.clone(), &mut physically_released)
            .await;
        if result.is_ok() || physically_released {
            return physically_released;
        }
        // Nothing was released here. If the lease is still ours (the audited
        // path failed before releasing), roll it back directly; if it is gone,
        // it expired or was handed off, which is not a release by this call.
        lease_owned_by(session_id) && self.rollback_unacknowledged_lease(session_id)
    }

    fn current_profile(&self, session_id: &str) -> Option<ToolProfileKind> {
        self.tool_profile_snapshot(Some(session_id))
            .ok()
            .map(|snapshot| snapshot.profile)
    }
}

fn lease_owned_by(session_id: &str) -> bool {
    synapse_action::lease::status().owner_session_id.as_deref() == Some(session_id)
}
