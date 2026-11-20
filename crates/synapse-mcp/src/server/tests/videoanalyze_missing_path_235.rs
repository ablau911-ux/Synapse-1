//! #235: `videoanalyze` on a missing file must name the path and say it was
//! not found, not surface a bare OS error.

use super::*;

#[test]
fn must_fail_missing_video_file_names_the_path() {
    let path = std::path::Path::new(r"C:\does\not\exist.mp4");

    let error = source_metadata(path)
        .err()
        .unwrap_or_else(|| panic!("a missing video file must be refused"));

    assert!(
        error.message.contains(r"C:\does\not\exist.mp4") && error.message.contains("not found"),
        "message must name the missing file: {}",
        error.message
    );
    assert_eq!(
        error.data.as_ref().and_then(|data| data.get("code")),
        Some(&serde_json::json!("TOOL_PARAMS_INVALID"))
    );
}

#[test]
fn must_fail_empty_video_path_says_the_path_is_empty() {
    for empty in ["", "   "] {
        let error = source_metadata(empty)
            .err()
            .unwrap_or_else(|| panic!("an empty path must be refused"));
        assert!(
            error.message.contains("path must not be empty"),
            "empty path refusal must say so: {}",
            error.message
        );
        assert_eq!(
            error.data.as_ref().and_then(|data| data.get("code")),
            Some(&serde_json::json!(error_codes::TOOL_PARAMS_INVALID)),
            "empty path {empty:?}"
        );
    }
}

#[test]
fn must_pass_existing_file_metadata_is_returned() {
    let file = tempfile::NamedTempFile::new().unwrap_or_else(|error| panic!("tempfile: {error}"));

    let metadata = source_metadata(file.path())
        .unwrap_or_else(|error| panic!("existing file must be readable: {}", error.message));

    assert!(metadata.is_file());
}
