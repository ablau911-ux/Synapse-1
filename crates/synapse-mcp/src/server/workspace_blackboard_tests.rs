use std::{num::NonZeroUsize, path::Path};

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{m2::M2ServiceConfig, m3::M3ServiceConfig, m4::M4ServiceConfig};

pub(super) fn service_with_db(path: &Path) -> anyhow::Result<SynapseService> {
    SynapseService::try_with_m2_shutdown_reason_and_m3_config(
        CancellationToken::new(),
        "test",
        CancellationToken::new(),
        &M2ServiceConfig::default(),
        M3ServiceConfig::from_cli_parts(
            Some(path.join("db")),
            Some(path.to_path_buf()),
            false,
            "127.0.0.1:0".to_owned(),
            NonZeroUsize::new(8)
                .ok_or_else(|| anyhow::anyhow!("max subscriptions must be nonzero"))?,
            false,
            true,
            None,
            false,
            None,
        ),
        M4ServiceConfig::default(),
    )
}

fn error_code(error: &rmcp::ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

fn error_detail_code(error: &rmcp::ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("detail_code"))
        .and_then(Value::as_str)
}

#[test]
fn must_fail_expired_cleanup_preserves_concurrent_refresh_and_counts_only_deletes()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let run_id = "run-issue90";
    let key = "race/refresh";
    service.workspace_put_impl_at(
        WorkspacePutParams {
            run_id: Some(run_id.to_owned()),
            key: key.to_owned(),
            expected_version: None,
            value: Some(json!({"generation": "expired"})),
            artifact: None,
            ttl_ms: 10_000,
        },
        "session-a",
        10_000,
    )?;
    service.workspace_put_impl_at(
        WorkspacePutParams {
            run_id: Some(run_id.to_owned()),
            key: key.to_owned(),
            expected_version: Some(1),
            value: Some(json!({"generation": "expired-v2"})),
            artifact: None,
            ttl_ms: 1,
        },
        "session-a",
        10_001,
    )?;
    let db = service.workspace_db()?;
    let observed = read_workspace_raw_row_optional(&db, &workspace_row_key(run_id, key))?
        .ok_or_else(|| anyhow::anyhow!("expired row missing before race"))?;
    let refreshed = service.workspace_put_impl_at(
        WorkspacePutParams {
            run_id: Some(run_id.to_owned()),
            key: key.to_owned(),
            expected_version: None,
            value: Some(json!({"generation": "fresh"})),
            artifact: None,
            ttl_ms: 10_000,
        },
        "session-b",
        10_002,
    )?;
    let observed_entry = decode_workspace_row(observed.key.clone(), observed.encoded.clone())
        .map_err(anyhow::Error::msg)?
        .entry;
    assert_ne!(observed_entry.version, refreshed.version);
    assert_ne!(observed_entry.value, Some(json!({"generation": "fresh"})));
    assert_ne!(
        hash_bytes(&observed.encoded),
        refreshed.storage_readback.value_sha256
    );
    let guard = workspace_write_lock()?;
    let deleted = delete_observed_expired_workspace_rows_locked(&db, vec![observed], &guard)?;
    drop(guard);
    assert_eq!(deleted, 0);
    let after = read_workspace_raw_row_optional(&db, &workspace_row_key(run_id, key))?
        .ok_or_else(|| anyhow::anyhow!("refreshed row was deleted"))?;
    assert_eq!(
        hash_bytes(&after.encoded),
        refreshed.storage_readback.value_sha256
    );
    let entry = decode_workspace_row(after.key, after.encoded)
        .map_err(anyhow::Error::msg)?
        .entry;
    assert_eq!(entry.version, refreshed.version);
    assert_eq!(entry.value, Some(json!({"generation": "fresh"})));
    Ok(())
}

