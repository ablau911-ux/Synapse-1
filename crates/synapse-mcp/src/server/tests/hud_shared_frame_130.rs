//! #130 G6b: drive real HUD population through existing observation-frame cache.
use super::*;
use std::sync::Arc;
use synapse_core::{ForegroundContext, HudParser, HudRegion, HudValue};

fn profile() -> Profile {
    let mut profile = synapse_profiles::parse_profile_bytes(
        "luanti.minetest.toml",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../synapse-profiles/profiles/luanti.minetest.toml"
        )),
        SystemTime::now(),
        synapse_profiles::ScreenBounds::default(),
    )
    .expect("bundled profile")
    .profile;
    profile.hud = vec![HudFieldSpec {
        name: "frame_contrast".into(),
        region: HudRegion::FractionOfWindow {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        },
        extractor: HudExtractor::ColorRatio {
            sample_points: vec![],
            mapping: "luma_stddev_0_1".into(),
        },
        parser: HudParser::Number,
        confidence_threshold: 0.0,
    }];
    profile
}

fn input() -> synapse_perception::ObservationInput {
    synapse_perception::ObservationInput::new(ForegroundContext {
        hwnd: 130,
        pid: 130,
        process_name: "fixture.exe".into(),
        process_path: String::new(),
        window_title: "#130 cached-frame fixture".into(),
        window_bounds: Rect {
            x: -30000,
            y: -30000,
            w: 4,
            h: 2,
        },
        monitor_index: 0,
        dpi_scale: 1.0,
        profile_id: None,
        steam_appid: None,
        is_fullscreen: false,
        is_dwm_composed: true,
    })
}

fn frame(input: &synapse_perception::ObservationInput) -> Arc<synapse_capture::CapturedBgraBitmap> {
    // Two black columns + two white columns: population stddev=127.5; normalized /128.
    Arc::new(synapse_capture::CapturedBgraBitmap {
        region: input.foreground.window_bounds,
        width: 4,
        height: 2,
        bytes: [
            0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255,
        ]
        .repeat(2),
    })
}

#[test]
fn must_pass_hud_reads_cached_target_bitmap_even_outside_desktop() {
    let mut input = input();
    let frame = frame(&input);
    input
        .target_window_frame
        .set((130, 130, Some(frame.clone())))
        .expect("cache initially empty");
    assert!(Arc::ptr_eq(
        &crate::m1::browser_ocr_target_frame(&input).expect("cached frame"),
        &frame
    ));
    populate_profile_hud(&mut input, &profile(), Path::new("."));
    assert!(input.hud.errors.is_empty());
    let reading = &input.hud.by_name["frame_contrast"];
    assert_eq!(reading.parsed, HudValue::Number(0.996094));
    assert_eq!(reading.raw_text, "0.996094 region=4x2@-30000,-30000");
    assert_eq!(reading.stale_ms, 0);
    assert!(Arc::ptr_eq(
        &input
            .target_window_frame
            .get()
            .expect("cache retained")
            .2
            .clone()
            .expect("frame"),
        &frame
    ));
}

#[test]
fn must_pass_empty_hud_never_initializes_frame_capture() {
    let mut input = input();
    let mut profile = profile();
    profile.hud.clear();
    assert!(input.target_window_frame.get().is_none());
    populate_profile_hud(&mut input, &profile, Path::new("."));
    assert!(input.target_window_frame.get().is_none());
    assert!(input.hud.by_name.is_empty());
    assert!(input.hud.errors.is_empty());
}

