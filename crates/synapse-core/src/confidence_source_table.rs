//! Frozen behaviour table for issue #311 rows 311a and 311c (confidence provenance).
//! Written by the manager from the table in issue #311 (work method v3). Code agents must
//! not edit this file.

use crate::ConfidenceSource;

const SCORED: [ConfidenceSource; 4] = [
    ConfidenceSource::Engine,
    ConfidenceSource::Uia,
    ConfidenceSource::Synthetic,
    ConfidenceSource::Heuristic,
];

#[test]
fn meets_threshold_table() {
    for source in SCORED {
        assert!(
            source.meets_threshold(0.7, 0.5),
            "row scored_above {source:?}"
        );
        assert!(
            source.meets_threshold(0.5, 0.5),
            "row scored_equal {source:?}"
        );
        assert!(
            !source.meets_threshold(0.3, 0.5),
            "row scored_below {source:?}"
        );
        assert!(
            source.meets_threshold(0.0, 0.0),
            "row scored_zero_threshold {source:?}"
        );
        assert!(
            !source.meets_threshold(f32::NAN, 0.5),
            "row scored_nan {source:?}"
        );
    }
    // no score exists: the threshold cannot judge it, the word is kept and carries its source
    for source in [
        ConfidenceSource::Unsupported,
        ConfidenceSource::NotApplicable,
    ] {
        assert!(
            source.meets_threshold(0.0, 0.5),
            "row unscored_kept {source:?}"
        );
        assert!(
            source.meets_threshold(0.0, 1.0),
            "row unscored_kept_max {source:?}"
        );
    }
}

#[test]
fn default_and_wire_names_table() {
    assert_eq!(
        ConfidenceSource::default(),
        ConfidenceSource::Unsupported,
        "row default_unsupported"
    );
    let names = [
        (ConfidenceSource::Engine, "engine"),
        (ConfidenceSource::Uia, "uia"),
        (ConfidenceSource::Synthetic, "synthetic"),
        (ConfidenceSource::Heuristic, "heuristic"),
        (ConfidenceSource::Unsupported, "unsupported"),
        (ConfidenceSource::NotApplicable, "not_applicable"),
    ];
    for (source, name) in names {
        assert_eq!(source.as_str(), name, "row as_str {name}");
        assert_eq!(
            serde_json::to_value(source).ok(),
            Some(serde_json::json!(name)),
            "row serde {name}"
        );
    }
}

#[test]
fn least_trusted_table() {
    use ConfidenceSource::{Engine, Heuristic, NotApplicable, Synthetic, Uia, Unsupported};
    assert_eq!(Engine.least_trusted(Engine), Engine, "row same");
    assert_eq!(Engine.least_trusted(Uia), Uia, "row engine_uia");
    assert_eq!(Uia.least_trusted(Synthetic), Synthetic, "row uia_synthetic");
    assert_eq!(
        Synthetic.least_trusted(Heuristic),
        Heuristic,
        "row synthetic_heuristic"
    );
    assert_eq!(
        Heuristic.least_trusted(NotApplicable),
        NotApplicable,
        "row heuristic_na"
    );
    assert_eq!(
        NotApplicable.least_trusted(Unsupported),
        Unsupported,
        "row na_unsupported"
    );
    assert_eq!(
        Unsupported.least_trusted(Engine),
        Unsupported,
        "row order_free"
    );
}
