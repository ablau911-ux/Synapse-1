macro_rules! assert_literal {
    ($name:ident) => {
        assert_eq!(synapse_core::error_codes::$name, stringify!($name));
    };
}

#[test]
fn must_pass_error_codes_match_literal_names() {
    assert_perception_error_codes();
    assert_action_error_codes();
    assert_reflex_profile_session_storage_error_codes();
    assert_model_and_safety_error_codes();
    assert_action_agent_target_error_codes();
    assert_browser_notify_profile_storage_error_codes();
}

/// Guard: every `pub const` declared in `error_codes.rs` must be asserted by
/// `assert_literal!` in this file, so a newly added code cannot silently skip
/// the literal-name check.
#[test]
fn must_pass_every_error_code_const_is_listed() {
    let source = include_str!("../src/error_codes.rs");
    let this_file = include_str!("error_codes_literal.rs");
    let declared: Vec<&str> = source
        .split("pub const ")
        .skip(1)
        .filter_map(|decl| decl.split_once(':').map(|(name, _rest)| name.trim()))
        .collect();
    assert!(
        declared.len() > 100,
        "source scan found too few consts ({}); the scan itself is broken",
        declared.len()
    );
    let missing: Vec<&str> = declared
        .into_iter()
        .filter(|name| !this_file.contains(&format!("assert_literal!({name});")))
        .collect();
    assert!(
        missing.is_empty(),
        "error codes declared but not asserted here: {missing:?}"
    );
}

fn assert_action_agent_target_error_codes() {
    assert_literal!(ACTION_AGENT_SPAWN_FAILED);
    assert_literal!(ACTION_AGENT_SPAWN_SESSION_TIMEOUT);
    assert_literal!(ACTION_AGENT_SPAWN_TASK_NOT_STARTED);
    assert_literal!(ACTION_BUDGET_EXPIRED);
    assert_literal!(ACTION_FOCUS_WINDOW_FAILED);
    assert_literal!(ACTION_LAUNCH_FOREGROUND_FAILED);
    assert_literal!(ACTION_LAUNCH_URL_NOT_REACHED);
    assert_literal!(ACTION_LAUNCH_WINDOW_NOT_FOUND);
    assert_literal!(ACTION_REMOTE_PROCESS_CLEANUP_UNVERIFIED);
    assert_literal!(ACTION_WINDOW_AMBIGUOUS);
    assert_literal!(ACTION_WINDOW_NOT_FOUND);
    assert_literal!(AGENT_TASK_INVALID_TRANSITION);
    assert_literal!(AGENT_TASK_NOT_FOUND);
    assert_literal!(AGENT_TEMPLATE_NOT_FOUND);
    assert_literal!(TARGET_CDP_UNRESOLVED);
    assert_literal!(TARGET_CLAIM_ADOPT_REFUSED);
    assert_literal!(TARGET_CLAIM_LIMIT_REACHED);
    assert_literal!(TARGET_CLAIM_NOT_FOUND);
    assert_literal!(TARGET_CLAIM_OWNER_ACTIVE);
    assert_literal!(TARGET_CO_OWNED);
    assert_literal!(TARGET_NOT_SET);
    assert_literal!(TARGET_WINDOW_NOT_FOUND);
    assert_literal!(TOOL_PROFILE_POLICY_DENIED);
    assert_literal!(RECIPIENT_UNKNOWN);
    assert_literal!(REQUEST_CANCELLED);
    assert_literal!(SUGGESTION_NOT_FOUND);
    assert_literal!(DAEMON_RESTARTING);
    assert_literal!(HTTP_CONNECTION_CAPACITY);
}

