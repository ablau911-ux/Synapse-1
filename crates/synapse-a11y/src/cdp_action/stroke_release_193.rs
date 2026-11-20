//! #193 regression: CDP background drag budget follows the planned duration, and a
//! stroke that errors or times out after `mousePressed` releases the page button.
//! A tiny in-process WebSocket server stands in for the CDP page socket.

use std::time::Duration;

use futures_util::StreamExt as _;
use serde_json::Value;
use synapse_core::error_codes;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};

use super::stroke_bounded::{cdp_stroke_budget, dispatch_cdp_mouse_stroke_bounded};
use super::{CdpMouseStrokePoint, MouseButton};

type Ws = WebSocketStream<TcpStream>;

fn point(x: f64, elapsed_ms: f64) -> CdpMouseStrokePoint {
    CdpMouseStrokePoint {
        x,
        y: 10.0,
        elapsed_ms,
    }
}

fn paced_points(count: u32, step_ms: f64) -> Vec<CdpMouseStrokePoint> {
    (0..count)
        .map(|i| point(f64::from(i) * 5.0, f64::from(i) * step_ms))
        .collect()
}

async fn bind() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    (listener, format!("http://127.0.0.1:{port}"))
}

async fn next_ws(listener: &TcpListener, within: Duration) -> Option<Ws> {
    let (stream, _) = tokio::time::timeout(within, listener.accept())
        .await
        .ok()?
        .ok()?;
    accept_async(stream).await.ok()
}

fn event_type(message: &Message) -> Option<String> {
    let Message::Text(text) = message else {
        return None;
    };
    let value: Value = serde_json::from_str(text.as_str()).ok()?;
    value["params"]["type"].as_str().map(str::to_owned)
}

/// Reads CDP mouse event types until `stop` says so, the peer closes, or `within` elapses.
async fn read_types(
    ws: &mut Ws,
    within: Duration,
    stop: impl Fn(&[String]) -> bool,
) -> Vec<String> {
    let mut types = Vec::new();
    let deadline = tokio::time::Instant::now() + within;
    while !stop(&types) {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Ok(Some(Ok(message))) => types.extend(event_type(&message)),
            _ => break,
        }
    }
    types
}

#[test]
fn must_pass_budget_covers_a_sixty_second_stroke_plus_margin() {
    // Budget = planned duration (last sample's elapsed_ms) + the 30 s margin.
    let long = [point(0.0, 0.0), point(100.0, 60_000.0)];
    assert_eq!(cdp_stroke_budget(&long), Duration::from_secs(90));
    let forty = [point(0.0, 0.0), point(100.0, 40_000.0)];
    assert_eq!(cdp_stroke_budget(&forty), Duration::from_secs(70));
    let fractional = [point(0.0, 0.0), point(100.0, 1_500.0)];
    assert_eq!(
        cdp_stroke_budget(&fractional),
        Duration::from_millis(31_500)
    );
}

#[test]
fn must_fail_budget_has_a_floor_for_instant_strokes() {
    assert!(cdp_stroke_budget(&[point(0.0, 0.0)]) >= Duration::from_secs(5));
}

#[tokio::test]
async fn must_fail_socket_error_after_press_releases_the_page_button_on_a_fresh_socket() {
    let (listener, endpoint) = bind().await;
    let server = tokio::spawn(async move {
        let mut first = next_ws(&listener, Duration::from_secs(5))
            .await
            .expect("conn1");
        let seen = read_types(&mut first, Duration::from_secs(5), |t| {
            t.iter().any(|x| x == "mousePressed") && t.len() >= 3
        })
        .await;
        drop(first); // abrupt close mid-drag
        let mut second = next_ws(&listener, Duration::from_secs(3)).await;
        let released = match second.as_mut() {
            Some(ws) => read_types(ws, Duration::from_secs(2), |t| !t.is_empty()).await,
            None => Vec::new(),
        };
        (seen, released)
    });
    let result = dispatch_cdp_mouse_stroke_bounded(
        &endpoint,
        "T1",
        &paced_points(30, 100.0),
        MouseButton::Left,
        Duration::from_secs(20),
    )
    .await;
    let error = result.expect_err("dead socket must fail the stroke");
    // A lost page socket is a CDP command failure (the crate's raw-input send
    // failure code), not a budget timeout.
    assert_eq!(
        error.code(),
        error_codes::A11Y_CDP_AXTREE_FAILED,
        "{error:?}"
    );
    let (seen, released) = server.await.expect("server task");
    assert!(
        seen.iter().any(|x| x == "mousePressed"),
        "press seen: {seen:?}"
    );
    assert_eq!(released, ["mouseReleased"], "fresh-socket release expected");
}