#[test]
fn must_fail_missing_or_wrong_identity_frame_removes_stale_reading() {
    for wrong_identity in [false, true] {
        let mut input = input();
        let cached = wrong_identity.then(|| frame(&input));
        input
            .target_window_frame
            .set((if wrong_identity { 999 } else { 130 }, 130, cached))
            .expect("cache initially empty");
        input.hud.by_name.insert(
            "frame_contrast".into(),
            HudReading {
                raw_text: "old".into(),
                parsed: HudValue::Number(1.0),
                confidence: 1.0,
                confidence_source: synapse_core::ConfidenceSource::Synthetic,
                stale_ms: 0,
            },
        );
        assert!(input.hud.by_name.contains_key("frame_contrast"));
        populate_profile_hud(&mut input, &profile(), Path::new("."));
        assert!(!input.hud.by_name.contains_key("frame_contrast"));
        assert_eq!(
            input.hud.errors["frame_contrast"].code,
            "HUD_EXTRACTION_FAILED"
        );
        assert!(
            input.hud.errors["frame_contrast"]
                .detail
                .contains("target observation frame unavailable")
        );
        assert_eq!(
            input.target_window_frame.get().expect("cache retained").0,
            if wrong_identity { 999 } else { 130 }
        );
    }
}

#[test]
fn must_fail_hud_crop_outside_cached_frame_publishes_no_reading() {
    let mut input = input();
    input
        .target_window_frame
        .set((130, 130, Some(frame(&input))))
        .expect("cache initially empty");
    let mut profile = profile();
    profile.hud[0].region = HudRegion::Absolute {
        x: 20,
        y: 20,
        w: 2,
        h: 2,
    };
    populate_profile_hud(&mut input, &profile, Path::new("."));
    assert!(input.hud.by_name.is_empty());
    assert_eq!(
        input.hud.errors["frame_contrast"].code,
        "HUD_EXTRACTION_FAILED"
    );
}

// NEEDS-SEAM: real populate_profile_hud must delegate to:
// fn populate_profile_hud_with_ocr(
//   input: &mut ObservationInput, profile: &Profile, profile_dir: &Path,
//   ocr: impl Fn(Rect, u32, u32, &[u8]) -> PerceptionResult<Vec<TextRegion>>,
// )
// Pass read_text_from_bgra_bitmap in production. Same callback must back direct
// WinRT and template-fallback providers, after cropping the cached observation frame.
#[test]
fn must_pass_hud_text_and_template_fallback_ocr_receive_cached_pixels_and_origin() {
    use std::cell::Cell;
    for extractor in [
        HudExtractor::WinrtOcr,
        HudExtractor::TemplateMatch { templates: vec![] },
    ] {
        let mut input = input();
        let cached = frame(&input);
        input
            .target_window_frame
            .set((130, 130, Some(cached.clone())))
            .expect("empty cache");
        let mut profile = profile();
        profile.hud[0].extractor = extractor;
        let calls = Cell::new(0);
        populate_profile_hud_with_ocr(
            &mut input,
            &profile,
            Path::new("."),
            |region, w, h, bytes| {
                calls.set(calls.get() + 1);
                assert_eq!(region, cached.region);
                assert_eq!((w, h), (4, 2));
                assert_eq!(
                    bytes,
                    cached.bytes.as_slice(),
                    "OCR must receive this observation's pixels"
                );
                Ok(vec![TextRegion {
                    text: "130".into(),
                    bbox: region,
                    confidence: 1.0,
                    confidence_source: synapse_core::ConfidenceSource::Synthetic,
                }])
            },
        );
        assert_eq!(calls.get(), 1);
        assert!(input.hud.errors.is_empty());
        assert_eq!(
            input.hud.by_name["frame_contrast"].parsed,
            HudValue::Number(130.0)
        );
    }
}

#[test]
fn must_fail_unavailable_target_frame_never_calls_hud_ocr_or_retains_stale_text() {
    let mut input = input();
    input
        .target_window_frame
        .set((130, 130, None))
        .expect("empty cache");
    let mut profile = profile();
    profile.hud[0].extractor = HudExtractor::WinrtOcr;
    populate_profile_hud_with_ocr(&mut input, &profile, Path::new("."), |_, _, _, _| {
        panic!("no frame must mean no OCR, UIA lookup or desktop fallback")
    });
    assert!(input.hud.by_name.is_empty());
    assert_eq!(
        input.hud.errors["frame_contrast"].code,
        "HUD_EXTRACTION_FAILED"
    );
    assert!(
        input
            .target_window_frame
            .get()
            .expect("retained")
            .2
            .is_none()
    );
}
