//! #258: the explicit owner override of the user-activity gate.
//!
//! The override is per call: `owner_approved=true` plus a non-empty reason on
//! that one request, audited with the reason. An approved
//! `act operation=foreground` exposes its approval to its own inner focus and
//! SendInput gates through a task-local bound to that call's escalation task,
//! so a concurrent or later call of the same session never inherits it.
//! Shell run/start uses the same task-local, carrying the reason into its audits.

use std::future::Future;

use serde_json::Value;
use synapse_core::error_codes;

use super::{ErrorData, mcp_error};

tokio::task_local! {
    /// The approved call whose task is running (unset everywhere else).
    static APPROVED_CALL: ApprovedCall;
}

/// A validated owner override: `owner_approved=true` with a non-empty reason.
#[derive(Clone, Debug)]
pub(crate) struct OwnerOverride {
    reason: String,
}

impl OwnerOverride {
    /// `None` unless `owner_approved` is true. Approved without a non-empty
    /// reason is `TOOL_PARAMS_INVALID`, so nothing runs.
    pub(crate) fn from_request(
        route: &str,
        owner_approved: Option<bool>,
        reason: Option<&str>,
    ) -> Result<Option<Self>, ErrorData> {
        if owner_approved != Some(true) {
            return Ok(None);
        }
        let Some(reason) = super::tool_profiles::normalize_reason(reason)? else {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "{route} owner_approved=true requires a non-empty reason; the owner override is audited with it"
                ),
            ));
        };
        Ok(Some(Self { reason }))
    }
}

/// Records the per-call override, or its absence, in an audit payload.
pub(crate) fn audit_owner_override(payload: &mut Value, owner_override: Option<&OwnerOverride>) {
    if let Value::Object(map) = payload {
        map.insert(
            "owner_approved".to_owned(),
            Value::Bool(owner_override.is_some()),
        );
        map.insert(
            "owner_approved_reason".to_owned(),
            owner_override.map_or(Value::Null, |approved| {
                Value::String(approved.reason.clone())
            }),
        );
    }
}

/// The one owner-approved call, by session, inside its own task.
#[derive(Clone, Debug)]
pub(crate) struct ApprovedCall {
    session_id: String,
    owner_override: Option<OwnerOverride>,
}

impl ApprovedCall {
    pub(crate) fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_owned(),
            owner_override: None,
        }
    }

    pub(crate) fn with_override(session_id: &str, owner_override: OwnerOverride) -> Self {
        Self {
            session_id: session_id.to_owned(),
            owner_override: Some(owner_override),
        }
    }

    /// Runs `future` (the approved call) with its approval visible to the
    /// gates that call reaches, and to nothing else.
    pub(crate) async fn scope<F: Future>(approval: Option<Self>, future: F) -> F::Output {
        match approval {
            Some(approval) => APPROVED_CALL.scope(approval, future).await,
            None => future.await,
        }
    }
}

/// Adds the shell call's approval to the same persistent audits as its command.
pub(crate) fn audit_call_owner_override(payload: &mut Value, session_id: &str) {
    let owner_override = APPROVED_CALL
        .try_with(|approved| {
            (approved.session_id == session_id)
                .then(|| approved.owner_override.clone())
                .flatten()
        })
        .ok()
        .flatten();
    audit_owner_override(payload, owner_override.as_ref());
}

/// True only inside the approved call's own task, for that call's session.
pub(crate) fn in_approved_call(session_id: Option<&str>) -> bool {
    APPROVED_CALL
        .try_with(|approved| session_id == Some(approved.session_id.as_str()))
        .unwrap_or(false)
}
