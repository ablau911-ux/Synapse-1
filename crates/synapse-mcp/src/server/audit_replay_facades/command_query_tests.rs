use super::*;
use crate::server::command_audit::CommandAuditQueryFilters;

fn response(partial: bool, cursor: Option<&str>) -> CommandAuditQueryResponse {
    CommandAuditQueryResponse {
        source_of_truth: "CF_ACTION_LOG bounded scan",
        cf_name: "CF_ACTION_LOG",
        filters: CommandAuditQueryFilters {
            start_ts_ns: None,
            end_ts_ns: None,
            session_id: None,
            tool: Some("act".to_owned()),
            status: None,
            error_code: None,
            row_kind: None,
        },
        limit: 1,
        scan_limit: 20,
        scanned_rows: 6,
        matched_rows: 1,
        returned_count: 1,
        corrupt_row_count: 0,
        partial,
        exhausted: !partial,
        start_key_hex: None,
        next_start_key_hex: cursor.map(str::to_owned),
        rows: Vec::new(),
    }
}

#[test]
fn must_pass_clean_partial_page_returns_counts_and_cursor() {
    let page = summarize_command_query(response(true, Some("0011"))).expect("clean partial page");
    assert!(!page.complete);
    assert_eq!(page.scanned_rows, 6);
    assert_eq!(page.matched_rows, 1);
    assert_eq!(page.returned_count, 1);
    assert_eq!(page.next_start_key_hex.as_deref(), Some("0011"));
    assert_eq!(page.cf_name, "CF_ACTION_LOG");
}

#[test]
fn must_pass_complete_page_has_no_cursor() {
    let page = summarize_command_query(response(false, None)).expect("complete page");
    assert!(page.complete);
    assert!(page.next_start_key_hex.is_none());
}

#[test]
fn must_fail_corrupt_partial_page_remains_a_storage_error() {
    let mut source = response(true, Some("0011"));
    source.corrupt_row_count = 1;
    let error = summarize_command_query(source).expect_err("corruption must fail");
    assert!(error.message.contains("corrupt CF_ACTION_LOG rows"));
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(|code| code.as_str()),
        Some(error_codes::STORAGE_READ_FAILED)
    );
}
