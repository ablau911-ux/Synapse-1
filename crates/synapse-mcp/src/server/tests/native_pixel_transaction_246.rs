//! NEEDS-SEAM: extract the actual capture_window_pixel_evidence body into
//! capture_window_pixel_evidence_with (signature in FU6/findings.md).
//! Native caller delegates with reject_blank=false; passive fallback policy
//! uses true. Do not implement a separate test-only transaction.
use super::*;
use std::cell::Cell;
use synapse_capture::CapturedBgraBitmap;

fn region() -> Rect {
    Rect {
        x: 0,
        y: 0,
        w: 32,
        h: 32,
    }
}

fn frame(uniform: bool) -> CapturedWindowBgraBitmap {
    let mut bytes = vec![0; 32 * 32 * 4];
    for (index, pixel) in bytes.chunks_exact_mut(4).enumerate() {
        let value = if uniform || index % 2 == 0 { 0 } else { 255 };
        pixel.copy_from_slice(&[value, value, value, 255]);
    }
    CapturedWindowBgraBitmap {
        bitmap: CapturedBgraBitmap {
            region: region(),
            width: 32,
            height: 32,
            bytes,
        },
        capture_backend: "graphics_capture_window_bgra",
        capture_attempts: 1,
        capture_retry_count: 0,
        capture_elapsed_ms: 0,
        capture_retry_backoff_ms: 0,
    }
}

fn assert_refused(evidence: WindowPixelEvidence, code: &str) {
    for signature in [evidence.window, evidence.point.expect("point requested")] {
        assert_eq!(signature.status, "unavailable");
        assert_eq!(
            signature.bitmap_sha256, None,
            "refused frame must publish no digest"
        );
        assert!(
            signature
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains(code)),
            "expected {code}, got {:?}",
            signature.detail
        );
    }
}

#[test]
fn must_pass_stable_hwnd_pid_publishes_exact_pixel_signature() {
    let reads = Cell::new(0);
    let captures = Cell::new(0);
    let captured = frame(false);
    assert_eq!(captured.bitmap.bytes.len(), 4096);
    assert_eq!(reads.get(), 0);
    assert_eq!(captures.get(), 0);
    let mut hasher = Sha256::new();
    hasher.update(32u32.to_le_bytes());
    hasher.update(32u32.to_le_bytes());
    hasher.update(&captured.bitmap.bytes);
    let digest = hex_encode(&hasher.finalize());
    let evidence = capture_window_pixel_evidence_with(
        246,
        region(),
        None,
        false,
        || {
            reads.set(reads.get() + 1);
            Ok((246, 10))
        },
        || {
            captures.set(captures.get() + 1);
            Ok((captured, Some(Point { x: 0, y: 0 })))
        },
    );
    assert_eq!(
        reads.get(),
        3,
        "initial identity plus transaction pre/post reads"
    );
    assert_eq!(captures.get(), 1);
    assert_eq!(evidence.window.status, "ok");
    assert_eq!(evidence.window.region, region());
    assert_eq!(evidence.window.bitmap_sha256, Some(digest));
    assert_eq!(
        evidence.window.detail.as_deref(),
        Some("captured 32x32 BGRA bytes=4096 backend=graphics_capture_window_bgra")
    );
    assert_eq!(evidence.point, None);
}

#[test]
fn must_fail_changed_hwnd_or_pid_between_reads_publishes_nothing() {
    for changed in [(247, 10), (246, 11)] {
        let identity = Cell::new((246, 10));
        let captures = Cell::new(0);
        assert_eq!(identity.get(), (246, 10));
        assert_eq!(captures.get(), 0);
        let evidence = capture_window_pixel_evidence_with(
            246,
            region(),
            Some(Point { x: 16, y: 16 }),
            false,
            || Ok(identity.get()),
            || {
                captures.set(captures.get() + 1);
                identity.set(changed);
                Ok((frame(false), Some(Point { x: 0, y: 0 })))
            },
        );
        assert_eq!(
            identity.get(),
            changed,
            "capture changed actual injected identity"
        );
        assert_eq!(captures.get(), 1);
        assert_refused(evidence, synapse_core::error_codes::CAPTURE_TARGET_INVALID);
    }
}

#[test]
fn must_fail_uniform_or_near_uniform_fallback_publishes_nothing() {
    for one_channel_step in [false, true] {
        let mut captured = frame(true);
        if one_channel_step {
            captured.bitmap.bytes[0] = 1;
        }
        assert_eq!(captured.bitmap.bytes[0], u8::from(one_channel_step));
        assert_eq!(captured.bitmap.bytes[4], 0);
        let captures = Cell::new(0);
        assert_eq!(captures.get(), 0);
        let evidence = capture_window_pixel_evidence_with(
            246,
            region(),
            Some(Point { x: 16, y: 16 }),
            true,
            || Ok((246, 10)),
            || {
                captures.set(captures.get() + 1);
                Ok((captured, Some(Point { x: 0, y: 0 })))
            },
        );
        assert_eq!(captures.get(), 1);
        assert_refused(
            evidence,
            synapse_core::error_codes::CAPTURE_FRAME_UNAVAILABLE,
        );
    }
}

#[test]
fn must_pass_uniform_native_frame_keeps_existing_signature_policy() {
    let captured = frame(true);
    assert_eq!(captured.bitmap.bytes[0..8], [0, 0, 0, 255, 0, 0, 0, 255]);
    let evidence = capture_window_pixel_evidence_with(
        246,
        region(),
        None,
        false,
        || Ok((246, 10)),
        || Ok((captured, Some(Point { x: 0, y: 0 }))),
    );
    assert_eq!(evidence.window.status, "ok");
    assert_eq!(evidence.window.region, region());
    assert_eq!(
        evidence.window.bitmap_sha256.as_deref(),
        Some("291a6f0d96245b2139590ce9cb0704e9d1233da9d9f5918506809ec0b6a377ea")
    );
}
