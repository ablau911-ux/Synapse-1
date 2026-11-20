//! #74: a background UIA/point click on a WebView2 window that is NOT the OS
//! foreground is refused BEFORE any provider dispatch, and the refusal is
//! decided only by the three injected probes of the real preflight
//! (`preflight_background_webview_uia_click_with`, the function the handler's
//! `preflight_background_webview_uia_click` delegates to). Probe counters
//! prove which reads happened; the source-order guard proves the handler runs
//! the preflight before every delivery route.

use std::cell::Cell;

use serde_json::{Value, json};
use synapse_core::{CdpDiagnostics, CdpStatus, error_codes};

use super::*;

const TARGET_ROOT: i64 = 0x2562_6122;
const HUMAN_ROOT: i64 = 0x0013_1304;

fn element_click() -> ActClickParams {
    serde_json::from_value(json!({ "target": { "element_id": "0x25626122:2a00000001" } }))
        .expect("valid UIA element click params")
}

fn cdp(status: CdpStatus) -> CdpDiagnostics {
    let mut cdp = CdpDiagnostics::unreachable_with_probe(
        "Finance Manager.exe",
        error_codes::A11Y_CDP_UNREACHABLE,
        vec![9227],
        "WebView2 browser child has no --remote-debugging-port",
    );
    cdp.status = status;
    cdp
}

#[derive(Default)]
struct Probes {
    root: Cell<u32>,
    cdp: Cell<u32>,
    foreground: Cell<u32>,
}

fn run(
    params: &ActClickParams,
    probes: &Probes,
    cdp_result: Option<CdpDiagnostics>,
    foreground: Result<i64, ErrorData>,
) -> Result<(), ErrorData> {
    preflight_background_webview_uia_click_with(
        params,
        |_params| {
            probes.root.set(probes.root.get() + 1);
            Ok(Some(TARGET_ROOT))
        },
        |root| {
            assert_eq!(root, TARGET_ROOT, "CDP is probed on the resolved root");
            probes.cdp.set(probes.cdp.get() + 1);
            cdp_result
        },
        || {
            probes.foreground.set(probes.foreground.get() + 1);
            foreground
        },
    )
}

fn code(error: &ErrorData) -> Option<&str> {
    error.data.as_ref()?.get("code").and_then(Value::as_str)
}

#[test]
fn must_fail_background_webview_click_without_cdp_is_refused_unreachable() {
    let probes = Probes::default();
    let error = run(
        &element_click(),
        &probes,
        Some(cdp(CdpStatus::Unreachable)),
        Ok(HUMAN_ROOT),
    )
    .expect_err("background WebView2 click must be refused");
    assert_eq!(code(&error), Some(error_codes::A11Y_CDP_UNREACHABLE));
    let message = error.message.to_string();
    assert!(message.contains("before delivery"), "{message}");
    assert!(message.contains("0x25626122"), "{message}");
    assert!(message.contains("0x131304"), "{message}");
    assert_eq!(
        (probes.root.get(), probes.cdp.get(), probes.foreground.get()),
        (1, 1, 1)
    );
}

#[test]
fn must_fail_background_webview_click_with_reachable_cdp_is_refused_target_invalid() {
    let probes = Probes::default();
    let error = run(
        &element_click(),
        &probes,
        Some(cdp(CdpStatus::Ok)),
        Ok(HUMAN_ROOT),
    )
    .expect_err("UIA click into a background WebView2 must be refused");
    assert_eq!(code(&error), Some(error_codes::ACTION_TARGET_INVALID));
    assert!(
        error.message.contains("CDP element id"),
        "{}",
        error.message
    );
}

#[test]
fn must_fail_unreadable_foreground_fails_closed() {
    let probes = Probes::default();
    let error = run(
        &element_click(),
        &probes,
        Some(cdp(CdpStatus::Unreachable)),
        Err(mcp_error(error_codes::A11Y_NO_FOREGROUND, "no foreground")),
    )
    .expect_err("an unreadable foreground must not let the click through");
    assert_eq!(code(&error), Some(error_codes::A11Y_NO_FOREGROUND));
}

#[test]
fn must_pass_webview_target_that_is_the_foreground_is_allowed() {
    let probes = Probes::default();
    let result = run(
        &element_click(),
        &probes,
        Some(cdp(CdpStatus::Unreachable)),
        Ok(TARGET_ROOT),
    );
    assert!(matches!(result, Ok(())), "{result:?}");
    assert_eq!(probes.foreground.get(), 1);
}

#[test]
fn must_pass_non_webview_window_skips_the_foreground_probe() {
    let probes = Probes::default();
    let result = run(&element_click(), &probes, None, Ok(HUMAN_ROOT));
    assert!(matches!(result, Ok(())), "{result:?}");
    assert_eq!(
        (probes.root.get(), probes.cdp.get(), probes.foreground.get()),
        (1, 1, 0)
    );
}

#[test]
fn must_pass_cdp_element_click_reads_no_probe() {
    let params: ActClickParams = serde_json::from_value(json!({
        "target": { "element_id": web_element_id_fixture() },
    }))
    .expect("valid CDP element click params");
    let probes = Probes::default();
    let result = run(&params, &probes, Some(cdp(CdpStatus::Ok)), Ok(HUMAN_ROOT));
    assert!(matches!(result, Ok(())), "{result:?}");
    assert_eq!(
        (probes.root.get(), probes.cdp.get(), probes.foreground.get()),
        (0, 0, 0)
    );
}

/// A CDP element id as minted by `synapse_a11y` (precondition: the real
/// classifier recognises it, so the test cannot pass on a malformed id).
fn web_element_id_fixture() -> String {
    let id = synapse_a11y::cdp_element_id(TARGET_ROOT, 42);
    assert!(
        synapse_a11y::cdp_backend_from_element_id(&id).is_some(),
        "precondition: fixture is a CDP element id"
    );
    id.as_str().to_owned()
}

/// Handler wiring: inside `act_click`, the WebView2 preflight runs before the
/// auto-wait and before every delivery route (PostMessage, verified router,
/// leased direct click). Source-order guard (last resort: the handler needs a
/// live rmcp RequestContext).
#[test]
fn must_pass_act_click_runs_the_preflight_before_every_delivery_route() {
    let source = include_str!("../m2_tools.rs");
    let start = source
        .find("pub async fn act_click(")
        .expect("act_click handler present");
    let body = &source[start..];
    let end = body.find("\n    #[tool(").unwrap_or(body.len());
    let body = &body[..end];
    let preflight = body
        .find("preflight_background_webview_uia_click(&params)")
        .expect("act_click calls the WebView2 preflight");
    for route in [
        "maybe_auto_wait_for_actionability(",
        "act_click_claimed_point_postmessage(",
        "act_click_with_verified_router(",
        "act_click_with_handle_and_lease(",
    ] {
        let at = body
            .find(route)
            .unwrap_or_else(|| panic!("{route} present in act_click"));
        assert!(preflight < at, "preflight must precede {route}");
    }
}
