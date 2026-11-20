//! Foreground guard for real-keyboard typing (#188).
//!
//! `act_stroke` already watches the OS foreground while it runs. Foreground
//! typing (`act_type`, the `act_set_field_text` foreground tier, EverQuest
//! literal commands) sends keys with SendInput into whatever window is in
//! front, so a toast or chat window taking focus mid-run would receive the rest
//! of the text (a password, for example).
//!
//! The guard protects the preflight-verified target window, never whatever
//! happens to be in front when typing starts. Before each dispatched chunk it
//! refuses when the live foreground root is not the target root, so no key of
//! that chunk is sent. While the chunk runs it polls the foreground and raises
//! the release interrupt (which the backend honors before its next key) on
//! every departure from the target, not only the first, and reports
//! `ACTION_FOREGROUND_LOST`.
//!
//! Reflex/routine `TypeText` goes through [`RoutineForegroundTypeGuard`]: the
//! same check and monitor, bound to the automation's stored window.

use std::{future::Future, sync::Arc, time::Duration};

use synapse_action::{ActionError, ActionHandle, ActionResult};
use synapse_core::{Action, ReflexId};
use synapse_reflex::{ReflexForegroundTypeGuard, ReflexForegroundTypeGuardHandle};
use tokio::{sync::oneshot, task::JoinHandle};
use tokio_util::sync::CancellationToken;

/// Poll interval; matches the `act_stroke` foreground monitor.
const FOREGROUND_TYPE_MONITOR_INTERVAL_MS: u64 = 10;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ForegroundSnapshot {
    pub(crate) hwnd: i64,
    pub(crate) pid: u32,
    pub(crate) title: String,
}

/// One foreground read, reduced to what the departure decision needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ForegroundObservation {
    Window(i64),
    Unreadable,
}

impl ForegroundObservation {
    fn of(read: &Result<ForegroundSnapshot, String>) -> Self {
        match read {
            Ok(snapshot) => Self::Window(snapshot.hwnd),
            Err(_) => Self::Unreadable,
        }
    }
}

/// A read is a new departure when it is off the target and differs from the
/// previous read: target -> other, other -> another, or other -> unreadable.
/// Staying on the same foreign window is not re-raised; returning to the target
/// and leaving again is.
fn is_new_departure(
    target_hwnd: i64,
    last: ForegroundObservation,
    current: ForegroundObservation,
) -> bool {
    current != ForegroundObservation::Window(target_hwnd) && current != last
}

pub(crate) struct TypeForegroundMonitor {
    cancel: CancellationToken,
    task: JoinHandle<Option<ActionError>>,
}

impl TypeForegroundMonitor {
    /// Watch the foreground against `expected` (the preflight target root).
    pub(crate) fn start_with<R, L>(
        expected: ForegroundSnapshot,
        read: R,
        on_lost: L,
        interval: Duration,
    ) -> Self
    where
        R: Fn() -> Result<ForegroundSnapshot, String> + Send + 'static,
        L: Fn() + Send + 'static,
    {
        let cancel = CancellationToken::new();
        let task = tokio::spawn(watch_foreground(
            expected,
            read,
            on_lost,
            interval,
            cancel.clone(),
        ));
        Self { cancel, task }
    }

    /// Stop watching and return the first foreground-lost error, if any.
    pub(crate) async fn finish(self) -> Option<ActionError> {
        self.cancel.cancel();
        match self.task.await {
            Ok(error) => error,
            Err(error) => Some(ActionError::ForegroundLost {
                detail: format!("foreground typing monitor failed: {error}"),
            }),
        }
    }
}

