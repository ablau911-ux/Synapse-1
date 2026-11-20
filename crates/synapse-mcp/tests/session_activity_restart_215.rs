//! criteria_b2.md:10. Activity must reach the durable restore row through real
//! HTTP requests, without a test calling refresh_activity for the daemon.
#![cfg(windows)]
#[allow(dead_code)]
mod support;

use serde_json::json;
use std::time::Duration;
use support::HttpDaemon;

#[tokio::test]
async fn must_pass_active_client_survives_restart_after_initialize_window_215() -> anyhow::Result<()>
{
    // The configured idle window is deliberately shorter than the hard-coded
    // 60s activity flush. This is supported configuration, not a clock mock.
    const ENV: &[(&str, &str)] = &[("SYNAPSE_HTTP_SESSION_IDLE_TIMEOUT_SECS", "15")];
    let mut daemon = HttpDaemon::start(ENV).await?;
    let mut client = daemon.session("active-restart-215", &[]).await?;
    let original_id = client.session_id().to_owned();
    client.tools_list().await?;
    let started = tokio::time::Instant::now();
    while started.elapsed() < Duration::from_secs(17) {
        client.tools_call("health", json!({})).await?;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    client.tools_call("health", json!({})).await?;
    let last_activity = tokio::time::Instant::now();
    daemon.crash_and_restart(ENV).await?;
    assert!(
        last_activity.elapsed() < Duration::from_secs(15),
        "restart exceeded idle window; invalid reproduction"
    );
    // No initialize/new id: reuse the production-parity client's original
    // Mcp-Session-Id and validated catalog after the real process restart.
    let response = client.tools_call("health", json!({})).await?;
    assert_eq!(client.session_id(), original_id);
    assert_eq!(response["structuredContent"]["ok"], true, "{response}");
    drop(daemon.stop().await?);
    Ok(())
}
