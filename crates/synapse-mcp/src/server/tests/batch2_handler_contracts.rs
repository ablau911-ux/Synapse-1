//! Frozen criteria_b2.md lines 8-12. Real ServerHandler::call_tool routes.
//! Test fixtures seed state only; no substitute implementations of the behavior.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rmcp::{ErrorData, ServerHandler, model::CallToolResult};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use synapse_storage::cf;

use crate::server::own_window_fixture::{OwnWindows, WindowSpec, to_hwnd};
use crate::server::{SynapseService, g4_unit_support as fx};

const SESSION: &str = "batch2-handler-contract";

async fn call(
    service: &SynapseService,
    name: &str,
    args: Value,
) -> Result<CallToolResult, ErrorData> {
    let context = crate::test_support::request_context_for_session(SESSION).await;
    ServerHandler::call_tool(
        service,
        serde_json::from_value(json!({"name": name, "arguments": args})).expect("request"),
        context,
    )
    .await
}

fn body(result: Result<CallToolResult, ErrorData>) -> Value {
    let result = result.expect("public handler must succeed");
    assert_ne!(result.is_error, Some(true), "{result:?}");
    result.structured_content.expect("structured output")
}

fn window() -> OwnWindows {
    OwnWindows::spawn(vec![WindowSpec {
        x: 96,
        y: 96,
        w: 480,
        h: 200,
        label: Some("CACHE PRESSURE TEST"),
    }])
}

