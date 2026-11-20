//! #204: reflex dispatch (not only registration) is denied for live-server
//! profiles. The dispatch-time gate (`context.rs`
//! `ReflexScopeActionGate::ensure_action_allowed`) evaluates the target policy
//! for tool `reflex_dispatch`. These tests drive that evaluation
//! (`evaluate_supported_use_with_optional_process_for_tool`) and the real gate
//! (`reflex_action_gate_with_enforced`) with a fully configured
//! operator-attended live-server profile, so removing `reflex_dispatch` from
//! the deny list, the deny-list check, or the gate's policy call fails.
use std::collections::BTreeMap;

use synapse_core::{ForegroundContext, Profile, Rect, error_codes};

use super::{
    KEY_FOREGROUND_ONLY, KEY_LIVE_SERVER_ALLOWED, KEY_NO_ACCOUNT_OR_BILLING_AUTOMATION,
    KEY_NO_DESTRUCTIVE_UI_AUTOMATION, KEY_NO_MEMORY_OR_PROTOCOL_HOOKS,
    KEY_NO_PVP_GROUP_GUILD_RAID_AUTOMATION, KEY_NO_SOCIAL_OR_ECONOMY_AUTOMATION,
    KEY_NO_UNATTENDED_LOOPS, KEY_NO_UNATTENDED_SCALED_OPERATION, KEY_OPERATOR_ATTENDED_REQUIRED,
    KEY_OPERATOR_OWNED_CHARACTER_REQUIRED, KEY_RUNTIME_EVERQUEST_EXE, KEY_RUNTIME_EVERQUEST_SERVER,
    evaluate_supported_use_with_optional_process_for_tool, live_server_denied_tool_detail,
    target_policy_denied_error,
};

const GAME_EXE: &str = r"C:\Games\EverQuest\eqgame.exe";

/// A live-server profile that passes every metadata gate, so the only thing
/// left to decide is the per-tool deny list.
fn live_server_profile() -> Profile {
    let path = synapse_profiles::bundled_profiles_dir().join("everquest.live.toml");
    let mut profile = synapse_profiles::parse_profile_file(&path)
        .expect("bundled everquest.live profile parses")
        .profile;
    let mut metadata: BTreeMap<String, String> = [
        KEY_LIVE_SERVER_ALLOWED,
        KEY_OPERATOR_ATTENDED_REQUIRED,
        KEY_OPERATOR_OWNED_CHARACTER_REQUIRED,
        KEY_FOREGROUND_ONLY,
        KEY_NO_MEMORY_OR_PROTOCOL_HOOKS,
        KEY_NO_UNATTENDED_LOOPS,
        KEY_NO_SOCIAL_OR_ECONOMY_AUTOMATION,
        KEY_NO_UNATTENDED_SCALED_OPERATION,
        KEY_NO_ACCOUNT_OR_BILLING_AUTOMATION,
        KEY_NO_PVP_GROUP_GUILD_RAID_AUTOMATION,
        KEY_NO_DESTRUCTIVE_UI_AUTOMATION,
    ]
    .into_iter()
    .map(|key| (key.to_owned(), "true".to_owned()))
    .collect();
    metadata.insert(KEY_RUNTIME_EVERQUEST_EXE.to_owned(), GAME_EXE.to_owned());
    metadata.insert(
        KEY_RUNTIME_EVERQUEST_SERVER.to_owned(),
        "Frostreaver".to_owned(),
    );
    profile.metadata = metadata;
    profile
}

fn game_foreground(profile: &Profile) -> ForegroundContext {
    ForegroundContext {
        hwnd: 0x2040,
        pid: 4242,
        process_name: "eqgame.exe".to_owned(),
        process_path: GAME_EXE.to_owned(),
        window_title: "EverQuest".to_owned(),
        window_bounds: Rect {
            x: 0,
            y: 0,
            w: 1280,
            h: 720,
        },
        monitor_index: 0,
        dpi_scale: 1.0,
        profile_id: Some(profile.id.clone()),
        steam_appid: None,
        is_fullscreen: false,
        is_dwm_composed: true,
    }
}

