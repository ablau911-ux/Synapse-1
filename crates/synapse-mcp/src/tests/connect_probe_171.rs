//! Issue #171: the `--mode connect` bridge must never spawn a daemon because of
//! a 401/403 (revoked scoped credential) or a slow `/health`. It spawns only
//! when the port refuses connections and no live daemon holds the
//! single-instance lock.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::probe::{
    AuthRejectedError, DaemonProbe, HEALTH_PROBE_TIMEOUT, ProbeAction, plan_for_probe,
    probe_daemon, probe_daemon_refreshing_with,
};
use super::{ensure_daemon_running, reconnect_daemon_transport_with_retry};
use crate::single_instance::SingleInstanceGuard;

/// Serves one fixed HTTP status on every connection; returns `host:port`.
async fn serve_status(status_line: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let bind = listener.local_addr().expect("addr").to_string();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let reply = format!(
                    "HTTP/1.1 {status_line}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                );
                let _ = stream.write_all(reply.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    bind
}

/// Accepts connections but never answers (a daemon stalled under load).
async fn serve_silent() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let bind = listener.local_addr().expect("addr").to_string();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    bind
}

/// A loopback port nothing listens on (connection refused).
async fn closed_port() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let bind = listener.local_addr().expect("addr").to_string();
    drop(listener);
    bind
}

#[tokio::test]
async fn must_pass_probe_classifies_ok_auth_rejection_and_unhealthy_responses() {
    let ok = serve_status("200 OK").await;
    assert!(matches!(
        probe_daemon(&ok, "t", Duration::from_secs(2)).await,
        DaemonProbe::Healthy
    ));
    for (line, code) in [("401 Unauthorized", 401), ("403 Forbidden", 403)] {
        let bind = serve_status(line).await;
        assert!(
            matches!(probe_daemon(&bind, "t", Duration::from_secs(2)).await, DaemonProbe::AuthRejected(c) if c == code),
            "{line} must be an auth rejection"
        );
    }
    let busy = serve_status("503 Service Unavailable").await;
    assert!(matches!(
        probe_daemon(&busy, "t", Duration::from_secs(2)).await,
        DaemonProbe::Unhealthy(503)
    ));
}

#[tokio::test]
async fn must_fail_probe_distinguishes_timeout_from_connection_refused() {
    let silent = serve_silent().await;
    assert!(matches!(
        probe_daemon(&silent, "t", Duration::from_millis(300)).await,
        DaemonProbe::TimedOut
    ));
    let closed = closed_port().await;
    assert!(matches!(
        probe_daemon(&closed, "t", Duration::from_secs(5)).await,
        DaemonProbe::Unreachable
    ));
}

#[test]
fn must_pass_health_probe_timeout_exceeds_the_daemon_health_bound() {
    // The daemon bounds /health at 3 s (#146 measured 1.8 s under build load).
    assert!(HEALTH_PROBE_TIMEOUT >= Duration::from_secs(4));
}

#[test]
fn must_pass_only_connection_refused_with_no_live_daemon_lock_spawns() {
    let lock_free = || false;
    let lock_held = || true;
    assert_eq!(
        plan_for_probe(&DaemonProbe::Unreachable, lock_free),
        ProbeAction::Spawn
    );
    // A live daemon holds the single-instance lock (starting up, not yet bound).
    assert_eq!(
        plan_for_probe(&DaemonProbe::Unreachable, lock_held),
        ProbeAction::WaitForDaemon
    );
    assert_eq!(
        plan_for_probe(&DaemonProbe::Healthy, lock_free),
        ProbeAction::Proceed
    );
    for probe in [
        DaemonProbe::TimedOut,
        DaemonProbe::Unhealthy(503),
        DaemonProbe::Failed,
    ] {
        assert_eq!(
            plan_for_probe(&probe, lock_free),
            ProbeAction::WaitForDaemon,
            "{probe:?} must never spawn"
        );
    }
    for status in [401, 403] {
        assert_eq!(
            plan_for_probe(&DaemonProbe::AuthRejected(status), lock_free),
            ProbeAction::AuthRejected(status)
        );
    }
}

#[test]
fn must_fail_the_lock_is_only_consulted_when_the_port_refuses() {
    for probe in [
        DaemonProbe::Healthy,
        DaemonProbe::TimedOut,
        DaemonProbe::AuthRejected(401),
        DaemonProbe::Unhealthy(503),
        DaemonProbe::Failed,
    ] {
        let _ = plan_for_probe(&probe, || panic!("lock probed for {probe:?}"));
    }
}

#[test]
fn must_pass_single_instance_probe_reports_a_live_holder_without_touching_the_lock() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(!SingleInstanceGuard::is_held(dir.path()), "no pid file yet");
    assert!(
        !dir.path()
            .join(crate::single_instance::DAEMON_LOCK_FILE)
            .exists(),
        "probe must not create the lock"
    );
    let guard = SingleInstanceGuard::acquire(dir.path())
        .map_err(|e| anyhow::anyhow!("{e}"))
        .expect("acquire");
    assert!(SingleInstanceGuard::is_held(dir.path()));
    drop(guard);
    assert!(
        !SingleInstanceGuard::is_held(dir.path()),
        "released lock is not held"
    );
}

#[test]
fn must_fail_single_instance_probe_never_blocks_an_acquirer() {
    // A daemon starting while the bridge probes must still get the lock, and the
    // probe must work while the lock is held exclusively elsewhere.
    let dir = tempfile::tempdir().expect("tempdir");
    for _ in 0..50 {
        assert!(!SingleInstanceGuard::is_held(dir.path()));
        let guard = SingleInstanceGuard::acquire(dir.path()).map_err(|e| anyhow::anyhow!("{e}"));
        assert!(guard.is_ok(), "probe must leave no lock behind");
        assert!(SingleInstanceGuard::is_held(dir.path()));
    }
}