#[test]
fn must_fail_expired_cleanup_handles_delete_recreate_and_multiple_observations()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let run_id = "run-issue90-multi";
    for key in ["race/recreated", "race/expired", "race/peer"] {
        service.workspace_put_impl_at(
            WorkspacePutParams {
                run_id: Some(run_id.to_owned()),
                key: key.to_owned(),
                expected_version: None,
                value: Some(json!({"key": key})),
                artifact: None,
                ttl_ms: if key == "race/peer" { 10_000 } else { 1 },
            },
            "session-a",
            20_000,
        )?;
    }
    let db = service.workspace_db()?;
    let scan = scan_workspace_run(&db, run_id, 20_002)?;
    assert_eq!(scan.expired_rows.len(), 2);
    let peer_before =
        read_workspace_raw_row_optional(&db, &workspace_row_key(run_id, "race/peer"))?
            .ok_or_else(|| anyhow::anyhow!("peer missing"))?;
    service.workspace_delete_impl_at(
        WorkspaceDeleteParams {
            run_id: Some(run_id.to_owned()),
            key: "race/recreated".to_owned(),
            raw_row_key: None,
            expected_version: Some(1),
            expected_corrupt_sha256: None,
        },
        "session-b",
        20_002,
    )?;
    let recreated = service.workspace_put_impl_at(
        WorkspacePutParams {
            run_id: Some(run_id.to_owned()),
            key: "race/recreated".to_owned(),
            expected_version: None,
            value: Some(json!({"key": "new"})),
            artifact: None,
            ttl_ms: 10_000,
        },
        "session-b",
        20_002,
    )?;
    let guard = workspace_write_lock()?;
    let deleted = delete_observed_expired_workspace_rows_locked(&db, scan.expired_rows, &guard)?;
    drop(guard);
    assert_eq!(deleted, 0); // put already cleaned the unrelated expired row
    let recreated_after =
        read_workspace_raw_row_optional(&db, &workspace_row_key(run_id, "race/recreated"))?
            .ok_or_else(|| anyhow::anyhow!("recreated row deleted"))?;
    assert_eq!(
        hash_bytes(&recreated_after.encoded),
        recreated.storage_readback.value_sha256
    );
    let peer_after = read_workspace_raw_row_optional(&db, &workspace_row_key(run_id, "race/peer"))?
        .ok_or_else(|| anyhow::anyhow!("peer deleted"))?;
    assert_eq!(peer_after.encoded, peer_before.encoded);
    Ok(())
}

#[test]
fn must_pass_expired_get_returns_refresh_written_after_observation() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let run_id = "run-issue90-get";
    let key = "race/get";
    for (expected_version, generation, at, ttl_ms) in [
        (None, "old-v1", 30_000, 10_000),
        (Some(1), "expired-v2", 30_001, 1),
    ] {
        service.workspace_put_impl_at(
            WorkspacePutParams {
                run_id: Some(run_id.to_owned()),
                key: key.to_owned(),
                expected_version,
                value: Some(json!({"generation": generation})),
                artifact: None,
                ttl_ms,
            },
            "session-a",
            at,
        )?;
    }
    let db = service.workspace_db()?;
    let before = read_workspace_raw_row_optional(&db, &workspace_row_key(run_id, key))?
        .ok_or_else(|| anyhow::anyhow!("expired get row missing"))?;
    let mut refreshed = None;
    let get = service.workspace_get_impl_at_after_observation(
        WorkspaceGetParams {
            run_id: Some(run_id.to_owned()),
            key: key.to_owned(),
        },
        "session-reader",
        30_003,
        || {
            refreshed = Some(service.workspace_put_impl_at(
                WorkspacePutParams {
                    run_id: Some(run_id.to_owned()),
                    key: key.to_owned(),
                    expected_version: None,
                    value: Some(json!({"generation": "fresh-v1"})),
                    artifact: None,
                    ttl_ms: 10_000,
                },
                "session-writer",
                30_003,
            )?);
            Ok(())
        },
    )?;
    let refreshed = refreshed.ok_or_else(|| anyhow::anyhow!("observation hook not reached"))?;
    assert_eq!(refreshed.version, 1);
    assert_ne!(
        hash_bytes(&before.encoded),
        refreshed.storage_readback.value_sha256
    );
    assert_eq!(get.entry.value, Some(json!({"generation": "fresh-v1"})));
    assert_eq!(get.entry.version, refreshed.version);
    assert_eq!(
        get.storage_readback.value_sha256,
        refreshed.storage_readback.value_sha256
    );
    let physical = read_workspace_raw_row_optional(&db, &workspace_row_key(run_id, key))?
        .ok_or_else(|| anyhow::anyhow!("fresh get row deleted"))?;
    assert_eq!(
        hash_bytes(&physical.encoded),
        refreshed.storage_readback.value_sha256
    );
    Ok(())
}

