//! #175: `read_text_web_element` and `capture_cdp_target_screenshot_to_file`
//! must offload blocking window/endpoint probing, WinRT OCR, Lanczos3
//! downscaling, PNG/JPEG encoding, and disk write/readback to
//! `tokio::task::spawn_blocking` instead of stalling Tokio async workers.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use synapse_core::{OcrBackend, Rect, error_codes};
use tempfile::TempDir;

use super::{
    CaptureScreenshotFormat, CaptureScreenshotParams, CdpTargetOwner, ReadTextParams,
    SynapseService,
    tests::{insert_test_target_claim, seed_session_client, service_with_temp_db},
};

fn error_code(error: &rmcp::ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str)
}

fn seed_cdp_owner(
    service: &SynapseService,
    session_id: &str,
    hwnd: i64,
    target_id: &str,
    endpoint: &str,
) -> anyhow::Result<()> {
    let now = crate::server::session_registry::unix_time_ms_now();
    seed_session_client(service, session_id, "claude-code", now)?;
    service.register_cdp_target_owner(CdpTargetOwner {
        session_id: session_id.to_owned(),
        window_hwnd: hwnd,
        endpoint: endpoint.to_owned(),
        chrome_window_id: None,
        capture_window_hwnd: None,
        cdp_target_id: target_id.to_owned(),
        requested_url: "about:blank#175".to_owned(),
        target_url: "about:blank#175".to_owned(),
        created_at_unix_ms: now,
    })?;
    insert_test_target_claim(service, session_id, hwnd, target_id)?;
    let rows = service.read_persisted_cdp_target_owners_for_target_id(target_id)?;
    assert_eq!(rows.len(), 1, "precondition: CDP owner row must be stored");
    Ok(())
}

