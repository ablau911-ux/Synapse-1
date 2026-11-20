use synapse_action::{HumanizeError, TimedPathPoint, humanize_timed_path};
use synapse_core::{HumanizeParams, PathPoint};

#[test]
fn must_pass_disabled_humanization_preserves_exact_timed_path()
-> Result<(), Box<dyn std::error::Error>> {
    let base = base_timed_path();
    let no_params = humanize_timed_path(&base, None)?;
    let zero_params = humanize_timed_path(
        &base,
        Some(HumanizeParams {
            tremor_base_stddev_px: 0.0,
            tremor_velocity_scale: 10.0,
            overshoot_prob: 0.0,
            overshoot_factor_range: (1.03, 1.12),
            micro_pause_prob: 0.0,
            micro_pause_ms_range: (15, 40),
            seed: Some(42),
        }),
    )?;

    println!(
        "readback=humanize edge=disabled before={base:?} after_none={no_params:?} after_zero={zero_params:?}"
    );
    assert_eq!(no_params, base);
    assert_eq!(zero_params, base);
    Ok(())
}

#[test]
fn must_pass_seeded_humanization_is_byte_stable_and_adds_correction()
-> Result<(), Box<dyn std::error::Error>> {
    let base = base_timed_path();
    let params = HumanizeParams {
        tremor_base_stddev_px: 0.25,
        tremor_velocity_scale: 2.0,
        overshoot_prob: 1.0,
        overshoot_factor_range: (1.10, 1.10),
        micro_pause_prob: 1.0,
        micro_pause_ms_range: (15, 15),
        seed: Some(42),
    };

    let first = humanize_timed_path(&base, Some(params))?;
    let second = humanize_timed_path(&base, Some(params))?;
    let rendered_first = format!("{first:?}");
    let rendered_second = format!("{second:?}");

    println!(
        "readback=humanize edge=seeded before=params:{params:?} after={rendered_first} result_value=len:{} deterministic:{}",
        first.len(),
        rendered_first == rendered_second
    );
    assert_eq!(rendered_first.as_bytes(), rendered_second.as_bytes());
    assert_eq!(
        first.last().map(|sample| sample.point),
        base.last().map(|sample| sample.point)
    );
    assert!(first.len() > base.len());
    assert!(timestamps_are_monotonic(&first));

    let max_x = first
        .iter()
        .map(|sample| sample.point.x)
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(max_x > base.last().expect("base has endpoint").point.x);

    // Pin the exact seeded output (tremor, 1.10 overshoot, 15 ms pause and the
    // correction), not only its shape, so any change to the humanizer shows.
    insta::with_settings!({ prepend_module_to_snapshot => false }, {
        insta::assert_snapshot!("humanize_seed_42", rendered_first);
    });
    Ok(())
}

#[test]
fn must_pass_tremor_amplitude_is_larger_at_lower_velocity() -> Result<(), Box<dyn std::error::Error>>
{
    let base = vec![
        timed(0.0, 0.0, 0.0, 0.0),
        timed(100.0, 1.0, 1.0, 0.0),
        timed(101.0, 101.0, 101.0, 0.0),
        timed(102.0, 201.0, 201.0, 0.0),
    ];
    let params = HumanizeParams {
        tremor_base_stddev_px: 1.0,
        tremor_velocity_scale: 25.0,
        overshoot_prob: 0.0,
        overshoot_factor_range: (1.03, 1.12),
        micro_pause_prob: 0.0,
        micro_pause_ms_range: (15, 40),
        seed: Some(11),
    };

    let humanized = humanize_timed_path(&base, Some(params))?;
    let low_velocity_delta = distance(base[1].point, humanized[1].point);
    let high_velocity_delta = distance(base[2].point, humanized[2].point);

    println!(
        "readback=humanize edge=tremor_velocity before={base:?} after={humanized:?} result_value=low_delta:{low_velocity_delta:.6} high_delta:{high_velocity_delta:.6}"
    );
    assert!(low_velocity_delta > high_velocity_delta);
    Ok(())
}

#[test]
fn must_fail_invalid_humanize_params_fail_closed() {
    let base = base_timed_path();
    let error = humanize_timed_path(
        &base,
        Some(HumanizeParams {
            tremor_base_stddev_px: f32::NAN,
            tremor_velocity_scale: 0.0,
            overshoot_prob: 0.0,
            overshoot_factor_range: (1.03, 1.12),
            micro_pause_prob: 0.0,
            micro_pause_ms_range: (15, 40),
            seed: None,
        }),
    )
    .expect_err("NaN tremor must be rejected");

    println!(
        "readback=humanize edge=invalid_param before=tremor_base_stddev_px:NaN after={error:?}"
    );
    assert!(matches!(
        error,
        HumanizeError::InvalidNonNegative {
            field: "tremor_base_stddev_px",
            ..
        }
    ));
}

const fn valid_params() -> HumanizeParams {
    HumanizeParams {
        tremor_base_stddev_px: 0.25,
        tremor_velocity_scale: 2.0,
        overshoot_prob: 0.5,
        overshoot_factor_range: (1.03, 1.12),
        micro_pause_prob: 0.5,
        micro_pause_ms_range: (15, 40),
        seed: Some(7),
    }
}

