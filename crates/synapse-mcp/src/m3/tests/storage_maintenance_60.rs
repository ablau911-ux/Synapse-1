//! Regression tests for #60 item 1: the periodic storage GC and disk-pressure
//! tasks are registered by the daemon, and `storage gc_once` work runs off the
//! async worker.

use std::collections::HashMap;
use std::time::Duration;

use synapse_core::SCHEMA_VERSION;
use synapse_storage::Db;

use super::*;

fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    move |name| map.get(name).cloned()
}

#[test]
fn must_pass_defaults_enable_both_tasks() {
    let config = StorageMaintenanceConfig::from_lookup(lookup(&[]), false).expect("defaults");
    assert_eq!(config.gc_interval, Some(Duration::from_secs(300)));
    assert_eq!(config.pressure_interval, Some(Duration::from_secs(30)));
}

#[test]
fn must_pass_env_overrides_and_zero_disables() {
    let config = StorageMaintenanceConfig::from_lookup(
        lookup(&[(GC_INTERVAL_ENV, " 45 "), (PRESSURE_INTERVAL_ENV, "0")]),
        false,
    )
    .expect("overrides");
    assert_eq!(config.gc_interval, Some(Duration::from_secs(45)));
    assert_eq!(config.pressure_interval, None);
}

#[test]
fn must_fail_invalid_interval_warns_and_keeps_default() {
    let config = StorageMaintenanceConfig::from_lookup(lookup(&[(GC_INTERVAL_ENV, "soon")]), false)
        .expect("garbage interval must not stop the daemon");
    assert_eq!(
        config.gc_interval,
        Some(Duration::from_secs(DEFAULT_GC_INTERVAL_SECS))
    );
}

#[test]
fn must_pass_pinned_pressure_sample_skips_the_real_probe() {
    let config = StorageMaintenanceConfig::from_lookup(lookup(&[]), true).expect("pinned");
    assert_eq!(config.pressure_interval, None);
    assert_eq!(
        config.gc_interval,
        Some(Duration::from_secs(DEFAULT_GC_INTERVAL_SECS))
    );
}

#[tokio::test]
async fn must_pass_startup_registers_both_periodic_tasks() {
    // Drives the daemon entry used by main.rs and http/transport.rs. It reads
    // the real environment; no test in this crate sets these variables.
    assert!(
        std::env::var_os(GC_INTERVAL_ENV).is_none()
            && std::env::var_os(PRESSURE_INTERVAL_ENV).is_none(),
        "unset {GC_INTERVAL_ENV} and {PRESSURE_INTERVAL_ENV} to run this test"
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let state = crate::m3::M3State::from_parts_with_sse_state(
        Some(temp.path().join("db")),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        tokio_util::sync::CancellationToken::new(),
        "shutdown",
        None,
        crate::http::sse::SseState::from_env(),
    )
    .expect("m3 state");
    let shared: crate::m3::SharedM3State = std::sync::Arc::new(std::sync::Mutex::new(state));

    let tasks = spawn_for_daemon(&shared, false).expect("spawn");

    assert!(tasks.gc_running());
    assert!(tasks.pressure_running());
    // The daemon entry opened the shared storage handle it scheduled.
    assert!(
        temp.path().join("db").exists(),
        "spawn_for_daemon must open the configured db"
    );
}

#[tokio::test]
async fn must_fail_disabled_tasks_are_not_registered() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = Db::open(&temp.path().join("db"), SCHEMA_VERSION).expect("open db");
    let config = StorageMaintenanceConfig::from_lookup(
        lookup(&[(GC_INTERVAL_ENV, "0"), (PRESSURE_INTERVAL_ENV, "0")]),
        false,
    )
    .expect("disabled");

    let tasks = spawn_storage_maintenance(&db, &config).expect("spawn");

    assert!(!tasks.gc_running());
    assert!(!tasks.pressure_running());
}

#[test]
fn must_fail_spawning_without_a_runtime_fails_loudly() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = Db::open(&temp.path().join("db"), SCHEMA_VERSION).expect("open db");
    let config = StorageMaintenanceConfig::from_lookup(lookup(&[]), false).expect("defaults");

    let Err(error) = spawn_storage_maintenance(&db, &config) else {
        panic!("spawning periodic tasks without a Tokio runtime must fail");
    };
    let storage_error = error
        .downcast_ref::<synapse_storage::StorageError>()
        .unwrap_or_else(|| panic!("expected a StorageError, got {error:#}"));
    assert!(
        matches!(
            storage_error,
            synapse_storage::StorageError::WriteFailed { .. }
        ),
        "{storage_error:?}"
    );
    let text = format!("{error:#}");
    assert!(
        text.contains("Tokio") && text.contains("runtime"),
        "the error must name the missing Tokio runtime: {text}"
    );
}

