//! #169 through the REAL transport shutdown step
//! (`transport.rs::close_active_mcp_sessions_for_shutdown`), not the gate alone:
//! the stale-session sweeper must be stopped and joined BEFORE the rmcp session
//! map is drained, and the later input cleanup must still see the drained
//! sessions. Moving or removing the `ShutdownSessionGate::begin` call in the
//! transport fails these tests.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use rmcp::transport::streamable_http_server::SessionManager as _;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tokio_util::sync::CancellationToken;

use super::{ShutdownSessionGate, close_active_mcp_sessions_for_shutdown};

/// Stand-in for the stale sweeper: counts every tick that finds a session map
/// emptier than at start (what the real sweeper would tear down as "vanished").
struct FakeSweeper {
    stopped: Arc<AtomicBool>,
    ticks: Arc<AtomicUsize>,
    vanished_ticks: Arc<AtomicUsize>,
}

fn register_fake_sweeper(
    gate: &ShutdownSessionGate,
    manager: &Arc<LocalSessionManager>,
) -> FakeSweeper {
    let stopped = Arc::new(AtomicBool::new(false));
    let vanished_ticks = Arc::new(AtomicUsize::new(0));
    let ticks = Arc::new(AtomicUsize::new(0));
    let ticks_task = Arc::clone(&ticks);
    let (stop, manager) = (gate.sweeper_stop_token(), Arc::clone(manager));
    let (stopped_task, vanished_task) = (Arc::clone(&stopped), Arc::clone(&vanished_ticks));
    gate.register_sweeper(tokio::spawn(async move {
        let baseline = manager.sessions.read().await.len();
        let mut interval = tokio::time::interval(Duration::from_millis(1));
        loop {
            tokio::select! {
                () = stop.cancelled() => break,
                _ = interval.tick() => {
                    ticks_task.fetch_add(1, Ordering::SeqCst);
                    if manager.sessions.read().await.len() < baseline {
                        vanished_task.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        }
        stopped_task.store(true, Ordering::SeqCst);
    }));
    FakeSweeper {
        stopped,
        ticks,
        vanished_ticks,
    }
}

#[tokio::test]
async fn must_pass_shutdown_close_stops_sweeper_then_drains_and_keeps_snapshot() {
    let manager = Arc::new(LocalSessionManager::default());
    let mut keep = Vec::new();
    let mut ids = Vec::new();
    for _ in 0..2 {
        let (id, transport) = manager.create_session().await.unwrap();
        ids.push(id.as_ref().to_owned());
        keep.push(transport);
    }
    ids.sort();
    assert_eq!(
        manager.sessions.read().await.len(),
        2,
        "precondition: 2 live sessions"
    );
    let gate = ShutdownSessionGate::new(&CancellationToken::new());
    let sweeper = register_fake_sweeper(&gate, &manager);

    let report = close_active_mcp_sessions_for_shutdown(&manager, &gate, "test_169").await;

    assert!(
        sweeper.stopped.load(Ordering::SeqCst),
        "the sweeper must be stopped and joined before the shutdown close returns"
    );
    assert_eq!(report.sessions_before, 2);
    assert_eq!(report.close_attempted, 2);
    let mut closed = report.session_ids.clone();
    closed.sort();
    assert_eq!(closed, ids);
    assert!(manager.sessions.read().await.is_empty(), "map drained");
    let cleanup: Vec<String> = gate
        .sessions_for_cleanup(&manager)
        .await
        .into_iter()
        .collect();
    assert_eq!(
        cleanup, ids,
        "input cleanup must still see the drained sessions"
    );
    drop(keep);
}

#[tokio::test]
async fn must_fail_running_sweeper_never_sees_drained_sessions_as_vanished() {
    let manager = Arc::new(LocalSessionManager::default());
    let (_id, transport) = manager.create_session().await.unwrap();
    let gate = ShutdownSessionGate::new(&CancellationToken::new());
    let sweeper = register_fake_sweeper(&gate, &manager);
    // Let the sweeper tick at least once on the full map (obstacle is live).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while sweeper.ticks.load(Ordering::SeqCst) == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "precondition: sweeper never ticked"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    let _report = close_active_mcp_sessions_for_shutdown(&manager, &gate, "test_169").await;
    // Grace window the real daemon waits after the drain (DRAIN_RESPONSE_GRACE).
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        sweeper.vanished_ticks.load(Ordering::SeqCst),
        0,
        "sweeper observed the drained map; it would tear down live sessions"
    );
    drop(transport);
}

#[tokio::test]
async fn must_fail_shutdown_with_no_sessions_still_stops_sweeper_and_cleans_nothing() {
    let manager = Arc::new(LocalSessionManager::default());
    let gate = ShutdownSessionGate::new(&CancellationToken::new());
    let sweeper = register_fake_sweeper(&gate, &manager);

    let report = close_active_mcp_sessions_for_shutdown(&manager, &gate, "test_169").await;

    assert!(sweeper.stopped.load(Ordering::SeqCst));
    assert_eq!(report.sessions_before, 0);
    assert_eq!(report.close_attempted, 0);
    assert!(gate.sessions_for_cleanup(&manager).await.is_empty());
}