#[test]
fn must_fail_expired_get_does_not_claim_it_deleted_a_changed_expired_row() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let run_id = "run-issue90-expired-replacement";
    let key = "race/get";
    service.workspace_put_impl_at(
        WorkspacePutParams {
            run_id: Some(run_id.to_owned()),
            key: key.to_owned(),
            expected_version: None,
            value: Some(json!({"generation": "old"})),
            artifact: None,
            ttl_ms: 1,
        },
        "session-a",
        50_000,
    )?;
    let mut replacement = None;
    let error = match service.workspace_get_impl_at_after_observation(
        WorkspaceGetParams {
            run_id: Some(run_id.to_owned()),
            key: key.to_owned(),
        },
        "session-reader",
        50_005,
        || {
            replacement = Some(service.workspace_put_impl_at(
                WorkspacePutParams {
                    run_id: Some(run_id.to_owned()),
                    key: key.to_owned(),
                    expected_version: None,
                    value: Some(json!({"generation": "new-but-expired"})),
                    artifact: None,
                    ttl_ms: 1,
                },
                "session-writer",
                50_004,
            )?);
            Ok(())
        },
    ) {
        Ok(response) => anyhow::bail!("expired replacement returned live: {response:?}"),
        Err(error) => error,
    };
    assert_eq!(error_code(&error), Some(WORKSPACE_KEY_ABSENT));
    assert_eq!(error_detail_code(&error), Some("WORKSPACE_ROW_EXPIRED"));
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("expires_at_unix_ms")),
        Some(&json!(50_005))
    );
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("observed_expires_at_unix_ms")),
        Some(&json!(50_001))
    );
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("deleted_by_cleanup")),
        Some(&json!(false))
    );
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("physical_row_present")),
        Some(&json!(true))
    );
    let replacement = replacement.ok_or_else(|| anyhow::anyhow!("replacement was not written"))?;
    let db = service.workspace_db()?;
    let physical = read_workspace_raw_row_optional(&db, &workspace_row_key(run_id, key))?
        .ok_or_else(|| anyhow::anyhow!("changed expired row was deleted"))?;
    assert_eq!(
        hash_bytes(&physical.encoded),
        replacement.storage_readback.value_sha256
    );
    Ok(())
}

