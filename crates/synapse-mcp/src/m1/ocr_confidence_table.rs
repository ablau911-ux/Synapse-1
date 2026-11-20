//! Frozen behaviour table for issue #311 rows 311b and 311c at the OCR mapping layer.
//! Written by the manager from the table in issue #311 (work method v3). Code agents must
//! not edit this file.

use synapse_core::{ConfidenceSource, Rect};
use synapse_perception::TextRegion;

use super::{aggregate_confidence_source, word_confidence, word_confidence_source};

fn region(text: &str, confidence: f32, source: ConfidenceSource) -> TextRegion {
    TextRegion {
        text: text.to_owned(),
        bbox: Rect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        },
        confidence,
        confidence_source: source,
    }
}

#[test]
fn word_mapping_table() {
    // engine gave no score: never reported as a number the engine did not give
    assert_eq!(
        word_confidence("Invoice", 1.0, ConfidenceSource::Unsupported),
        0.0,
        "row unsupported_zero"
    );
    assert_eq!(
        word_confidence_source("Invoice", 1.0, ConfidenceSource::Unsupported),
        ConfidenceSource::Unsupported,
        "row unsupported_source"
    );
    // a real engine score on a plain word passes through
    assert!(
        (word_confidence("Invoice", 0.9, ConfidenceSource::Engine) - 0.9).abs() < 1e-6,
        "row engine_kept"
    );
    assert_eq!(
        word_confidence_source("Invoice", 0.9, ConfidenceSource::Engine),
        ConfidenceSource::Engine,
        "row engine_source"
    );
}

#[test]
fn aggregate_table() {
    assert_eq!(
        aggregate_confidence_source(&[]),
        ConfidenceSource::Unsupported,
        "row empty"
    );
    let engine = [
        region("a", 0.9, ConfidenceSource::Engine),
        region("b", 0.8, ConfidenceSource::Engine),
    ];
    assert_eq!(
        aggregate_confidence_source(&engine),
        ConfidenceSource::Engine,
        "row all_engine"
    );
    let mixed = [
        region("a", 0.9, ConfidenceSource::Engine),
        region("b", 0.0, ConfidenceSource::Unsupported),
    ];
    assert_eq!(
        aggregate_confidence_source(&mixed),
        ConfidenceSource::Unsupported,
        "row mixed_least_trusted"
    );
}
