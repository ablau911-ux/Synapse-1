//! Regression tests for #216: one undecodable or mismatched `CF_SESSIONS`
//! continuity row must not break the CDP step of every teardown (and the
//! per-tick stale-candidate scans). Bad rows are skipped, warned once and left
//! in place for repair; healthy rows of other sessions keep working.

use super::skipped_rows::SkippedRowWarnings;
use super::*;
use synapse_core::SCHEMA_VERSION;

fn owner_row(session_id: &str, target_id: &str, hwnd: i64) -> PersistedCdpTargetOwner {
    let endpoint = "chrome-extension://synapse-test/chrome.tabs".to_owned();
    let owner_key = format!(
        "cdp:0x{hwnd:x}:{}:{}",
        endpoint,
        target_id.trim().to_ascii_lowercase()
    );
    PersistedCdpTargetOwner {
        schema_version: 1,
        owner_key,
        stored_at_unix_ms: 1_000,
        owner_session_id: session_id.to_owned(),
        owner_client_name: Some("claude-code".to_owned()),
        owner_agent_kind: "claude".to_owned(),
        owner_started_at_unix_ms: Some(900),
        owner: CdpTargetOwner {
            session_id: session_id.to_owned(),
            window_hwnd: hwnd,
            endpoint,
            chrome_window_id: Some(42),
            capture_window_hwnd: Some(hwnd),
            cdp_target_id: target_id.to_owned(),
            requested_url: "http://127.0.0.1/test".to_owned(),
            target_url: "http://127.0.0.1/test".to_owned(),
            created_at_unix_ms: 950,
        },
    }
}

fn put_owner(db: &Db, row: &PersistedCdpTargetOwner) -> anyhow::Result<()> {
    db.put_batch_pressure_bypass(
        cf::CF_SESSIONS,
        [(
            cdp_target_owner_row_key(&row.owner_key, &row.owner.cdp_target_id),
            synapse_storage::encode_json(row)?,
        )],
    )?;
    Ok(())
}

fn garbled_key(name: &str) -> Vec<u8> {
    format!("{SESSION_CDP_TARGET_OWNER_PREFIX}{name}").into_bytes()
}

fn put_garbled(db: &Db, name: &str) -> anyhow::Result<Vec<u8>> {
    let key = garbled_key(name);
    db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(key.clone(), b"{not-json".to_vec())])?;
    Ok(key)
}

fn put_wrong_schema(db: &Db, session_id: &str, target_id: &str) -> anyhow::Result<()> {
    let mut row = owner_row(session_id, target_id, 0x9000);
    row.schema_version = 2;
    put_owner(db, &row)
}

#[test]
fn must_pass_healthy_rows_are_returned_per_session() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), SCHEMA_VERSION)?;
    put_owner(
        &db,
        &owner_row("session-216-p", "chrome-tab:Pass216", 0x1100),
    )?;
    put_owner(
        &db,
        &owner_row("session-216-q", "chrome-tab:Other216", 0x1200),
    )?;

    let rows = read_persisted_cdp_target_owners_for_session_from_db(&db, "session-216-p")
        .map_err(anyhow::Error::msg)?;

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1.owner.cdp_target_id, "chrome-tab:Pass216");
    Ok(())
}

#[test]
fn must_fail_per_session_scan_survives_an_undecodable_row_and_returns_own_rows()
-> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), SCHEMA_VERSION)?;
    let own = owner_row("session-216-a", "chrome-tab:Own216", 0x1000);
    put_owner(&db, &own)?;
    let bad_key = put_garbled(&db, "garbled-216")?;

    let rows = read_persisted_cdp_target_owners_for_session_from_db(&db, "session-216-a")
        .map_err(anyhow::Error::msg)?;

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1.owner.cdp_target_id, "chrome-tab:Own216");
    // The bad row is left in place for repair, never deleted by the scan.
    assert!(
        db.scan_cf_prefix(cf::CF_SESSIONS, &bad_key)?
            .iter()
            .any(|(key, _value)| key == &bad_key)
    );
    Ok(())
}