fn solid_bgra(width: u32, height: u32, bgra_px: [u8; 4]) -> synapse_a11y::CdpNodeBitmap {
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..(width * height) {
        bgra.extend_from_slice(&bgra_px);
    }
    synapse_a11y::CdpNodeBitmap {
        width,
        height,
        bgra,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_read_text_web_element_offloads_endpoint_probe_before_returning_unreachable()
-> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    // Real web-node id shape (`<hwnd>:cdcd<backend hex>`): the old literal
    // `cdp_...` runtime part is not hex, so `ElementId::parts` refused it before
    // the endpoint probe ever ran (ACTION_ELEMENT_NOT_RESOLVED).
    let web_el = synapse_a11y::cdp_element_id(0x1751, 7);
    let params = ReadTextParams {
        region: None,
        element_id: Some(web_el.clone()),
        window_hwnd: None,
        lang_hint: Some("en-US".to_owned()),
        backend: OcrBackend::Winrt,
    };
    let pre_capture_yielded = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&pre_capture_yielded);
    let _probe = tokio::spawn(async move {
        flag.store(true, Ordering::SeqCst);
    });

    let err = service
        .read_text_web_element(&web_el, 7, &params)
        .await
        .expect_err("window with no CDP endpoint or owner must fail closed");

    assert_eq!(error_code(&err), Some(error_codes::A11Y_CDP_UNREACHABLE));
    assert!(
        pre_capture_yielded.load(Ordering::SeqCst),
        "read_text_web_element must offload blocking endpoint_for_window/foreground_context to spawn_blocking so sibling tasks run"
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_read_text_web_element_offloads_post_capture_ocr_to_blocking_pool()
-> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    // CDP target ids are hex; the target-aware id carries it (`cdcd<target><backend>`).
    let (session, hwnd, target) = ("i175-ocr-session", 0x1752, "175A0C12");
    seed_cdp_owner(&service, session, hwnd, target, "http://127.0.0.1:9222")?;
    let web_el = synapse_a11y::cdp_element_id_for_target(hwnd, target, 9);
    let params = ReadTextParams {
        region: None,
        element_id: Some(web_el.clone()),
        window_hwnd: None,
        lang_hint: Some("en-US".to_owned()),
        backend: OcrBackend::Winrt,
    };
    let post_capture_yielded = Arc::new(AtomicBool::new(false));
    let post_flag = Arc::clone(&post_capture_yielded);

    let result = service
        .read_text_web_element_with_capture(
            &web_el,
            9,
            &params,
            move |_endpoint, _title, _target_id, _node_id| async move {
                tokio::spawn(async move {
                    post_flag.store(true, Ordering::SeqCst);
                });
                Ok(solid_bgra(64, 64, [255, 255, 255, 255]))
            },
        )
        .await;

    assert!(
        post_capture_yielded.load(Ordering::SeqCst),
        "ocr_result_from_web_bitmap after CDP capture must run on spawn_blocking, not inline on the async worker"
    );
    if let Ok(ocr) = result {
        assert_eq!(
            ocr.region,
            Rect {
                x: 0,
                y: 0,
                w: 64,
                h: 64
            }
        );
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn must_fail_capture_cdp_target_screenshot_offloads_pre_and_post_capture_work_to_blocking_pool()
-> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let (session, hwnd, target) = ("i175-shot-session", 0x1753, "TARGET175SHOT");
    seed_cdp_owner(&service, session, hwnd, target, "http://127.0.0.1:9222")?;
    let output_path = dir.path().join("cdp_shot_175.png");
    let params = CaptureScreenshotParams {
        path: output_path.display().to_string(),
        region: None,
        window_hwnd: None,
        overwrite: true,
        max_pixels: None,
        max_long_edge: Some(16),
    };
    let pre_yielded = Arc::new(AtomicBool::new(false));
    let pre_flag = Arc::clone(&pre_yielded);
    let _pre_probe = tokio::spawn(async move {
        pre_flag.store(true, Ordering::SeqCst);
    });
    let post_yielded = Arc::new(AtomicBool::new(false));
    let post_flag = Arc::clone(&post_yielded);

    let response = service
        .capture_cdp_target_screenshot_to_file_with_capture(
            &params,
            session,
            hwnd,
            target,
            move |_endpoint, _target_id, _region| async move {
                assert!(
                    pre_yielded.load(Ordering::SeqCst),
                    "pre-capture endpoint/path/UIA setup must run on spawn_blocking before CDP capture"
                );
                tokio::spawn(async move {
                    post_flag.store(true, Ordering::SeqCst);
                });
                Ok(solid_bgra(32, 32, [32, 64, 192, 255]))
            },
        )
        .await
        .map_err(|err| anyhow::anyhow!("capture_cdp_target_screenshot_to_file failed: {err:?}"))?;

    assert!(
        post_yielded.load(Ordering::SeqCst),
        "post-capture downscale, PNG encode, and file write/readback must run on spawn_blocking"
    );
    assert_eq!(response.format, CaptureScreenshotFormat::Png);
    assert_eq!(
        response.capture_backend,
        "raw_cdp_page_capture_screenshot_bgra"
    );
    assert_eq!((response.width, response.height), (16, 16));
    assert_eq!((response.native_width, response.native_height), (32, 32));
    assert!((response.scale - 0.5).abs() < f64::EPSILON);
    assert_eq!(
        std::fs::metadata(&output_path)?.len(),
        response.bytes_written
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn must_pass_capture_cdp_target_screenshot_rejects_mismatched_owner_window_without_writing_file()
-> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let service = service_with_temp_db(dir.path())?;
    let (session, owner_hwnd, wrong_hwnd, target) =
        ("i175-mismatch-session", 0x1754, 0x1755, "TARGET175MISMATCH");
    seed_cdp_owner(
        &service,
        session,
        owner_hwnd,
        target,
        "http://127.0.0.1:9222",
    )?;
    let output_path = dir.path().join("mismatched_175.png");
    let params = CaptureScreenshotParams {
        path: output_path.display().to_string(),
        region: None,
        window_hwnd: None,
        overwrite: true,
        max_pixels: None,
        max_long_edge: None,
    };

    let err = service
        .capture_cdp_target_screenshot_to_file(&params, session, wrong_hwnd, target)
        .await
        .expect_err("bound window mismatching CDP owner window must be refused");

    assert_eq!(error_code(&err), Some(error_codes::ACTION_TARGET_INVALID));
    assert!(
        !output_path.exists(),
        "refused screenshot must not create output file"
    );
    Ok(())
}
