//! Issue #229: the bridge's "wait for the daemon" budget is wall-clock time, not
//! a probe count, and a slow probe must not stretch it past the budget.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use tokio::net::TcpListener;

use super::wait_until_healthy_within;

/// Accepts connections and never answers, so every `/health` probe hangs until
/// its own timeout. Returns `host:port`.
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

#[tokio::test]
async fn must_fail_wait_budget_is_not_stretched_by_a_hanging_probe() {
    let bind = serve_silent().await;
    let budget = Duration::from_millis(600);
    let started = Instant::now();
    let error = wait_until_healthy_within(&bind, "token", true, budget)
        .await
        .expect_err("a silent daemon never becomes healthy");
    let elapsed = started.elapsed();
    assert!(
        error.to_string().contains("MCP_DAEMON_SPAWN_FAILED"),
        "unexpected error: {error}"
    );
    assert!(
        elapsed < budget + Duration::from_millis(1500),
        "wait took {elapsed:?} for a {budget:?} budget; probe timeout stretched it"
    );
}

#[tokio::test]
async fn must_fail_wait_budget_is_time_bounded_on_refused_port() {
    // Bind then drop to get a port that refuses connections.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let bind = listener.local_addr().expect("addr").to_string();
    drop(listener);
    let budget = Duration::from_millis(500);
    let started = Instant::now();
    let error = wait_until_healthy_within(&bind, "token", false, budget)
        .await
        .expect_err("nothing listens");
    assert!(
        error
            .to_string()
            .contains("MCP_CONNECT_DAEMON_UNRESPONSIVE")
    );
    let elapsed = started.elapsed();
    // Time-bounded, not count-bounded: the wait runs out the whole budget and
    // stops promptly after it.
    assert!(
        elapsed >= budget,
        "wait gave up after {elapsed:?}, before the {budget:?} budget"
    );
    assert!(
        elapsed < budget + Duration::from_millis(500),
        "wait took {elapsed:?} for a {budget:?} budget"
    );
}

/// Answers the n-th `/health` connection with `statuses[n]` (the last status
/// repeats) and closes it. Returns `host:port` and the accepted-connection
/// count (one per probe: every probe builds a fresh client).
async fn serve_statuses(statuses: Vec<u16>) -> (String, Arc<AtomicUsize>) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let bind = listener.local_addr().expect("addr").to_string();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&accepted);
    tokio::spawn(async move {
        let mut served = 0_usize;
        while let Ok((mut stream, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            let status = statuses
                .get(served)
                .or_else(|| statuses.last())
                .copied()
                .unwrap_or(200);
            served += 1;
            let mut request = [0_u8; 2048];
            let _read = stream.read(&mut request).await;
            let response =
                format!("HTTP/1.1 {status} X\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            let _write = stream.write_all(response.as_bytes()).await;
            let _shutdown = stream.shutdown().await;
        }
    });
    (bind, accepted)
}

#[tokio::test]
async fn must_pass_healthy_daemon_is_accepted_within_the_budget() {
    let (bind, accepted) = serve_statuses(vec![200]).await;
    let budget = Duration::from_secs(5);
    let started = Instant::now();
    wait_until_healthy_within(&bind, "token", true, budget)
        .await
        .expect("a daemon answering 200 on /health is healthy");
    assert!(
        started.elapsed() < budget,
        "a healthy daemon must be accepted before the budget runs out"
    );
    assert_eq!(accepted.load(Ordering::SeqCst), 1, "first 200 is accepted");
}

#[tokio::test]
async fn must_pass_daemon_that_becomes_healthy_later_is_waited_for() {
    // Two unhealthy answers, then healthy: the wait keeps polling by time.
    let (bind, accepted) = serve_statuses(vec![503, 503, 200]).await;
    wait_until_healthy_within(&bind, "token", false, Duration::from_secs(5))
        .await
        .expect("a daemon that turns healthy inside the budget is accepted");
    // A 503 must never count as healthy: exactly the third probe is accepted.
    assert_eq!(accepted.load(Ordering::SeqCst), 3);
}
