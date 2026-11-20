//! #195 through the queue-full diagnostic handler body (not the guard API).
//!
//! NEEDS-SEAM (G5b raid): the part of `action_diagnostic_queue_full_setup`
//! after the lease is acquired must be reachable as
//! `async fn queue_full_setup_with_lease(lease: &mut crate::m2::ForegroundInputLeaseGuard,
//!  handle: &synapse_action::ActionHandle, from: Point, blocker_duration_ms: u32)
//!  -> Result<ActionDiagnosticQueueFullSetupResponse, ErrorData>`
//! (queue the blocker, keep the lease for its TTL, settle, saturate the queue).
//! The real handler reads the real cursor and injects a real stroke, so the
//! test injects the action emitter instead: its backend blocks the blocker
//! stroke until the test opens a gate and never touches the desktop.
//!
//! The lease must outlive the handler while the blocker still runs, and must be
//! released on return when no blocker was queued.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use synapse_action::{ActionBackend, ActionEmitter, ActionError, EmitState};
use tokio_util::sync::CancellationToken;

use super::*;

const SESSION: &str = "issue195-handler-session";
const TOOL: &str = "action_diagnostic_queue_full_setup";
const BLOCKER_MS: u32 = 2_000;
const FROM: Point = Point { x: 10, y: 10 };

/// Backend whose blocker stroke blocks until `open()`; every action is a no-op.
#[derive(Default)]
struct GatedBackend {
    open: Mutex<bool>,
    opened: Condvar,
    stroke_started: AtomicBool,
}

impl GatedBackend {
    fn open(&self) {
        *self.open.lock().expect("gate") = true;
        self.opened.notify_all();
    }
}

impl ActionBackend for GatedBackend {
    fn execute(&self, action: &Action, _state: &mut EmitState) -> Result<(), ActionError> {
        if matches!(action, Action::MouseStroke { .. }) {
            self.stroke_started.store(true, Ordering::SeqCst);
            let mut open = self.open.lock().expect("gate");
            while !*open {
                open = self.opened.wait(open).expect("gate wait");
            }
        }
        Ok(())
    }
}

fn acquire_lease() -> crate::m2::ForegroundInputLeaseGuard {
    let mut lease = crate::m2::acquire_foreground_input_lease_with_ttl(
        TOOL,
        Some(SESSION),
        lease_ttl_for_hold_ms(BLOCKER_MS),
    )
    .expect("acquire lease");
    // Same as the handler: never move the real cursor back.
    lease.disable_context_restore("issue195_test_no_cursor_restore");
    let status = synapse_action::lease::status();
    assert!(status.held, "precondition: lease held: {status:?}");
    assert_eq!(status.owner_session_id.as_deref(), Some(SESSION));
    lease
}

fn wait_for(flag: &AtomicBool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !flag.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::yield_now();
    }
    flag.load(Ordering::SeqCst)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn must_pass_lease_outlives_the_handler_while_the_blocker_runs() {
    let _serial = crate::test_support::lease_serial("queue_full_lease_hold_195");
    let backend = Arc::new(GatedBackend::default());
    let dyn_backend: Arc<dyn ActionBackend> = backend.clone();
    let cancel = CancellationToken::new();
    let (handle, _snapshot, join) = ActionEmitter::spawn_with_backend(cancel.clone(), dyn_backend);
    let mut lease = acquire_lease();

    let response = queue_full_setup_with_lease(&mut lease, &handle, FROM, BLOCKER_MS)
        .await
        .expect("queue saturated behind the blocker");
    drop(lease); // the handler's guard drops when the tool call returns

    assert!(response.blocker_queued);
    assert!(response.queue_full_observed);
    assert!(
        wait_for(&backend.stroke_started),
        "precondition: blocker running"
    );
    let status = synapse_action::lease::status();
    assert!(
        status.held,
        "lease released while the blocker still runs: {status:?}"
    );
    assert_eq!(status.owner_session_id.as_deref(), Some(SESSION));

    backend.open();
    cancel.cancel();
    let _ = join.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn must_fail_unqueued_blocker_releases_the_lease_on_return() {
    let _serial = crate::test_support::lease_serial("queue_full_lease_hold_195");
    let backend: Arc<dyn ActionBackend> = Arc::new(GatedBackend::default());
    let cancel = CancellationToken::new();
    let (handle, _snapshot, join) = ActionEmitter::spawn_with_backend(cancel.clone(), backend);
    cancel.cancel();
    let _ = join.await; // emitter gone: the blocker cannot be queued
    let mut lease = acquire_lease();

    let result = queue_full_setup_with_lease(&mut lease, &handle, FROM, BLOCKER_MS).await;
    drop(lease);

    let Err(error) = result else {
        panic!("a blocker that was never queued must fail the setup");
    };
    let code = error
        .data
        .as_ref()
        .and_then(|d| d.get("code"))
        .and_then(Value::as_str);
    assert_eq!(
        code,
        Some(error_codes::ACTION_BACKEND_UNAVAILABLE),
        "{error:?}"
    );
    let status = synapse_action::lease::status();
    assert!(
        !status.held,
        "no blocker runs, so no lease may stay: {status:?}"
    );
}