fn service() -> (tempfile::TempDir, SynapseService) {
    let (dir, service) = fx::isolated_service();
    fx::seed_live_session(&service, SESSION);
    (dir, service)
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_explicit_lease_joins_real_escalation_and_survives_unwind_205() {
    let _serial = crate::test_support::lease_serial("batch2-205");
    let (_dir, service) = service();
    fx::use_synthetic_notepad_foreground(&service);
    fx::seed_live_session(&service, "batch2-205-other");
    let context = crate::test_support::request_context_for_session(SESSION).await;
    let joined = std::cell::RefCell::new(None);
    // Use the router's actual acquire/escalate/unwind path. Only the physical
    // input step is replaced: it issues a second real handler request while
    // the escalation's EXPLICIT lease and session fence are both held.
    let result = service
        .act_foreground_escalated_with(
            serde_json::from_value(json!({"verb":"focus_window"})).expect("action"),
            "#205 explicit join during escalation".to_owned(),
            30_000,
            SESSION,
            &context,
            |_| async {
                assert!(!synapse_action::lease::owns_automatic_lease(SESSION));
                assert_eq!(
                    synapse_action::lease::status().owner_session_id.as_deref(),
                    Some(SESSION)
                );
                *joined.borrow_mut() = Some(
                    call(
                        &service,
                        "act",
                        json!({"operation":"lease_acquire","ttl_ms":30000}),
                    )
                    .await,
                );
                (
                    None,
                    Err(ErrorData::internal_error(
                        "#205 deliberate action failure; exercise unwind",
                        None,
                    )),
                )
            },
        )
        .await;
    assert!(result.is_err(), "deliberately failed step must unwind");
    let response = body(joined.into_inner().expect("real action step reached"));
    assert!(
        matches!(
            response["lease"]["outcome"].as_str(),
            Some("acquired" | "renewed")
        ),
        "{response}"
    );
    let physical = synapse_action::lease::status();
    assert!(
        physical.held,
        "explicit hold must survive action completion: {physical:?}"
    );
    assert_eq!(physical.owner_session_id.as_deref(), Some(SESSION));
    let other = service
        .control_lease_acquire(
            rmcp::handler::server::wrapper::Parameters(
                crate::server::lease_tools::ControlLeaseAcquireParams { ttl_ms: 30000 },
            ),
            crate::test_support::request_context_for_session("batch2-205-other").await,
        )
        .await;
    let Err(other) = other else {
        panic!("promoted lease excludes another session after unwind");
    };
    assert_eq!(
        other.data.as_ref().and_then(|d| d["code"].as_str()),
        Some("ACTION_FOREGROUND_LEASE_BUSY")
    );
    let db = service
        .m3_state_handle()
        .lock()
        .expect("m3")
        .ensure_storage()
        .expect("db");
    let rows = db
        .scan_cf_prefix(cf::CF_SESSIONS, b"mcp/session-lease/v1/")
        .expect("rows");
    assert!(
        rows.iter().any(|(k, _)| k.ends_with(SESSION.as_bytes())),
        "explicit lease journal"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_reality_baseline_omitted_include_retains_focused_and_elements_279() {
    let (_dir, service) = service();
    fx::use_synthetic_notepad_foreground(&service);
    let response = body(call(&service, "reality", json!({
        "operation":"baseline","baseline":{"force_new_epoch":true,"depth":2,"max_elements":20}
    })).await);
    let head = stored_head(&service, &response);
    assert!(
        head["compact_state"]["focused"].is_object(),
        "default focused slot lost: {head}"
    );
    assert!(
        head["compact_state"]["elements"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "default elements slot lost: {head}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_explicit_unsupported_winrt_language_at_read_text_handler_83() {
    use windows::{Globalization::Language, Media::Ocr::OcrEngine, core::HSTRING};
    let _ocr = crate::server::m1_tools::ocr_cache_best_effort_272::OCR_TEST_GATE
        .lock()
        .await;
    let windows = window();
    let (_dir, service) = service();
    // Independently query OS capability. Never hard-code a missing Hebrew pack.
    let unsupported = ["he", "ar", "ja", "ko", "th", "hi"]
        .into_iter()
        .find(|tag| {
            let lang = Language::CreateLanguage(&HSTRING::from(*tag)).expect("valid language");
            !OcrEngine::IsLanguageSupported(&lang).expect("OS capability query")
        })
        .expect("host needs one unsupported OCR language for this regression");
    let mut rect = windows::Win32::Foundation::RECT::default();
    // SAFETY: fixture owns the live window until test ends; rect is writable.
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetWindowRect(
            to_hwnd(windows.hwnd(0)),
            &raw mut rect,
        )
    }
    .expect("own window rectangle");
    let region =
        json!({"x":rect.left,"y":rect.top,"w":rect.right-rect.left,"h":rect.bottom-rect.top});
    let control = body(
        call(
            &service,
            "read_text",
            json!({"region":region,"backend":"winrt"}),
        )
        .await,
    );
    assert!(
        control["full_text"]
            .as_str()
            .is_some_and(|s| s.contains("PRESSURE")),
        "real OCR precondition: {control}"
    );

    let result = call(
        &service,
        "read_text",
        json!({
            "region":region,"backend":"winrt","lang_hint":unsupported,
        }),
    )
    .await;
    let error = result.expect_err("explicit unsupported language must not be labelled successful");
    let code = error.data.as_ref().and_then(|d| d["code"].as_str());
    assert_eq!(code, Some("OCR_LANGUAGE_UNSUPPORTED"), "{error:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_minimized_screenshot_promptly_without_artifact_224() {
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_MINIMIZE, ShowWindow};
    let windows = window();
    let (dir, service) = service();
    let hwnd = windows.hwnd(0);
    // SAFETY: only this test's own window is minimized.
    unsafe {
        let _ = ShowWindow(to_hwnd(hwnd), SW_MINIMIZE);
    }
    assert!(
        unsafe { IsIconic(to_hwnd(hwnd)) }.as_bool(),
        "minimized precondition"
    );
    let path = dir.path().join("minimized.png");
    assert!(!path.exists());
    let started = Instant::now();
    let result = call(
        &service,
        "screenshot",
        json!({
            "operation":"capture","window_hwnd":hwnd,"path":path,
        }),
    )
    .await;
    let elapsed = started.elapsed();
    // Read filesystem even when error mapping is wrong.
    assert!(!path.exists(), "minimized capture must not publish");
    let error = result.expect_err("minimized target is refused");
    assert_eq!(
        error.data.as_ref().and_then(|d| d["code"].as_str()),
        Some("CAPTURE_TARGET_MINIMIZED"),
        "{error:?}; elapsed={elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "minimized preflight took {elapsed:?}"
    );
}

async fn baseline(service: &SynapseService) -> Value {
    body(
        call(
            service,
            "reality",
            json!({"operation":"baseline","baseline":{
                "force_new_epoch":true,"include":[],"depth":1,"max_elements":20,
            }}),
        )
        .await,
    )
}

fn stored_head(service: &SynapseService, baseline: &Value) -> Value {
    let key = baseline["baseline"]["readback_rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find_map(|row| row["row_key"].as_str().filter(|s| s.contains("/head/")))
        .expect("head readback key");
    let runtime = service.reflex_runtime().expect("reflex");
    let bytes = runtime
        .lock()
        .expect("reflex lock")
        .storage_kv_row(key.as_bytes())
        .expect("separate row read")
        .expect("persisted head");
    synapse_storage::decode_json(&bytes).expect("head JSON")
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_reality_baseline_observes_session_target_276() {
    let windows = window();
    let (_dir, service) = service();
    fx::use_synthetic_notepad_foreground(&service);
    fx::set_window_target(&service, SESSION, windows.hwnd(0));
    let response = baseline(&service).await;
    let head = stored_head(&service, &response);
    assert_eq!(
        head["compact_state"]["foreground"]["hwnd"],
        windows.hwnd(0),
        "{head}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_reality_delta_observes_new_session_target_276() {
    let windows = window();
    let (_dir, service) = service();
    fx::use_synthetic_notepad_foreground(&service);
    // No selected target: existing foreground fallback must remain available.
    let before = baseline(&service).await;
    assert_eq!(
        stored_head(&service, &before)["compact_state"]["foreground"]["hwnd"],
        fx::SYNTHETIC_NOTEPAD_HWND
    );
    fx::set_window_target(&service, SESSION, windows.hwnd(0));
    let delta = body(
        call(
            &service,
            "reality",
            json!({"operation":"delta","delta":{
                "since_epoch":before["baseline"]["baseline"]["epoch_id"],"since_seq":0,
                "include":[],"depth":1,"max_elements":20,
            }}),
        )
        .await,
    );
    assert_eq!(delta["delta"]["baseline_required"], false, "{delta}");
    let after = stored_head(&service, &before);
    assert_eq!(
        after["compact_state"]["foreground"]["hwnd"],
        windows.hwnd(0),
        "{after}"
    );
}
