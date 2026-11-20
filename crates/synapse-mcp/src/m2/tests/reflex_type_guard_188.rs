//! #188 follow-up (B): the daemon's reflex/routine foreground typing guard.
//!
//! Since #312 (a0a9d7ad) a routine carries its stored target window, which the
//! dispatcher binds on the action handle, and the guard protects that window.
//! It must refuse before any key when the foreground cannot be read, enqueue
//! the text exactly once (in routine order) when that window is in front, and
//! raise the release interrupt when the foreground leaves that window while
//! the step runs. The release epoch is read before the one pre-key foreground
//! check, so an interrupt raised between the check and the enqueue refuses the
//! step before any key. An unbound handle is refused like an unbound routine.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use synapse_action::{ActionHandle, ActionMessage, InputDeliveryTarget};
use synapse_core::{
    Action, Backend, KeystrokeDynamics, ReflexId, error_codes, types::StoredInputTarget,
};
use synapse_reflex::ReflexForegroundTypeGuard;
use tokio::sync::mpsc;

use super::{
    ForegroundSnapshot, RoutineForegroundTypeGuard, RoutineTypeGuardHooks, ensure_no_release_since,
};

const TARGET: i64 = 0x10;
const POLL: Duration = Duration::from_millis(1);

fn snapshot(hwnd: i64) -> ForegroundSnapshot {
    ForegroundSnapshot {
        hwnd,
        pid: 1,
        title: format!("window {hwnd:#x}"),
    }
}

/// Walks `script` one entry per read, then repeats the last entry. `None`
/// means the foreground is unreadable.
fn scripted_reader(
    script: Vec<Option<i64>>,
) -> Arc<dyn Fn() -> Result<ForegroundSnapshot, String> + Send + Sync> {
    let reads = AtomicUsize::new(0);
    Arc::new(move || {
        let index = reads.fetch_add(1, Ordering::SeqCst);
        script
            .get(index)
            .or_else(|| script.last())
            .copied()
            .flatten()
            .map(snapshot)
            .ok_or_else(|| "GetForegroundWindow returned null".to_owned())
    })
}

fn hooks(
    read: Arc<dyn Fn() -> Result<ForegroundSnapshot, String> + Send + Sync>,
    lost: &Arc<AtomicUsize>,
    epoch: &Arc<AtomicU64>,
) -> RoutineTypeGuardHooks {
    let lost = Arc::clone(lost);
    let epoch = Arc::clone(epoch);
    RoutineTypeGuardHooks {
        read,
        on_lost: Arc::new(move || {
            lost.fetch_add(1, Ordering::SeqCst);
        }),
        release_epoch: Arc::new(move || epoch.load(Ordering::SeqCst)),
        interval: POLL,
    }
}

fn guard(hooks: RoutineTypeGuardHooks) -> RoutineForegroundTypeGuard {
    RoutineForegroundTypeGuard::with(tokio::runtime::Handle::current(), hooks)
}

/// The action channel of a routine whose stored window (`TARGET`, pid 1) the
/// dispatcher bound on the handle (#312).
fn bound_channel() -> (ActionHandle, mpsc::Receiver<ActionMessage>) {
    let (handle, rx) = ActionHandle::channel();
    let stored = StoredInputTarget {
        root_hwnd: TARGET,
        pid: 1,
    };
    (
        handle.with_delivery_target(Some(InputDeliveryTarget::from_stored(stored))),
        rx,
    )
}

fn reflex_id() -> ReflexId {
    ReflexId::from("routine-type-guard-188")
}

fn type_text(text: &str) -> Action {
    Action::TypeText {
        text: text.to_owned(),
        dynamics: KeystrokeDynamics::Burst,
        backend: Backend::Software,
    }
}

