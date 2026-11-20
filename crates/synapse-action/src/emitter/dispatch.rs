use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::sync::oneshot;

use synapse_core::{Action, ComboInput, error_codes};

use super::routing::{action_consumes_rate_limit, action_kind, resolved_backend_for_action};
use super::{ActionEmitter, EmitState, HeldKeyTimerKey};
use crate::{ActionBackend, ActionError, ActionResult, ResolvedBackend};

impl ActionEmitter {
    #[tracing::instrument(skip_all, fields(action_kind = %action_kind(&action)))]
    pub(super) async fn execute(&mut self, action: Action) -> ActionResult<()> {
        crate::validate_action(&action)?;

        if matches!(action, Action::ReleaseAll) {
            return self.do_release_all("tool_invocation", &mut None).await;
        }

        let resolved = resolved_backend_for_action(&action, self.backend_resolution_policy()?)?;

        if action_consumes_rate_limit(&action) {
            self.consume_rate_limit(resolved)?;
        }

        self.cancel_timers_for_release_actions(&action, resolved);

        let backend = self.backends.pick(resolved);
        let result = self
            .dispatch_via_backend(resolved, backend, action.clone())
            .await;

        if result.is_ok() {
            self.schedule_timers_for_held_keys(&action, resolved);
        }

        result
    }

    async fn dispatch_via_backend(
        &mut self,
        resolved: ResolvedBackend,
        backend: Arc<dyn ActionBackend>,
        action: Action,
    ) -> ActionResult<()> {
        let mut state = std::mem::take(&mut self.state);
        let task = tokio::task::spawn_blocking(move || {
            state.set_active_backend(resolved);
            let result = backend.execute(&action, &mut state);
            state.set_delivery_target(None);
            state.clear_active_backend();
            (result, state)
        });
        match task.await {
            Ok((result, state)) => {
                self.state = state;
                result
            }
            Err(join_error) => {
                // The blocking task panicked or was cancelled; we lost the
                // moved EmitState. Fail-closed: surface the cause and reset
                // held-state to empty so the next action starts clean.
                self.state = EmitState::new();
                Err(ActionError::BackendUnavailable {
                    detail: format!(
                        "code={} backend.execute spawn_blocking join failed: {join_error}",
                        error_codes::ACTION_BACKEND_UNAVAILABLE
                    ),
                })
            }
        }
    }

    /// Cancels in-flight auto-release timers for any key the action is about
    /// to release. Done before backend dispatch so a timer cannot fire mid-call
    /// and enqueue a duplicate `KeyUp` against a key the backend has already
    /// released.
    fn cancel_timers_for_release_actions(&mut self, action: &Action, backend: ResolvedBackend) {
        match action {
            Action::KeyPress { key, .. } | Action::KeyUp { key, .. } => {
                self.cancel_held_key_timer(&HeldKeyTimerKey::new(key.clone(), backend));
            }
            Action::KeyChord { keys, .. } => {
                for key in keys {
                    self.cancel_held_key_timer(&HeldKeyTimerKey::new(key.clone(), backend));
                }
            }
            Action::Combo { steps, .. } => {
                for step in steps {
                    if let ComboInput::KeyUp { key } | ComboInput::KeyPress { key, .. } =
                        &step.input
                    {
                        self.cancel_held_key_timer(&HeldKeyTimerKey::new(key.clone(), backend));
                    }
                }
            }
            _ => {}
        }
    }