type GcRuntime = (
    tempfile::TempDir,
    std::sync::Arc<std::sync::Mutex<synapse_reflex::ReflexRuntime>>,
);

fn gc_runtime() -> GcRuntime {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = std::sync::Arc::new(Db::open(&temp.path().join("db"), SCHEMA_VERSION).expect("db"));
    let (action, _rx) = synapse_action::ActionHandle::channel();
    let runtime =
        synapse_reflex::ReflexRuntime::spawn(db, action, synapse_reflex::EventBus::default())
            .expect("reflex runtime");
    (temp, std::sync::Arc::new(std::sync::Mutex::new(runtime)))
}

fn gc_params() -> StorageGcOnceParams {
    StorageGcOnceParams {
        cf_name: "CF_OCR_CACHE".to_owned(),
        soft_cap_rows: 1,
        hard_cap_rows: 1,
        run_id: None,
        now_ns: None,
        max_age_ns: None,
        dedupe_window_ns: None,
        profile_id: None,
    }
}

/// Holds the reflex-runtime mutex on another OS thread and releases it only
/// once `ready()` holds (or after a 5 s safety deadline). `storage gc_once`
/// needs that mutex: run inline on the single async worker it would block the
/// worker, `ready()` could never become true, and the holder would time out.
/// Returns whether the release was earned (`true`) or timed out (`false`).
fn hold_runtime_until(
    runtime: &std::sync::Arc<std::sync::Mutex<synapse_reflex::ReflexRuntime>>,
    ready: impl Fn() -> bool + Send + 'static,
) -> std::thread::JoinHandle<bool> {
    let runtime = std::sync::Arc::clone(runtime);
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _guard = runtime.lock().expect("runtime lock");
        let _sent = locked_tx.send(());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if ready() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    });
    locked_rx.recv().expect("holder took the lock");
    holder
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_blocking_work_runs_off_the_async_worker() {
    let (_temp, runtime) = gc_runtime();
    let progressed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let holder = {
        let progressed = std::sync::Arc::clone(&progressed);
        hold_runtime_until(&runtime, move || {
            progressed.load(std::sync::atomic::Ordering::SeqCst)
        })
    };
    // Only the async worker can set the flag; it must be free while gc waits.
    let marker = {
        let progressed = std::sync::Arc::clone(&progressed);
        tokio::spawn(async move {
            progressed.store(true, std::sync::atomic::Ordering::SeqCst);
        })
    };

    let response = run_storage_gc_once_blocking(std::sync::Arc::clone(&runtime), gc_params())
        .await
        .expect("gc_once result");
    marker.await.expect("marker task");

    assert!(
        holder.join().expect("holder thread"),
        "gc_once ran on the async worker: the worker stayed blocked until the holder timed out"
    );
    assert_eq!(response.cf_name, "CF_OCR_CACHE");
    assert_eq!(response.before_rows, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_async_worker_keeps_ticking_while_blocking_work_runs() {
    let (_temp, runtime) = gc_runtime();
    let ticks = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let holder = {
        let ticks = std::sync::Arc::clone(&ticks);
        hold_runtime_until(&runtime, move || {
            ticks.load(std::sync::atomic::Ordering::SeqCst) >= 5
        })
    };
    let ticker = {
        let ticks = std::sync::Arc::clone(&ticks);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(1)).await;
                ticks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        })
    };

    let response = run_storage_gc_once_blocking(std::sync::Arc::clone(&runtime), gc_params())
        .await
        .expect("gc_once result");
    ticker.abort();

    assert!(
        holder.join().expect("holder thread"),
        "the single async worker was starved while gc_once waited"
    );
    assert_eq!(response.cf_name, "CF_OCR_CACHE");
}

#[tokio::test]
async fn must_fail_panicking_blocking_job_becomes_a_tool_error() {
    let result: Result<(), _> = run_on_blocking_pool(|| panic!("boom")).await;
    let error = result.expect_err("panic must surface as an error");
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(serde_json::Value::as_str),
        Some(synapse_core::error_codes::TOOL_INTERNAL_ERROR)
    );
    assert!(error.message.contains("did not complete"));
}