fn assert_browser_notify_profile_storage_error_codes() {
    assert_literal!(AUDIT_EXPORT_CONSENT_REQUIRED);
    assert_literal!(AUDIT_EXPORT_PAYLOAD_TOO_LARGE);
    assert_literal!(AUDIT_EXPORT_REDACTION_REQUIRED);
    assert_literal!(BROWSER_WAIT_TIMEOUT);
    assert_literal!(BROWSER_EVALUATE_TIMEOUT);
    assert_literal!(CAPTURE_FRAME_UNAVAILABLE);
    assert_literal!(CAPTURE_OWNED_BY_OTHER_SESSION);
    assert_literal!(CHROME_DOM_ACTION_POSTCONDITION_FAILED);
    assert_literal!(CHROME_DOM_ACTION_UNSUPPORTED);
    assert_literal!(CHROME_DOM_ELEMENT_AMBIGUOUS);
    assert_literal!(CHROME_DOM_ELEMENT_NOT_ACTIONABLE);
    assert_literal!(CHROME_DOM_ELEMENT_NOT_FOUND);
    assert_literal!(CHROME_DOM_SELECTOR_INVALID);
    assert_literal!(CHROME_SCRIPTING_EXECUTE_FAILED);
    assert_literal!(MODEL_API_KEY_DECRYPT_FAILED);
    assert_literal!(MODEL_API_KEY_MISSING);
    assert_literal!(MODEL_API_KEY_STORE_FAILED);
    assert_literal!(NOTIFY_AUMID_REGISTRATION_FAILED);
    assert_literal!(NOTIFY_DELIVERY_UNVERIFIED);
    assert_literal!(NOTIFY_DISABLED_BY_GROUP_POLICY);
    assert_literal!(NOTIFY_DISABLED_BY_MANIFEST);
    assert_literal!(NOTIFY_DISABLED_FOR_APPLICATION);
    assert_literal!(NOTIFY_DISABLED_FOR_USER);
    assert_literal!(NOTIFY_SHOW_FAILED);
    assert_literal!(NOTIFY_UNSUPPORTED_PLATFORM);
    assert_literal!(NOTIFY_WORKER_FAILED);
    assert_literal!(NOTIFY_XML_PAYLOAD_INVALID);
    assert_literal!(PROFILE_AUTHORING_CANDIDATE_NOT_FOUND);
    assert_literal!(PROFILE_AUTHORING_CONFLICTING_EVIDENCE);
    assert_literal!(PROFILE_AUTHORING_INSUFFICIENT_EVIDENCE);
    assert_literal!(PROFILE_AUTHORING_INVALID_STATE);
    assert_literal!(PROFILE_AUTHORING_UNSAFE_ESCALATION);
    assert_literal!(PROFILE_ROLLBACK_UNAVAILABLE);
    assert_literal!(PROFILE_TRUST_VERIFICATION_FAILED);
    assert_literal!(STORAGE_NAMESPACE_QUOTA_REACHED);
    assert_literal!(STORAGE_WRITE_SHED);
    assert_literal!(EXECUTION_FINALIZATION_FAILED);
}

fn assert_perception_error_codes() {
    assert_literal!(OBSERVE_NO_PERCEPTION_AVAILABLE);
    assert_literal!(OBSERVE_INTERNAL);
    assert_literal!(CAPTURE_GRAPHICS_API_UNSUPPORTED);
    assert_literal!(CAPTURE_PRINTWINDOW_DISABLED);
    assert_literal!(CAPTURE_PRINTWINDOW_BLACK);
    assert_literal!(CAPTURE_WGC_FRAME_TIMEOUT);
    assert_literal!(CAPTURE_TARGET_MINIMIZED);
    assert_literal!(OCR_LANGUAGE_UNSUPPORTED);
    assert_literal!(CAPTURE_TARGET_LOST);
    assert_literal!(CAPTURE_NO_DIRTY_REGIONS);
    assert_literal!(A11Y_NOT_AVAILABLE);
    assert_literal!(A11Y_ELEMENT_STALE);
    assert_literal!(A11Y_NO_FOREGROUND);
    assert_literal!(A11Y_CDP_UNREACHABLE);
    assert_literal!(A11Y_CDP_ATTACH_FAILED);
    assert_literal!(A11Y_CDP_AXTREE_FAILED);
    assert_literal!(A11Y_CDP_EXTENSION_UNAVAILABLE);
    assert_literal!(A11Y_CDP_EXTENSION_DETACHED);
    assert_literal!(A11Y_CDP_EXTENSION_TIMEOUT);
    assert_literal!(A11Y_CDP_STROKE_TIMEOUT);
    assert_literal!(A11Y_CDP_DEBUGGER_WARNING_UNSUPPRESSED);
    assert_literal!(CHROME_BRIDGE_EXTENSION_STALE);
    assert_literal!(BROWSER_BRIDGE_KIND_UNRESOLVED);
    assert_literal!(BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED);
    assert_literal!(BROWSER_URL_SCHEME_UNSUPPORTED);
    assert_literal!(BROWSER_NAVIGATION_FAILED);
    assert_literal!(A11Y_UIA_WORKER_TIMEOUT);
    assert_literal!(A11Y_TARGET_WINDOW_MINIMIZED_UIA_UNAVAILABLE);
    assert_literal!(DETECTION_MODEL_NOT_LOADED);
    assert_literal!(DETECTION_MODEL_INFER_FAILED);
    assert_literal!(DETECTION_NO_FRAME);
    assert_literal!(OCR_NO_TEXT);
    assert_literal!(OCR_BACKEND_UNAVAILABLE);
    assert_literal!(HUD_NO_ACTIVE_PROFILE);
    assert_literal!(HUD_FIELD_NOT_DEFINED);
    assert_literal!(HUD_EXTRACTION_FAILED);
    assert_literal!(AUDIO_DEVICE_LOST);
    assert_literal!(AUDIO_LOOPBACK_INIT_FAILED);
    assert_literal!(AUDIO_STT_MODEL_NOT_LOADED);
}

