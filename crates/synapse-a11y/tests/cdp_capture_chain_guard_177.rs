//! #177 guard (variant scan): no function on the raw-CDP page/element capture
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! path may activate the user's tab or raise Chrome. The earlier guard only
//! scanned `cdp_capture.rs`; restoring `.screenshot(...)` (chromiumoxide
//! `Page::screenshot` sends `Target.activateTarget`) or an activate call in
//! either public capture fn or its helpers would have escaped it.
//!
//! Scanned (bodies only, `//` comment lines removed):
//! - `src/cdp_capture.rs` (whole module, minus tests);
//! - `src/cdp_action.rs`: `cdp_capture_node_bgra`, `cdp_capture_page_bgra`,
//!   `with_target_page`, `get_target_page_with_discovery`,
//!   `get_page_with_discovery`, `prime_target_page_for_input`,
//!   `resolve_owning_page`, `page_owns_backend_node`, `node_content_rect`;
//! - `synapse-mcp/src/server/m1_tools.rs` callers:
//!   `capture_cdp_target_screenshot_to_file`, `read_text_web_element`.
//!
//! Last-resort source guard (QA step 2): the behavioural proof needs a live
//! Chrome and stays a manual VBV item (tab selection + foreground unchanged).

use std::path::{Path, PathBuf};

const BANNED: [&str; 8] = [
    ".screenshot(",
    ".activate(",
    "bring_to_front",
    "BringToFront",
    "ActivateTarget",
    "activate_target",
    "SetForegroundWindow",
    "focus_window",
];

fn crate_file(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn code_only(src: &str) -> String {
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Body of `fn <name>` (signature through its matching closing brace).
fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
    let start = [
        format!("async fn {name}("),
        format!("async fn {name}<"),
        format!("fn {name}("),
    ]
    .iter()
    .find_map(|needle| src.find(needle.as_str()))
    .unwrap_or_else(|| panic!("precondition: fn {name} exists"));
    let open = start + src[start..].find('{').expect("fn body brace");
    let mut depth = 0_i32;
    for (offset, ch) in src[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[start..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced body for fn {name}");
}

fn assert_clean(label: &str, code: &str) {
    for banned in BANNED {
        assert!(!code.contains(banned), "#177: {label} contains `{banned}`");
    }
}

#[test]
fn must_fail_capture_module_never_activates_target() {
    let src = std::fs::read_to_string(crate_file("src/cdp_capture.rs")).expect("cdp_capture.rs");
    let module = src.split("#[cfg(test)]").next().unwrap_or_default();
    assert!(
        module.contains("capture_png_without_activation"),
        "precondition: capture helper"
    );
    assert_clean("cdp_capture.rs", &code_only(module));
}

#[test]
fn must_fail_cdp_action_capture_chain_never_activates_target() {
    let src = code_only(
        &std::fs::read_to_string(crate_file("src/cdp_action.rs")).expect("cdp_action.rs"),
    );
    for name in [
        "cdp_capture_node_bgra",
        "cdp_capture_page_bgra",
        "with_target_page",
        "get_target_page_with_discovery",
        "get_page_with_discovery",
        "prime_target_page_for_input",
        "resolve_owning_page",
        "page_owns_backend_node",
        "node_content_rect",
    ] {
        assert_clean(&format!("cdp_action::{name}"), fn_body(&src, name));
    }
    for name in ["cdp_capture_node_bgra", "cdp_capture_page_bgra"] {
        assert!(
            fn_body(&src, name).contains("capture_png_without_activation"),
            "#177: {name} must capture through capture_png_without_activation"
        );
    }
}

#[test]
fn must_fail_mcp_capture_callers_never_activate_target() {
    let path = crate_file("../synapse-mcp/src/server/m1_tools.rs");
    let src = code_only(&std::fs::read_to_string(&path).expect("m1_tools.rs"));
    for name in [
        "capture_cdp_target_screenshot_to_file",
        "read_text_web_element",
    ] {
        assert_clean(&format!("m1_tools::{name}"), fn_body(&src, name));
    }
}

#[test]
fn must_pass_guard_detects_an_activation_in_a_scanned_body() {
    let planted = "async fn cdp_capture_page_bgra(x: u8) { page.activate().await; }";
    let body = fn_body(planted, "cdp_capture_page_bgra");
    assert!(
        BANNED.iter().any(|banned| body.contains(banned)),
        "guard must see the plant"
    );
}
