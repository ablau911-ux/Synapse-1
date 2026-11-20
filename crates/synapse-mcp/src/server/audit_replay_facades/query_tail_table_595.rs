//! FROZEN manager table B1-B14. Install beside command_query.rs; coder never edits.
use super::*;
use crate::server::command_audit::{CommandAuditQueryFilters, CommandAuditTailQuery};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type RawRow = (Vec<u8>, Vec<u8>);

fn filters() -> CommandAuditQueryFilters {
    CommandAuditQueryFilters {
        start_ts_ns: None,
        end_ts_ns: None,
        session_id: None,
        tool: None,
        status: None,
        error_code: None,
        row_kind: None,
    }
}

fn row(ts: u64, seq: u32, session: &str, tool: &str) -> Result<RawRow, serde_json::Error> {
    let mut key = ts.to_be_bytes().to_vec();
    key.extend_from_slice(&seq.to_be_bytes());
    let value = json!({
        "row_kind": "command_audit", "audit_id": format!("{ts}-{seq}"),
        "ts_ns": ts, "actor": {"session_id": session}, "tool": tool,
        "phase": "final", "outcome": "ok", "payload_bounded": {"synthetic": true}
    });
    Ok((key, serde_json::to_vec(&value)?))
}

fn page(
    rows: Vec<RawRow>,
    limit: usize,
    scan: usize,
    f: CommandAuditQueryFilters,
) -> Result<AuditCommandQueryResponse, Box<dyn std::error::Error>> {
    let query = CommandAuditTailQuery::new(limit, scan, f, None)?.ok_or("expected tail query")?;
    // Digest algorithm is an unchanged dependency, not a behavior under test.
    // No call to install_key/DPAPI/randomness/files on an L row.
    Ok(summarize_command_query(
        query.page(rows, |_| "fixture-digest".to_owned()),
    )?)
}

fn ids(p: &AuditCommandQueryResponse) -> Vec<&str> {
    p.rows.iter().map(|row| row.audit_id.as_str()).collect()
}

fn nonempty_control() -> TestResult {
    let p = page(
        vec![row(10, 0, "a", "act")?, row(20, 0, "a", "act")?],
        2,
        20,
        filters(),
    )?;
    assert_eq!(
        ids(&p),
        ["20-0", "10-0"],
        "CONTROL: default must return recent rows, not nothing or oldest-first"
    );
    assert_eq!(p.scan_order, "newest_first");
    assert_eq!(p.rows[0].value_sha256, "fixture-digest");
    Ok(())
}

#[test]
fn b1_default_returns_latest_matches() -> TestResult {
    let p = page(
        vec![
            row(10, 0, "a", "act")?,
            row(20, 0, "a", "act")?,
            row(30, 0, "a", "act")?,
        ],
        2,
        20,
        filters(),
    )?;
    assert_eq!(ids(&p), ["30-0", "20-0"]);
    assert_eq!(
        (p.scanned_rows, p.matched_rows, p.returned_count),
        (3, 2, 2)
    );
    assert!(p.has_older);
    assert!(!p.complete);
    assert!(p.next_start_key_hex.is_none());
    assert_eq!(p.oldest_returned_ts_ns, Some(20));
    nonempty_control()
}

#[test]
fn b2_empty_log_is_complete() -> TestResult {
    let p = page(vec![], 2, 20, filters())?;
    assert!(p.rows.is_empty());
    assert!(p.complete);
    assert!(!p.has_older);
    assert_eq!(p.oldest_returned_ts_ns, None);
    assert_eq!(p.scanned_rows, 0);
    nonempty_control()
}

#[test]
fn b3_exact_return_limit_below_scan_cap_is_complete() -> TestResult {
    let p = page(
        vec![row(10, 0, "a", "act")?, row(20, 0, "a", "act")?],
        2,
        20,
        filters(),
    )?;
    assert_eq!(ids(&p), ["20-0", "10-0"]);
    assert!(p.complete);
    assert!(!p.has_older);
    nonempty_control()
}

#[test]
fn b4_scan_capped_no_match_reports_possible_history() -> TestResult {
    let mut f = filters();
    f.tool = Some("missing".to_owned());
    let p = page(
        vec![row(20, 0, "a", "act")?, row(30, 0, "a", "act")?],
        2,
        2,
        f,
    )?;
    assert!(p.rows.is_empty());
    assert!(p.has_older);
    assert!(!p.complete);
    assert_eq!(p.scanned_rows, 2);
    assert!(p.next_start_key_hex.is_none());
    nonempty_control()
}