fn assert_action_error_codes() {
    assert_literal!(ACTION_QUEUE_FULL);
    assert_literal!(ACTION_RATE_LIMITED);
    assert_literal!(ACTION_BACKEND_UNAVAILABLE);
    assert_literal!(ACTION_INPUT_TARGET_NOT_READY);
    assert_literal!(ACTION_TARGET_INVALID);
    assert_literal!(ACTION_INPUT_TARGET_REQUIRED);
    assert_literal!(ACTION_HOLD_EXCEEDED_MAX);
    assert_literal!(ACTION_VIGEM_NOT_INSTALLED);
    assert_literal!(ACTION_VIGEM_PLUGIN_FAILED);
    assert_literal!(ACTION_ELEMENT_NOT_RESOLVED);
    assert_literal!(ACTION_ELEMENT_PATTERN_UNSUPPORTED);
    assert_literal!(TRANSIENT_ELEMENT_EXPIRED);
    assert_literal!(ACTION_FOREGROUND_LOST);
    assert_literal!(ACTION_NO_OBSERVED_DELTA);
    assert_literal!(ACTION_VERIFY_SURFACE_UNAVAILABLE);
    assert_literal!(ACTION_POSTCONDITION_FAILED);
    assert_literal!(ACTION_UNSUPPORTED_KEY);
    assert_literal!(ACTION_DRAG_DISTANCE_EXCEEDS_LIMIT);
    assert_literal!(STUCK_KEY_AUTO_RELEASED);
    assert_literal!(SAFETY_RELEASE_ALL_FIRED);
    assert_literal!(SAFETY_OPERATOR_HOTKEY_FIRED);
    assert_literal!(ACTION_FOREGROUND_LEASE_BUSY);
    assert_literal!(ACTION_FOREGROUND_LEASE_NOT_HELD);
    assert_literal!(FOREGROUND_ACTIVATION_REFUSED);
    assert_literal!(ACTION_FOREGROUND_CONTEXT_CAPTURE_FAILED);
    assert_literal!(ACTION_FOREGROUND_CONTEXT_RESTORE_FAILED);
    assert_literal!(ACTION_FOREGROUND_CONTEXT_RESTORE_SKIPPED);
    assert_literal!(FOREGROUND_RESTORE_SKIPPED_HUMAN_MOVED);
    assert_literal!(ACTION_ELEMENT_VALUE_READ_ONLY);
}

