//! #209: `session list` must run its ambient process/window scan off the tokio
//! worker. A slow scan is injected through the REAL shared cache
//! (`AMBIENT_SCAN_CACHE`): another OS thread holds the cache slot inside a
//! scan, so `session_list_impl_blocking` (the dashboard / tool entry point)
//! must wait for it. On a single-threaded runtime the runtime keeps serving
//! other work during that wait only if the list runs on the blocking pool;
//! an inline scan freezes the runtime and these tests fail.

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use tempfile::TempDir;

use super::{AMBIENT_SCAN_CACHE, AmbientScan};
use crate::server::SynapseService;
use crate::server::session_registry::unix_time_ms_now;

const HOLD: Duration = Duration::from_millis(1_500);

/// Holds the scan-cache slot on an OS thread until `release` fires or `HOLD`
/// passes; returns once the slot is held (fixture readback).
fn hold_scan_cache() -> anyhow::Result<(mpsc::Sender<()>, std::thread::JoinHandle<()>)> {
    let (locked_tx, locked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            // Another session-list reader may fill the slot between reset and
            // get. A cache hit is not proof that our slow scan acquired it.
            AMBIENT_SCAN_CACHE.reset();
            let mut scanned = false;
            let _scan = AMBIENT_SCAN_CACHE.get(true, || {
                scanned = true;
                let _ = locked_tx.send(());
                let _ = release_rx.recv_timeout(HOLD);
                AmbientScan {
                    windows_by_pid: std::collections::BTreeMap::new(),
                    window_lookup_error: None,
                    candidates: Vec::new(),
                }
            });
            if scanned || Instant::now() >= deadline {
                break;
            }
            std::thread::yield_now();
        }
    });
    locked_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| anyhow::anyhow!("precondition: slow scan never took the cache slot"))?;
    Ok((release_tx, holder))
}

fn service_with_open_session(temp: &TempDir) -> anyhow::Result<SynapseService> {
    let service = SynapseService::try_new_isolated_for_tests(temp.path())?;
    service
        .session_registry_ref()
        .lock()
        .map_err(|_| anyhow::anyhow!("session registry poisoned"))?
        .record_seen("open-209-offload", None, unix_time_ms_now());
    Ok(service)
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_slow_ambient_scan_cannot_stall_other_runtime_work() -> anyhow::Result<()> {
    // Both tests manipulate one process-global cache. The lease guard also
    // excludes transient lease owners from the session-list read model.
    let _lease = crate::test_support::lease_serial("session_list_offload_209");
    let temp = TempDir::new()?;
    let service = service_with_open_session(&temp)?;
    let (release, holder) = hold_scan_cache()?;

    let list = tokio::spawn(async move { service.session_list_impl_blocking(false).await });
    let started = Instant::now();
    let mut ticks = 0_u32;
    while started.elapsed() < Duration::from_millis(300) {
        tokio::time::sleep(Duration::from_millis(10)).await;
        ticks += 1;
    }
    assert!(
        ticks >= 10,
        "runtime froze during the scan (only {ticks} ticks in 300 ms): scan ran on the async worker"
    );
    assert!(
        !list.is_finished(),
        "precondition: session list must be waiting on the injected slow scan"
    );

    let _ = release.send(());
    let response = list
        .await?
        .map_err(|error| anyhow::anyhow!("session list failed: {error:?}"))?;
    holder
        .join()
        .map_err(|_| anyhow::anyhow!("holder panicked"))?;
    AMBIENT_SCAN_CACHE.reset();
    assert!(
        response
            .sessions
            .iter()
            .any(|summary| summary.registry.session_id == "open-209-offload"),
        "list result after the slow scan must still carry the seeded session"
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_session_list_shares_the_finished_slow_scan() -> anyhow::Result<()> {
    let _lease = crate::test_support::lease_serial("session_list_offload_209");
    let temp = TempDir::new()?;
    let service = service_with_open_session(&temp)?;
    let (release, holder) = hold_scan_cache()?;
    let list = tokio::spawn(async move { service.session_list_impl_blocking(false).await });
    let _ = release.send(());
    holder
        .join()
        .map_err(|_| anyhow::anyhow!("holder panicked"))?;

    let response = list
        .await?
        .map_err(|error| anyhow::anyhow!("session list failed: {error:?}"))?;
    AMBIENT_SCAN_CACHE.reset();
    let ids: Vec<&str> = response
        .sessions
        .iter()
        .map(|summary| summary.registry.session_id.as_str())
        .collect();
    assert_eq!(ids, vec!["open-209-offload"]);
    Ok(())
}