    /// Schedules safety auto-release timers for any key the action just put
    /// into a held-down state. Called only when the backend dispatch
    /// succeeded — and only for keys whose post-dispatch state actually
    /// shows them held. A `Combo` like
    /// `[KeyDown(ctrl), KeyUp(ctrl)]` nets to zero held keys, so no
    /// timer is scheduled even though the combo contained a `KeyDown` step.
    fn schedule_timers_for_held_keys(&mut self, action: &Action, backend: ResolvedBackend) {
        match action {
            Action::KeyDown { key, .. } if self.state.is_key_held_for_backend(key, backend) => {
                self.schedule_held_key_auto_release(key.clone(), backend);
            }
            Action::Combo { steps, .. } => {
                for step in steps {
                    if let ComboInput::KeyDown { key } = &step.input
                        && self.state.is_key_held_for_backend(key, backend)
                    {
                        self.schedule_held_key_auto_release(key.clone(), backend);
                    }
                }
            }
            _ => {}
        }
    }

    fn consume_rate_limit(&self, backend: ResolvedBackend) -> ActionResult<()> {
        self.rate_limits.consume(backend)
    }

    #[tracing::instrument(skip_all, fields(action_kind = "release_all"))]
    pub(super) async fn release_all(&mut self, reason: &'static str) {
        let _release_result = self.do_release_all(reason, &mut None).await;
    }