fn assert_reflex_profile_session_storage_error_codes() {
    assert_literal!(REFLEX_CAP_REACHED);
    assert_literal!(REFLEX_KIND_INVALID);
    assert_literal!(REFLEX_PARAMS_INVALID);
    assert_literal!(REFLEX_TARGET_INVALID);
    assert_literal!(REFLEX_FILTER_INVALID);
    assert_literal!(REFLEX_PRIORITY_INVALID);
    assert_literal!(REFLEX_TICK_LATE);
    assert_literal!(REFLEX_TRACK_LOST);
    assert_literal!(REFLEX_STARVED);
    assert_literal!(REFLEX_DISABLED_BY_OPERATOR);
    assert_literal!(REFLEX_LIFETIME_EXPIRED);
    assert_literal!(REFLEX_RECURSION_LIMIT);
    assert_literal!(REFLEX_ACTION_PERMISSION_DENIED);
    assert_literal!(REFLEX_DEBOUNCED);

    assert_literal!(PROFILE_NOT_FOUND);
    assert_literal!(PROFILE_PARSE_ERROR);
    assert_literal!(PROFILE_VERSION_INCOMPATIBLE);
    assert_literal!(PROFILE_KEYMAP_INVALID);
    assert_literal!(PROFILE_HUD_REGION_INVALID);
    assert_literal!(CAPTURE_TARGET_INVALID);
    assert_literal!(PERCEPTION_MODE_INVALID);

    assert_literal!(SESSION_NOT_FOUND);
    assert_literal!(SESSION_EXPIRED);
    assert_literal!(SUBSCRIPTION_NOT_FOUND);
    assert_literal!(SUBSCRIPTION_CAP_REACHED);
    assert_literal!(TOOL_NOT_FOUND);
    assert_literal!(TOOL_PARAMS_INVALID);
    assert_literal!(USER_ACTIVE);
    assert_literal!(USER_ACTIVITY_UNKNOWN);
    assert_literal!(USER_WORKSTATION_LOCKED);
    assert_literal!(USER_RETURNED);
    assert_literal!(TOOL_INTERNAL_ERROR);
    assert_literal!(HTTP_BIND_NON_LOOPBACK_REFUSED);
    assert_literal!(HTTP_TOKEN_INVALID);
    assert_literal!(HTTP_ORIGIN_REFUSED);
    assert_literal!(HTTP_SESSION_INVALID);
    assert_literal!(REPLAY_TARGET_INVALID);
    assert_literal!(REPLAY_FORMAT_INVALID);

    assert_literal!(STORAGE_OPEN_FAILED);
    assert_literal!(STORAGE_WRITE_FAILED);
    assert_literal!(STORAGE_READ_FAILED);
    assert_literal!(STORAGE_CORRUPTED);
    assert_literal!(STORAGE_SCHEMA_MISMATCH);
    assert_literal!(STORAGE_DISK_PRESSURE_LEVEL_1);
    assert_literal!(STORAGE_DISK_PRESSURE_LEVEL_2);
    assert_literal!(STORAGE_DISK_PRESSURE_LEVEL_3);
    assert_literal!(STORAGE_DISK_PRESSURE_LEVEL_4);
    assert_literal!(STORAGE_CF_HARD_CAP_REACHED);

    assert_literal!(EPISODE_NOT_FOUND);
    assert_literal!(ROUTINE_NOT_FOUND);
    assert_literal!(ROUTINE_NOT_MINED);
    assert_literal!(ARMED_ROUTINE_NOT_FOUND);
}

fn assert_model_and_safety_error_codes() {
    assert_literal!(MODEL_DOWNLOAD_FAILED);
    assert_literal!(MODEL_HASH_MISMATCH);
    assert_literal!(MODEL_LOAD_FAILED);
    assert_literal!(MODEL_BACKEND_UNAVAILABLE);
    assert_literal!(MODEL_TOOLS_UNSUPPORTED);
    assert_literal!(MODEL_EMPTY_COMPLETION);
    assert_literal!(MODEL_ENDPOINT_UNREACHABLE);
    assert_literal!(MODEL_REGISTRY_NOT_FOUND);
    assert_literal!(MODEL_REGISTRY_CONFLICT);
    assert_literal!(MODEL_REGISTRY_DISABLED);
    assert_literal!(MODEL_REGISTRY_UNPROBED);
    assert_literal!(MODEL_REGISTRY_PROBE_STALE);

    assert_literal!(SAFETY_KILLSWITCH_ACTIVE);
    assert_literal!(SAFETY_PROCESS_DENYLISTED);
    assert_literal!(SAFETY_SHELL_DENIED_BY_POLICY);
    assert_literal!(SAFETY_SHELL_GLOBAL_INPUT_DENIED);
    assert_literal!(SAFETY_SHELL_RESERVED_VARIABLE_COLLISION);
    assert_literal!(SAFETY_SHELL_RECURSIVE_DELETE_UNCONTAINED);
    assert_literal!(SAFETY_LAUNCH_DENIED_BY_POLICY);
    assert_literal!(SAFETY_SECRET_REDACTED);
    assert_literal!(SAFETY_PERMISSION_DENIED);
    assert_literal!(SAFETY_PROFILE_ACTION_DENIED);
}
