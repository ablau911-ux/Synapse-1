//! Token-bucket admission per backend. These tests assert only production
//! values: the bucket's accept/refuse decision, its token counts and its
//! `retry_after_ms`. The `ActionError::RateLimited` the emitter builds from a
//! refusal lives in the crate-private emitter path and is covered by the
//! emitter's own rate-limit unit tests.

use std::time::Duration;

use synapse_action::{ResolvedBackend, TokenBucket};
use tokio::time;

#[tokio::test(start_paused = true)]
async fn must_fail_software_5100_events_in_one_window_limits_exactly_100() {
    let bucket = TokenBucket::for_backend(ResolvedBackend::Software);
    align_bucket_clock(&bucket).await;
    let before = bucket.snapshot();

    let (accepted, refused_retries) = submit_events(&bucket, 5_100);
    let after_batch = bucket.snapshot();
    time::advance(Duration::from_secs(1)).await;
    bucket.refill();
    let after_refill = bucket.snapshot();

    assert_eq!(before.tokens, 5_000);
    assert_eq!(accepted, 5_000);
    assert_eq!(refused_retries.len(), 100);
    assert_eq!(after_batch.tokens, 0);
    assert_eq!(after_refill.tokens, 5_000);
    assert_all_retry_after(&refused_retries, 1);
    println!(
        "readback=token_bucket edge=overshoot before=tokens:{} after=tokens:{} accepted={} limited={} first_retry_after_ms={:?} after_refill=tokens:{}",
        before.tokens,
        after_batch.tokens,
        accepted,
        refused_retries.len(),
        refused_retries.first(),
        after_refill.tokens
    );
}

#[tokio::test(start_paused = true)]
async fn must_fail_software_exact_capacity_has_no_rate_limited_errors() {
    let bucket = TokenBucket::for_backend(ResolvedBackend::Software);
    align_bucket_clock(&bucket).await;
    let before = bucket.snapshot();

    let (accepted, refused_retries) = submit_events(&bucket, 5_000);
    let after = bucket.snapshot();

    assert_eq!(before.tokens, 5_000);
    assert_eq!(accepted, 5_000);
    assert!(refused_retries.is_empty());
    assert_eq!(after.tokens, 0);
    println!(
        "readback=token_bucket edge=exact_capacity before=tokens:{} after=tokens:{} accepted={} limited={}",
        before.tokens,
        after.tokens,
        accepted,
        refused_retries.len()
    );
}

#[tokio::test(start_paused = true)]
async fn must_fail_vigem_1100_events_limits_exactly_100() {
    let bucket = TokenBucket::for_backend(ResolvedBackend::Vigem);
    align_bucket_clock(&bucket).await;
    let before = bucket.snapshot();

    let (accepted, refused_retries) = submit_events(&bucket, 1_100);
    let after = bucket.snapshot();

    assert_eq!(before.tokens, 1_000);
    assert_eq!(accepted, 1_000);
    assert_eq!(refused_retries.len(), 100);
    assert_eq!(after.tokens, 0);
    assert_all_retry_after(&refused_retries, 1);
    println!(
        "readback=token_bucket edge=vigem_overshoot before=tokens:{} after=tokens:{} accepted={} limited={} first_retry_after_ms={:?}",
        before.tokens,
        after.tokens,
        accepted,
        refused_retries.len(),
        refused_retries.first()
    );
}

#[tokio::test(start_paused = true)]
async fn must_pass_software_partial_window_refills_proportionally() {
    let bucket = TokenBucket::for_backend(ResolvedBackend::Software);
    align_bucket_clock(&bucket).await;

    let (drained, drain_refusals) = submit_events(&bucket, 5_000);
    time::advance(Duration::from_millis(100)).await;
    bucket.refill();
    let after_partial_refill = bucket.snapshot();
    let (accepted, refused_retries) = submit_events(&bucket, 501);
    let after = bucket.snapshot();

    assert_eq!(drained, 5_000);
    assert!(drain_refusals.is_empty());
    assert_eq!(after_partial_refill.tokens, 500);
    assert_eq!(accepted, 500);
    assert_eq!(refused_retries.len(), 1);
    assert_eq!(after.tokens, 0);
    assert_all_retry_after(&refused_retries, 1);
    println!(
        "readback=token_bucket edge=partial_refill after_partial_refill={after_partial_refill:?} accepted={accepted} limited={} after={after:?}",
        refused_retries.len()
    );
}

#[tokio::test(start_paused = true)]
async fn must_pass_under_capacity_bursts_are_all_accepted_per_backend() {
    for (backend, capacity) in [
        (ResolvedBackend::Software, 5_000_u32),
        (ResolvedBackend::Hardware, 5_000_u32),
        (ResolvedBackend::Vigem, 1_000_u32),
    ] {
        let bucket = TokenBucket::for_backend(backend);
        align_bucket_clock(&bucket).await;
        let before = bucket.snapshot();

        let burst = usize::try_from(capacity - 1).unwrap_or(usize::MAX);
        let (accepted, refused_retries) = submit_events(&bucket, burst);
        let after = bucket.snapshot();

        assert_eq!(before.capacity, capacity, "backend={}", backend.as_str());
        assert_eq!(before.tokens, capacity, "backend={}", backend.as_str());
        assert_eq!(accepted, burst, "backend={}", backend.as_str());
        assert!(refused_retries.is_empty(), "backend={}", backend.as_str());
        assert_eq!(after.tokens, 1, "backend={}", backend.as_str());
        assert_eq!(bucket.retry_after_ms(1), 0, "backend={}", backend.as_str());
        println!(
            "readback=token_bucket edge=under_capacity backend={} before={before:?} after={after:?} accepted={accepted}",
            backend.as_str()
        );
    }
}

/// Submits `count` one-token events. Returns the accepted count and, for each
/// refused event, the bucket's own `retry_after_ms(1)` at the refusal.
fn submit_events(bucket: &TokenBucket, count: usize) -> (usize, Vec<u64>) {
    let mut accepted = 0;
    let mut refused_retries = Vec::new();
    for _event_index in 0..count {
        if bucket.try_consume(1) {
            accepted += 1;
        } else {
            refused_retries.push(bucket.retry_after_ms(1));
        }
    }
    (accepted, refused_retries)
}

async fn align_bucket_clock(bucket: &TokenBucket) {
    time::advance(Duration::from_millis(1)).await;
    bucket.refill();
}

fn assert_all_retry_after(refused_retries: &[u64], expected_retry_after_ms: u64) {
    for retry_after_ms in refused_retries {
        assert_eq!(*retry_after_ms, expected_retry_after_ms);
    }
}
