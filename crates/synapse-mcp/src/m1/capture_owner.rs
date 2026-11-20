//! Session ownership of the daemon's single continuous capture (#112).
//!
//! The capture controller is global, but the session that started a capture
//! owns it: another session may join an equivalent capture and read frames, but
//! may not stop it or retarget it away. Ownership lapses when the owning
//! session has terminated, so a dead session never pins the capture.
//!
//! Liveness comes from a session snapshot taken before the M1 lock, so it can
//! predate a claim. An owner claimed after the snapshot was taken is therefore
//! always treated as live (it registered before it claimed).

use std::time::Instant;

/// The session that started the currently recorded capture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureOwner {
    pub capture_id: String,
    pub session_id: String,
    /// When the claim was recorded; compared with the liveness snapshot age.
    pub claimed_at: Instant,
}

/// A refused cross-session stop or retarget, naming the owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureOwnershipConflict {
    pub capture_id: String,
    pub owner_session_id: String,
}

impl CaptureOwnershipConflict {
    /// Operator-facing message naming the owner session and the way out.
    #[must_use]
    pub fn message(&self, operation: &str) -> String {
        format!(
            "operation={operation} refused: capture {} is owned by session {}; only the owner \
             session may stop or retarget it (read status for the owner, or wait until that \
             session terminates)",
            self.capture_id, self.owner_session_id
        )
    }
}

/// Ownership record for the one active capture.
#[derive(Clone, Debug, Default)]
pub struct CaptureOwnership {
    owner: Option<CaptureOwner>,
}

impl CaptureOwnership {
    /// Records `session_id` as owner of `capture_id`. An unscoped caller
    /// (stdio, no session) leaves the capture unowned.
    pub fn claim(&mut self, capture_id: &str, session_id: Option<&str>) {
        self.owner = session_id.map(|session_id| CaptureOwner {
            capture_id: capture_id.to_owned(),
            session_id: session_id.to_owned(),
            claimed_at: Instant::now(),
        });
    }

    /// The owner of the capture that is active right now. A record for a
    /// capture that has since been replaced or ended is not an owner.
    #[must_use]
    pub fn owner_of(&self, active_capture_id: Option<&str>) -> Option<&CaptureOwner> {
        let active = active_capture_id?;
        self.owner
            .as_ref()
            .filter(|owner| owner.capture_id == active)
    }

    /// Gate for stop / retarget. `Ok` when there is no active capture, no
    /// owner, the caller is the owner, the caller is unscoped, or the owner
    /// session is no longer live. Production uses
    /// [`Self::check_mutation_with_snapshot`]; this wall-clock form serves the
    /// #112 regression tests.
    #[cfg(test)]
    #[allow(
        dead_code,
        reason = "used by tests/livecapture_owner_112.rs, which includes this file by path"
    )]
    pub fn check_mutation(
        &self,
        active_capture_id: Option<&str>,
        caller_session_id: Option<&str>,
        owner_is_live: impl Fn(&str) -> bool,
    ) -> Result<(), CaptureOwnershipConflict> {
        self.check_mutation_with_snapshot(
            active_capture_id,
            caller_session_id,
            Instant::now(),
            owner_is_live,
        )
    }

    /// Like [`Self::check_mutation`] for a liveness view captured at
    /// `snapshot_taken_at`. An owner claimed after that instant is live
    /// regardless of the view, because the view cannot know about it (#112).
    pub fn check_mutation_with_snapshot(
        &self,
        active_capture_id: Option<&str>,
        caller_session_id: Option<&str>,
        snapshot_taken_at: Instant,
        owner_is_live: impl Fn(&str) -> bool,
    ) -> Result<(), CaptureOwnershipConflict> {
        let Some(owner) = self.owner_of(active_capture_id) else {
            return Ok(());
        };
        let Some(caller) = caller_session_id else {
            return Ok(());
        };
        let predates_snapshot = owner.claimed_at <= snapshot_taken_at;
        if owner.session_id == caller || (predates_snapshot && !owner_is_live(&owner.session_id)) {
            return Ok(());
        }
        Err(CaptureOwnershipConflict {
            capture_id: owner.capture_id.clone(),
            owner_session_id: owner.session_id.clone(),
        })
    }

    /// Forgets the owner (capture stopped).
    pub fn clear(&mut self) {
        self.owner = None;
    }
}
