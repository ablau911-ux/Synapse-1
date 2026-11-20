//! #188 second review: the foreground typing guard must protect the
//! preflight-verified target window (not whatever is in front when the monitor
//! starts), refuse before the first key when the target is not the foreground,
//! and detect every foreground departure during a run, not only the first.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use synapse_action::ActionError;
use synapse_core::error_codes;

use super::{
    ForegroundObservation, ForegroundSnapshot, TypeForegroundMonitor, ensure_foreground_on_target,
    is_new_departure, run_on_foreground_target,
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

/// A foreground reader that walks `script` one entry per read and then keeps
/// returning the last entry. `None` means the foreground is unreadable.
fn scripted_reader(
    script: Vec<Option<i64>>,
) -> impl Fn() -> Result<ForegroundSnapshot, String> + Send + 'static {
    let reads = AtomicUsize::new(0);
    move || {
        let index = reads.fetch_add(1, Ordering::SeqCst);
        let entry = script
            .get(index)
            .or_else(|| script.last())
            .copied()
            .flatten();
        entry
            .map(snapshot)
            .ok_or_else(|| "GetForegroundWindow returned null".to_owned())
    }
}

fn counting(counter: &Arc<AtomicUsize>) -> impl Fn() + Send + 'static {
    let counter = Arc::clone(counter);
    move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }
}

