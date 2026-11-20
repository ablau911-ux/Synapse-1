//! #103: all still routes share this request-owned staging/commit boundary.
use super::*;
use tokio_util::sync::CancellationToken;

fn params(path: &Path, overwrite: bool) -> CaptureScreenshotParams {
    CaptureScreenshotParams {
        path: path.to_string_lossy().into_owned(),
        region: None,
        window_hwnd: None,
        overwrite,
        max_pixels: None,
        max_long_edge: None,
    }
}

fn stage(params: &CaptureScreenshotParams) -> CaptureScreenshotResponse {
    let bitmap = synapse_capture::CapturedBgraBitmap {
        region: Rect {
            x: 0,
            y: 0,
            w: 2,
            h: 1,
        },
        width: 2,
        height: 1,
        bytes: vec![0, 0, 255, 255, 0, 0, 255, 255],
    };
    write_screenshot_bitmap(
        params,
        PathBuf::from(&params.path),
        CaptureScreenshotFormat::Png,
        bitmap,
        "synthetic_bgra",
        String::new(),
        None,
        None,
    )
    .expect("real PNG encode/write")
}

#[test]
fn must_fail_cancelled_still_commit_publishes_no_file_and_removes_owned_staging() {
    let dir = tempfile::tempdir().expect("directory");
    let final_path = dir.path().join("cancel-103.png");
    assert!(!final_path.exists());
    let (publication, staged) =
        ScreenshotPublication::prepare(params(&final_path, false)).expect("prepare");
    let response = stage(&staged);
    let staging_path = PathBuf::from(&staged.path);
    assert_eq!(image::open(&staging_path).expect("staged PNG").width(), 2);
    assert!(!final_path.exists());
    let token = CancellationToken::new();
    token.cancel();
    assert!(token.is_cancelled());
    let error = publication
        .publish(response, &token)
        .expect_err("cancel before commit");
    assert_eq!(error.code, ErrorCode(-32800));
    assert_eq!(error.data.as_ref().unwrap()["code"], "REQUEST_CANCELLED");
    assert!(!final_path.exists());
    assert!(!staging_path.exists());
    assert_eq!(
        std::fs::read_dir(dir.path())
            .expect("read directory")
            .count(),
        0
    );
}

#[test]
fn must_fail_cancelled_overwrite_keeps_existing_final_bytes() {
    let dir = tempfile::tempdir().expect("directory");
    let final_path = dir.path().join("existing-103.png");
    std::fs::write(&final_path, b"caller-owned-original").expect("original");
    assert_eq!(
        std::fs::read(&final_path).expect("before"),
        b"caller-owned-original"
    );
    let (publication, staged) =
        ScreenshotPublication::prepare(params(&final_path, true)).expect("prepare");
    let response = stage(&staged);
    let token = CancellationToken::new();
    token.cancel();
    let error = publication
        .publish(response, &token)
        .expect_err("cancel overwrite");
    assert_eq!(error.data.as_ref().unwrap()["code"], "REQUEST_CANCELLED");
    assert_eq!(
        std::fs::read(&final_path).expect("after"),
        b"caller-owned-original"
    );
    assert!(!Path::new(&staged.path).exists());
}

#[test]
fn must_pass_committed_still_survives_later_cancellation_with_exact_pixels_and_hash() {
    let dir = tempfile::tempdir().expect("directory");
    let final_path = dir.path().join("commit-103.png");
    let (publication, staged) =
        ScreenshotPublication::prepare(params(&final_path, false)).expect("prepare");
    let response = stage(&staged);
    let token = CancellationToken::new();
    assert!(!token.is_cancelled());
    let response = publication.publish(response, &token).expect("commit");
    let committed = std::fs::read(&final_path).expect("committed file");
    token.cancel();
    assert_eq!(std::fs::read(&final_path).expect("after cancel"), committed);
    let pixels = image::open(&final_path).expect("PNG").to_rgba8();
    assert_eq!(pixels.dimensions(), (2, 1));
    assert_eq!(pixels.into_raw(), vec![255, 0, 0, 255, 255, 0, 0, 255]);
    assert_eq!(response.path, final_path.to_string_lossy());
    assert_eq!(
        response.bitmap_sha256,
        sha256_file(&final_path).expect("separate hash").1
    );
    assert!(!Path::new(&staged.path).exists());
}