#[test]
fn must_fail_expired_list_skips_refreshed_cleanup_and_preserves_peer() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let run_id = "run-issue90-list";
    for (key, ttl_ms) in [
        ("race/expired", 1),
        ("race/peer", 10_000),
        ("race/peer2", 10_000),
    ] {
        service.workspace_put_impl_at(
            WorkspacePutParams {
                run_id: Some(run_id.to_owned()),
                key: key.to_owned(),
                expected_version: None,
                value: Some(json!({"key": key})),
                artifact: None,
                ttl_ms,
            },
            "session-a",
            40_000,
        )?;
    }
    let db = service.workspace_db()?;
    let old = read_workspace_raw_row_optional(&db, &workspace_row_key(run_id, "race/expired"))?
        .ok_or_else(|| anyhow::anyhow!("expired list row missing"))?;
    let peer_before =
        read_workspace_raw_row_optional(&db, &workspace_row_key(run_id, "race/peer"))?
            .ok_or_else(|| anyhow::anyhow!("peer missing"))?;
    let mut refreshed = None;
    let list = service.workspace_list_impl_at_after_observation(
        WorkspaceListParams {
            run_id: Some(run_id.to_owned()),
            prefix: Some("race/".to_owned()),
            limit: 1,
            include_values: true,
            cursor: None,
        },
        "session-reader",
        40_002,
        || {
            refreshed = Some(service.workspace_put_impl_at(
                WorkspacePutParams {
                    run_id: Some(run_id.to_owned()),
                    key: "race/expired".to_owned(),
                    expected_version: None,
                    value: Some(json!({"generation": "fresh"})),
                    artifact: None,
                    ttl_ms: 10_000,
                },
                "session-writer",
                40_002,
            )?);
            Ok(())
        },
    )?;
    let refreshed = refreshed.ok_or_else(|| anyhow::anyhow!("observation hook not reached"))?;
    assert_eq!(list.expired_rows_deleted, 0);
    assert!(list.has_next);
    assert!(list.next_cursor.is_some());
    assert_eq!(list.returned_count, 1);
    assert_eq!(list.entries[0].key, "race/peer");
    let next = service.workspace_list_impl_at(
        WorkspaceListParams {
            run_id: Some(run_id.to_owned()),
            prefix: Some("race/".to_owned()),
            limit: 1,
            include_values: true,
            cursor: list.next_cursor.clone(),
        },
        "session-reader",
        40_002,
    )?;
    assert!(next.complete);
    assert_eq!(next.entries[0].key, "race/peer2");
    assert_ne!(
        hash_bytes(&old.encoded),
        refreshed.storage_readback.value_sha256
    );
    let fresh = read_workspace_raw_row_optional(&db, &workspace_row_key(run_id, "race/expired"))?
        .ok_or_else(|| anyhow::anyhow!("refreshed list row deleted"))?;
    assert_eq!(
        hash_bytes(&fresh.encoded),
        refreshed.storage_readback.value_sha256
    );
    let peer_after = read_workspace_raw_row_optional(&db, &workspace_row_key(run_id, "race/peer"))?
        .ok_or_else(|| anyhow::anyhow!("peer deleted"))?;
    assert_eq!(peer_after.encoded, peer_before.encoded);
    assert_eq!(
        hash_bytes(&peer_after.encoded),
        hash_bytes(&peer_before.encoded)
    );
    assert_eq!(
        decode_workspace_row(peer_after.key, peer_after.encoded)
            .map_err(anyhow::Error::msg)?
            .entry
            .version,
        decode_workspace_row(peer_before.key, peer_before.encoded)
            .map_err(anyhow::Error::msg)?
            .entry
            .version,
    );
    Ok(())
}

#[test]
fn must_fail_expired_get_uses_absent_code_and_deletes_physical_row() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;

    service.workspace_put_impl_at(
        WorkspacePutParams {
            run_id: Some("run-issue85".to_owned()),
            key: "ttl/expired".to_owned(),
            expected_version: None,
            value: Some(json!({"issue": 85, "kind": "ttl"})),
            artifact: None,
            ttl_ms: 1,
        },
        "session-a",
        10_000,
    )?;

    let before = service.workspace_exists_impl_at(
        WorkspaceExistsParams {
            run_id: Some("run-issue85".to_owned()),
            key: "ttl/expired".to_owned(),
        },
        "session-b",
        10_002,
    )?;
    assert!(!before.exists);
    assert!(before.physical_row_present);
    assert_eq!(before.state, WorkspaceExistenceState::Expired);

    let error = match service.workspace_get_impl_at(
        WorkspaceGetParams {
            run_id: Some("run-issue85".to_owned()),
            key: "ttl/expired".to_owned(),
        },
        "session-c",
        10_003,
    ) {
        Ok(response) => anyhow::bail!("expired get unexpectedly succeeded: {response:?}"),
        Err(error) => error,
    };
    assert_eq!(error_code(&error), Some(WORKSPACE_KEY_ABSENT));
    assert_eq!(error_detail_code(&error), Some("WORKSPACE_ROW_EXPIRED"));
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("deleted_by_cleanup")),
        Some(&json!(true))
    );
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("physical_row_present")),
        Some(&json!(false))
    );

    let after = service.workspace_exists_impl_at(
        WorkspaceExistsParams {
            run_id: Some("run-issue85".to_owned()),
            key: "ttl/expired".to_owned(),
        },
        "session-d",
        10_004,
    )?;
    assert!(!after.exists);
    assert!(!after.physical_row_present);
    assert_eq!(after.state, WorkspaceExistenceState::Absent);
    assert_eq!(
        after
            .absent_readback
            .as_ref()
            .map(|readback| readback.exact_match_count),
        Some(0)
    );
    Ok(())
}

