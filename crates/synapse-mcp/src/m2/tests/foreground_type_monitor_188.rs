//! #188: foreground typing (act_type, act_set_field_text foreground tier) must
//! stop and report `ACTION_FOREGROUND_LOST` when the OS foreground leaves the
//! window it started in, as `act_stroke` already does.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use synapse_core::error_codes;

use super::foreground_type_monitor::{ForegroundSnapshot, TypeForegroundMonitor};

fn snapshot(hwnd: i64) -> ForegroundSnapshot {
    ForegroundSnapshot {
        hwnd,
        pid: 1,
        title: format!("window {hwnd:#x}"),
    }
}

async fn wait_until(counter: &AtomicUsize, at_least: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while counter.load(Ordering::SeqCst) < at_least && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

#[tokio::test]
async fn must_fail_foreground_leaving_the_target_interrupts_and_reports_lost() {
    let reads = Arc::new(AtomicUsize::new(0));
    let lost = Arc::new(AtomicUsize::new(0));
    let reader_reads = Arc::clone(&reads);
    let lost_calls = Arc::clone(&lost);
    let monitor = TypeForegroundMonitor::start_with(
        snapshot(0x10),
        move || {
            // First two polls still see the target, then a toast takes focus.
            if reader_reads.fetch_add(1, Ordering::SeqCst) < 2 {
                Ok(snapshot(0x10))
            } else {
                Ok(snapshot(0x99))
            }
        },
        move || {
            lost_calls.fetch_add(1, Ordering::SeqCst);
        },
        Duration::from_millis(1),
    );
    wait_until(&lost, 1).await;
    let error = monitor
        .finish()
        .await
        .expect("a foreground change mid-typing must be reported");
    assert_eq!(error.code(), error_codes::ACTION_FOREGROUND_LOST);
    assert!(
        error.detail().contains("0x99"),
        "detail: {}",
        error.detail()
    );
    assert_eq!(
        lost.load(Ordering::SeqCst),
        1,
        "release interrupt requested once"
    );
}

#[tokio::test]
async fn must_pass_stable_foreground_reports_nothing_and_requests_no_interrupt() {
    let lost = Arc::new(AtomicUsize::new(0));
    let lost_calls = Arc::clone(&lost);
    let reads = Arc::new(AtomicUsize::new(0));
    let reader_reads = Arc::clone(&reads);
    let monitor = TypeForegroundMonitor::start_with(
        snapshot(0x10),
        move || {
            reader_reads.fetch_add(1, Ordering::SeqCst);
            Ok(snapshot(0x10))
        },
        move || {
            lost_calls.fetch_add(1, Ordering::SeqCst);
        },
        Duration::from_millis(1),
    );
    wait_until(&reads, 3).await;
    assert!(monitor.finish().await.is_none());
    assert_eq!(lost.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn must_fail_unreadable_foreground_fails_closed() {
    let lost = Arc::new(AtomicUsize::new(0));
    let lost_calls = Arc::clone(&lost);
    let monitor = TypeForegroundMonitor::start_with(
        snapshot(0x10),
        || Err("GetForegroundWindow returned null".to_owned()),
        move || {
            lost_calls.fetch_add(1, Ordering::SeqCst);
        },
        Duration::from_millis(1),
    );
    wait_until(&lost, 1).await;
    let error = monitor
        .finish()
        .await
        .expect("an unreadable foreground must stop typing");
    assert_eq!(error.code(), error_codes::ACTION_FOREGROUND_LOST);
}
