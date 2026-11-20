//! Public audit facade with real CF_ACTION_LOG pages, never fabricated responses.
use super::types::AuditParams;
use crate::server::{
    Parameters,
    g7b_support::{code, now_ns, service},
};
use serde_json::json;
use synapse_storage::cf;

fn params(cursor: Option<String>) -> AuditParams {
    serde_json::from_value(json!({"operation": "command_query", "command_query": {
        "limit": 1, "scan_limit": 1, "start_key_hex": cursor, "row_kind": "all"
    }}))
    .expect("facade params")
}

fn key(ts: u64, seq: u32) -> Vec<u8> {
    [ts.to_be_bytes().as_slice(), seq.to_be_bytes().as_slice()].concat()
}

#[tokio::test]
async fn must_pass_audit_facade_chains_clean_partial_pages_to_completion_84() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.m3_storage()?;
    let ts = now_ns();
    let rows: Vec<_> = (1..=3)
        .map(|seq| {
            (
                key(ts, seq),
                serde_json::to_vec(
                    &json!({"ts_ns": ts, "row_kind": "command_audit", "tool": "g7b"}),
                )
                .expect("encode"),
            )
        })
        .collect();
    assert_eq!(db.scan_cf(cf::CF_ACTION_LOG)?.len(), 0);
    db.put_batch(cf::CF_ACTION_LOG, rows.clone())?;
    assert_eq!(db.scan_cf(cf::CF_ACTION_LOG)?, rows);
    let mut cursor = None;
    let mut keys = Vec::new();
    for page_index in 0..3 {
        let response = svc
            .audit(Parameters(params(cursor)))
            .await?
            .0
            .command_query
            .expect("page");
        assert_eq!(response.scanned_rows, 1);
        assert_eq!(response.matched_rows, 1);
        assert_eq!(response.returned_count, 1);
        assert_eq!(response.corrupt_row_count, 0);
        assert_eq!(response.complete, page_index == 2);
        keys.push(response.rows[0].key_hex.clone());
        cursor = response.next_start_key_hex;
        assert_eq!(cursor.is_none(), page_index == 2);
    }
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), 3);
    assert_eq!(db.scan_cf(cf::CF_ACTION_LOG)?, rows);
    Ok(())
}

#[tokio::test]
async fn must_fail_corrupt_partial_page_and_bad_cursor_preserve_rows_84() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let svc = service(dir.path())?;
    let db = svc.m3_storage()?;
    let ts = now_ns();
    let rows = vec![
        (key(ts, 1), b"{broken".to_vec()),
        (key(ts, 2), serde_json::to_vec(&json!({"ts_ns": ts}))?),
    ];
    db.put_batch(cf::CF_ACTION_LOG, rows.clone())?;
    assert_eq!(db.scan_cf(cf::CF_ACTION_LOG)?, rows);
    let corrupt = svc
        .audit(Parameters(params(None)))
        .await
        .err()
        .expect("corrupt page");
    assert_eq!(code(&corrupt), "STORAGE_READ_FAILED");
    let invalid = svc
        .audit(Parameters(params(Some("not-hex".into()))))
        .await
        .err()
        .expect("malformed cursor");
    assert_eq!(code(&invalid), "TOOL_PARAMS_INVALID");
    assert_eq!(db.scan_cf(cf::CF_ACTION_LOG)?, rows);
    Ok(())
}