#[test]
fn must_fail_stale_workspace_put_reports_conflict_without_changing_row() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let run_id = "run-issue87";
    let key = "cas/item";
    let first = service.workspace_put_impl_at(
        WorkspacePutParams {
            run_id: Some(run_id.to_owned()),
            key: key.to_owned(),
            expected_version: None,
            value: Some(json!({"quantity": 3})),
            artifact: None,
            ttl_ms: 60_000,
        },
        "session-a",
        10_000,
    )?;
    let before = service.workspace_get_impl_at(
        WorkspaceGetParams {
            run_id: Some(run_id.to_owned()),
            key: key.to_owned(),
        },
        "session-b",
        10_001,
    )?;
    let error = match service.workspace_put_impl_at(
        WorkspacePutParams {
            run_id: Some(run_id.to_owned()),
            key: key.to_owned(),
            expected_version: Some(999),
            value: Some(json!({"quantity": 4})),
            artifact: None,
            ttl_ms: 60_000,
        },
        "session-c",
        10_002,
    ) {
        Ok(response) => anyhow::bail!("stale put unexpectedly succeeded: {response:?}"),
        Err(error) => error,
    };
    assert_eq!(error_code(&error), Some("WORKSPACE_VERSION_CONFLICT"));
    assert_eq!(
        error_detail_code(&error),
        Some("WORKSPACE_VERSION_CONFLICT")
    );
    assert_eq!(error.code.0, -32099);
    let after = service.workspace_get_impl_at(
        WorkspaceGetParams {
            run_id: Some(run_id.to_owned()),
            key: key.to_owned(),
        },
        "session-d",
        10_003,
    )?;
    assert_eq!(first.version, 1);
    assert_eq!(before.entry.version, after.entry.version);
    assert_eq!(before.entry.value, after.entry.value);
    assert_eq!(
        before.storage_readback.value_sha256,
        after.storage_readback.value_sha256
    );
    Ok(())
}

#[test]
fn must_pass_workspace_list_pages_recover_every_physical_row() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let run_id = "run-issue86-chain";
    for (index, quantity) in [(1, 12), (2, 23), (3, 34)] {
        service.workspace_put_impl_at(
            WorkspacePutParams {
                run_id: Some(run_id.to_owned()),
                key: format!("orders/{index:02}"),
                expected_version: None,
                value: Some(json!({"quantity": quantity})),
                artifact: None,
                ttl_ms: 60_000,
            },
            "writer",
            10_000,
        )?;
    }
    let db = service.workspace_db()?;
    let physical = db.scan_cf_prefix(
        cf::CF_KV,
        format!("{}{}", workspace_run_prefix(run_id), hex_bytes(b"orders/")).as_bytes(),
    )?;
    assert_eq!(physical.len(), 3);

    let mut cursor: Option<String> = None;
    let mut observed = Vec::new();
    for page_number in 0..3 {
        let params: WorkspaceListParams = serde_json::from_value(json!({
            "run_id": run_id,
            "prefix": "orders/",
            "limit": 2,
            "include_values": false,
            "cursor": cursor,
        }))?;
        let page = service.workspace_list_impl_at(params, "reader", 10_001)?;
        assert!(page.entries.iter().all(|entry| entry.value.is_none()));
        observed.extend(
            page.readback_rows
                .iter()
                .map(|row| (row.row_key.as_bytes().to_vec(), row.value_sha256.clone())),
        );
        let wire = serde_json::to_value(&page)?;
        let complete = wire.get("complete").and_then(Value::as_bool);
        let has_next = wire.get("has_next").and_then(Value::as_bool);
        let next = wire.get("next_cursor").and_then(Value::as_str);
        if page_number == 0 {
            assert_eq!(complete, Some(false));
            assert_eq!(has_next, Some(true));
            cursor = Some(
                next.ok_or_else(|| anyhow::anyhow!("missing continuation"))?
                    .to_owned(),
            );
        } else {
            assert_eq!(complete, Some(true));
            assert_eq!(has_next, Some(false));
            assert!(next.is_none());
            break;
        }
    }
    let expected = physical
        .iter()
        .map(|(key, encoded)| (key.clone(), hash_bytes(encoded)))
        .collect::<Vec<_>>();
    assert_eq!(observed, expected);
    Ok(())
}