#[test]
fn must_fail_single_instance_probe_ignores_dead_and_reused_pids() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_file = dir.path().join(crate::single_instance::DAEMON_PID_FILE);
    // Crashed daemon: stale pid file naming a dead process.
    std::fs::write(&pid_file, (u32::MAX - 1).to_string()).expect("write pid");
    assert!(!SingleInstanceGuard::is_held(dir.path()));
    // Reused pid: a live process (this one) that started long after the pid file
    // was written is not the daemon that wrote it.
    std::fs::write(&pid_file, std::process::id().to_string()).expect("write pid");
    let long_ago = std::time::UNIX_EPOCH + Duration::from_secs(86_400);
    std::fs::File::options()
        .write(true)
        .open(&pid_file)
        .expect("open pid file")
        .set_modified(long_ago)
        .expect("set mtime");
    assert!(!SingleInstanceGuard::is_held(dir.path()));
}

/// Answers 200 only to `Bearer <good>`, else 401.
async fn serve_for_token(good: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let bind = listener.local_addr().expect("addr").to_string();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                let status = if request.contains(&format!("bearer {good}")) {
                    "200 OK"
                } else {
                    "401 Unauthorized"
                };
                let reply =
                    format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
                let _ = stream.write_all(reply.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    bind
}

#[tokio::test]
async fn must_pass_rotated_token_file_is_reloaded_once_on_401() {
    let bind = serve_for_token("new-token").await;
    let (probe, used) =
        probe_daemon_refreshing_with(&bind, "old-token", Duration::from_secs(2), || {
            Ok("new-token".to_owned())
        })
        .await;
    assert_eq!(probe, DaemonProbe::Healthy);
    assert_eq!(used, "new-token");
}

#[tokio::test]
async fn must_fail_unchanged_or_unreadable_token_is_a_hard_stop_after_one_reload() {
    let bind = serve_for_token("new-token").await;
    for reload in [
        Ok("old-token".to_owned()),
        Err(anyhow::anyhow!("token.txt gone")),
    ] {
        let (probe, used) =
            probe_daemon_refreshing_with(&bind, "old-token", Duration::from_secs(2), move || {
                reload
            })
            .await;
        assert_eq!(probe, DaemonProbe::AuthRejected(401));
        assert_eq!(used, "old-token");
    }
    // A reloaded token that is still rejected is also terminal (one retry only).
    let (probe, used) =
        probe_daemon_refreshing_with(&bind, "old-token", Duration::from_secs(2), || {
            Ok("still-wrong".to_owned())
        })
        .await;
    assert_eq!(probe, DaemonProbe::AuthRejected(401));
    assert_eq!(used, "still-wrong");
}

#[tokio::test]
async fn must_pass_a_healthy_probe_never_reloads_the_token() {
    let bind = serve_status("200 OK").await;
    let (probe, used) = probe_daemon_refreshing_with(&bind, "tok", Duration::from_secs(2), || {
        panic!("reload on a healthy daemon")
    })
    .await;
    assert_eq!(probe, DaemonProbe::Healthy);
    assert_eq!(used, "tok");
}

#[tokio::test]
async fn must_fail_ensure_daemon_running_reports_auth_rejection_instead_of_spawning() {
    let bind = serve_status("401 Unauthorized").await;
    let dir = tempfile::tempdir().expect("tempdir");
    let error = tokio::time::timeout(
        Duration::from_secs(20),
        ensure_daemon_running(&bind, Some(dir.path()), "synapse-spawn-v1.revoked"),
    )
    .await
    .expect("must not wait/poll for a daemon that answered 401")
    .expect_err("401 is an error");
    let rejected = error
        .downcast_ref::<AuthRejectedError>()
        .expect("typed auth error");
    assert_eq!(rejected.status(), 401);
    assert!(format!("{error:#}").contains("MCP_CONNECT_AUTH_REJECTED"));
    assert!(
        !dir.path()
            .join(crate::single_instance::DAEMON_PID_FILE)
            .exists(),
        "nothing spawned"
    );
}

#[tokio::test]
async fn must_fail_reconnect_stops_at_once_on_auth_rejection() {
    // A scoped credential is memory-only on the daemon, so after a restart it is
    // 401 forever. The bridge used to retry (and spawn) for ~9 minutes.
    let bind = serve_status("401 Unauthorized").await;
    let dir = tempfile::tempdir().expect("tempdir");
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        reconnect_daemon_transport_with_retry(
            &bind,
            &format!("http://{bind}/mcp"),
            Some(dir.path()),
            "synapse-spawn-v1.revoked",
            None,
            None,
        ),
    )
    .await
    .expect("reconnect must stop on 401, not retry through the window");
    let Err(error) = result else {
        panic!("401 is terminal");
    };
    let Some(rejected) = error.downcast_ref::<AuthRejectedError>() else {
        panic!("typed auth error expected: {error:#}");
    };
    assert_eq!(rejected.status(), 401);
    assert!(
        format!("{error:#}").contains("MCP_CONNECT_AUTH_REJECTED"),
        "{error:#}"
    );
    assert!(
        !dir.path()
            .join(crate::single_instance::DAEMON_PID_FILE)
            .exists(),
        "a 401 must not spawn a daemon"
    );
}
