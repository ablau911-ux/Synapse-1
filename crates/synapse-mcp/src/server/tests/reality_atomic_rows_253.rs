//! NEEDS-SEAM, child of reality.rs:
//! write_reality_rows_with(rows:&[(String,Value)],
//! commit:impl FnOnce(Vec<(Vec<u8>,Vec<u8>)>)->Result<(),ErrorData>,
//! read:impl FnMut(&[u8])->Result<Option<Vec<u8>>,ErrorData>)
//! -> Result<Vec<RealityRowReadback>,ErrorData>.
//! Baseline and delta callers must supply all rows + head in ONE invocation;
//! production commit invokes existing storage_put_kv_rows exactly once.
use super::*;
use std::{cell::RefCell, collections::BTreeMap};

/// #270: use the real baseline/delta callers and real pressure-aware storage,
/// rather than the injected commit closure used by the atomicity tests below.
#[tokio::test]
async fn must_fail_reality_baseline_and_delta_pressure_writes_with_shed_270() -> anyhow::Result<()>
{
    use super::tests::{
        install_synthetic_input, service_with_profile_dir, synthetic_input, write_profile,
    };

    let temp = tempfile::tempdir()?;
    let profiles = temp.path().join("profiles");
    std::fs::create_dir_all(&profiles)?;
    write_profile(
        &profiles.join("synthetic.toml"),
        "synthetic",
        "single_player",
    )?;
    let service = service_with_profile_dir(temp.path(), &profiles)?;
    install_synthetic_input(&service, synthetic_input("Before pressure"))?;
    let baseline_params = RealityBaselineParams {
        profile_id: Some("synthetic".to_owned()),
        epoch_id: None,
        force_new_epoch: true,
        include: Vec::new(),
        depth: DEFAULT_DEPTH,
        max_elements: DEFAULT_MAX_ELEMENTS,
    };
    let baseline = service
        .reality_baseline(Parameters(baseline_params.clone()))
        .await?
        .0;
    let db = service
        .reflex_runtime()?
        .lock()
        .map_err(|_| anyhow::anyhow!("reflex runtime lock poisoned"))?
        .storage_db();
    let report = db.run_pressure_check_with_free_bytes_sample(0)?;
    assert_eq!(
        report.current_level,
        synapse_storage::DiskPressureLevel::Level4
    );
    assert!(!db.pressure_permits_write(cf::CF_KV));
    let before = db.scan_cf(cf::CF_KV)?;
    assert!(
        before
            .iter()
            .any(|(key, _)| key == baseline.head.baseline_row_key.as_bytes())
    );
    install_synthetic_input(&service, synthetic_input("Changed under pressure"))?;

    // Collect both results before asserting codes, so a baseline-code failure
    // does not prevent exercising the delta writer in this regression.
    let Err(baseline_error) = service.reality_baseline(Parameters(baseline_params)).await else {
        panic!("replacement baseline must be shed");
    };
    assert_eq!(
        db.scan_cf(cf::CF_KV)?,
        before,
        "baseline wrote under pressure"
    );
    let Err(delta_error) = service
        .observe_delta(Parameters(ObserveDeltaParams {
            profile_id: Some("synthetic".to_owned()),
            since_epoch: Some(baseline.baseline.epoch_id),
            since_seq: Some(baseline.head.head_seq),
            include: Vec::new(),
            depth: DEFAULT_DEPTH,
            max_elements: DEFAULT_MAX_ELEMENTS,
            max_deltas: DEFAULT_MAX_DELTAS,
        }))
        .await
    else {
        panic!("changed delta must be shed");
    };
    assert_eq!(
        db.scan_cf(cf::CF_KV)?,
        before,
        "delta/head wrote under pressure"
    );
    for (path, error) in [("baseline", baseline_error), ("delta", delta_error)] {
        let expected = mcp_error(error_codes::STORAGE_WRITE_SHED, "reference");
        assert_eq!(error.data, expected.data, "{path}: {error:?}");
        assert_eq!(error.code, expected.code, "{path}: JSON-RPC error code");
    }
    Ok(())
}

fn rows(delta: bool) -> Vec<(String, Value)> {
    let kind = if delta { "delta" } else { "baseline" };
    vec![
        (
            format!("reality/{kind}/253"),
            json!({"epoch_id":"epoch253","seq":1}),
        ),
        (
            "reality/head/253".into(),
            json!({"epoch_id":"epoch253","head_seq":1}),
        ),
    ]
}

