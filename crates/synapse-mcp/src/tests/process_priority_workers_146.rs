//! #146: only tokio scheduler workers are raised; the spawn_blocking pool
//! (RocksDB, OCR, readback) stays at normal thread priority.
use std::{sync::Mutex, sync::atomic::Ordering, time::Duration};

use windows::Win32::System::Threading::{
    GetCurrentThread, GetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
};

use super::{WORKER_BOOST, build_runtime};

/// The boost flag is process-global; serialize the tests that flip it.
static SERIAL: Mutex<()> = Mutex::new(());

fn current_thread_priority() -> i32 {
    // SAFETY: pseudo-handle for the calling thread; read-only query.
    unsafe { GetThreadPriority(GetCurrentThread()) }
}

async fn priority_on_a_worker() -> Result<i32, tokio::task::JoinError> {
    // Sleeping parks every worker; the timer wake-up unparks one, which runs
    // the boost hook before the task resumes.
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        current_thread_priority()
    })
    .await
}

#[test]
fn must_pass_workers_are_raised_but_the_blocking_pool_is_not() -> anyhow::Result<()> {
    let _serial = SERIAL
        .lock()
        .map_err(|_| anyhow::anyhow!("serial lock poisoned"))?;
    WORKER_BOOST.store(true, Ordering::Release);
    let runtime = build_runtime()?;
    let worker = runtime.block_on(priority_on_a_worker())?;
    let blocking =
        runtime.block_on(async { tokio::task::spawn_blocking(current_thread_priority).await })?;
    assert_eq!(worker, THREAD_PRIORITY_ABOVE_NORMAL.0);
    assert_eq!(
        blocking, 0,
        "spawn_blocking threads stay THREAD_PRIORITY_NORMAL"
    );
    Ok(())
}

#[test]
fn must_pass_disabled_policy_leaves_workers_normal() -> anyhow::Result<()> {
    let _serial = SERIAL
        .lock()
        .map_err(|_| anyhow::anyhow!("serial lock poisoned"))?;
    WORKER_BOOST.store(false, Ordering::Release);
    let runtime = build_runtime()?;
    assert_eq!(runtime.block_on(priority_on_a_worker())?, 0);
    Ok(())
}
