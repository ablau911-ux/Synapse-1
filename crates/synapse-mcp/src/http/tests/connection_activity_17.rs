//! Unit tests for the per-connection idle deadline (#17).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use tokio::time::Instant as TokioInstant;

use super::*;

const DEADLINE: Duration = Duration::from_millis(150);
const GENEROUS: Duration = Duration::from_secs(3);

#[tokio::test]
async fn must_pass_expires_after_the_deadline_when_no_exchange_ever_starts() {
    // Same clock as production (tokio Instant), read BEFORE the idle stamp, so
    // the lower bound is exact: no slack.
    let started = TokioInstant::now();
    let activity = ConnectionActivity::new(DEADLINE);
    tokio::time::timeout(GENEROUS, activity.expired())
        .await
        .expect("an untouched connection must expire");
    let elapsed = started.elapsed();
    assert!(elapsed >= DEADLINE, "expired early after {elapsed:?}");
}

#[tokio::test]
async fn must_fail_does_not_expire_while_an_exchange_is_in_flight() {
    let activity = ConnectionActivity::new(DEADLINE);
    let guard = activity.begin();
    let held = tokio::time::timeout(DEADLINE * 4, activity.expired()).await;
    assert!(
        held.is_err(),
        "an in-flight exchange (SSE) must keep the connection"
    );
    drop(guard);
    tokio::time::timeout(GENEROUS, activity.expired())
        .await
        .expect("expires once the exchange ended");
}

#[tokio::test]
async fn must_pass_deadline_restarts_when_the_last_exchange_ends() {
    let activity = ConnectionActivity::new(DEADLINE);
    let guard = activity.begin();
    tokio::time::sleep(DEADLINE * 2).await;
    // Read before the drop stamps idle_since, so expiry is >= DEADLINE exactly.
    let ended = TokioInstant::now();
    drop(guard);
    tokio::time::timeout(GENEROUS, activity.expired())
        .await
        .expect("expires after the exchange ended");
    let elapsed = ended.elapsed();
    assert!(
        elapsed >= DEADLINE,
        "idle time spent inside the exchange must not count towards the deadline ({elapsed:?})"
    );
}

#[tokio::test]
async fn must_fail_upgraded_connection_never_expires() {
    let activity = ConnectionActivity::new(DEADLINE);
    activity.mark_upgraded();
    let result = tokio::time::timeout(DEADLINE * 4, activity.expired()).await;
    assert!(
        result.is_err(),
        "a WebSocket connection leaves the idle policy"
    );
}
