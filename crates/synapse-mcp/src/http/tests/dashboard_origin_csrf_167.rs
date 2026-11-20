//! #167: dashboard routes and the agent-terminal WebSocket must refuse
//! cross-origin requests (CSRF / cross-site WebSocket hijack) and bad Hosts.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    middleware,
    routing::{get, post},
};
use tower::ServiceExt as _;

use super::{DashboardOriginGuard, HOST_REFUSED, ORIGIN_REFUSED, require_dashboard_origin};

const PORT: u16 = 7700;
const HOST: &str = "127.0.0.1:7700";
const OWN_ORIGIN: &str = "http://127.0.0.1:7700";

fn app() -> Router {
    Router::new()
        .route("/dashboard/state.json", get(|| async { "state" }))
        .route(
            "/dashboard/timeline/resume",
            post(|| async { "resumed" })
                .put(|| async { "put" })
                .delete(|| async { "deleted" }),
        )
        .route("/dashboard/agent-terminal/{id}/ws", get(|| async { "ws" }))
        .layer(middleware::from_fn_with_state(
            DashboardOriginGuard::new(PORT),
            require_dashboard_origin,
        ))
}

async fn send(method: &str, uri: &str, headers: &[(&str, &str)]) -> (StatusCode, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = app()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

async fn post_resume(headers: &[(&str, &str)]) -> (StatusCode, String) {
    send("POST", "/dashboard/timeline/resume", headers).await
}

async fn ws_upgrade(headers: &[(&str, &str)]) -> (StatusCode, String) {
    let mut all = vec![("upgrade", "websocket"), ("connection", "Upgrade")];
    all.extend_from_slice(headers);
    send(
        "GET",
        "/dashboard/agent-terminal/spawn-1/ws?mode=controller",
        &all,
    )
    .await
}

#[tokio::test]
async fn must_fail_cross_origin_post_is_rejected_with_403() {
    let (status, body) = post_resume(&[("host", HOST), ("origin", "http://evil.example")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, ORIGIN_REFUSED);
}

#[tokio::test]
async fn must_fail_other_loopback_port_and_null_origins_are_cross_origin() {
    for origin in [
        "http://127.0.0.1:9999",
        "http://localhost",
        "https://127.0.0.1:7700",
        "null",
        "http://127.0.0.1:7700.evil.example",
        "http://127.0.0.1:7700/path",
    ] {
        let (status, body) = post_resume(&[("host", HOST), ("origin", origin)]).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "origin {origin}");
        assert_eq!(body, ORIGIN_REFUSED, "origin {origin}");
    }
}

#[tokio::test]
async fn must_pass_same_origin_post_is_accepted() {
    for origin in [OWN_ORIGIN, "http://localhost:7700", "http://[::1]:7700"] {
        let (status, body) = post_resume(&[("host", HOST), ("origin", origin)]).await;
        assert_eq!(status, StatusCode::OK, "origin {origin}");
        assert_eq!(body, "resumed");
    }
}

#[tokio::test]
async fn must_pass_requests_without_origin_are_allowed_for_local_clients() {
    for (method, expected) in [("POST", "resumed"), ("PUT", "put"), ("DELETE", "deleted")] {
        let (status, body) = send(method, "/dashboard/timeline/resume", &[("host", HOST)]).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::OK, expected),
            "{method} without Origin must not be refused"
        );
    }
    let (status, _) = send("GET", "/dashboard/timeline/resume", &[("host", HOST)]).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn must_fail_bad_host_is_rejected_for_every_method() {
    for host in [
        "evil.example",
        "evil.example:7700",
        "192.168.1.20:7700",
        "127.0.0.1:7701",
        "127.0.0.1",
        "127.0.0.1:7700:1",
    ] {
        let (status, body) = send("GET", "/dashboard/state.json", &[("host", host)]).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "host {host}");
        assert_eq!(body, HOST_REFUSED, "host {host}");

        let (status, _) = post_resume(&[("host", host), ("origin", OWN_ORIGIN)]).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "host {host} POST");
    }
    let (status, body) = send("GET", "/dashboard/state.json", &[]).await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::FORBIDDEN, HOST_REFUSED)
    );
}

#[tokio::test]
async fn must_pass_loopback_hosts_are_accepted() {
    for host in [
        "127.0.0.1:7700",
        "localhost:7700",
        "[::1]:7700",
        "LOCALHOST:7700",
    ] {
        let (status, _) = send("GET", "/dashboard/state.json", &[("host", host)]).await;
        assert_eq!(status, StatusCode::OK, "host {host}");
    }
}

#[tokio::test]
async fn must_pass_get_without_origin_returns_state() {
    let (status, body) = send("GET", "/dashboard/state.json", &[("host", HOST)]).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "state"));
}

#[tokio::test]
async fn must_fail_cross_origin_get_is_403() {
    let (status, body) = send(
        "GET",
        "/dashboard/state.json",
        &[("host", HOST), ("origin", "http://evil.example")],
    )
    .await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::FORBIDDEN, ORIGIN_REFUSED)
    );
}

#[tokio::test]
async fn must_fail_websocket_upgrade_with_bad_origin_is_rejected() {
    let (status, body) = ws_upgrade(&[("host", HOST), ("origin", "http://evil.example")]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body, ORIGIN_REFUSED);
}

#[tokio::test]
async fn must_pass_websocket_upgrade_with_own_or_no_origin_is_accepted() {
    let (status, body) = ws_upgrade(&[("host", HOST), ("origin", OWN_ORIGIN)]).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "ws"));
    let (status, body) = ws_upgrade(&[("host", HOST)]).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "ws"));
}

#[tokio::test]
async fn must_fail_websocket_upgrade_with_bad_host_is_403() {
    let (status, body) = ws_upgrade(&[("host", "evil.example"), ("origin", OWN_ORIGIN)]).await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::FORBIDDEN, HOST_REFUSED)
    );
}

#[test]
fn must_pass_guard_is_wired_onto_the_dashboard_router() {
    // Fallback source scan: transport::router is private and loads the real
    // bearer token, so it cannot be driven from here (SEAM NEEDED).
    let transport = include_str!("../transport.rs");
    let start = transport
        .find("let dashboard_routes = Router::new()")
        .expect("dashboard_routes builder");
    let len = transport[start..]
        .find(".merge(dashboard_routes)")
        .expect("dashboard_routes merged into the app");
    let code: String = transport[start..start + len]
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    for route in [
        "\"/dashboard/timeline/resume\"",
        "\"/dashboard/agent-terminal/{spawn_id}/ws\"",
        "\"/dashboard/state.json\"",
    ] {
        assert!(code.contains(route), "{route} must be a dashboard route");
    }
    let guard_at = code
        .find("dashboard_origin_guard::require_dashboard_origin")
        .expect("dashboard_routes must be layered with the Origin/Host guard");
    let layer_at = code[..guard_at]
        .rfind(".layer(")
        .expect("guard installed via .layer");
    let last_route = code.rfind(".route(").expect("dashboard routes");
    assert!(
        last_route < layer_at,
        "a route added after the guard layer would bypass it"
    );
    assert!(
        !code[guard_at..].contains(".route("),
        "no dashboard route may be registered after the guard layer"
    );
}