#[test]
fn must_fail_humanize_rejects_each_invalid_param() {
    let base = base_timed_path();
    let reject = |params: HumanizeParams| match humanize_timed_path(&base, Some(params)) {
        Ok(path) => panic!("params {params:?} must be refused, got {path:?}"),
        Err(error) => error,
    };

    assert_eq!(
        reject(HumanizeParams {
            tremor_velocity_scale: -0.001,
            ..valid_params()
        }),
        HumanizeError::InvalidNonNegative {
            field: "tremor_velocity_scale",
            value: -0.001,
        }
    );
    assert_eq!(
        reject(HumanizeParams {
            overshoot_prob: 1.001,
            ..valid_params()
        }),
        HumanizeError::InvalidProbability {
            field: "overshoot_prob",
            value: 1.001,
        }
    );
    assert_eq!(
        reject(HumanizeParams {
            micro_pause_prob: -0.001,
            ..valid_params()
        }),
        HumanizeError::InvalidProbability {
            field: "micro_pause_prob",
            value: -0.001,
        }
    );
    assert!(matches!(
        reject(HumanizeParams {
            micro_pause_prob: f32::NAN,
            ..valid_params()
        }),
        HumanizeError::InvalidProbability {
            field: "micro_pause_prob",
            ..
        }
    ));
    assert_eq!(
        reject(HumanizeParams {
            overshoot_factor_range: (0.999, 1.12),
            ..valid_params()
        }),
        HumanizeError::InvalidOvershootFactor {
            field: "overshoot_factor_range.0",
            value: 0.999,
        }
    );
    assert_eq!(
        reject(HumanizeParams {
            overshoot_factor_range: (1.03, 0.999),
            ..valid_params()
        }),
        HumanizeError::InvalidOvershootFactor {
            field: "overshoot_factor_range.1",
            value: 0.999,
        }
    );
    assert_eq!(
        reject(HumanizeParams {
            overshoot_factor_range: (1.2, 1.1),
            ..valid_params()
        }),
        HumanizeError::InvalidOvershootRange { min: 1.2, max: 1.1 }
    );
    assert_eq!(
        reject(HumanizeParams {
            micro_pause_ms_range: (41, 40),
            ..valid_params()
        }),
        HumanizeError::InvalidMicroPauseRange {
            min_ms: 41,
            max_ms: 40,
        }
    );
}

#[test]
fn must_fail_humanize_rejects_invalid_sample_streams() {
    let params = Some(valid_params());
    assert_eq!(
        humanize_timed_path(&[], params),
        Err(HumanizeError::NotEnoughSamples { samples: 0 })
    );
    assert_eq!(
        humanize_timed_path(&[timed(0.0, 0.0, 0.0, 0.0)], params),
        Err(HumanizeError::NotEnoughSamples { samples: 1 })
    );

    let mut non_finite = base_timed_path();
    non_finite[2].point.x = f64::NAN;
    assert_eq!(
        humanize_timed_path(&non_finite, params),
        Err(HumanizeError::NonFiniteSample { index: 2 })
    );

    let mut backwards = base_timed_path();
    backwards[3].elapsed_ms = backwards[2].elapsed_ms - 1.0;
    assert_eq!(
        humanize_timed_path(&backwards, params),
        Err(HumanizeError::NonMonotonicTimestamp { index: 3 })
    );
}

#[test]
fn must_pass_humanize_accepts_the_closest_valid_params() {
    let base = base_timed_path();
    // Two samples is the minimum stream.
    let two = [timed(0.0, 0.0, 0.0, 0.0), timed(10.0, 10.0, 10.0, 0.0)];
    let boundary = HumanizeParams {
        tremor_base_stddev_px: 0.0,
        tremor_velocity_scale: 0.0,
        overshoot_prob: 1.0,
        overshoot_factor_range: (1.0, 1.0),
        micro_pause_prob: 1.0,
        micro_pause_ms_range: (15, 15),
        seed: Some(3),
    };
    for (edge, samples) in [("base", base.as_slice()), ("two_samples", &two[..])] {
        let result = humanize_timed_path(samples, Some(boundary));
        assert!(
            result.is_ok(),
            "edge={edge}: prob 1.0, factor 1.0 and equal ranges are valid: {result:?}"
        );
    }
    let zero_prob = HumanizeParams {
        overshoot_prob: 0.0,
        micro_pause_prob: 0.0,
        ..boundary
    };
    assert!(humanize_timed_path(&base, Some(zero_prob)).is_ok());
}

fn base_timed_path() -> Vec<TimedPathPoint> {
    vec![
        timed(0.0, 0.0, 0.0, 0.0),
        timed(25.0, 25.0, 25.0, 0.0),
        timed(50.0, 50.0, 50.0, 0.0),
        timed(75.0, 75.0, 75.0, 0.0),
        timed(100.0, 100.0, 100.0, 0.0),
    ]
}

const fn timed(elapsed_ms: f64, arclen: f64, x: f64, y: f64) -> TimedPathPoint {
    TimedPathPoint {
        elapsed_ms,
        arclen,
        point: PathPoint { x, y },
    }
}

fn distance(left: PathPoint, right: PathPoint) -> f64 {
    left.distance_to(right)
}

fn timestamps_are_monotonic(samples: &[TimedPathPoint]) -> bool {
    samples
        .windows(2)
        .all(|window| window[0].elapsed_ms <= window[1].elapsed_ms)
}