fn list_params(
    run_id: &str,
    prefix: &str,
    limit: usize,
    cursor: Option<String>,
) -> WorkspaceListParams {
    WorkspaceListParams {
        run_id: Some(run_id.to_owned()),
        prefix: Some(prefix.to_owned()),
        limit,
        include_values: false,
        cursor,
    }
}

#[test]
fn must_fail_workspace_list_exact_limit_and_empty_prefix_are_complete() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let run_id = "run-issue86-boundary";
    for key in ["orders/01", "orders/02", "other/01"] {
        service.workspace_put_impl_at(
            WorkspacePutParams {
                run_id: Some(run_id.to_owned()),
                key: key.to_owned(),
                expected_version: None,
                value: Some(json!({"key": key})),
                artifact: None,
                ttl_ms: 60_000,
            },
            "writer",
            10_000,
        )?;
    }
    let exact = service.workspace_list_impl_at(
        list_params(run_id, "orders/", 2, None),
        "reader",
        10_001,
    )?;
    assert_eq!(exact.entries.len(), 2);
    assert_eq!(exact.scanned_rows, 2);
    assert!(exact.complete);
    assert!(!exact.has_next);
    assert!(exact.next_cursor.is_none());

    let empty = service.workspace_list_impl_at(
        list_params(run_id, "missing/", 2, None),
        "reader",
        10_001,
    )?;
    assert!(empty.entries.is_empty());
    assert_eq!(empty.scanned_rows, 0);
    assert!(empty.complete);
    assert!(!empty.has_next);
    assert!(empty.next_cursor.is_none());
    Ok(())
}

#[test]
fn must_fail_workspace_list_empty_expired_page_resumes_to_corrupt_and_live_rows()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let run_id = "run-issue86-dead-window";
    let live = service.workspace_put_impl_at(
        WorkspacePutParams {
            run_id: Some(run_id.to_owned()),
            key: "bucket/live".to_owned(),
            expected_version: None,
            value: Some(json!({"status": "live"})),
            artifact: None,
            ttl_ms: 60_000,
        },
        "writer",
        10_000,
    )?;
    let db = service.workspace_db()?;
    let template = service
        .workspace_get_impl_at(
            WorkspaceGetParams {
                run_id: Some(run_id.to_owned()),
                key: "bucket/live".to_owned(),
            },
            "reader",
            10_000,
        )?
        .entry;
    let mut fixture = Vec::new();
    for index in 0..WORKSPACE_LIST_RAW_BUDGET {
        let key = format!("bucket/dead/{index:04}");
        let row_key = workspace_row_key(run_id, &key);
        let mut entry = template.clone();
        entry.key = key;
        entry.row_key = row_key.clone();
        entry.expires_at_unix_ms = 10_001;
        fixture.push((row_key.into_bytes(), synapse_storage::encode_json(&entry)?));
    }
    let corrupt_key = workspace_row_key(run_id, "bucket/error");
    fixture.push((corrupt_key.into_bytes(), b"not-json".to_vec()));
    db.put_batch(cf::CF_KV, fixture)?;
    db.flush()?;

    let first = service.workspace_list_impl_at(
        list_params(run_id, "bucket/", 1, None),
        "reader",
        10_002,
    )?;
    assert_eq!(first.scanned_rows, WORKSPACE_LIST_RAW_BUDGET);
    assert_eq!(first.expired_rows_deleted, WORKSPACE_LIST_RAW_BUDGET);
    assert!(first.entries.is_empty());
    assert!(first.has_next);
    let second = service.workspace_list_impl_at(
        list_params(run_id, "bucket/", 1, first.next_cursor),
        "reader",
        10_003,
    )?;
    assert_eq!(second.scanned_rows, 2);
    assert_eq!(second.corrupt_rows_skipped.len(), 1);
    assert_eq!(
        second.corrupt_rows_skipped[0].value_sha256,
        hash_bytes(b"not-json")
    );
    assert_eq!(second.entries.len(), 1);
    assert_eq!(second.entries[0].key, "bucket/live");
    assert_eq!(
        second.readback_rows[0].value_sha256,
        live.storage_readback.value_sha256
    );
    assert!(second.complete);
    assert!(second.next_cursor.is_none());
    let remaining = db.scan_cf_prefix(cf::CF_KV, workspace_run_prefix(run_id).as_bytes())?;
    assert_eq!(remaining.len(), 2);
    Ok(())
}

