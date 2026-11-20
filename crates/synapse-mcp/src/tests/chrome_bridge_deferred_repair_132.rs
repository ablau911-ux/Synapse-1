//! #132: bridge health names a repair that setup deferred to the daemon.

use super::{append_note, read_reason};

#[test]
fn must_pass_reads_reason_from_first_line() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("marker.txt");
    std::fs::write(
        &path,
        "existing_extension_ui_reload_no_eligible_chrome_window_deferred_to_daemon\r\n",
    )?;
    assert_eq!(
        read_reason(&path).as_deref(),
        Some("existing_extension_ui_reload_no_eligible_chrome_window_deferred_to_daemon")
    );
    Ok(())
}

#[test]
fn must_fail_missing_empty_or_invalid_marker_is_ignored() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("marker.txt");
    assert_eq!(read_reason(&path), None);
    for bad in ["", "   \n", "has space", "semi;colon", &"a".repeat(129)] {
        std::fs::write(&path, bad)?;
        assert_eq!(read_reason(&path), None, "marker {bad:?} must be rejected");
    }
    Ok(())
}

#[test]
fn must_pass_note_is_appended_to_detail_and_absent_note_changes_nothing() {
    let note = Some("deferred_repair=r".to_owned());
    assert_eq!(
        append_note(Some("tab_control_available=true".to_owned()), note.clone()).as_deref(),
        Some("tab_control_available=true deferred_repair=r")
    );
    assert_eq!(
        append_note(None, note).as_deref(),
        Some("deferred_repair=r")
    );
    assert_eq!(
        append_note(Some("x".to_owned()), None).as_deref(),
        Some("x")
    );
    assert_eq!(append_note(None, None), None);
}
