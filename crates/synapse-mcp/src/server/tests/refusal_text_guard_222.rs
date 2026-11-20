//! #222 guards. The background WebView2 click refusal (#79) fires for claimed
//! AND unclaimed targets, so its text must never assert a claim.
//!
//! The live refusal needs a background WebView2 host window (not creatable in a
//! unit test; manual VBV covers it). These tests pin the exact text, that the
//! one production call site refuses through this formatter with
//! A11Y_CDP_UNREACHABLE, and that no production string anywhere in
//! synapse-mcp reintroduces "claimed HWND".

use std::path::{Path, PathBuf};

use super::webview2_background_click_refusal_message;

const ROUTER_SOURCE: &str = include_str!("../background_router.rs");
const PREFIX: &str = "target_act background WebView2 click refused before delivery for target HWND 0x1a2b: no reachable page CDP endpoint. No foreground permission or activation is needed for a target-scoped CDP action. ";

#[test]
fn must_pass_refusal_text_is_exact_with_launch_hint_or_detail() {
    assert_eq!(
        webview2_background_click_refusal_message(0x1a2b, None),
        format!("{PREFIX}Launch the app through Synapse process launch with cdp_debug=true")
    );
    assert_eq!(
        webview2_background_click_refusal_message(0x1a2b, Some("CDP port 9222 closed")),
        format!("{PREFIX}CDP port 9222 closed")
    );
}

#[test]
fn must_fail_the_call_site_refuses_through_the_formatter_with_cdp_unreachable() {
    let calls: Vec<usize> = ROUTER_SOURCE
        .match_indices("webview2_background_click_refusal_message(hwnd,")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(calls.len(), 1, "exactly one production call site");
    let window = &ROUTER_SOURCE[calls[0].saturating_sub(200)..calls[0]];
    assert!(
        window.contains("error_codes::A11Y_CDP_UNREACHABLE"),
        "the refusal keeps code A11Y_CDP_UNREACHABLE"
    );
}

fn production_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name != "tests") {
                production_sources(&path, out);
            }
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// VARIANT GUARD: no string literal in synapse-mcp production code says a
/// target is a "claimed HWND" (test directories excluded).
#[test]
fn must_fail_no_production_string_calls_a_target_a_claimed_hwnd() {
    let mut files = Vec::new();
    production_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(
        files.len() > 50,
        "precondition: production sources found ({})",
        files.len()
    );
    let offenders: Vec<String> = files
        .iter()
        .flat_map(|path| {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            text.lines()
                .enumerate()
                .filter(|(_, line)| {
                    let trimmed = line.trim_start();
                    !trimmed.starts_with("//")
                        && line.contains('"')
                        && line.to_ascii_lowercase().contains("claimed hwnd")
                })
                .map(|(number, _)| format!("{}:{}", path.display(), number + 1))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "claimed-HWND refusal text: {offenders:?}"
    );
}
