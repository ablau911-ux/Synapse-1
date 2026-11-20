//! Frozen behaviour table for issue #311 row 311d (transcription confidence).
//! Written by the manager from the table in issue #311 (work method v3). Code agents must
//! not edit this file.

use synapse_core::ConfidenceSource;

use super::transcription_confidence;

#[test]
fn transcription_confidence_table() {
    assert_eq!(
        transcription_confidence(""),
        (0.0, ConfidenceSource::NotApplicable),
        "row empty"
    );
    assert_eq!(
        transcription_confidence("   "),
        (0.0, ConfidenceSource::NotApplicable),
        "row blank"
    );
    // the old heuristic gave 0.86 for these hint words: no invented score any more
    assert_eq!(
        transcription_confidence("hello synapse this is the world"),
        (0.0, ConfidenceSource::Unsupported),
        "row hint_words_not_scored"
    );
    assert_eq!(
        transcription_confidence("Invoice four seven one one"),
        (0.0, ConfidenceSource::Unsupported),
        "row plain_text"
    );
}
