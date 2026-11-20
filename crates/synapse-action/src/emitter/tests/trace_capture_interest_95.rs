//! #95 regression: emitter log-capture tests lost `SAFETY_RELEASE_ALL_FIRED`
//! when another thread with no scoped subscriber (e.g. the multi-thread
//! early-ack actor test) registered the callsite first while the capturing
//! test's subscriber was the only live dispatcher. tracing-core then takes
//! its single-dispatcher fast path, asks the registering thread's default
//! (none) and caches `Interest::never` for every thread.

use super::*;

fn emit_probe() {
    tracing::warn!(code = "TRACE_CAPTURE_PROBE_95", "trace capture probe");
}

#[test]
fn must_pass_scoped_capture_survives_callsite_registration_on_subscriberless_thread_95() {
    let trace_buffer = SharedTraceBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(trace_buffer.clone())
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .with_level(false)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);

    // First registration of the probe callsite happens on a thread that has
    // no scoped subscriber.
    std::thread::spawn(emit_probe)
        .join()
        .unwrap_or_else(|_panic| panic!("probe thread should not panic"));
    emit_probe();
    drop(guard);

    let log_line = find_log_line(&trace_buffer.text(), "TRACE_CAPTURE_PROBE_95");
    assert!(
        log_line.contains("trace capture probe"),
        "log_line={log_line}"
    );
}

#[test]
fn must_fail_scoped_capture_does_not_leak_other_threads_or_post_guard_events_95() {
    let trace_buffer = SharedTraceBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(trace_buffer.clone())
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .with_level(false)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);

    // The fix for #95 must not turn the scoped capture into a global one: an
    // event from a subscriber-less thread stays out of this buffer.
    std::thread::spawn(emit_probe)
        .join()
        .unwrap_or_else(|_panic| panic!("probe thread should not panic"));
    emit_probe();
    drop(guard);
    // After the scoped guard is dropped, nothing reaches the buffer.
    emit_probe();

    let text = trace_buffer.text();
    let captured = text
        .lines()
        .filter(|line| line.contains("TRACE_CAPTURE_PROBE_95"))
        .count();
    assert_eq!(
        captured, 1,
        "only the in-scope same-thread probe may be captured, got {text:?}"
    );
}
