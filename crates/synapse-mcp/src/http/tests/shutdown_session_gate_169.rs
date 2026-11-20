//! #169: the stale-session sweeper must be stopped and joined before the
//! shutdown snapshot, and the snapshot must survive the session-map drain.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use rmcp::transport::streamable_http_server::SessionManager;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tokio_util::sync::CancellationToken;

use super::ShutdownSessionGate;

async fn manager_with_sessions(count: usize) -> (Arc<LocalSessionManager>, Vec<String>) {
    let manager = Arc::new(LocalSessionManager::default());
    let mut ids = Vec::new();
    for _ in 0..count {
        let (id, _transport) = manager.create_session().await.unwrap();
        ids.push(id.as_ref().to_owned());
    }
    ids.sort();
    (manager, ids)
}

#[tokio::test]
async fn must_pass_begin_joins_sweeper_before_returning() {
    let (manager, _ids) = manager_with_sessions(1).await;
    let daemon_cancel = CancellationToken::new();
    let gate = ShutdownSessionGate::new(&daemon_cancel);
    let stop = gate.sweeper_stop_token();
    let finished = Arc::new(AtomicBool::new(false));
    let ticks = Arc::new(AtomicUsize::new(0));
    let (finished_task, ticks_task) = (Arc::clone(&finished), Arc::clone(&ticks));
    gate.register_sweeper(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(5));
        loop {
            tokio::select! {
                () = stop.cancelled() => break,
                _ = interval.tick() => { ticks_task.fetch_add(1, Ordering::SeqCst); }
            }
        }
        // A sweep that is still unwinding when shutdown starts.
        tokio::time::sleep(Duration::from_millis(50)).await;
        finished_task.store(true, Ordering::SeqCst);
    }));
    tokio::time::sleep(Duration::from_millis(20)).await;

    gate.begin(&manager).await;

    assert!(
        finished.load(Ordering::SeqCst),
        "begin must wait for the sweeper task to finish"
    );
    let after_begin = ticks.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(
        after_begin,
        ticks.load(Ordering::SeqCst),
        "sweeper must not tick after begin"
    );
}

#[tokio::test]
async fn must_pass_snapshot_survives_session_map_drain() {
    let (manager, ids) = manager_with_sessions(2).await;
    let gate = ShutdownSessionGate::new(&CancellationToken::new());

    let captured = gate.begin(&manager).await;
    assert_eq!(captured.iter().cloned().collect::<Vec<_>>(), ids);

    manager.sessions.write().await.clear();
    let again = gate.begin(&manager).await;
    assert_eq!(again.iter().cloned().collect::<Vec<_>>(), ids);

    let cleanup = gate.sessions_for_cleanup(&manager).await;
    assert_eq!(
        cleanup.iter().cloned().collect::<Vec<_>>(),
        ids,
        "cleanup must still see sessions after the drain emptied the map"
    );
}

#[tokio::test]
async fn must_pass_daemon_cancel_also_stops_sweeper_token() {
    let daemon_cancel = CancellationToken::new();
    let gate = ShutdownSessionGate::new(&daemon_cancel);
    let stop = gate.sweeper_stop_token();
    assert!(!stop.is_cancelled());
    daemon_cancel.cancel();
    assert!(stop.is_cancelled());
}

#[tokio::test]
async fn must_fail_begin_without_registered_sweeper_still_snapshots() {
    let (manager, ids) = manager_with_sessions(1).await;
    let gate = ShutdownSessionGate::new(&CancellationToken::new());
    let captured = gate.begin(&manager).await;
    assert_eq!(captured.iter().cloned().collect::<Vec<_>>(), ids);
    assert!(gate.sweeper_stop_token().is_cancelled());
}
