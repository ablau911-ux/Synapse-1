use serde_json::json;
use synapse_core::{PathPoint, PathSpec, VelocityProfile};

#[test]
fn must_pass_path_spec_json_round_trips_and_defaults_catmull_alpha()
-> Result<(), Box<dyn std::error::Error>> {
    let input = json!({
        "kind": "catmull_rom",
        "waypoints": [
            {"x": 0.0, "y": 0.0},
            {"x": 10.0, "y": 0.0},
            {"x": 10.0, "y": 10.0},
            {"x": 20.0, "y": 10.0}
        ],
        "tension": 0.0,
        "closed": false
    });
    let parsed = serde_json::from_value::<PathSpec>(input.clone())?;
    let serialized = serde_json::to_value(&parsed)?;
    println!("readback=path_types edge=catmull_default_alpha before={input} after={serialized}");

    match parsed {
        PathSpec::CatmullRom { alpha, .. } => assert_eq!(alpha.to_bits(), 0.5_f64.to_bits()),
        other => panic!("expected catmull_rom path spec, got {other:?}"),
    }

    let closed_circle = PathSpec::Circle {
        center: PathPoint::new(5.0, -5.0),
        radius: 20.0,
    };
    let round_trip = serde_json::from_value::<PathSpec>(serde_json::to_value(&closed_circle)?)?;
    println!("readback=path_types edge=circle_round_trip after={round_trip:?}");
    assert_eq!(round_trip, closed_circle);
    Ok(())
}

#[test]
fn must_fail_path_spec_with_unknown_field_or_variant_is_rejected() {
    let unknown_field = json!({
        "kind": "line",
        "from": {"x": 0.0, "y": 0.0},
        "to": {"x": 1.0, "y": 1.0},
        "extra": true
    });
    assert_rejected::<PathSpec>(unknown_field, "unknown field `extra`");
    let invalid_variant = json!({"kind": "quadratic_bezier"});
    assert_rejected::<PathSpec>(invalid_variant, "unknown variant `quadratic_bezier`");
    assert_rejected::<VelocityProfile>(json!("teleport"), "unknown variant `teleport`");
}

/// The rejection must carry the named serde reason, so a different decode
/// failure cannot satisfy the case under test.
fn assert_rejected<T>(value: serde_json::Value, reason: &str)
where
    T: serde::de::DeserializeOwned + std::fmt::Debug,
{
    match serde_json::from_value::<T>(value) {
        Ok(decoded) => panic!("expected rejection `{reason}`, decoded {decoded:?}"),
        Err(err) => assert!(
            err.to_string().contains(reason),
            "expected `{reason}`, got `{err}`"
        ),
    }
}

#[test]
fn must_pass_every_path_spec_variant_round_trips_known_json()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        json!({
            "kind": "line",
            "from": {"x": 0.0, "y": 0.0},
            "to": {"x": 10.0, "y": 20.0}
        }),
        json!({
            "kind": "arc",
            "center": {"x": 0.0, "y": 0.0},
            "radius": 100.0,
            "start_angle_rad": 0.0,
            "sweep_angle_rad": std::f64::consts::FRAC_PI_2
        }),
        json!({
            "kind": "circle",
            "center": {"x": 5.0, "y": -5.0},
            "radius": 20.0
        }),
        json!({
            "kind": "cubic_bezier",
            "p0": {"x": 0.0, "y": 0.0},
            "p1": {"x": 0.0, "y": 100.0},
            "p2": {"x": 100.0, "y": 100.0},
            "p3": {"x": 100.0, "y": 0.0}
        }),
        json!({
            "kind": "polyline",
            "points": [
                {"x": 0.0, "y": 0.0},
                {"x": 10.0, "y": 0.0},
                {"x": 10.0, "y": 10.0}
            ],
            "closed": true
        }),
        json!({
            "kind": "catmull_rom",
            "waypoints": [
                {"x": 0.0, "y": 0.0},
                {"x": 10.0, "y": 0.0},
                {"x": 10.0, "y": 10.0},
                {"x": 20.0, "y": 10.0}
            ],
            "alpha": 0.5,
            "tension": 0.25,
            "closed": false
        }),
    ];

    for input in cases {
        let parsed = serde_json::from_value::<PathSpec>(input.clone())?;
        let output = serde_json::to_value(&parsed)?;
        println!("readback=path_types edge=path_spec_variant before={input} after={output}");
        assert_eq!(output, input);
    }
    Ok(())
}

#[test]
fn must_pass_velocity_profile_json_round_trips() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (VelocityProfile::Constant, "constant"),
        (VelocityProfile::Linear, "linear"),
        (VelocityProfile::EaseInOut, "ease_in_out"),
        (VelocityProfile::MinimumJerk, "minimum_jerk"),
    ];

    for (profile, wire) in cases {
        let json = serde_json::to_value(profile)?;
        assert_eq!(json, json!(wire), "exact wire string");
        let parsed = serde_json::from_value::<VelocityProfile>(json.clone())?;
        println!("readback=path_types edge=velocity_profile_round_trip after={json}");
        assert_eq!(parsed, profile);
    }

    Ok(())
}