#[test]
fn must_fail_per_session_scan_ignores_another_sessions_wrong_schema_row() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), SCHEMA_VERSION)?;
    put_owner(
        &db,
        &owner_row("session-216-b", "chrome-tab:Own216b", 0x1000),
    )?;
    // Written by another daemon build sharing the DB (schema_version != 1).
    put_wrong_schema(&db, "session-216-other", "chrome-tab:Foreign216")?;

    let rows = read_persisted_cdp_target_owners_for_session_from_db(&db, "session-216-b")
        .map_err(anyhow::Error::msg)?;

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1.owner_session_id, "session-216-b");
    Ok(())
}

#[test]
fn must_fail_session_id_scans_skip_bad_rows_instead_of_failing_every_tick() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), SCHEMA_VERSION)?;
    put_owner(&db, &owner_row("session-216-c", "chrome-tab:C216", 0x1000))?;
    put_garbled(&db, "garbled-216-ids")?;
    put_wrong_schema(&db, "session-216-d", "chrome-tab:D216")?;

    let ids =
        read_persisted_cdp_target_owner_session_ids_from_db(&db).map_err(anyhow::Error::msg)?;

    assert_eq!(ids, BTreeSet::from(["session-216-c".to_owned()]));

    let bad_target_key = session_target_key("garbled-target-216");
    db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(bad_target_key, b"{not-json".to_vec())])?;
    let target_ids =
        read_persisted_session_target_session_ids_from_db(&db).map_err(anyhow::Error::msg)?;
    assert!(target_ids.is_empty());
    Ok(())
}

#[test]
fn must_fail_a_per_session_scan_does_not_make_another_sessions_bad_row_warn_again()
-> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), SCHEMA_VERSION)?;
    put_wrong_schema(&db, "session-other", "tab-other")?;
    put_owner(&db, &owner_row("session-own", "tab-own", 0x9100))?;
    let bad_key = cdp_target_owner_row_key(
        &owner_row("session-other", "tab-other", 0x9000).owner_key,
        "tab-other",
    );
    let warnings = SkippedRowWarnings::default();

    // session_list runs the per-session scan and the all-ids scan every poll.
    for _poll in 0..3 {
        read_persisted_cdp_target_owner_session_ids_with_warnings(&db, &warnings)
            .map_err(anyhow::Error::msg)?;
        assert!(warnings.is_remembered(&bad_key));
        let own = read_persisted_cdp_target_owners_for_session_with_warnings(
            &db,
            "session-own",
            &warnings,
        )
        .map_err(anyhow::Error::msg)?;
        assert_eq!(own.len(), 1);
        // Still remembered: the next all-ids scan stays quiet.
        assert!(warnings.is_remembered(&bad_key));
    }
    Ok(())
}

#[test]
fn must_fail_a_bad_row_is_remembered_so_it_warns_once_not_every_scan() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let db = Db::open(&temp.path().join("db"), SCHEMA_VERSION)?;
    let bad_key = put_garbled(&db, "garbled-216-warn-once")?;
    // A private instance keeps this test independent of the daemon-wide one.
    let warnings = SkippedRowWarnings::default();

    for _scan in 0..3 {
        read_persisted_cdp_target_owner_session_ids_with_warnings(&db, &warnings)
            .map_err(anyhow::Error::msg)?;
        assert!(warnings.is_remembered(&bad_key));
        assert_eq!(warnings.remembered(), 1);
    }

    // Repaired (deleted) row is forgotten after the next scan.
    db.delete_batch(cf::CF_SESSIONS, [bad_key.clone()])?;
    read_persisted_cdp_target_owner_session_ids_with_warnings(&db, &warnings)
        .map_err(anyhow::Error::msg)?;
    assert!(!warnings.is_remembered(&bad_key));
    Ok(())
}
