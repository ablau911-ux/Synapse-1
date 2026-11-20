//! NEEDS-SEAM: AuditProducer::with_writer(capacity:usize,
//! writer:impl Fn(StoredReflexAudit)->StorageResult<()>+Send+'static)->AuditProducer;
//! try_enqueue(&self, StoredReflexAudit)->Result<(),AuditEnqueueError>;
//! stats(&self)->AuditProducerStats { overflow_count:u64, write_error_count:u64 }.
//! AuditEnqueueError::Full. Drop drains/joins. Scheduler must use this producer.
//! Channels prove ordering; no sleeps, disk timing, or process-global hooks.
use super::*;
use std::{sync::mpsc, time::Duration};

fn row(id: &str) -> StoredReflexAudit {
    serde_json::from_value(serde_json::json!({"schema_version":1,"audit_id":id,
        "reflex_id":"reflex-252","ts_ns":252,"status":"active","steps":[],
        "details":{"synthetic":"KEEP_252"},"redacted":false}))
    .expect("audit fixture")
}

#[test]
fn must_pass_async_producer_preserves_exact_audit_content_and_order() {
    let (tx, rx) = mpsc::channel();
    let producer = AuditProducer::with_writer(4, move |audit| {
        tx.send(serde_json::to_value(audit).expect("wire"))
            .expect("receiver");
        Ok(())
    });
    assert_eq!(producer.stats().overflow_count, 0);
    for id in ["a", "b"] {
        let input = row(id);
        let expected = serde_json::to_value(&input).expect("expected wire");
        assert_eq!(producer.try_enqueue(input), Ok(()));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2))
                .expect("writer deadline"),
            expected
        );
    }
    drop(producer);
}

#[test]
fn must_fail_full_queue_is_observable_without_blocking_or_writing_rejected_row() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (written_tx, written_rx) = mpsc::channel();
    let producer = AuditProducer::with_writer(1, move |audit| {
        if audit.audit_id == "active" {
            entered_tx.send(()).expect("entered");
            release_rx
                .recv_timeout(Duration::from_secs(4))
                .expect("cleanup deadline");
        }
        written_tx.send(audit.audit_id).expect("written");
        Ok(())
    });
    assert_eq!(producer.try_enqueue(row("active")), Ok(()));
    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("writer held");
    assert_eq!(producer.try_enqueue(row("queued")), Ok(()));
    let (done_tx, done_rx) = mpsc::channel();
    let join = std::thread::spawn(move || {
        let result = producer.try_enqueue(row("rejected"));
        done_tx
            .send((result, producer.stats().overflow_count))
            .expect("done");
        producer
    });
    let observed = done_rx.recv_timeout(Duration::from_secs(1));
    release_tx
        .send(())
        .expect("always release before assertions");
    let producer = join.join().expect("producer thread");
    assert_eq!(
        observed.expect("enqueue must return while writer held"),
        (Err(AuditEnqueueError::Full), 1)
    );
    drop(producer);
    assert_eq!(
        written_rx.into_iter().collect::<Vec<_>>(),
        vec!["active", "queued"]
    );
}

#[test]
fn must_fail_storage_rejection_is_counted_without_silently_claiming_persistence() {
    let (tx, rx) = mpsc::channel();
    let producer = AuditProducer::with_writer(2, move |audit| {
        tx.send(audit.audit_id).expect("attempt");
        Err(synapse_storage::StorageError::WriteFailed {
            cf_name: cf::CF_REFLEX_AUDIT.into(),
            detail: "synthetic disk refusal".into(),
        })
    });
    assert_eq!(producer.stats().write_error_count, 0);
    assert_eq!(producer.try_enqueue(row("denied")), Ok(()));
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2))
            .expect("attempt deadline"),
        "denied"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while producer.stats().write_error_count == 0 && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert_eq!(producer.stats().write_error_count, 1);
    assert_eq!(producer.stats().overflow_count, 0);
    drop(producer);
}

#[test]
fn must_fail_real_time_producers_never_call_synchronous_write_audit() {
    for path in [
        "scheduler_stateful.rs",
        "scheduler_loop.rs",
        "scheduler_tick.rs",
        "kinds/on_event.rs",
    ] {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join(path),
        )
        .expect("production source");
        assert!(
            !source.contains("write_audit(db,"),
            "blocking producer remains in {path}"
        );
    }
}