pub(crate) async fn execute_on_foreground_target(
    handle: &ActionHandle,
    action: Action,
    target_hwnd: i64,
) -> Result<(), ActionError> {
    // Read before the foreground check: a release interrupt raised between the
    // check and the enqueue (which reads its own epoch) is otherwise missed.
    let release_epoch = synapse_action::operator_release_epoch();
    // #294: the keys carry `target_hwnd` as their delivery window, so the
    // backend re-checks it against the live foreground before every batch.
    let delivery_target = handle.delivery_target_for_window(target_hwnd, "foreground_type")?;
    let handle = handle.with_delivery_target(Some(delivery_target));
    run_on_foreground_target(
        preflight_target(target_hwnd),
        live_foreground,
        synapse_action::request_release_interrupt,
        Duration::from_millis(FOREGROUND_TYPE_MONITOR_INTERVAL_MS),
        move || async move {
            ensure_no_release_since(release_epoch, synapse_action::operator_release_epoch())?;
            handle.execute(action).await
        },
    )
    .await
}

/// [`execute_on_foreground_target`] with the foreground reader, interrupt
/// raiser and dispatch injected.
pub(crate) async fn run_on_foreground_target<R, L, D, F>(
    target: ForegroundSnapshot,
    read: R,
    on_lost: L,
    interval: Duration,
    dispatch: D,
) -> Result<(), ActionError>
where
    R: Fn() -> Result<ForegroundSnapshot, String> + Send + 'static,
    L: Fn() + Send + 'static,
    D: FnOnce() -> F,
    F: Future<Output = Result<(), ActionError>>,
{
    ensure_foreground_on_target(&target, read())?;
    let monitor = TypeForegroundMonitor::start_with(target, read, on_lost, interval);
    let result = dispatch().await;
    match monitor.finish().await {
        Some(error) => Err(error),
        None => result,
    }
}

/// The check run before the first key of every chunk: the live foreground root
/// must be the target root. An unreadable foreground fails closed.
pub(crate) fn ensure_foreground_on_target(
    target: &ForegroundSnapshot,
    current: Result<ForegroundSnapshot, String>,
) -> Result<(), ActionError> {
    let detail = match current {
        Ok(current) if current.hwnd == target.hwnd => return Ok(()),
        Ok(current) => format!(
            "foreground typing refused before the first key: target hwnd 0x{:x} ({}) is not the foreground; the foreground is hwnd 0x{:x} ({}); no input was sent",
            target.hwnd, target.title, current.hwnd, current.title
        ),
        Err(read_error) => format!(
            "foreground typing refused before the first key: the foreground could not be read (target hwnd 0x{:x} {}): {read_error}; no input was sent",
            target.hwnd, target.title
        ),
    };
    tracing::warn!(
        code = synapse_core::error_codes::ACTION_FOREGROUND_LOST,
        target_hwnd = target.hwnd,
        detail = %detail,
        "foreground typing: target is not the foreground; chunk refused before any key"
    );
    Err(ActionError::ForegroundLost { detail })
}

async fn watch_foreground<R, L>(
    expected: ForegroundSnapshot,
    read: R,
    on_lost: L,
    interval: Duration,
    cancel: CancellationToken,
) -> Option<ActionError>
where
    R: Fn() -> Result<ForegroundSnapshot, String>,
    L: Fn(),
{
    let mut last = ForegroundObservation::Window(expected.hwnd);
    let mut first_lost: Option<ActionError> = None;
    loop {
        tokio::select! {
            () = cancel.cancelled() => return first_lost,
            () = tokio::time::sleep(interval) => {}
        }
        let current = read();
        let observation = ForegroundObservation::of(&current);
        if is_new_departure(expected.hwnd, last, observation) {
            on_lost();
            let error = foreground_departure_error(&expected, &current);
            first_lost.get_or_insert(error);
        }
        last = observation;
    }
}

