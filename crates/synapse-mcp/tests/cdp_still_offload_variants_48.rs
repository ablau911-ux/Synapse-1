//! #48 source guard for bridge/stitch routes without an injectable bridge.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Supporting evidence only: the raw-CDP route is exercised by #175 tests.
use std::{fs, path::Path};

fn source() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server/m1_tools.rs"))
        .expect("production call sites")
}

#[test]
fn must_pass_bridge_still_conversion_hash_and_write_share_blocking_closure() {
    let source = source();
    let route = source
        .split("if is_chrome_debugger_endpoint(&endpoint) {")
        .find(|part| part.contains("chrome_capture_visible_tab_data_url_to_bgra"))
        .expect("bridge still branch");
    let worker = route
        .split("return tokio::task::spawn_blocking(move || {")
        .nth(1)
        .expect("bridge post-capture blocking boundary");
    let body = worker.split(".await").next().expect("worker body");
    for call in [
        "chrome_capture_visible_tab_data_url_to_bgra(",
        "sha256_hex(",
        "write_screenshot_bitmap(",
    ] {
        assert!(
            body.contains(call),
            "{call} must execute inside bridge blocking worker"
        );
    }
}

#[test]
fn must_fail_stitched_browser_route_cannot_encode_or_write_before_blocking_boundary() {
    let source = source();
    let route = source
        .split("async fn browser_screenshot_impl(")
        .nth(1)
        .expect("browser screenshot route")
        .split("async fn browser_pdf_impl(")
        .next()
        .expect("route body");
    let (before, worker) = route
        .split_once(
            "let (captured, screenshot, page_region) = tokio::task::spawn_blocking(move || {",
        )
        .expect("stitched pipeline blocking boundary");
    for call in [
        "stitch_browser_screenshot_tiles(",
        "write_screenshot_bitmap_with_quality(",
    ] {
        assert!(!before.contains(call), "{call} must not block async worker");
        assert!(
            worker
                .split(".await")
                .next()
                .expect("worker")
                .contains(call)
        );
    }
}
