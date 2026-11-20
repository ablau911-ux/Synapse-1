//! #58 regression: `model operation=probe` writes probe-evidence rows (the
//! contract row declares `mutates_state=true`), so it must be refused outside a
//! maintenance profile exactly like its register/update/remove siblings, while
//! the read-only `list` and `status` stay available to `normal_agent`.

use super::*;

#[test]
fn must_fail_probe_requires_maintenance_profile_58() {
    assert!(
        requires_maintenance_profile(ModelOperation::Probe),
        "probe persists probe evidence and must be gated"
    );
}

#[test]
fn must_fail_mutating_siblings_stay_gated_58() {
    for operation in [
        ModelOperation::Register,
        ModelOperation::Update,
        ModelOperation::Remove,
    ] {
        assert!(requires_maintenance_profile(operation), "{operation:?}");
    }
}

const SEEDED_MODEL: &str = "probe-seed-58";
/// `m3/local_models.rs` `REGISTRY_PREFIX` / `PROBE_EVIDENCE_PREFIX`.
const REGISTRY_ROWS: &[u8] = b"local_model_registry/v1/model/name_hex/";
const PROBE_EVIDENCE_ROWS: &[u8] = b"local_model_registry/v1/probe/name_hex/";

fn model_params(value: serde_json::Value) -> ModelParams {
    serde_json::from_value(value).expect("valid synthetic model params")
}

fn error_code(error: &ErrorData) -> Option<&str> {
    error
        .data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(serde_json::Value::as_str)
}

fn cf_kv_rows(service: &SynapseService, prefix: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    service
        .m3_storage()
        .expect("m3 storage")
        .scan_cf_prefix(synapse_storage::cf::CF_KV, prefix)
        .expect("scan CF_KV")
}

/// Seeds one registry row so an ungated probe would have something to update
/// (`probe_local_model` rewrites the row's `last_probe`), then reads it back
/// through the read-only `model operation=list` facade.
async fn seed_registry_row(service: &SynapseService) {
    let name_hex: String = SEEDED_MODEL
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let row_key = format!("{}{name_hex}", String::from_utf8_lossy(REGISTRY_ROWS));
    let row = crate::m3::local_models::LocalModelRegistryRow {
        schema_version: 1,
        row_key: row_key.clone(),
        name: SEEDED_MODEL.to_owned(),
        // Port 9 (discard) on loopback: nothing real is ever contacted.
        base_url: "http://127.0.0.1:9/v1".to_owned(),
        model_id: "seed-model".to_owned(),
        api_shape: crate::m3::local_models::LocalModelApiShape::default(),
        runtime_preset: crate::m3::local_models::LocalModelRuntimePreset::default(),
        context_length: None,
        max_tools: None,
        notes: None,
        enabled: true,
        allow_non_loopback: false,
        api_key_env_var: None,
        created_at_unix_ms: 1,
        updated_at_unix_ms: 1,
        created_by_session: "seed-58".to_owned(),
        updated_by_session: "seed-58".to_owned(),
        last_probe: None,
        has_api_key_secret: false,
    };
    service
        .m3_storage()
        .expect("m3 storage")
        .put_batch_pressure_bypass(
            synapse_storage::cf::CF_KV,
            [(
                row_key.into_bytes(),
                serde_json::to_vec(&row).expect("encode seed row"),
            )],
        )
        .expect("seed registry row");
    // Fixture precondition readback: the facade itself sees the row, unprobed.
    let listed = service
        .model_for_session(
            // The facade requires exactly one payload matching the operation
            // (`validation.rs` `validate_model_params`); `{}` = list defaults
            // (all rows incl. disabled, `LocalModelListParams` serde defaults).
            model_params(serde_json::json!({ "operation": "list", "list": {} })),
            None,
        )
        .await
        .expect("stdio list")
        .0
        .list
        .expect("list response");
    let seeded = listed
        .rows
        .iter()
        .find(|row| row.name == SEEDED_MODEL)
        .expect("seeded row is visible to model operation=list");
    assert!(seeded.last_probe.is_none(), "seed row starts unprobed");
}

/// Contract: issue #58 + `operational_facades/policy.rs`
/// `require_maintenance_profile_for_session` — `model operation=probe`
/// mutates state, so a session outside `break_glass`/`full_capability` is
/// refused with `TOOL_PROFILE_POLICY_DENIED` before any probe runs, and no
/// registry or probe-evidence row changes.
#[tokio::test]
async fn must_fail_model_probe_under_normal_agent_is_refused_and_writes_no_row_58() {
    let root = tempfile::tempdir().expect("temp dir");
    let service = SynapseService::try_new_isolated_for_tests(root.path()).expect("service");
    seed_registry_row(&service).await;
    let session = "normal-agent-58";
    // Precondition readback: a fresh MCP session gets the least-privilege
    // default profile (`tool_profiles.rs` `ensure_tool_profile_assignment`).
    assert_eq!(
        service
            .tool_profile_snapshot(Some(session))
            .expect("profile snapshot")
            .profile,
        crate::server::tool_profiles::ToolProfileKind::NormalAgent
    );
    let registry_before = cf_kv_rows(&service, REGISTRY_ROWS);
    assert!(cf_kv_rows(&service, PROBE_EVIDENCE_ROWS).is_empty());

    let error = service
        .model_for_session(
            model_params(serde_json::json!({
                "operation": "probe",
                "probe": { "name": SEEDED_MODEL },
            })),
            Some(session),
        )
        .await
        .err()
        .expect("normal_agent must not run model operation=probe");

    assert_eq!(
        error_code(&error),
        Some(synapse_core::error_codes::TOOL_PROFILE_POLICY_DENIED),
        "{error:?}"
    );
    assert_eq!(
        cf_kv_rows(&service, REGISTRY_ROWS),
        registry_before,
        "registry rows must be byte-identical after a refused probe"
    );
    assert!(
        cf_kv_rows(&service, PROBE_EVIDENCE_ROWS).is_empty(),
        "a refused probe writes no probe-evidence row"
    );
}

/// Must NOT change (same contract): the maintenance profile passes the gate.
/// Unscoped stdio is `break_glass` (`tool_profile_snapshot(None)`), so a probe
/// of an unregistered name reaches the registry and fails there with
/// `MODEL_REGISTRY_NOT_FOUND` (`local_models.rs` `read_model_row_required`),
/// not with the policy refusal.
#[tokio::test]
async fn must_pass_model_probe_under_maintenance_profile_passes_the_gate_58() {
    let root = tempfile::tempdir().expect("temp dir");
    let service = SynapseService::try_new_isolated_for_tests(root.path()).expect("service");
    assert_eq!(
        service
            .tool_profile_snapshot(None)
            .expect("profile snapshot")
            .profile,
        crate::server::tool_profiles::ToolProfileKind::BreakGlass
    );

    let error = service
        .model_for_session(
            model_params(serde_json::json!({
                "operation": "probe",
                "probe": { "name": "absent-model-58" },
            })),
            None,
        )
        .await
        .err()
        .expect("an unregistered model cannot be probed");

    assert_eq!(
        error_code(&error),
        Some(synapse_core::error_codes::MODEL_REGISTRY_NOT_FOUND),
        "{error:?}"
    );
}

#[test]
fn must_pass_read_only_operations_stay_open_to_normal_agent_58() {
    for operation in [ModelOperation::List, ModelOperation::Status] {
        assert!(!requires_maintenance_profile(operation), "{operation:?}");
    }
}