fn foreground_departure_error(
    expected: &ForegroundSnapshot,
    current: &Result<ForegroundSnapshot, String>,
) -> ActionError {
    let detail = match current {
        Ok(current) => {
            tracing::error!(
                code = synapse_core::error_codes::ACTION_FOREGROUND_LOST,
                expected_hwnd = expected.hwnd,
                actual_hwnd = current.hwnd,
                expected_pid = expected.pid,
                actual_pid = current.pid,
                "foreground typing: foreground left the target mid-run; release interrupt requested"
            );
            format!(
                "foreground typing expected foreground hwnd 0x{:x} ({}) but the foreground is now hwnd 0x{:x} ({}); typing was interrupted and part of the text may already have been delivered",
                expected.hwnd, expected.title, current.hwnd, current.title
            )
        }
        Err(read_error) => {
            tracing::error!(
                code = synapse_core::error_codes::ACTION_FOREGROUND_LOST,
                expected_hwnd = expected.hwnd,
                read_error = %read_error,
                "foreground typing: foreground unreadable mid-run; release interrupt requested"
            );
            format!(
                "foreground typing could not read the foreground mid-run (expected hwnd 0x{:x} {}): {read_error}; typing was interrupted and part of the text may already have been delivered",
                expected.hwnd, expected.title
            )
        }
    };
    ActionError::ForegroundLost { detail }
}

/// The preflight target as a snapshot of its top-level root window. Title and
/// pid are best effort (they only feed diagnostics); the hwnd is what counts.
fn preflight_target(target_hwnd: i64) -> ForegroundSnapshot {
    let hwnd = root_hwnd(target_hwnd);
    match synapse_a11y::foreground_context(hwnd) {
        Ok(context) => ForegroundSnapshot {
            hwnd,
            pid: context.pid,
            title: context.window_title,
        },
        Err(_) => ForegroundSnapshot {
            hwnd,
            pid: 0,
            title: String::new(),
        },
    }
}

/// The live foreground, normalized to its top-level root so a child or owned
/// window hwnd compares equal to the target root.
fn live_foreground() -> Result<ForegroundSnapshot, String> {
    synapse_a11y::current_foreground_context()
        .map(|context| ForegroundSnapshot {
            hwnd: root_hwnd(context.hwnd),
            pid: context.pid,
            title: context.window_title,
        })
        .map_err(|error| error.to_string())
}

fn root_hwnd(hwnd: i64) -> i64 {
    synapse_a11y::top_level_root_hwnd(hwnd).unwrap_or(hwnd)
}

/// The check run right before a guarded typing action is handed to the action
/// queue: no release interrupt may have been raised since `start_epoch`, the
/// release epoch read before the foreground check. The queue reads its own
/// epoch at enqueue, so without this an interrupt raised between the check and
/// the enqueue would never stop the text.
pub(crate) fn ensure_no_release_since(
    start_epoch: u64,
    current_epoch: u64,
) -> Result<(), ActionError> {
    if start_epoch == current_epoch {
        return Ok(());
    }
    Err(ActionError::SafetyOperatorHotkeyFired {
        detail: "foreground typing refused before the first key: a release interrupt was raised after the foreground check; no input was sent".to_owned(),
    })
}

type ForegroundReader = Arc<dyn Fn() -> Result<ForegroundSnapshot, String> + Send + Sync>;

/// The side effects the routine typing guard depends on, injectable so tests
/// need no desktop.
pub(crate) struct RoutineTypeGuardHooks {
    pub(crate) read: ForegroundReader,
    pub(crate) on_lost: Arc<dyn Fn() + Send + Sync>,
    pub(crate) release_epoch: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub(crate) interval: Duration,
}

impl RoutineTypeGuardHooks {
    fn live() -> Self {
        Self {
            read: Arc::new(live_foreground),
            on_lost: Arc::new(synapse_action::request_release_interrupt),
            release_epoch: Arc::new(synapse_action::operator_release_epoch),
            interval: Duration::from_millis(FOREGROUND_TYPE_MONITOR_INTERVAL_MS),
        }
    }
}