#[test]
fn must_pass_baseline_and_delta_each_commit_all_rows_and_head_once() {
    for delta in [false, true] {
        let db = RefCell::new(BTreeMap::<Vec<u8>, Vec<u8>>::new());
        assert_eq!(db.borrow().len(), 0);
        let mut batches = Vec::new();
        let input = rows(delta);
        let readbacks = write_reality_rows_with(
            &input,
            |batch| {
                batches.push(batch.clone());
                db.borrow_mut().extend(batch);
                Ok(())
            },
            |key| Ok(db.borrow().get(key).cloned()),
        )
        .expect("commit");
        assert_eq!(batches.len(), 1, "one atomic storage batch per transition");
        assert_eq!(batches[0].len(), 2);
        assert_eq!(readbacks.len(), 2);
        for (key, value) in input {
            let bytes = db
                .borrow()
                .get(key.as_bytes())
                .expect("physical row")
                .clone();
            assert_eq!(serde_json::from_slice::<Value>(&bytes).expect("row"), value);
        }
    }
}

#[test]
fn must_fail_storage_rejection_leaves_prior_head_and_history_unchanged() {
    for delta in [false, true] {
        let original = BTreeMap::from([(b"reality/head/253".to_vec(), b"prior-head".to_vec())]);
        let db = RefCell::new(original.clone());
        assert_eq!(*db.borrow(), original);
        let mut reads = 0;
        let result = write_reality_rows_with(
            &rows(delta),
            |batch| {
                assert_eq!(batch.len(), 2);
                Err(mcp_error(
                    error_codes::STORAGE_WRITE_FAILED,
                    "synthetic atomic batch rejection",
                ))
            },
            |key| {
                reads += 1;
                Ok(db.borrow().get(key).cloned())
            },
        );
        let error = result.expect_err("storage refusal propagated");
        assert_eq!(
            error
                .data
                .as_ref()
                .and_then(|v| v.get("code"))
                .and_then(Value::as_str),
            Some(error_codes::STORAGE_WRITE_FAILED)
        );
        assert_eq!(reads, 0, "failed commit cannot produce success readbacks");
        assert_eq!(*db.borrow(), original);
    }
}

#[test]
fn must_fail_baseline_and_delta_callers_cannot_bypass_atomic_pair_commit() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server/reality.rs"),
    )
    .expect("real caller source");
    let baseline = source
        .split("fn existing_baseline_response(")
        .next()
        .expect("baseline section");
    assert!(!baseline.contains("self.write_kv_json_readback(&baseline_row_key"));
    assert!(
        baseline.contains("write_reality_rows"),
        "baseline bypasses tested batch extraction"
    );
    let delta = source
        .split("fn observe_reality_delta(")
        .nth(1)
        .expect("delta caller")
        .split("fn ")
        .next()
        .expect("delta body");
    assert!(
        !delta.contains("self.write_kv_json_readback("),
        "delta/head still separate"
    );
    assert!(
        delta.contains("write_reality_rows"),
        "delta bypasses tested batch extraction"
    );
}

#[test]
fn must_pass_user_content_digest_remains_keyed_while_row_integrity_is_plain_sha256() {
    let expected = format!(
        "hmac-sha256:{}",
        crate::content_digest::keyed_digest_hex(b"value253")
    );
    assert_eq!(non_empty_keyed_hash("  value253  "), Some(expected));
    assert_eq!(non_empty_keyed_hash(" \n\t "), None);
    let plain = format!(
        "sha256:{}",
        <Sha256 as sha2::Digest>::digest(b"value253")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    assert_eq!(hash_bytes(b"value253"), plain);
    assert_ne!(non_empty_keyed_hash("value253"), Some(plain));
}

fn keys(schema: &Value) -> std::collections::BTreeSet<String> {
    schema["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .cloned()
        .collect()
}

fn expected(fields: &[&str]) -> std::collections::BTreeSet<String> {
    fields.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn must_pass_baseline_public_response_fields_remain_unchanged() {
    let baseline =
        serde_json::to_value(rmcp::schemars::schema_for!(RealityBaselineResponse)).expect("schema");
    assert_eq!(
        keys(&baseline),
        expected(&[
            "ok",
            "created",
            "profile_key",
            "baseline",
            "baseline_required",
            "rebase_required",
            "reason",
            "head",
            "readback_rows",
            "size_bytes",
            "size_estimate_tokens"
        ])
    );
}

#[test]
fn must_pass_delta_public_response_fields_remain_unchanged() {
    let delta =
        serde_json::to_value(rmcp::schemars::schema_for!(ObserveDeltaResponse)).expect("schema");
    assert_eq!(
        keys(&delta),
        expected(&[
            "ok",
            "profile_key",
            "epoch_id",
            "from_seq",
            "to_seq",
            "deltas",
            "cursor",
            "baseline_required",
            "rebase_required",
            "reason",
            "readback_rows",
            "published_sse_events",
            "size_bytes",
            "size_estimate_tokens"
        ])
    );
}