#[test]
fn must_fail_workspace_list_cursor_rejects_tampering_and_cross_scope_use() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let run_id = "run-issue86-cursor";
    for key in ["bucket/01", "bucket/02"] {
        service.workspace_put_impl_at(
            WorkspacePutParams {
                run_id: Some(run_id.to_owned()),
                key: key.to_owned(),
                expected_version: None,
                value: Some(json!({"key": key})),
                artifact: None,
                ttl_ms: 60_000,
            },
            "writer",
            10_000,
        )?;
    }
    let page = service.workspace_list_impl_at(
        list_params(run_id, "bucket/", 1, None),
        "reader",
        10_001,
    )?;
    let cursor = page
        .next_cursor
        .ok_or_else(|| anyhow::anyhow!("expected cursor"))?;
    let mut tampered = cursor.clone().into_bytes();
    tampered[0] = if tampered[0] == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(tampered)?;
    let encoded_payload = cursor
        .split_once('.')
        .ok_or_else(|| anyhow::anyhow!("cursor missing signature"))?
        .0;
    let mut wrong_version: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded_payload)?)?;
    wrong_version["version"] = json!(2);
    let wrong_version_payload = serde_json::to_vec(&wrong_version)?;
    let wrong_version_cursor = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&wrong_version_payload),
        URL_SAFE_NO_PAD.encode(workspace_cursor_signature(&wrong_version_payload))
    );
    for params in [
        list_params(run_id, "bucket/", 1, Some(tampered)),
        list_params("another-run", "bucket/", 1, Some(cursor.clone())),
        list_params(run_id, "other/", 1, Some(cursor.clone())),
        list_params(run_id, "bucket/", 1, Some("not-a-cursor".to_owned())),
        list_params(run_id, "bucket/", 1, Some(wrong_version_cursor)),
    ] {
        let error = match service.workspace_list_impl_at(params, "reader", 10_002) {
            Ok(response) => anyhow::bail!("hostile cursor succeeded: {response:?}"),
            Err(error) => error,
        };
        assert_eq!(error_code(&error), Some(error_codes::TOOL_PARAMS_INVALID));
        assert_eq!(error_detail_code(&error), Some("WORKSPACE_CURSOR_INVALID"));
    }
    Ok(())
}

