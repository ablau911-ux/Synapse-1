//! Acknowledgement plumbing for queued actions (#95).
//!
//! Every queued action carries an [`ActionAck`]. The `completion` slot always
//! receives the final result of the action. A `ReleaseAll` from the K1 hotkey
//! additionally carries a `primary` slot that is filled as soon as the PRIMARY
//! backend release finished, before the slower `ViGEm`/hardware secondary drains
//! run, so the hotkey can stay inside its latency budget while the combined
//! result (including secondary failures) is still delivered afterwards.

use std::time::Duration;

use tokio::sync::oneshot;

use crate::{ActionResult, error::ActionError, hotkey::operator_release_epoch};

/// Sender half of a queued action's acknowledgement.
///
/// It also carries the operator release epoch observed when the action was
/// handed to the queue (#188). The backend measures interrupts against that
/// epoch, so a release interrupt raised after enqueue but before the backend
/// starts the action still stops it before its first key.
#[derive(Debug)]
pub struct ActionAck {
    completion: oneshot::Sender<ActionResult<()>>,
    primary: Option<oneshot::Sender<ActionResult<()>>>,
    release_epoch: u64,
    delivery_target: Option<crate::InputDeliveryTarget>,
}

impl ActionAck {
    /// Acknowledgement that only carries the final result. The release epoch is
    /// read now, at enqueue time.
    #[must_use]
    pub fn new(completion: oneshot::Sender<ActionResult<()>>) -> Self {
        Self {
            completion,
            primary: None,
            release_epoch: operator_release_epoch(),
            delivery_target: None,
        }
    }

    /// Acknowledgement that also reports the primary-backend release early.
    #[must_use]
    pub fn with_primary(
        completion: oneshot::Sender<ActionResult<()>>,
        primary: oneshot::Sender<ActionResult<()>>,
    ) -> Self {
        Self {
            completion,
            primary: Some(primary),
            release_epoch: operator_release_epoch(),
            delivery_target: None,
        }
    }

    /// Replaces the enqueue epoch with one the caller read earlier, when the
    /// action was handed over before an await (for example a gate wait).
    #[must_use]
    pub(crate) const fn with_release_epoch(mut self, release_epoch: u64) -> Self {
        self.release_epoch = release_epoch;
        self
    }

    /// The operator release epoch observed when the action was enqueued.
    #[must_use]
    pub const fn release_epoch(&self) -> u64 {
        self.release_epoch
    }

    #[must_use]
    pub(crate) fn with_delivery_target(
        mut self,
        target: Option<crate::InputDeliveryTarget>,
    ) -> Self {
        self.delivery_target = target;
        self
    }

    pub(crate) fn take_delivery_target(&mut self) -> Option<crate::InputDeliveryTarget> {
        self.delivery_target.take()
    }

    /// Delivers the final result. A still-unsent primary slot receives the same
    /// result first, so a waiter on it is never left hanging.
    ///
    /// # Errors
    ///
    /// Gives the result back when the completion receiver is gone, mirroring
    /// [`oneshot::Sender::send`].
    pub fn send(mut self, result: ActionResult<()>) -> Result<(), ActionResult<()>> {
        if let Some(primary) = self.primary.take() {
            let _primary_send = primary.send(result.clone());
        }
        self.completion.send(result)
    }

    /// Splits into `(completion, primary)` so the emitter can send the primary
    /// result from inside the release and the final result afterwards.
    pub(crate) fn into_parts(
        self,
    ) -> (
        oneshot::Sender<ActionResult<()>>,
        Option<oneshot::Sender<ActionResult<()>>>,
    ) {
        (self.completion, self.primary)
    }
}

/// State of the combined (primary + secondary drains) `ReleaseAll` result when
/// the K1 caller stopped waiting.
#[derive(Clone, Debug)]
pub enum ReleaseAllCompletion {
    /// Every drain finished; carries the combined result.
    Complete(ActionResult<()>),
    /// Secondary drains were still running when the bounded wait ended.
    Pending,
}

/// What a K1 `ReleaseAll` observed: the early primary result, how long it
/// took, and the combined result when it arrived inside the bounded wait.
#[derive(Clone, Debug)]
pub struct ReleaseAllOutcome {
    pub primary: ActionResult<()>,
    pub primary_ack_after: Duration,
    pub completion: ReleaseAllCompletion,
}

impl ReleaseAllOutcome {
    /// Collapses to a single result: the primary error, else a secondary-drain
    /// error. A still-pending secondary drain counts as success because it has
    /// not failed (yet); inspect [`Self::completion`] to tell the two apart.
    ///
    /// # Errors
    ///
    /// Returns the primary or the combined secondary failure.
    pub fn into_result(self) -> ActionResult<()> {
        self.primary?;
        match self.completion {
            ReleaseAllCompletion::Complete(result) => result,
            ReleaseAllCompletion::Pending => Ok(()),
        }
    }

    /// True only when every drain finished and succeeded.
    #[must_use]
    pub const fn fully_released(&self) -> bool {
        self.primary.is_ok() && matches!(self.completion, ReleaseAllCompletion::Complete(Ok(())))
    }
}

pub(crate) fn closed_before_completion() -> ActionError {
    ActionError::BackendUnavailable {
        detail: "release_all completion channel closed before the combined result".to_owned(),
    }
}