    /// Drives a `ReleaseAll` through the resolved backend (so the production
    /// path actually emits `SendInput` `KeyUp`s for every held key/button),
    /// aborts in-flight auto-release timers, and logs the drained snapshot.
    /// The blocking backend work runs on `spawn_blocking` so the runtime
    /// stays responsive.
    ///
    /// When `early_ack` holds a sender, the PRIMARY (resolved) backend result
    /// is sent on it as soon as that release finishes, before the `ViGEm` and
    /// hardware secondary drains run (#95). That early value is only a latency
    /// signal for the K1 hotkey: the returned result always combines the
    /// primary and every secondary drain, and callers that need correctness
    /// (the `release_all` tool, K1's bounded completion wait) consume it. The
    /// slot is left `Some` if the release failed before the primary phase
    /// started so the caller can still send the failure on it.
    #[allow(clippy::too_many_lines)]
    pub(super) async fn do_release_all(
        &mut self,
        reason: &'static str,
        early_ack: &mut Option<oneshot::Sender<ActionResult<()>>>,
    ) -> ActionResult<()> {
        let started = Instant::now();
        let queue_wait = crate::safety::take_release_all_queue_wait();
        let before = self.snapshot();
        let mut held_pad_ids: Vec<_> = before.pad_state.keys().copied().collect();
        held_pad_ids.sort_unstable();
        let released_keys = before.held_keys.len();
        let released_buttons = before.held_buttons.len();
        let released_pads = before.pad_state.len();

        let cancelled_key_timers = self.abort_all_held_key_timers();

        let resolved =
            resolved_backend_for_action(&Action::ReleaseAll, self.backend_resolution_policy()?)?;
        let primary_backend = self.backends.pick(resolved);
        let mut release_backends = vec![resolved.as_str()];
        let mut hardware_release_ok = None;
        let primary_started = Instant::now();
        let mut result = self
            .dispatch_via_backend(resolved, Arc::clone(&primary_backend), Action::ReleaseAll)
            .await;
        let primary_release = primary_started.elapsed();
        if let Some(ack) = early_ack.take() {
            let _send_result = ack.send(result.clone());
        }
        let acked_after = started.elapsed();

        let mut vigem_drain = None;
        let vigem_has_held_state = snapshot_has_backend_held_state(&before, ResolvedBackend::Vigem);
        if resolved != ResolvedBackend::Vigem && (released_pads != 0 || vigem_has_held_state) {
            release_backends.push("vigem");
            let vigem_backend = self.backends.pick_vigem_for_release();
            let vigem_started = Instant::now();
            let vigem_result = self
                .dispatch_via_backend(ResolvedBackend::Vigem, vigem_backend, Action::ReleaseAll)
                .await;
            vigem_drain = Some(vigem_started.elapsed());
            result = combine_release_results(result, vigem_result);
        }

        // Keys/buttons held through an explicit `backend: software` request
        // while the profile resolves to another backend must be released too;
        // otherwise they stay down at the OS after shutdown.
        let mut software_drain = None;
        if resolved != ResolvedBackend::Software
            && snapshot_has_backend_held_state(&before, ResolvedBackend::Software)
        {
            release_backends.push("software");
            let software_backend = self.backends.pick(ResolvedBackend::Software);
            let software_started = Instant::now();
            let software_result = self
                .dispatch_via_backend(
                    ResolvedBackend::Software,
                    software_backend,
                    Action::ReleaseAll,
                )
                .await;
            software_drain = Some(software_started.elapsed());
            result = combine_release_results(result, software_result);
        }

        let mut hardware_drain = None;
        let hardware_has_held_state =
            snapshot_has_backend_held_state(&before, ResolvedBackend::Hardware);
        if resolved != ResolvedBackend::Hardware
            && (hardware_has_held_state || self.backends.hardware_release_enabled())
        {
            release_backends.push("hardware");
            let hardware_backend = self.backends.pick_hardware_for_release();
            let hardware_started = Instant::now();
            let hardware_result = self
                .dispatch_via_backend(
                    ResolvedBackend::Hardware,
                    hardware_backend,
                    Action::ReleaseAll,
                )
                .await;
            hardware_drain = Some(hardware_started.elapsed());
            hardware_release_ok = Some(hardware_result.is_ok());
            result = combine_release_results(result, hardware_result);
        }
        let release_backend = if hardware_release_ok.is_some() {
            "hardware"
        } else {
            resolved.as_str()
        };

        tracing::warn!(
            code = error_codes::SAFETY_RELEASE_ALL_FIRED,
            reason,
            backend = release_backend,
            primary_backend = resolved.as_str(),
            release_backends = ?release_backends,
            held_keys = ?before.held_keys,
            held_key_bits = ?before.held_key_bits,
            held_key_timer_keys = ?before.held_key_timer_keys,
            held_key_timer_count = before.held_key_timer_count,
            held_buttons = ?before.held_buttons,
            held_button_bits = ?before.held_button_bits,
            held_pad_ids = ?held_pad_ids,
            released_keys,
            released_buttons,
            released_pads,
            cancelled_key_timers,
            hardware_release_ok,
            queue_wait_ms = ?queue_wait.map(millis),
            primary_release_ms = millis(primary_release),
            ack_after_ms = millis(acked_after),
            vigem_drain_ms = ?vigem_drain.map(millis),
            software_drain_ms = ?software_drain.map(millis),
            hardware_drain_ms = ?hardware_drain.map(millis),
            total_ms = millis(started.elapsed()),
            backend_ok = result.is_ok(),
            "release_all drained action emitter held state"
        );

        // If the backend failed mid-release, clear the held bitmaps anyway —
        // the actor's state must reflect what the safety path attempted, so
        // the next ReleaseAll snapshot does not loop on the same keys.
        if result.is_err() {
            self.state.release_all();
        }
        result
    }

    fn backend_resolution_policy(&self) -> ActionResult<crate::BackendResolutionPolicy> {
        self.backend_resolution
            .read()
            .map(|policy| *policy)
            .map_err(|_err| ActionError::BackendUnavailable {
                detail: format!(
                    "code={} backend resolution policy lock poisoned",
                    error_codes::ACTION_BACKEND_UNAVAILABLE
                ),
            })
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn snapshot_has_backend_held_state(
    snapshot: &super::ActionStateSnapshot,
    backend: ResolvedBackend,
) -> bool {
    snapshot
        .held_keys_by_backend
        .get(&backend)
        .is_some_and(|keys| !keys.is_empty())
        || snapshot
            .held_buttons_by_backend
            .get(&backend)
            .is_some_and(|buttons| !buttons.is_empty())
}

fn combine_release_results(current: ActionResult<()>, next: ActionResult<()>) -> ActionResult<()> {
    match current {
        Ok(()) => next,
        error @ Err(_) => error,
    }
}
