//! #140 regression: `/health` must stay bounded. It previously refreshed the
//! full process table twice with `ProcessRefreshKind::everything()` and rebuilt
//! the child shell environment (registry + PATH x PATHEXT stats) per request,
//! synchronously on a tokio worker.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::server::SynapseService;

#[test]
fn must_pass_chrome_process_refresh_kind_reads_only_command_lines() {
    let kind = crate::chrome_debugger_bridge::chrome_process_refresh_kind();
    assert!(
        matches!(kind.cmd(), sysinfo::UpdateKind::Always),
        "command lines are required for chrome risk scans"
    );
    assert!(!kind.memory(), "memory must not be refreshed for health");
    assert!(!kind.cpu(), "cpu must not be refreshed for health");
    assert!(
        !kind.disk_usage(),
        "disk usage must not be refreshed for health"
    );
    assert!(
        matches!(kind.environ(), sysinfo::UpdateKind::Never),
        "environment blocks must not be read for health"
    );
    assert!(
        matches!(kind.exe(), sysinfo::UpdateKind::Never),
        "exe paths must not be read for health"
    );
}

#[test]
fn must_pass_shell_search_tool_readback_is_cached_within_ttl() {
    let now = Instant::now();
    let first = crate::m4::shell_search_tool_readback_cached(now);
    let started = Instant::now();
    let second = crate::m4::shell_search_tool_readback_cached(now + Duration::from_secs(1));
    assert_eq!(first, second);
    assert!(
        started.elapsed() < Duration::from_millis(20),
        "cached readback must not re-probe PATH: {:?}",
        started.elapsed()
    );
    assert!(crate::m4::SHELL_SEARCH_TOOL_READBACK_TTL >= Duration::from_secs(10));
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_http_health_payload_runs_off_the_async_worker_and_is_bounded() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let service =
        Arc::new(SynapseService::try_new_isolated_for_tests(dir.path()).expect("isolated service"));
    // Warm caches once, then require the steady-state call to leave the
    // single async worker free. A wall-clock bound tighter than a hang guard is
    // flaky under parallel test load (#135), so the load-immune property is
    // asserted directly: a ticker on the same current_thread runtime must keep
    // advancing while health is computed on the blocking pool.
    let _warm = super::health_payload_off_async_worker(Arc::clone(&service), 1).await;
    let ticks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ticker_ticks = Arc::clone(&ticks);
    let ticker = tokio::spawn(async move {
        loop {
            ticker_ticks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    tokio::task::yield_now().await;
    let before = ticks.load(std::sync::atomic::Ordering::SeqCst);
    let health = tokio::time::timeout(
        Duration::from_mins(1),
        super::health_payload_off_async_worker(service, 1),
    )
    .await
    .expect("steady-state health must not hang");
    let after = ticks.load(std::sync::atomic::Ordering::SeqCst);
    ticker.abort();
    assert!(health.subsystems.contains_key("chrome_bridge"));
    assert!(health.subsystems.contains_key("action"));
    assert!(
        after > before,
        "async worker must stay responsive while health runs: before={before} after={after}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_panicking_health_compute_answers_degraded_instead_of_failing_the_worker() {
    let health = tokio::time::timeout(
        Duration::from_secs(10),
        super::bounded_health_payload(
            || panic!("synthetic health probe failure"),
            Duration::from_secs(5),
            super::degraded_health_payload,
        ),
    )
    .await
    .expect("a failed health probe must answer, not hang");
    assert!(!health.ok, "a failed probe must never report ok");
    let marker = health.subsystems.get("health").expect("health subsystem");
    assert_eq!(marker.status, "degraded");
    assert_eq!(
        marker.detail.as_deref(),
        Some("health blocking task failed")
    );
    let bridge = health
        .subsystems
        .get("chrome_bridge")
        .expect("chrome_bridge must stay present for synapse-setup.ps1");
    assert_eq!(bridge.status, "unknown");
    assert_eq!(bridge.detail.as_deref(), Some("health_unavailable"));
}
