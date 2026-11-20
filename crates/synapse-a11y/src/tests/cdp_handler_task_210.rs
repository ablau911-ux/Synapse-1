//! #210: a CDP handler task is aborted when its guard is dropped, so a caller
//! cut off by a timeout (the future is dropped before its explicit `abort`)
//! never leaks the task and its websocket.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use super::CdpHandlerTask;

/// Sets the flag when dropped, i.e. when the task owning it is aborted.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn never_ending_task(stopped: &Arc<AtomicBool>) -> CdpHandlerTask {
    let flag = DropFlag(Arc::clone(stopped));
    CdpHandlerTask::from_task(tokio::spawn(async move {
        let _flag = flag;
        std::future::pending::<()>().await;
    }))
}

#[tokio::test]
async fn must_fail_a_caller_cut_off_by_a_timeout_does_not_leak_its_handler_task() {
    let stopped = Arc::new(AtomicBool::new(false));
    let cut_off = tokio::time::timeout(Duration::from_millis(50), async {
        let _handler = never_ending_task(&stopped);
        std::future::pending::<()>().await;
    })
    .await;
    assert!(cut_off.is_err());
    // Poll with a deadline instead of trusting one fixed sleep.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !stopped.load(Ordering::SeqCst) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        stopped.load(Ordering::SeqCst),
        "the handler task must be aborted when its caller is dropped"
    );
}

#[tokio::test]
async fn must_pass_a_live_guard_keeps_its_handler_running_until_aborted() {
    let stopped = Arc::new(AtomicBool::new(false));
    let handler = never_ending_task(&stopped);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!stopped.load(Ordering::SeqCst));
    assert!(!handler.is_finished());
    handler.abort();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(stopped.load(Ordering::SeqCst));
}