fn assert_nothing_enqueued(rx: &mut mpsc::Receiver<ActionMessage>) {
    assert!(
        matches!(rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "no input may be enqueued"
    );
}

async fn wait_until(counter: &AtomicUsize, at_least: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while counter.load(Ordering::SeqCst) < at_least && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

#[tokio::test]
async fn must_pass_routine_text_with_target_in_front_is_enqueued_once() {
    let lost = Arc::new(AtomicUsize::new(0));
    let epoch = Arc::new(AtomicU64::new(7));
    let reads = Arc::new(AtomicUsize::new(0));
    let inner = scripted_reader(vec![Some(TARGET)]);
    let counted: Arc<dyn Fn() -> Result<ForegroundSnapshot, String> + Send + Sync> = {
        let reads = Arc::clone(&reads);
        Arc::new(move || {
            reads.fetch_add(1, Ordering::SeqCst);
            inner()
        })
    };
    let guard = guard(hooks(counted, &lost, &epoch));
    let (handle, mut rx) = bound_channel();
    guard
        .dispatch_type_text(&reflex_id(), type_text("hello routine"), &handle)
        .expect("the routine's window is in front, the text goes through");
    let (action, ack) = rx.try_recv().expect("the TypeText must be enqueued");
    assert_eq!(action, type_text("hello routine"));
    assert_nothing_enqueued(&mut rx);
    // The monitor must really poll while the step runs (preflight read plus at
    // least three monitor reads); staying on target is no departure.
    wait_until(&reads, 4).await;
    assert!(
        reads.load(Ordering::SeqCst) >= 4,
        "the foreground monitor never polled during the step"
    );
    assert_eq!(lost.load(Ordering::SeqCst), 0, "no release interrupt");
    let _sent = ack.send(Ok(()));
    assert_nothing_enqueued(&mut rx);
    assert_eq!(lost.load(Ordering::SeqCst), 0, "no release interrupt");
}

#[tokio::test]
async fn must_fail_routine_text_with_unreadable_foreground_sends_no_key() {
    let lost = Arc::new(AtomicUsize::new(0));
    let epoch = Arc::new(AtomicU64::new(7));
    let guard = guard(hooks(scripted_reader(vec![None]), &lost, &epoch));
    let (handle, mut rx) = bound_channel();
    let error = guard
        .dispatch_type_text(&reflex_id(), type_text("password"), &handle)
        .expect_err("an unreadable foreground must refuse the step");
    assert_eq!(error.code(), error_codes::ACTION_FOREGROUND_LOST);
    assert!(
        error.detail().contains("no input was sent"),
        "detail: {}",
        error.detail()
    );
    assert_nothing_enqueued(&mut rx);
    assert_eq!(lost.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn must_fail_routine_text_departure_mid_step_raises_the_interrupt() {
    let lost = Arc::new(AtomicUsize::new(0));
    let epoch = Arc::new(AtomicU64::new(7));
    // Read 1 is the pre-key check, the monitor then reads the target twice,
    // then a toast takes the foreground while the step is still running.
    let reader = scripted_reader(vec![Some(TARGET), Some(TARGET), Some(TARGET), Some(0x99)]);
    let guard = guard(hooks(reader, &lost, &epoch));
    let (handle, mut rx) = bound_channel();
    guard
        .dispatch_type_text(&reflex_id(), type_text("secret"), &handle)
        .expect("the target is in front when the step starts");
    let (_action, ack) = rx.try_recv().expect("the TypeText must be enqueued");
    // The action has not finished: the departure must be flagged mid-step.
    wait_until(&lost, 1).await;
    assert_eq!(
        lost.load(Ordering::SeqCst),
        1,
        "the departure raises the release interrupt"
    );
    let _sent = ack.send(Ok(()));
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(
        lost.load(Ordering::SeqCst),
        1,
        "one departure, one interrupt"
    );
}

#[tokio::test]
async fn must_fail_release_raised_between_check_and_enqueue_sends_no_key() {
    let lost = Arc::new(AtomicUsize::new(0));
    let epoch = Arc::new(AtomicU64::new(7));
    let reads = Arc::new(AtomicUsize::new(0));
    let bump = Arc::clone(&epoch);
    let counted = Arc::clone(&reads);
    // The operator hotkey fires right after the pre-key foreground check,
    // which is the first read since #312 (the stored window replaced the
    // read that picked the target).
    let reader: Arc<dyn Fn() -> Result<ForegroundSnapshot, String> + Send + Sync> =
        Arc::new(move || {
            if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                bump.fetch_add(1, Ordering::SeqCst);
            }
            Ok(snapshot(TARGET))
        });
    let guard = guard(hooks(reader, &lost, &epoch));
    let (handle, mut rx) = bound_channel();
    let error = guard
        .dispatch_type_text(&reflex_id(), type_text("secret"), &handle)
        .expect_err("a release raised after the check must refuse the step");
    assert_eq!(error.code(), error_codes::SAFETY_OPERATOR_HOTKEY_FIRED);
    assert_nothing_enqueued(&mut rx);

    // The same pure rule guards act_type's dispatch.
    assert!(ensure_no_release_since(3, 3).is_ok());
    assert!(ensure_no_release_since(3, 4).is_err());
}

#[tokio::test]
async fn must_fail_unbound_routine_text_sends_no_key_312() {
    let lost = Arc::new(AtomicUsize::new(0));
    let epoch = Arc::new(AtomicU64::new(7));
    let guard = guard(hooks(scripted_reader(vec![Some(TARGET)]), &lost, &epoch));
    let (handle, mut rx) = ActionHandle::channel();
    let error = guard
        .dispatch_type_text(&reflex_id(), type_text("password"), &handle)
        .expect_err("a routine with no stored window must not type into the foreground");
    assert_eq!(error.code(), error_codes::ACTION_INPUT_TARGET_REQUIRED);
    assert_nothing_enqueued(&mut rx);
}

#[tokio::test]
async fn must_pass_reflex_scope_gate_exposes_the_guard() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let service = crate::server::SynapseService::try_new_isolated_for_tests(dir.path())
        .expect("isolated service");
    let gate = service
        .reflex_action_gate()
        .expect("the daemon builds its reflex action gate");
    assert!(
        gate.foreground_type_guard().is_some(),
        "routine TypeText would be refused for every routine without the daemon guard"
    );
}
