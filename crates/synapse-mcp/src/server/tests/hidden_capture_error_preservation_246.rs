//! #246: exercise the actual hidden fallback and PiP worker, without a daemon.
//! Full native screenshot/GIF injection gaps are recorded in TF7/findings.md.
use super::*;
use std::cell::Cell;

fn params(path: &Path) -> CaptureScreenshotParams {
    CaptureScreenshotParams {
        path: path.to_string_lossy().into_owned(),
        region: None,
        window_hwnd: Some(246),
        overwrite: false,
        max_pixels: None,
        max_long_edge: None,
    }
}

fn seed_session(service: &SynapseService, id: &str) -> anyhow::Result<()> {
    use rmcp::model::{ClientCapabilities, Implementation, InitializeRequestParams};
    let state = rmcp::transport::streamable_http_server::session::SessionState::new(
        InitializeRequestParams::new(
            ClientCapabilities::default(),
            Implementation::new("capture-error-246", "test"),
        ),
    );
    service
        .session_registry_ref()
        .lock()
        .map_err(|_| anyhow::anyhow!("session registry poisoned"))?
        .record_initialized(
            id,
            &state,
            "http",
            super::super::session_registry::unix_time_ms_now(),
        );
    assert!(service.session_status_impl(id)?.found);
    assert!(service.session_hidden_desktop_readback(id)?.is_none());
    Ok(())
}

fn code(error: &ErrorData) -> Option<&str> {
    error.data.as_ref()?.get("code")?.as_str()
}

#[test]
fn must_fail_native_capture_without_hidden_desktop_preserves_original_error_and_diagnostics()
-> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let service = SynapseService::try_new_isolated_for_tests(root.path())?;
    seed_session(&service, "normal-246")?;
    let output = root.path().join("native-246.png");
    let params = params(&output);
    let region = Rect {
        x: 0,
        y: 0,
        w: 320,
        h: 200,
    };
    let context = ForegroundContext {
        hwnd: 246,
        pid: 246,
        process_name: "capture-246.exe".into(),
        process_path: "C:\\capture-246.exe".into(),
        window_title: "Native capture 246".into(),
        window_bounds: region,
        monitor_index: 0,
        dpi_scale: 1.0,
        profile_id: None,
        steam_appid: None,
        is_fullscreen: false,
        is_dwm_composed: true,
    };
    for session in [Some("normal-246"), None] {
        for native_error in [
            synapse_capture::CaptureError::WgcFrameTimeout {
                detail: "native timeout 246".into(),
            },
            synapse_capture::CaptureError::PrintWindowDisabled {
                detail: "native disabled 246".into(),
            },
            synapse_capture::CaptureError::TargetInvalid {
                detail: "native bounds 246".into(),
            },
        ] {
            let expected_code = native_error.code();
            // The same diagnostic constructor used by the real native capture failure.
            let original =
                capture_target_window_screenshot_error(246, region, Some(&context), native_error);
            let expected = serde_json::to_value(&original)?;
            assert_eq!(expected["data"]["target_pid"], 246);
            assert_eq!(expected["data"]["target_title"], "Native capture 246");
            assert!(expected["data"]["same_pid_visible_window_candidates"].is_array());
            assert!(expected["data"]["recommended_next_action"].is_string());
            assert!(!output.exists());
            let error = service
                .capture_hidden_desktop_screenshot_to_file(&params, session, 246, original)
                .err()
                .ok_or_else(|| anyhow::anyhow!("failed native capture unexpectedly published"))?;
            assert_eq!(
                code(&error),
                Some(expected_code),
                "#246: absent hidden desktop must not mask native failure"
            );
            assert_eq!(
                serde_json::to_value(error)?,
                expected,
                "#246: preserve entire original diagnostic envelope"
            );
            assert!(!output.exists(), "failed fallback must publish no file");
        }
    }
    Ok(())
}

#[test]
fn must_fail_hidden_desktop_pip_frame_not_found_keeps_target_window_not_found() -> anyhow::Result<()>
{
    let root = tempfile::tempdir()?;
    let service = SynapseService::try_new_isolated_for_tests(root.path())?;
    seed_session(&service, "pip-246")?;
    let output = root.path().join("pip-246.png");
    let params = HiddenDesktopPipFrameParams {
        watched_session_id: Some("pip-246".into()),
        window_hwnd: 246,
        path: output.to_string_lossy().into_owned(),
        region: None,
        overwrite: false,
    };
    assert!(!output.exists());
    let error = service
        .hidden_desktop_pip_frame_to_file(&params, "pip-246")
        .err()
        .ok_or_else(|| anyhow::anyhow!("missing PiP window unexpectedly captured"))?;
    assert_eq!(
        code(&error),
        Some(error_codes::TARGET_WINDOW_NOT_FOUND),
        "#246: PiP not-found is not identity drift"
    );
    assert!(
        error
            .message
            .contains("hidden_desktop_pip_frame target hwnd 0xf6 was not found")
    );
    assert!(error.message.contains("pip-246"));
    assert!(!output.exists());
    assert!(
        service
            .session_hidden_desktop_readback("pip-246")?
            .is_none()
    );
    assert!(service.session_status_impl("pip-246")?.found);
    Ok(())
}

#[test]
fn must_pass_identity_transaction_only_synthesizes_target_invalid_for_identity_drift()
-> anyhow::Result<()> {
    // Supporting transaction coverage; real native pixel seam coverage already
    // exists in native_pixel_transaction_246.rs. No helper-only entry-point claim.
    for after in [(246, 10), (247, 10), (246, 11)] {
        let identity = Cell::new((246, 10));
        let reads = Cell::new(0);
        let publications = Cell::new(0);
        let result = capture_with_verified_identity(
            (246, 10),
            false,
            || {
                reads.set(reads.get() + 1);
                Ok(identity.get())
            },
            || {
                identity.set(after);
                Ok(synapse_capture::CapturedBgraBitmap {
                    region: Rect {
                        x: 0,
                        y: 0,
                        w: 1,
                        h: 1,
                    },
                    width: 1,
                    height: 1,
                    bytes: vec![0, 0, 0, 255],
                })
            },
            |frame| {
                publications.set(publications.get() + 1);
                Ok(json!(frame.bytes))
            },
        );
        assert_eq!(reads.get(), 2);
        assert_eq!(identity.get(), after);
        if after == (246, 10) {
            assert_eq!(result?, json!([0, 0, 0, 255]));
            assert_eq!(
                publications.get(),
                1,
                "stable native flat frame remains publishable"
            );
        } else {
            let error = result
                .err()
                .ok_or_else(|| anyhow::anyhow!("identity drift published"))?;
            assert_eq!(code(&error), Some(error_codes::CAPTURE_TARGET_INVALID));
            assert_eq!(publications.get(), 0);
        }
    }
    Ok(())
}
