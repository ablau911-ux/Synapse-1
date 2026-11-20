use std::fmt::Write;

use sha2::{Digest, Sha256};
use synapse_action::sample_curve;
use synapse_core::{AimCurve, AimNaturalParams, Point};

#[test]
fn must_pass_natural_fast_seed_42_hash_matches_snapshot() {
    let start = Point { x: 0, y: 0 };
    let end = Point { x: 100, y: 100 };
    let curve = AimCurve::Natural {
        params: AimNaturalParams {
            seed: Some(42),
            ..AimNaturalParams::FAST
        },
    };

    let samples = sample_curve(&curve, start, end, 50, None);
    let serialized = serialize_samples(&samples);
    let sha256 = sha256_hex(&serialized);

    println!(
        "readback=curve_natural_seed_42 edge=happy before=start:(0,0) end:(100,100) duration_ms:50 seed:42 after_samples={samples:?} after_serialized_len={} result_value=sha256:{sha256}",
        serialized.len()
    );
    assert_eq!(samples.first().copied(), Some(start));
    assert_eq!(samples.last().copied(), Some(end));

    let params_seed_samples = sample_curve(&curve, start, end, 50, None);
    let explicit_seed_samples = sample_curve(
        &AimCurve::Natural {
            params: AimNaturalParams::FAST,
        },
        start,
        end,
        50,
        Some(42),
    );
    println!(
        "readback=curve_natural_seed_42 edge=params_seed_vs_explicit before=params_seed:42 after=params_eq_explicit:{} result_value=pass",
        params_seed_samples == explicit_seed_samples
    );
    assert_eq!(params_seed_samples, explicit_seed_samples);

    let overridden_samples = sample_curve(&curve, start, end, 50, Some(43));
    let overridden_sha256 = sha256_hex(&serialize_samples(&overridden_samples));
    println!(
        "readback=curve_natural_seed_42 edge=override_seed before=params_seed:42 override_seed:43 after_sha256:{overridden_sha256} result_value=different:{}",
        overridden_sha256 != sha256
    );
    assert_ne!(overridden_sha256, sha256);

    let zero_duration = sample_curve(&curve, start, end, 0, None);
    println!(
        "readback=curve_natural_seed_42 edge=zero_duration before=duration_ms:0 after_len={} after_first={:?} after_last={:?} result_value=pass",
        zero_duration.len(),
        zero_duration.first(),
        zero_duration.last()
    );
    assert_eq!(zero_duration.len(), 8);
    assert_eq!(zero_duration.first().copied(), Some(start));
    assert_eq!(zero_duration.last().copied(), Some(end));

    insta::with_settings!({ prepend_module_to_snapshot => false }, {
        insta::assert_snapshot!("curve_natural_seed_42", snapshot_text(&sha256, &samples));
    });
}

#[test]
fn must_fail_natural_seed_42_non_finite_and_negative_params_are_neutralised() {
    let start = Point { x: 0, y: 0 };
    let end = Point { x: 100, y: 100 };
    let hostile = AimNaturalParams {
        control_point_jitter: f32::NAN,
        tremor_stddev_px: f32::INFINITY,
        overshoot_prob: f32::NAN,
        overshoot_factor_range: (f32::NEG_INFINITY, f32::NAN),
        timing_stddev_ms: -5.0,
        seed: Some(42),
        ..AimNaturalParams::FAST
    };
    let neutral = AimNaturalParams {
        control_point_jitter: 0.0,
        tremor_stddev_px: 0.0,
        overshoot_prob: 0.0,
        timing_stddev_ms: 0.0,
        ..hostile
    };

    let hostile_samples =
        sample_curve(&AimCurve::Natural { params: hostile }, start, end, 50, None);
    let neutral_samples =
        sample_curve(&AimCurve::Natural { params: neutral }, start, end, 50, None);
    println!(
        "readback=curve_natural_seed_42 edge=hostile_params before=params:{hostile:?} after={hostile_samples:?} result_value=eq_neutral:{}",
        hostile_samples == neutral_samples
    );

    // Non-finite or negative noise parameters must act as "no noise", never
    // as NaN/inf coordinates, an overshoot, or a different sample count.
    assert_eq!(hostile_samples, neutral_samples);
    assert_eq!(hostile_samples.len(), neutral_samples.len());
    assert_eq!(hostile_samples.first().copied(), Some(start));
    assert_eq!(hostile_samples.last().copied(), Some(end));
    assert!(
        hostile_samples
            .iter()
            .all(|point| (0..=100).contains(&point.x) && (0..=100).contains(&point.y)),
        "hostile params must not push samples outside the start..end box: {hostile_samples:?}"
    );
}

#[test]
fn must_fail_natural_seed_42_extreme_coordinates_do_not_overflow() {
    let start = Point {
        x: i32::MIN,
        y: i32::MAX,
    };
    let end = Point {
        x: i32::MAX,
        y: i32::MIN,
    };
    let curve = AimCurve::Natural {
        params: AimNaturalParams {
            seed: Some(42),
            overshoot_prob: 1.0,
            ..AimNaturalParams::FAST
        },
    };

    let samples = sample_curve(&curve, start, end, 50, None);
    let again = sample_curve(&curve, start, end, 50, None);
    println!(
        "readback=curve_natural_seed_42 edge=extreme_coords before=start:{start:?} end:{end:?} after_len={} result_value=first:{:?} last:{:?}",
        samples.len(),
        samples.first(),
        samples.last()
    );

    assert_eq!(samples, again);
    assert_eq!(samples.first().copied(), Some(start));
    assert_eq!(samples.last().copied(), Some(end));
}

fn serialize_samples(samples: &[Point]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 8);
    for sample in samples {
        bytes.extend_from_slice(&sample.x.to_le_bytes());
        bytes.extend_from_slice(&sample.y.to_le_bytes());
    }
    bytes
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push(nibble_to_hex(byte >> 4));
        hex.push(nibble_to_hex(byte & 0x0f));
    }
    hex
}

fn nibble_to_hex(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        10..=15 => char::from(b'a' + (nibble - 10)),
        _ => '?',
    }
}

fn snapshot_text(sha256: &str, samples: &[Point]) -> String {
    let mut text = format!("sha256: {sha256}\nsamples:");
    for sample in samples {
        assert!(
            write!(&mut text, "\n  - x: {}, y: {}", sample.x, sample.y).is_ok(),
            "writing curve snapshot text to String failed"
        );
    }
    text
}
