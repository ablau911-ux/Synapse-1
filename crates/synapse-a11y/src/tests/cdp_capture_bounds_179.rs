//! #179 (re-review PARTIAL): raw-CDP captures must bound the requested and the
//! decoded bitmap size BEFORE any CDP call or allocation.
//!
//! Gaps found by the re-review: `cdp_capture_page_bgra` only rejects
//! non-positive clips (a 100000x100000 clip goes straight to Chrome), and
//! `decode_png_to_bgra` allocates `output_buffer_size()` for whatever size the
//! PNG header claims (png 0.18 `Limits` explicitly do not cover that buffer).
//!
//! Limits mirror the public `browser_screenshot` defaults
//! (`synapse-mcp/src/server/browser_screenshot_limits.rs`: 16384 px per edge,
//! 100_000_000 px total). The pre-connect refusal uses the same error family as
//! the existing empty-clip refusal (`A11Y_CDP_AXTREE_FAILED`), and the endpoint
//! is a closed loopback port, so a request that is NOT refused up front fails
//! later with `A11Y_CDP_ATTACH_FAILED` — that difference is the discriminator.
//! No test here allocates anything large, even on unfixed code. The decode
//! budget (NEEDS-SEAM) lives in `cdp_png_decode_budget_179.rs`.

use synapse_core::{Rect, error_codes};

use super::cdp_capture_page_bgra;

/// A loopback endpoint nothing listens on (connect is refused at once).
fn closed_endpoint() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind probe port");
    let port = listener.local_addr().expect("probe port").port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

async fn page_capture_code(w: i32, h: i32) -> (String, String) {
    let clip = Rect { x: 0, y: 0, w, h };
    match cdp_capture_page_bgra(&closed_endpoint(), "target-179", Some(clip)).await {
        Ok(bitmap) => panic!(
            "closed endpoint cannot capture, got {}x{}",
            bitmap.width, bitmap.height
        ),
        Err(error) => (error.code().to_owned(), error.to_string()),
    }
}

#[tokio::test]
async fn must_fail_huge_page_clip_is_refused_before_any_cdp_call() {
    for (w, h) in [
        (100_000, 100_000),
        (16_385, 1),
        (1, 16_385),
        (10_000, 10_001),
    ] {
        let (code, detail) = page_capture_code(w, h).await;
        assert_eq!(
            code,
            error_codes::A11Y_CDP_AXTREE_FAILED,
            "#179: clip {w}x{h} must be refused before connecting (got {code}: {detail})"
        );
        assert!(
            detail.contains("exceeds"),
            "refusal must name the limit: {detail}"
        );
    }
}

#[tokio::test]
async fn must_fail_in_budget_clip_passes_the_bound_and_reaches_the_endpoint() {
    for (w, h) in [(800, 600), (16_384, 6_103), (16_384, 1)] {
        let (code, detail) = page_capture_code(w, h).await;
        assert_eq!(
            code,
            error_codes::A11Y_CDP_ATTACH_FAILED,
            "clip {w}x{h} is within budget and must reach the (closed) endpoint: {detail}"
        );
    }
}