async fn wait_until(counter: &AtomicUsize, at_least: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while counter.load(Ordering::SeqCst) < at_least && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

#[tokio::test]
async fn must_pass_foreground_stays_on_target_every_chunk_is_typed_without_interrupt() {
    let lost = Arc::new(AtomicUsize::new(0));
    let dispatched = Arc::new(AtomicUsize::new(0));
    for _chunk in 0..3 {
        let chunk_dispatched = Arc::clone(&dispatched);
        let result = run_on_foreground_target(
            snapshot(TARGET),
            scripted_reader(vec![Some(TARGET)]),
            counting(&lost),
            POLL,
            move || async move {
                // Long enough for the monitor to poll several times.
                tokio::time::sleep(Duration::from_millis(15)).await;
                chunk_dispatched.fetch_add(1, Ordering::SeqCst);
                Ok::<(), ActionError>(())
            },
        )
        .await;
        assert!(result.is_ok(), "on-target chunk must type: {result:?}");
    }
    assert_eq!(dispatched.load(Ordering::SeqCst), 3);
    assert_eq!(lost.load(Ordering::SeqCst), 0, "no release interrupt");
}

#[tokio::test]
async fn must_fail_foreground_off_target_before_first_key_sends_nothing() {
    for script in [vec![Some(0x99)], vec![None]] {
        let lost = Arc::new(AtomicUsize::new(0));
        let dispatched = Arc::new(AtomicUsize::new(0));
        let chunk_dispatched = Arc::clone(&dispatched);
        let error = run_on_foreground_target(
            snapshot(TARGET),
            scripted_reader(script),
            counting(&lost),
            POLL,
            move || async move {
                chunk_dispatched.fetch_add(1, Ordering::SeqCst);
                Ok::<(), ActionError>(())
            },
        )
        .await
        .expect_err("typing must be refused before the first key");
        assert_eq!(error.code(), error_codes::ACTION_FOREGROUND_LOST);
        assert_eq!(
            dispatched.load(Ordering::SeqCst),
            0,
            "no key may be dispatched when the target is not the foreground"
        );
    }
}

#[tokio::test]
async fn must_fail_expected_window_is_the_preflight_target_even_if_foreground_moved_before_start() {
    // Preflight verified 0x10; by the time typing starts a toast (0x99) is in
    // front. The guard must compare against 0x10, not adopt 0x99 as expected.
    let error = run_on_foreground_target(
        snapshot(TARGET),
        scripted_reader(vec![Some(0x99)]),
        || {},
        POLL,
        || async { Ok::<(), ActionError>(()) },
    )
    .await
    .expect_err("the moved foreground must not become the protected window");
    assert_eq!(error.code(), error_codes::ACTION_FOREGROUND_LOST);
    assert!(
        error.detail().contains("0x10"),
        "detail: {}",
        error.detail()
    );
    assert!(
        error.detail().contains("0x99"),
        "detail: {}",
        error.detail()
    );

    // Same rule for the pure check the guard runs before the first key.
    assert!(ensure_foreground_on_target(&snapshot(TARGET), Ok(snapshot(TARGET))).is_ok());
    assert!(ensure_foreground_on_target(&snapshot(TARGET), Ok(snapshot(0x99))).is_err());
}

#[tokio::test]
async fn must_fail_foreground_leaving_mid_run_is_reported_against_the_target() {
    let lost = Arc::new(AtomicUsize::new(0));
    let lost_seen = Arc::clone(&lost);
    // Pre-check sees the target, then the foreground moves mid-run.
    let error = run_on_foreground_target(
        snapshot(TARGET),
        scripted_reader(vec![Some(TARGET), Some(TARGET), Some(0x99)]),
        counting(&lost),
        POLL,
        move || async move {
            wait_until(&lost_seen, 1).await;
            Ok::<(), ActionError>(())
        },
    )
    .await
    .expect_err("a mid-run foreground change wins over the dispatch result");
    assert_eq!(error.code(), error_codes::ACTION_FOREGROUND_LOST);
    assert!(
        error.detail().contains("0x99"),
        "detail: {}",
        error.detail()
    );
    assert_eq!(lost.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn must_fail_a_second_foreground_change_after_the_first_is_still_detected() {
    let lost = Arc::new(AtomicUsize::new(0));
    let monitor = TypeForegroundMonitor::start_with(
        snapshot(TARGET),
        scripted_reader(vec![Some(TARGET), Some(0x99), Some(TARGET), Some(0x77)]),
        counting(&lost),
        POLL,
    );
    wait_until(&lost, 2).await;
    let error = monitor
        .finish()
        .await
        .expect("foreground departures must be reported");
    assert_eq!(
        lost.load(Ordering::SeqCst),
        2,
        "each departure from the target raises the release interrupt"
    );
    assert_eq!(error.code(), error_codes::ACTION_FOREGROUND_LOST);
    assert!(
        error.detail().contains("0x99"),
        "first departure is reported"
    );
}

#[tokio::test]
async fn must_fail_a_move_between_two_foreign_windows_is_detected_again() {
    let lost = Arc::new(AtomicUsize::new(0));
    let monitor = TypeForegroundMonitor::start_with(
        snapshot(TARGET),
        scripted_reader(vec![Some(0x99), Some(0x77), None]),
        counting(&lost),
        POLL,
    );
    wait_until(&lost, 3).await;
    let error = monitor
        .finish()
        .await
        .expect("foreign-to-foreign moves must be reported");
    assert_eq!(lost.load(Ordering::SeqCst), 3);
    assert_eq!(error.code(), error_codes::ACTION_FOREGROUND_LOST);
    // The reported error is the first departure: target -> 0x99.
    let detail = error.detail();
    assert!(detail.contains("expected foreground hwnd 0x10"), "{detail}");
    assert!(detail.contains("foreground is now hwnd 0x99"), "{detail}");
}

#[test]
fn must_fail_every_departure_from_target_is_flagged_not_only_the_first() {
    use ForegroundObservation::{Unreadable, Window};
    let steps = [
        (Window(TARGET), Window(TARGET), false),
        (Window(TARGET), Window(0x99), true),
        (Window(0x99), Window(0x99), false),
        (Window(0x99), Window(TARGET), false),
        (Window(TARGET), Window(0x77), true),
        (Window(0x77), Window(0x99), true),
        (Window(0x99), Unreadable, true),
        (Unreadable, Unreadable, false),
        (Unreadable, Window(TARGET), false),
    ];
    for (last, current, expected) in steps {
        assert_eq!(
            is_new_departure(TARGET, last, current),
            expected,
            "last={last:?} current={current:?}"
        );
    }
}
