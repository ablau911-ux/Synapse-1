//! criteria_b2.md:14. Guard defect: RGB=0,A=255 passes is_all_zero_bgra;
//! native screenshot publication passes reject_blank=false. Exercise tools/call,
//! not either helper. WinForms live repro returned a typed capture error here;
//! this test isolates the confirmed blank-publication hole from that OS failure.
#![cfg(windows)]

mod g4_support;
mod g4_window;
#[allow(dead_code)]
mod support;

use g4_window::{OwnedWindow, Placement};
use image::{AnimationDecoder as _, GenericImageView as _};
use serde_json::json;

#[tokio::test]
async fn must_pass_genuine_black_window_region_is_published_278() -> anyhow::Result<()> {
    let daemon = g4_support::plain_daemon().await?;
    let mut client = g4_support::normal_session(&daemon, "black-region-278").await?;
    let target = OwnedWindow::open(Placement {
        x: 80,
        y: 80,
        w: 240,
        h: 160,
        topmost: true,
    });
    target.paint_black_region();
    let control_path = daemon.home().join("black-region-control.png");
    client
        .tools_call(
            "screenshot",
            json!({"path":control_path,"window_hwnd":target.hwnd()}),
        )
        .await?;
    let control = image::open(&control_path)?.to_rgba8();
    assert!(
        control.pixels().any(|p| p.0[..3] != [0, 0, 0]),
        "whole-window must be rendered"
    );
    assert!(
        control
            .view(48, 48, 64, 48)
            .pixels()
            .all(|(_, _, p)| p.0[..3] == [0, 0, 0]),
        "real WGC black patch precondition"
    );
    let path = daemon.home().join("valid-black-region.png");
    assert!(!path.exists());
    let result = client
        .tools_call(
            "screenshot",
            json!({
                "path":path,"window_hwnd":target.hwnd(),"region":{"x":48,"y":48,"w":64,"h":48}
            }),
        )
        .await;
    assert!(
        result.is_ok(),
        "genuine black crop must succeed: {result:?}"
    );
    let crop = image::open(&path)?.to_rgba8();
    assert_eq!(crop.dimensions(), (64, 48));
    assert!(crop.pixels().all(|p| p.0[..3] == [0, 0, 0]));
    // Same real facade must survive a temporary all-black fade in its window.
    // Keep colored frames on both sides, then decode the published artifact.
    let gif = daemon.home().join("black-fade.gif");
    let recording = client.tools_call(
        "screenshot",
        json!({
            "operation":"gif","path":gif,"window_hwnd":target.hwnd(),
            "duration_ms":2400,"interval_ms":200
        }),
    );
    let fade = async {
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        target.paint_black();
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        target.paint_black_region();
    };
    let (recorded, ()) = tokio::join!(recording, fade);
    recorded?;
    let decoder =
        image::codecs::gif::GifDecoder::new(std::io::BufReader::new(std::fs::File::open(&gif)?))?;
    assert!(
        !decoder.into_frames().collect_frames()?.is_empty(),
        "fade must not discard the recording"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_black_region_read_text_reports_ocr_no_text_278() -> anyhow::Result<()> {
    let daemon = g4_support::plain_daemon().await?;
    let mut client = g4_support::normal_session(&daemon, "black-region-ocr-278").await?;
    let target = OwnedWindow::open(Placement {
        x: 80,
        y: 80,
        w: 240,
        h: 160,
        topmost: true,
    });
    target.paint_black_region();
    let control_path = daemon.home().join("ocr-black-region-control.png");
    client
        .tools_call(
            "screenshot",
            json!({"path":control_path,"window_hwnd":target.hwnd()}),
        )
        .await?;
    let control = image::open(&control_path)?.to_rgba8();
    assert!(control.pixels().any(|p| p.0[..3] != [0, 0, 0]));
    assert!(
        control
            .view(48, 48, 64, 48)
            .pixels()
            .all(|(_, _, p)| p.0[..3] == [0, 0, 0])
    );
    let error = client
        .tools_call_error(
            "read_text",
            json!({
                "window_hwnd":target.hwnd(),"region":{"x":48,"y":48,"w":64,"h":48},"backend":"winrt"
            }),
        )
        .await?;
    assert_eq!(
        error["data"]["code"], "OCR_NO_TEXT",
        "valid black crop must reach OCR: {error}"
    );
    drop(daemon.stop().await?);
    Ok(())
}

#[tokio::test]
async fn must_fail_opaque_black_target_is_not_published_as_capture_success_278()
-> anyhow::Result<()> {
    let daemon = g4_support::plain_daemon().await?;
    let mut client = g4_support::normal_session(&daemon, "opaque-black-278").await?;
    let target = OwnedWindow::open(Placement {
        x: 80,
        y: 80,
        w: 240,
        h: 160,
        topmost: true,
    });
    let control_path = daemon.home().join("color-control.png");
    client
        .tools_call(
            "screenshot",
            json!({
                "path":control_path,"window_hwnd":target.hwnd(),
            }),
        )
        .await?;
    let control = image::open(&control_path)?.to_rgba8();
    assert!(
        control.pixels().any(|p| p.0[..3] != [0, 0, 0]),
        "colored WGC precondition"
    );
    target.paint_black();
    let path = daemon.home().join("black-must-not-publish.png");
    assert!(target.is_alive() && !path.exists());
    let result = client
        .tools_call_error(
            "screenshot",
            json!({
                "path":path,"window_hwnd":target.hwnd(),
            }),
        )
        .await;
    // Separate artifact readback, even when today's handler reports success.
    if path.exists() {
        let image = image::open(&path)?.to_rgba8();
        assert!(
            image.pixels().all(|p| p.0[..3] == [0, 0, 0]),
            "fixture did not settle to black; this is not the intended #278 failure"
        );
        panic!(
            "#278: real screenshot handler published all-black RGB as success (opaque alpha escaped guard)"
        );
    }
    let error = result?;
    assert!(
        error["data"]["code"]
            .as_str()
            .is_some_and(|c| c.starts_with("CAPTURE_")),
        "typed capture error: {error}"
    );
    drop(daemon.stop().await?);
    Ok(())
}
