//! NEEDS-SEAM in m1_tools: capture_with_verified_identity(expected:(i64,u32),
//! reject_blank:bool, identity:impl FnMut()->Result<(i64,u32),ErrorData>,
//! capture:impl FnOnce()->Result<synapse_capture::CapturedBgraBitmap,ErrorData>,
//! publish:impl FnOnce(synapse_capture::CapturedBgraBitmap)->Result<Value,ErrorData>)
//! -> Result<Value,ErrorData>.
//! Real hidden/native still and EACH GIF frame must run this transaction;
//! browser WGC fallback passes reject_blank=true. No test-only replacement.
use super::*;

fn bitmap(blank: bool) -> synapse_capture::CapturedBgraBitmap {
    let mut bytes = vec![0; 32 * 32 * 4];
    for (index, pixel) in bytes.chunks_exact_mut(4).enumerate() {
        let channel = if blank || index % 2 == 0 { 0 } else { 255 };
        pixel.copy_from_slice(&[channel, channel, channel, 255]);
    }
    synapse_capture::CapturedBgraBitmap {
        region: Rect {
            x: 0,
            y: 0,
            w: 32,
            h: 32,
        },
        width: 32,
        height: 32,
        bytes,
    }
}

fn error_code(error: &ErrorData) -> Option<&str> {
    error.data.as_ref()?.get("code")?.as_str()
}

#[test]
fn must_pass_stable_nonblank_target_publishes_exact_frame_once() {
    let dir = tempfile::tempdir().expect("fixture");
    let path = dir.path().join("published.bgra");
    assert!(!path.exists());
    let expected = bitmap(false).bytes;
    let mut reads = 0;
    let result = capture_with_verified_identity(
        (246, 10),
        true,
        || {
            reads += 1;
            Ok((246, 10))
        },
        || Ok(bitmap(false)),
        |frame| {
            std::fs::write(&path, frame.bytes).expect("publish fixture");
            Ok(json!({"path":path}))
        },
    )
    .expect("stable capture");
    assert_eq!(reads, 2);
    assert_eq!(result["path"], json!(path));
    assert_eq!(
        std::fs::read(path).expect("separate physical readback"),
        expected
    );
}

#[test]
fn must_fail_changed_hwnd_pid_or_initial_identity_never_publishes() {
    for (before, after) in [
        ((246, 10), (247, 10)),
        ((246, 10), (246, 11)),
        ((247, 10), (246, 10)),
    ] {
        let dir = tempfile::tempdir().expect("fixture");
        let path = dir.path().join("existing.bgra");
        std::fs::write(&path, b"KEEP_246").expect("seed");
        assert_eq!(std::fs::read(&path).expect("before"), b"KEEP_246");
        let mut reads = 0;
        let mut publications = 0;
        let result = capture_with_verified_identity(
            (246, 10),
            false,
            || {
                reads += 1;
                Ok(if reads == 1 { before } else { after })
            },
            || Ok(bitmap(false)),
            |_| {
                publications += 1;
                std::fs::write(&path, b"WRONG").expect("fixture");
                Ok(json!({}))
            },
        );
        assert_eq!(
            error_code(&result.expect_err("identity drift refused")),
            Some("CAPTURE_TARGET_INVALID")
        );
        assert_eq!(publications, 0);
        assert_eq!(std::fs::read(path).expect("after"), b"KEEP_246");
    }
}

#[test]
fn must_fail_uniform_and_single_outlier_fallback_frames_never_publish() {
    for near_uniform in [false, true] {
        let mut frame = bitmap(true);
        if near_uniform {
            frame.bytes[0] = 1;
        }
        let dir = tempfile::tempdir().expect("fixture");
        let path = dir.path().join("must-not-exist.png");
        assert!(!path.exists());
        let mut publications = 0;
        let result = capture_with_verified_identity(
            (246, 10),
            true,
            || Ok((246, 10)),
            || Ok(frame),
            |_| {
                publications += 1;
                std::fs::write(&path, b"WRONG").expect("fixture");
                Ok(json!({}))
            },
        );
        assert_eq!(
            error_code(&result.expect_err("blank fallback refused")),
            Some("CAPTURE_FRAME_UNAVAILABLE")
        );
        assert_eq!(publications, 0);
        assert!(!path.exists());
    }
}

#[test]
fn must_fail_capture_callers_cannot_skip_identity_transaction() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server");
    let stills = std::fs::read_to_string(root.join("m1_tools.rs")).expect("stills source");
    for name in [
        "capture_hidden_desktop_screenshot_to_file",
        "capture_target_window_screenshot_to_file",
    ] {
        // Search definition, not a call appearing earlier in the module.
        let marker = format!("fn {name}(");
        let body = stills
            .split(&marker)
            .nth(1)
            .expect("real capture caller")
            .split("\n    fn ")
            .next()
            .expect("method body")
            .split("\n    async fn ")
            .next()
            .expect("method body before async method")
            .split("\nfn ")
            .next()
            .expect("body");
        assert!(
            body.contains("capture_with_verified_identity("),
            "{name} bypasses identity check"
        );
    }
    let gif = std::fs::read_to_string(root.join("capture_gif.rs")).expect("GIF source");
    let frame_loop = gif
        .split("while started.elapsed()")
        .nth(1)
        .expect("GIF frame loop")
        .split("if frames.is_empty()")
        .next()
        .expect("frame section");
    assert!(
        frame_loop.contains("capture_with_verified_identity("),
        "each GIF frame needs transaction"
    );
}