#[test]
fn b5_interleaved_actor_stays_filtered() -> TestResult {
    let mut f = filters();
    f.session_id = Some("a".to_owned());
    let p = page(
        vec![
            row(10, 0, "a", "act")?,
            row(20, 0, "b", "act")?,
            row(30, 0, "a", "act")?,
        ],
        3,
        20,
        f,
    )?;
    assert_eq!(ids(&p), ["30-0", "10-0"]);
    nonempty_control()
}

#[test]
fn b6_other_tool_is_not_a_match() -> TestResult {
    let mut f = filters();
    f.tool = Some("own".to_owned());
    let p = page(
        vec![
            row(10, 0, "a", "own")?,
            row(20, 0, "b", "other")?,
            row(30, 0, "a", "own")?,
        ],
        3,
        20,
        f,
    )?;
    assert_eq!(ids(&p), ["30-0", "10-0"]);
    nonempty_control()
}

#[test]
fn b7_end_timestamp_is_inclusive() -> TestResult {
    let mut f = filters();
    f.end_ts_ns = Some(20);
    let p = page(
        vec![
            row(10, 0, "a", "act")?,
            row(20, 0, "a", "act")?,
            row(30, 0, "a", "act")?,
        ],
        3,
        20,
        f,
    )?;
    assert_eq!(ids(&p), ["20-0", "10-0"]);
    nonempty_control()
}

#[test]
fn b8_explicit_zero_start_keeps_forward_route() -> TestResult {
    let mut f = filters();
    f.start_ts_ns = Some(0);
    assert!(CommandAuditTailQuery::new(2, 20, f, None)?.is_none());
    nonempty_control()
}

#[test]
fn b9_nonempty_cursor_keeps_forward_route() -> TestResult {
    assert!(CommandAuditTailQuery::new(2, 20, filters(), Some("00"))?.is_none());
    // Hostile cursor is NOT swallowed by the new tail route; old decode_hex rejects it.
    assert!(CommandAuditTailQuery::new(2, 20, filters(), Some("zz"))?.is_none());
    nonempty_control()
}

#[test]
fn b10_corrupt_row_stays_a_storage_error() -> TestResult {
    let err = match page(vec![(vec![1], b"{".to_vec())], 2, 20, filters()) {
        Err(err) => err,
        Ok(_) => return Err("corrupt row was silently accepted".into()),
    };
    assert!(err.to_string().contains("corrupt CF_ACTION_LOG"));
    nonempty_control()
}

#[test]
fn b11_equal_timestamp_uses_reverse_key_sequence() -> TestResult {
    let p = page(
        vec![row(10, 1, "a", "act")?, row(10, 2, "a", "act")?],
        2,
        20,
        filters(),
    )?;
    assert_eq!(ids(&p), ["10-2", "10-1"]);
    assert_eq!(p.oldest_returned_ts_ns, Some(10));
    nonempty_control()
}

#[test]
fn b12_new_object_repeats_same_selection() -> TestResult {
    let rows = vec![row(10, 0, "a", "act")?, row(20, 0, "a", "act")?];
    let a = page(rows.clone(), 2, 20, filters())?;
    let b = page(rows, 2, 20, filters())?;
    assert_eq!(ids(&a), ["20-0", "10-0"]);
    assert_eq!(ids(&a), ids(&b));
    nonempty_control()
}

#[test]
fn b13_zero_and_over_limit_refused_blank_cursor_defaults() -> TestResult {
    assert!(CommandAuditTailQuery::new(0, 20, filters(), None).is_err());
    assert!(CommandAuditTailQuery::new(251, 20, filters(), None).is_err());
    assert!(CommandAuditTailQuery::new(2, 0, filters(), None).is_err());
    assert!(CommandAuditTailQuery::new(2, 5001, filters(), None).is_err());
    assert!(CommandAuditTailQuery::new(2, 20, filters(), Some("  "))?.is_some());
    nonempty_control()
}

#[test]
fn b14_forward_summary_keeps_partial_cursor() -> TestResult {
    let mut f = filters();
    f.start_ts_ns = Some(0);
    let source = CommandAuditQueryResponse {
        source_of_truth: "CF_ACTION_LOG bounded scan",
        cf_name: "CF_ACTION_LOG",
        filters: f,
        limit: 1,
        scan_limit: 2,
        scanned_rows: 2,
        matched_rows: 1,
        returned_count: 1,
        corrupt_row_count: 0,
        partial: true,
        exhausted: false,
        start_key_hex: Some("00".to_owned()),
        next_start_key_hex: Some("0011".to_owned()),
        rows: vec![],
    };
    let p = summarize_command_query(source)?;
    assert!(!p.complete);
    assert_eq!(p.next_start_key_hex.as_deref(), Some("0011"));
    assert_eq!(p.scan_order, "oldest_first");
    assert!(!p.has_older);
    nonempty_control()
}