#[test]
fn must_fail_reflex_dispatch_is_denied_by_the_live_server_target_policy() {
    let profile = live_server_profile();
    let foreground = game_foreground(&profile);

    let denial = evaluate_supported_use_with_optional_process_for_tool(
        &profile,
        &foreground,
        "reflex_dispatch",
        None,
    )
    .expect_err("reflex_dispatch must be denied for an operator-attended live-server profile");

    assert_eq!(denial.reason, "live_server_tool_not_foreground_input");
    assert_eq!(denial.profile_id, profile.id);
    assert_eq!(denial.foreground_pid, 4242);
    assert_eq!(
        Some(denial.detail.as_str()),
        live_server_denied_tool_detail("reflex_dispatch")
    );
    let error = target_policy_denied_error("reflex_dispatch", denial);
    assert_eq!(
        error.data.as_ref().and_then(|data| data.get("code")),
        Some(&serde_json::json!(
            error_codes::SAFETY_PROFILE_ACTION_DENIED
        ))
    );
}

/// Must NOT change: the same profile and foreground still allow supervised
/// foreground input, so the denial above is the deny list, not a metadata gap.
#[test]
fn must_pass_live_server_policy_still_allows_foreground_input_tools() {
    let profile = live_server_profile();
    let foreground = game_foreground(&profile);

    let state = evaluate_supported_use_with_optional_process_for_tool(
        &profile,
        &foreground,
        "act_press",
        None,
    )
    .expect("act_press is supervised foreground input on a live-server profile");

    assert_eq!(state.profile_id, profile.id);
    assert_eq!(state.foreground_pid, 4242);
}

#[test]
fn must_fail_live_server_policy_denies_reflex_register_and_dispatch_alike() {
    let profile = live_server_profile();
    let foreground = game_foreground(&profile);
    for tool in ["reflex_register", "reflex_dispatch"] {
        let denial = evaluate_supported_use_with_optional_process_for_tool(
            &profile,
            &foreground,
            tool,
            None,
        )
        .expect_err("reflex tools are not supervised foreground input");
        assert_eq!(
            denial.reason, "live_server_tool_not_foreground_input",
            "{tool}"
        );
    }
}

#[test]
fn must_pass_live_server_deny_list_still_ignores_unrelated_tools() {
    assert!(live_server_denied_tool_detail("act_press").is_none());
}

const GATE_PROFILE_ID: &str = "i204.live";
const GATE_EXE_NAME: &str = "i204game.exe";
const GATE_EXE_PATH: &str = r"C:\Games\I204\i204game.exe";

/// A profile file for the daemon's profile runtime with every operator-attended
/// live-server metadata key set (keys from the `KEY_*` contract constants).
fn write_live_server_profile(dir: &std::path::Path) {
    let mut metadata = String::new();
    for key in [
        KEY_LIVE_SERVER_ALLOWED,
        KEY_OPERATOR_ATTENDED_REQUIRED,
        KEY_OPERATOR_OWNED_CHARACTER_REQUIRED,
        KEY_FOREGROUND_ONLY,
        KEY_NO_MEMORY_OR_PROTOCOL_HOOKS,
        KEY_NO_UNATTENDED_LOOPS,
        KEY_NO_SOCIAL_OR_ECONOMY_AUTOMATION,
        KEY_NO_UNATTENDED_SCALED_OPERATION,
        KEY_NO_ACCOUNT_OR_BILLING_AUTOMATION,
        KEY_NO_PVP_GROUP_GUILD_RAID_AUTOMATION,
        KEY_NO_DESTRUCTIVE_UI_AUTOMATION,
    ] {
        metadata.push_str(&format!("\"{key}\" = \"true\"\n"));
    }
    metadata.push_str(&format!(
        "\"{KEY_RUNTIME_EVERQUEST_EXE}\" = '{GATE_EXE_PATH}'\n\"{KEY_RUNTIME_EVERQUEST_SERVER}\" = \"Frostreaver\"\n"
    ));
    std::fs::write(
        dir.join(format!("{GATE_PROFILE_ID}.toml")),
        format!(
            r#"
id = "{GATE_PROFILE_ID}"
label = "{GATE_PROFILE_ID}"
schema_version = 2
use_scope = "operator_owned_test"
mouse_velocity_profile_default = "natural"
keyboard_dynamics_default = "natural"

[[matches]]
exe = "{GATE_EXE_NAME}"

[detection]
classes_of_interest = ["window"]
confidence_threshold = 0.50
max_detections = 8

[metadata]
{metadata}"#
        ),
    )
    .expect("write live-server profile");
}