#[test]
fn must_fail_workspace_list_unicode_max_key_and_live_mutation_order() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let run_id = "run-issue86-unicode";
    let max_key = format!("é/{}", "x".repeat(MAX_KEY_BYTES - "é/".len()));
    for key in ["é/01", "é/03", &max_key, "β/01"] {
        service.workspace_put_impl_at(
            WorkspacePutParams {
                run_id: Some(run_id.to_owned()),
                key: key.to_owned(),
                expected_version: None,
                value: Some(json!({"key": key})),
                artifact: None,
                ttl_ms: 60_000,
            },
            "writer",
            10_000,
        )?;
    }
    let first =
        service.workspace_list_impl_at(list_params(run_id, "é/", 1, None), "reader", 10_001)?;
    assert_eq!(first.entries[0].key, "é/01");
    assert!(first.has_next);
    for key in ["é/00", "é/02"] {
        service.workspace_put_impl_at(
            WorkspacePutParams {
                run_id: Some(run_id.to_owned()),
                key: key.to_owned(),
                expected_version: None,
                value: Some(json!({"key": key})),
                artifact: None,
                ttl_ms: 60_000,
            },
            "writer",
            10_002,
        )?;
    }
    let mut cursor = first.next_cursor;
    let mut resumed = Vec::new();
    for _ in 0..4 {
        let page = service.workspace_list_impl_at(
            list_params(run_id, "é/", 1, cursor),
            "reader",
            10_003,
        )?;
        resumed.extend(page.entries.iter().map(|entry| entry.key.clone()));
        if page.complete {
            break;
        }
        cursor = page.next_cursor;
    }
    assert_eq!(resumed, vec!["é/02".to_owned(), "é/03".to_owned(), max_key]);
    let physical = service.workspace_db()?.scan_cf_prefix(
        cf::CF_KV,
        format!(
            "{}{}",
            workspace_run_prefix(run_id),
            hex_bytes("é/".as_bytes())
        )
        .as_bytes(),
    )?;
    assert_eq!(physical.len(), 5);
    Ok(())
}

#[test]
fn must_fail_workspace_list_value_budget_resumes_large_rows_without_gaps() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let service = service_with_db(temp.path())?;
    let run_id = "run-issue86-value-budget";
    service.workspace_put_impl_at(
        WorkspacePutParams {
            run_id: Some(run_id.to_owned()),
            key: "large/0000".to_owned(),
            expected_version: None,
            value: Some(json!({"seed": true})),
            artifact: None,
            ttl_ms: 60_000,
        },
        "writer",
        10_000,
    )?;
    let template = service
        .workspace_get_impl_at(
            WorkspaceGetParams {
                run_id: Some(run_id.to_owned()),
                key: "large/0000".to_owned(),
            },
            "reader",
            10_000,
        )?
        .entry;
    let db = service.workspace_db()?;
    // ~11 MB of values: over the 8 MiB page budget, under the 16 MiB workspace quota (#164).
    let mut fixture = Vec::new();
    for index in 1..=50 {
        let key = format!("large/{index:04}");
        let row_key = workspace_row_key(run_id, &key);
        let mut entry = template.clone();
        entry.key = key;
        entry.row_key = row_key.clone();
        entry.value = Some(json!({"blob": "x".repeat(220_000)}));
        fixture.push((row_key.into_bytes(), synapse_storage::encode_json(&entry)?));
    }
    db.put_batch(cf::CF_KV, fixture)?;
    db.flush()?;

    let first = service.workspace_list_impl_at(
        list_params(run_id, "large/", MAX_LIST_LIMIT, None),
        "reader",
        10_001,
    )?;
    assert!(first.scanned_value_bytes <= WORKSPACE_LIST_VALUE_BUDGET_BYTES);
    assert!(first.scanned_rows < 51);
    assert!(first.has_next);
    let second = service.workspace_list_impl_at(
        list_params(run_id, "large/", MAX_LIST_LIMIT, first.next_cursor),
        "reader",
        10_002,
    )?;
    assert!(second.complete);
    let observed = first
        .entries
        .into_iter()
        .chain(second.entries)
        .map(|entry| entry.key)
        .collect::<Vec<_>>();
    let expected = (0..=50)
        .map(|index| format!("large/{index:04}"))
        .collect::<Vec<_>>();
    assert_eq!(observed, expected);
    Ok(())
}
