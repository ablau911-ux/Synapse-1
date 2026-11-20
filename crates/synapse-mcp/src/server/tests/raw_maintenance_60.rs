//! NEEDS-SEAM: see criteria_060.md; observer must fire at real job entry.
use super::*;
use crate::server::g7b_support::{code, service};
use std::sync::{Arc, Mutex};
use synapse_storage::cf;

fn gc() -> StorageGcOnceParams {
    serde_json::from_value(serde_json::json!({
        "cf_name": "CF_EVENTS", "soft_cap_rows": 1, "hard_cap_rows": 1
    }))
    .expect("gc params")
}

fn observe(svc: &SynapseService) -> Arc<Mutex<Vec<std::thread::ThreadId>>> {
    let threads = Arc::new(Mutex::new(Vec::new()));
    let seen = threads.clone();
    // Proposed instance/DB-scoped test seam. No global hook or env mutation.
    svc.set_storage_maintenance_observer_for_test(Arc::new(move |_operation| {
        seen.lock()
            .expect("observer")
            .push(std::thread::current().id());
    }));
    threads
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_raw_gc_handler_executes_real_job_off_async_thread_60() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.m3_storage()?;
    let threads = observe(&svc);
    assert_eq!(db.scan_cf(cf::CF_EVENTS)?.len(), 0);
    assert_eq!(threads.lock().expect("before").len(), 0);
    let response = svc.storage_gc_once(Parameters(gc())).await?.0;
    assert_eq!(response.before_rows, 0);
    assert_eq!(response.after_rows, 0);
    let observed = threads.lock().expect("after");
    assert_eq!(observed.len(), 1, "actual GC body must execute");
    assert_ne!(
        observed[0],
        std::thread::current().id(),
        "raw handler blocked async worker"
    );
    assert_eq!(db.scan_cf(cf::CF_EVENTS)?.len(), 0);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_raw_pressure_handler_executes_real_job_off_async_thread_60() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.m3_storage()?;
    let threads = observe(&svc);
    let before = db.scan_cf(cf::CF_KV)?;
    svc.storage_pressure_sample(Parameters(StoragePressureSampleParams {
        free_bytes: 10_000_000_000,
    }))
    .await?;
    let observed = threads.lock().expect("after");
    assert_eq!(observed.len(), 1, "actual pressure body must execute");
    assert_ne!(
        observed[0],
        std::thread::current().id(),
        "raw pressure handler blocked async worker"
    );
    assert_eq!(db.scan_cf(cf::CF_KV)?, before);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_raw_gc_invalid_caps_leave_storage_unchanged_60() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.m3_storage()?;
    let before = db.scan_cf(cf::CF_EVENTS)?;
    let mut params = gc();
    params.soft_cap_rows = 0;
    let error = svc
        .storage_gc_once(Parameters(params))
        .await
        .err()
        .expect("invalid row cap");
    assert_eq!(code(&error), "TOOL_PARAMS_INVALID");
    assert_eq!(db.scan_cf(cf::CF_EVENTS)?, before);
    Ok(())
}
