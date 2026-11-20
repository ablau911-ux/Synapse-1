//! Drives the real launch verifier and observes its tracing writer.
use super::*;
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};
use tracing::instrument::WithSubscriber;

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);
impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("log lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

async fn verify(url: &str) -> anyhow::Result<(String, String)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let requested = url.to_owned();
    let route_url = requested.clone();
    let app = axum::Router::new().route(
        "/json/list",
        axum::routing::get(move || {
            let url = route_url.clone();
            async move { axum::Json(json!([{"type": "page", "url": url, "title": "G7b"}])) }
        }),
    );
    let stop = tokio_util::sync::CancellationToken::new();
    let shutdown = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
    });
    let params: ActLaunchParams =
        serde_json::from_value(json!({"target":"chrome.exe", "args":[requested]}))?;
    let launch = ChromiumCdpLaunch {
        user_data_dir: std::path::PathBuf::from("unused"),
        injected_args: vec![],
    };
    let cdp = LaunchedCdp {
        port: Some(addr.port()),
        endpoint: Some(format!("http://{addr}")),
        user_data_dir: None,
    };
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    assert_eq!(log.0.lock().expect("before").len(), 0);
    let result = verify_launched_chromium_url(&params, Some(&launch), &cdp, 1000)
        .with_subscriber(subscriber)
        .await;
    stop.cancel();
    server.await??;
    let verified = result?.expect("matching CDP target");
    let text = String::from_utf8(log.0.lock().expect("after").clone())?;
    Ok((verified.url, text))
}

#[tokio::test]
async fn must_pass_launch_verification_logs_safe_host_and_preserves_returned_url_185()
-> anyhow::Result<()> {
    let url = "https://example.test/callback";
    let (actual, log) = verify(url).await?;
    assert_eq!(actual, url);
    assert!(
        log.contains("M4_ACT_LAUNCH_CDP_URL_VERIFIED"),
        "actual call site must emit"
    );
    assert!(log.contains("https://example.test/"));
    Ok(())
}

#[tokio::test]
async fn must_fail_launch_success_log_never_persists_oauth_query_or_fragment_185()
-> anyhow::Result<()> {
    let url = "https://example.test/callback?code=G7B_SECRET_CODE#access_token=G7B_SECRET_TOKEN";
    let (actual, log) = verify(url).await?;
    assert_eq!(
        actual, url,
        "verification still compares the full requested URL"
    );
    assert!(
        log.contains("M4_ACT_LAUNCH_CDP_URL_VERIFIED"),
        "no-log false positive"
    );
    assert!(log.contains("https://example.test/"));
    for secret in [
        "G7B_SECRET_CODE",
        "G7B_SECRET_TOKEN",
        "access_token=",
        "?code=",
    ] {
        assert!(
            !log.contains(secret),
            "launch verifier leaked {secret}: {log}"
        );
    }
    Ok(())
}