#[tokio::test]
async fn must_fail_timeout_after_press_releases_the_page_button_and_reports_a_timeout_code() {
    let (listener, endpoint) = bind().await;
    let server = tokio::spawn(async move {
        let mut first = next_ws(&listener, Duration::from_secs(5))
            .await
            .expect("conn1");
        let _ = read_types(&mut first, Duration::from_secs(5), |t| {
            t.iter().any(|x| x == "mousePressed")
        })
        .await;
        let mut second = next_ws(&listener, Duration::from_secs(5)).await;
        let released = match second.as_mut() {
            Some(ws) => read_types(ws, Duration::from_secs(2), |t| !t.is_empty()).await,
            None => Vec::new(),
        };
        drop(first);
        released
    });
    let result = dispatch_cdp_mouse_stroke_bounded(
        &endpoint,
        "T2",
        &paced_points(60, 100.0),
        MouseButton::Left,
        Duration::from_millis(400),
    )
    .await;
    let error = result.expect_err("a 400 ms budget cannot finish a 5.9 s stroke");
    assert_eq!(error.code(), error_codes::A11Y_CDP_STROKE_TIMEOUT);
    let released = server.await.expect("server task");
    assert_eq!(
        released,
        ["mouseReleased"],
        "timeout must release the button"
    );
}

#[tokio::test]
async fn must_fail_hover_stroke_timeout_sends_no_release() {
    let (listener, endpoint) = bind().await;
    let server = tokio::spawn(async move {
        let mut first = next_ws(&listener, Duration::from_secs(5))
            .await
            .expect("conn1");
        let _ = read_types(&mut first, Duration::from_secs(1), |_| false).await;
        next_ws(&listener, Duration::from_millis(700))
            .await
            .is_some()
    });
    let result = dispatch_cdp_mouse_stroke_bounded(
        &endpoint,
        "T3",
        &paced_points(60, 100.0),
        MouseButton::None,
        Duration::from_millis(300),
    )
    .await;
    let error = result.expect_err("a 300 ms budget cannot finish a 5.9 s hover stroke");
    assert_eq!(error.code(), error_codes::A11Y_CDP_STROKE_TIMEOUT);
    assert!(
        !server.await.expect("server task"),
        "no button held, no release socket"
    );
}

#[tokio::test]
async fn must_pass_completed_stroke_releases_exactly_once_on_one_socket() {
    let (listener, endpoint) = bind().await;
    let server = tokio::spawn(async move {
        let mut first = next_ws(&listener, Duration::from_secs(5))
            .await
            .expect("conn1");
        let seen = read_types(&mut first, Duration::from_secs(3), |t| {
            t.iter().any(|x| x == "mouseReleased")
        })
        .await;
        let extra = next_ws(&listener, Duration::from_millis(500))
            .await
            .is_some();
        (seen, extra)
    });
    let result = dispatch_cdp_mouse_stroke_bounded(
        &endpoint,
        "T4",
        &paced_points(4, 20.0),
        MouseButton::Left,
        Duration::from_secs(10),
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    let (seen, extra) = server.await.expect("server task");
    assert_eq!(
        seen.iter().filter(|x| *x == "mouseReleased").count(),
        1,
        "{seen:?}"
    );
    assert!(!extra, "success must not open a release socket");
}