/// A daemon whose profile runtime loads the live-server profile above and
/// whose M1 foreground is a synthetic window of that game.
fn gate_service(root: &std::path::Path) -> crate::server::SynapseService {
    let profiles = root.join("profiles");
    std::fs::create_dir_all(&profiles).expect("profile dir");
    write_live_server_profile(&profiles);
    let service = crate::server::SynapseService::try_with_m2_shutdown_reason_and_m3_config(
        tokio_util::sync::CancellationToken::new(),
        "test",
        tokio_util::sync::CancellationToken::new(),
        &crate::m2::M2ServiceConfig::default(),
        crate::m3::M3ServiceConfig::from_cli_parts(
            Some(root.join("db")),
            Some(profiles),
            false,
            "127.0.0.1:0".to_owned(),
            std::num::NonZeroUsize::new(4).expect("nonzero"),
            false,
            true,
            None,
            false,
            None,
        ),
        crate::m4::M4ServiceConfig::default(),
    )
    .expect("service with profile dir");
    let foreground = ForegroundContext {
        hwnd: 0x2040,
        pid: 4242,
        process_name: GATE_EXE_NAME.to_owned(),
        process_path: GATE_EXE_PATH.to_owned(),
        window_title: "I204 Game".to_owned(),
        window_bounds: Rect {
            x: 0,
            y: 0,
            w: 1280,
            h: 720,
        },
        monitor_index: 0,
        dpi_scale: 1.0,
        profile_id: None,
        steam_appid: None,
        is_fullscreen: false,
        is_dwm_composed: true,
    };
    *service.m1_state.lock().expect("m1 state") = crate::m1::M1State {
        synthetic: Some(synapse_perception::ObservationInput::new(
            foreground.clone(),
        )),
        ..crate::m1::M1State::default()
    };
    // Fixture precondition readback: the runtime loaded the profile and the
    // synthetic foreground resolves to it.
    let runtime = service.profile_runtime().expect("profile runtime");
    let profile = runtime
        .profile(GATE_PROFILE_ID)
        .expect("profile read")
        .expect("precondition: live-server profile loaded");
    assert_eq!(
        profile
            .metadata
            .get(KEY_LIVE_SERVER_ALLOWED)
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        runtime
            .resolve_foreground(&super::foreground_window(&foreground))
            .expect("resolve foreground")
            .map(|resolution| resolution.profile_id),
        Some(GATE_PROFILE_ID.to_owned()),
        "precondition: the synthetic foreground resolves to the live-server profile"
    );
    service
}

fn scroll_action() -> synapse_core::Action {
    synapse_core::Action::MouseScroll {
        dy: 1,
        dx: 0,
        at: None,
        backend: synapse_core::Backend::Software,
    }
}

/// Contract: #204 + `target_policy.rs` `live_server_denied_tool_detail`
/// ("reflex_dispatch is denied for operator-attended live-server profiles") and
/// `context.rs` `ReflexScopeActionGate::ensure_action_allowed` (TOOL =
/// "reflex_dispatch"). With supported-use enforcement on, the real reflex
/// dispatch gate refuses every action on the live-server foreground, so the
/// reflex runtime dispatches nothing.
#[test]
fn must_fail_reflex_dispatch_gate_refuses_actions_on_a_live_server_profile() {
    let root = tempfile::tempdir().expect("temp dir");
    let service = gate_service(root.path());
    let gate = service
        .reflex_action_gate_with_enforced(true)
        .expect("reflex action gate");

    let denied = gate
        .ensure_action_allowed(&"reflex-i204".to_owned(), &scroll_action())
        .expect_err("reflex dispatch must be refused for a live-server profile");

    assert_eq!(
        denied.policy_code.as_deref(),
        Some(error_codes::SAFETY_PROFILE_ACTION_DENIED)
    );
    assert_eq!(
        denied.policy_reason.as_deref(),
        Some("live_server_tool_not_foreground_input")
    );
    assert_eq!(denied.profile_id.as_deref(), Some(GATE_PROFILE_ID));
    assert_eq!(
        Some(denied.detail.as_str()),
        live_server_denied_tool_detail("reflex_dispatch")
    );
}

/// Must NOT change (`target_policy.rs` `ensure_supported_use_allows_with`:
/// "Default posture: general Windows computer-control"): with enforcement off
/// the same gate, profile and foreground allow the action.
#[test]
fn must_pass_reflex_dispatch_gate_allows_actions_when_supported_use_is_not_enforced() {
    let root = tempfile::tempdir().expect("temp dir");
    let service = gate_service(root.path());
    let gate = service
        .reflex_action_gate_with_enforced(false)
        .expect("reflex action gate");

    gate.ensure_action_allowed(&"reflex-i204".to_owned(), &scroll_action())
        .expect("unenforced default posture dispatches the action");
}