/// The daemon's foreground typing guard for reflex/routine `TypeText` (#188).
///
/// The protected window is the stored root bound to the dispatch handle.
/// An unreadable or mismatched foreground refuses the step before any key.
/// The text is enqueued without
/// waiting (the reflex scheduler is synchronous and keeps its action order),
/// and a monitor on the captured daemon runtime raises the release interrupt on
/// every departure from that window until the action finishes.
pub(crate) struct RoutineForegroundTypeGuard {
    runtime: tokio::runtime::Handle,
    hooks: RoutineTypeGuardHooks,
}

impl RoutineForegroundTypeGuard {
    pub(crate) const fn with(
        runtime: tokio::runtime::Handle,
        hooks: RoutineTypeGuardHooks,
    ) -> Self {
        Self { runtime, hooks }
    }

    /// The live guard bound to the calling tokio runtime. Outside a runtime
    /// there is nowhere to run the monitor, so no guard is returned and reflex
    /// `TypeText` stays refused (fail closed).
    pub(crate) fn for_current_runtime() -> Option<ReflexForegroundTypeGuardHandle> {
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => Some(Arc::new(Self::with(runtime, RoutineTypeGuardHooks::live()))),
            Err(error) => {
                tracing::warn!(
                    code = synapse_core::error_codes::ACTION_FOREGROUND_LOST,
                    error = %error,
                    "routine foreground typing guard unavailable outside a tokio runtime; reflex TypeText stays refused"
                );
                None
            }
        }
    }

    fn start_monitor(&self, target: ForegroundSnapshot) -> TypeForegroundMonitor {
        let _runtime = self.runtime.enter();
        let read = Arc::clone(&self.hooks.read);
        let on_lost = Arc::clone(&self.hooks.on_lost);
        TypeForegroundMonitor::start_with(
            target,
            move || read(),
            move || on_lost(),
            self.hooks.interval,
        )
    }
}

impl ReflexForegroundTypeGuard for RoutineForegroundTypeGuard {
    fn dispatch_type_text(
        &self,
        reflex_id: &ReflexId,
        action: Action,
        handle: &ActionHandle,
    ) -> Result<(), ActionError> {
        let release_epoch = (self.hooks.release_epoch)();
        // The dispatcher binds the stored target before calling the guard; an
        // unbound handle gets the same refusal as an unbound automation.
        let bound = match handle.delivery_target()? {
            Some(bound) => bound,
            None => {
                synapse_action::delivery_target::require_stored_input_target(reflex_id, None, None)
                    .map(synapse_action::InputDeliveryTarget::from_stored)?
            }
        };
        let target = ForegroundSnapshot {
            hwnd: bound.root_hwnd,
            pid: bound.pid,
            title: String::new(),
        };
        ensure_foreground_on_target(&target, (self.hooks.read)())?;
        ensure_no_release_since(release_epoch, (self.hooks.release_epoch)())?;
        let completion = handle.try_execute_with_completion(action)?;
        // Started after the enqueue: a departure it flags then raises the
        // interrupt after the enqueue epoch, so the backend stops the text.
        let monitor = self.start_monitor(target);
        self.runtime.spawn(watch_routine_type_text(
            reflex_id.clone(),
            completion,
            monitor,
        ));
        Ok(())
    }
}

/// Keeps the monitor running until the routine's `TypeText` finishes, then
/// reports any departure (the interrupt was already raised when it happened).
async fn watch_routine_type_text(
    reflex_id: ReflexId,
    completion: oneshot::Receiver<ActionResult<()>>,
    monitor: TypeForegroundMonitor,
) {
    let action_result = completion.await;
    if let Some(error) = monitor.finish().await {
        tracing::error!(
            code = synapse_core::error_codes::ACTION_FOREGROUND_LOST,
            reflex_id = %reflex_id,
            detail = %error.detail(),
            action_result = ?action_result,
            "routine TypeText: the foreground left the routine's window mid-step; release interrupt raised"
        );
    }
}

#[cfg(test)]
#[path = "tests/type_monitor_target_188.rs"]
mod type_monitor_target_188_tests;

#[cfg(test)]
#[path = "tests/reflex_type_guard_188.rs"]
mod reflex_type_guard_188_tests;
