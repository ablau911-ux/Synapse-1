use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use synapse_action::{ActionHandle, ActionStateSnapshot, RecordingBackend};
use synapse_core::{Health, SubsystemHealth, error_codes, types::TimelineActor};
use tokio::sync::{Mutex as AsyncMutex, Notify, watch};
use tokio_util::sync::CancellationToken;

use crate::{
    http::sse::SseState,
    m1::{
        BrowserAddInitScriptParams, BrowserAddInitScriptResponse, BrowserAddScriptTagParams,
        BrowserAddStyleTagParams, BrowserAddTagResponse, BrowserAdoptActiveTabParams,
        BrowserAdoptActiveTabResponse, BrowserBindingCall, BrowserConsoleMessagesParams,
        BrowserConsoleMessagesResponse, BrowserContentParams, BrowserContentResponse,
        BrowserDownloadEntry, BrowserDownloadEvent, BrowserDownloadsOperation,
        BrowserDownloadsParams, BrowserDownloadsResponse, BrowserEvaluateParams,
        BrowserEvaluateResponse, BrowserExposeBindingOperation, BrowserExposeBindingParams,
        BrowserExposeBindingResponse, BrowserFrameLocator, BrowserInitScriptOperation,
        BrowserInspectParams, BrowserInspectResponse, BrowserLayoutRelation, BrowserLocateEngine,
        BrowserLocateParams, BrowserLocateResponse, BrowserLocatedFrame, BrowserNavOperation,
        BrowserNavParams, BrowserNavResponse, BrowserNetworkWaitEntry, BrowserPdfParams,
        BrowserPdfResponse, BrowserScreenshotParams, BrowserScreenshotResponse,
        BrowserScreenshotScope, BrowserScrollIntoViewParams, BrowserScrollIntoViewResponse,
        BrowserSetContentParams, BrowserSetContentResponse, BrowserTabEntry, BrowserTabsParams,
        BrowserTabsResponse, BrowserWaitConditionKind, BrowserWaitForFunctionParams,
        BrowserWaitForFunctionResponse, BrowserWaitForLoadStateParams,
        BrowserWaitForLoadStateResponse, BrowserWaitForLoadStateState,
        BrowserWaitForNetworkResponseParams, BrowserWaitForNetworkResponseResponse,
        BrowserWaitForParams, BrowserWaitForRequestParams, BrowserWaitForRequestResponse,
        BrowserWaitForResponse, BrowserWaitForSelectorParams, BrowserWaitForSelectorResponse,
        BrowserWaitForSelectorState, BrowserWaitForState, BrowserWaitForUrlMatchKind,
        BrowserWaitForUrlParams, BrowserWaitForUrlResponse, BrowserWaitParams, BrowserWaitResponse,
        CaptureGifParams, CaptureGifResponse, CaptureScreenshotFormat, CaptureScreenshotParams,
        CaptureScreenshotResponse, CdpActivateTabParams, CdpActivateTabResponse,
        CdpActiveElementInfo, CdpBridgeHostReadback, CdpBridgeReloadAckReadback,
        CdpBridgeReloadParams, CdpBridgeReloadResponse, CdpCloseTabParams, CdpCloseTabResponse,
        CdpLargestContentfulPaintInfo, CdpNavigateAction, CdpNavigateTabParams,
        CdpNavigateTabResponse, CdpOpenTabParams, CdpOpenTabResponse, CdpPageTextInfo,
        CdpPageVitalsInfo, CdpTargetInfoParams, CdpTargetInfoResponse, ConsoleMessage,
        ElementInspection, FindParams, FindResponse, HiddenDesktopPipFrameParams,
        HiddenDesktopPipFrameResponse, HiddenDesktopPipStreamStatus, M1State, ObserveParams,
        ReadTextParams, ScreenshotOperation, ScreenshotParams, ScreenshotResponse,
        SetCaptureTargetParams, SetCaptureTargetResponse, SetPerceptionModeParams,
        SetPerceptionModeResponse, SetTargetParam, SetTargetParams, SharedM1State, TargetResponse,
        TargetWire, WindowListEntry, WindowListParams, WindowListResponse,
        apply_profile_runtime_config_in_state, build_find_input, current_input, empty_input_schema,
        enrich_input_with_browser_ocr, enrich_input_with_cdp_for_target, find_cdp_max_nodes,
        find_snapshot_depth, match_find_input, mcp_error, observe_include, observe_input,
        populate_clipboard_summary, populate_detection_from_state, populate_fs_recent,
        read_text_request_uncached, resolve_read_text_request, set_capture_target_in_state,
        set_perception_mode_in_state, set_target_input_schema,
    },
    m2::{
        ActClickParams, ActClickResponse, ActClipboardParams, ActClipboardResponse,
        ActFocusWindowParams, ActFocusWindowResponse, ActKeymapParams, ActKeymapResponse,
        ActPadParams, ActPadResponse, ActPressParams, ActPressResponse, ActScrollParams,
        ActScrollResponse, ActSetValueParams, ActSetValueResponse, ActStrokeParams,
        ActStrokeResponse, ActTypeParams, ActTypeResponse, M2ServiceConfig, ReleaseAllParams,
        ReleaseAllResponse, SharedM2State, SharedSessionClipboardBuffers,
        act_click_with_handle_and_lease, act_clipboard_session_buffer, act_focus_window,
        act_focus_window_request_details, act_focus_window_target_hwnd, act_keymap_with_handle,
        act_pad_with_handle, act_press_with_handle, act_set_value, act_set_value_request_details,
        act_stroke_validation_failure_details, act_stroke_with_handle, act_type_with_handle,
        new_session_clipboards, release_all_with_handles,
        shared_m2_state_from_config_with_shutdown_reason, shared_m2_state_from_env,
        validate_act_stroke_params,
    },
    m3::{
        M3ServiceConfig, SharedM3State,
        activity_recorder::BrowserNavigationEvent,
        approvals::{
            ApprovalDecideParams, ApprovalDecideResponse, ApprovalListParams, ApprovalListResponse,
            ApprovalRequestParams, ApprovalRequestResponse, ApprovalToastDelivery, decide_approval,
            list_approvals, prepare_activation_links, request_approval,
            update_approval_toast_state,
        },
        audio::{
            AudioTailParams, AudioTailResponse, AudioTranscribeParams, AudioTranscribeResponse,
            populate_audio_summary, tail_audio, transcribe_audio,
        },
        audit_export::{AuditExportBundleParams, AuditExportBundleResponse, export_audit_bundle},
        demo_recording::{
            DemoRecordStartParams, DemoRecordStartResponse, DemoRecordStopParams,
            DemoRecordStopResponse, start_demo_recording, stop_demo_recording,
        },
        episodes::{
            EpisodeGetParams, EpisodeGetResponse, EpisodeListParams, EpisodeListResponse,
            EpisodeSegmentParams, EpisodeSegmentResponse, get_episode, list_episodes,
            segment_episodes,
        },
        hygiene::{
            HygieneFlagsParams, HygieneFlagsResponse, HygieneScanStorageParams,
            HygieneScanStorageResponse, HygieneScanTextParams, HygieneScanTextResponse,
            query_flags, scan_storage, scan_text_tool,
        },
        local_models::{
            LocalModelListParams, LocalModelListResponse, LocalModelProbeParams,
            LocalModelProbeResponse, LocalModelRegisterParams, LocalModelRegisterResponse,
            LocalModelRemoveParams, LocalModelRemoveResponse, LocalModelUpdateParams,
            LocalModelUpdateResponse, list_local_models, probe_local_model, register_local_model,
            remove_local_model, update_local_model,
        },
        permissions::{RequiredPermissions, authorization_error},
        profile::{
            ProfileActivateParams, ProfileActivateResponse, ProfileListParams, ProfileListResponse,
            activate_profile, list_profiles,
        },
        profile_authoring::{
            ProfileAuthoringDecideParams, ProfileAuthoringDecideResponse,
            ProfileAuthoringExportParams, ProfileAuthoringExportResponse,
            ProfileAuthoringGenerateParams, ProfileAuthoringGenerateResponse,
            ProfileAuthoringInspectParams, ProfileAuthoringInspectResponse,
            ProfileAuthoringListParams, ProfileAuthoringListResponse, RoutineAutomateParams,
            RoutineAutomateResponse, decide_profile_authoring_candidate,
            export_profile_authoring_candidate, generate_profile_authoring_candidate,
            generate_routine_automation_candidate, inspect_profile_authoring_candidate,
            list_profile_authoring_candidates,
        },
        profile_quality::{
            ProfileQualityRefreshParams, ProfileQualityRefreshResponse, refresh_profile_quality,
        },
        profile_registry::{
            AuditIntelligenceQueryParams, AuditIntelligenceQueryResponse,
            ProfileRegistryDisableParams, ProfileRegistryDisableResponse,
            ProfileRegistryExportParams, ProfileRegistryExportResponse,
            ProfileRegistryImportParams, ProfileRegistryImportResponse,
            ProfileRegistryInstallParams, ProfileRegistryInstallResponse,
            ProfileRegistryQueryParams, ProfileRegistryQueryResponse,
            ProfileRegistryRollbackParams, ProfileRegistryRollbackResponse,
            disable_registry_profile, export_registry, import_registry, install_registry_package,
            query_audit_intelligence, query_registry, rollback_registry_profile,
        },
        reflex::{
            ReflexCancelParams, ReflexCancelResponse, ReflexHistoryParams, ReflexHistoryResponse,
            ReflexListParams, ReflexListResponse, ReflexRegisterParams, ReflexRegisterResponse,
            cancel_file_jsonl_tail_watcher, cancel_reflex, history_reflexes,
            install_file_jsonl_tail_watcher, list_reflexes,
        },
        replay::{ReplayRecordParams, ReplayRecordResponse, record_replay},
        routines::{
            RoutineInspectParams, RoutineInspectResponse, RoutineListParams, RoutineListResponse,
            RoutineMineParams, RoutineMineResponse, RoutineUpdateParams, RoutineUpdateResponse,
            inspect_routine, list_routines, mine_and_store_routines, update_routine,
        },
        shared_m3_state_from_config_with_shutdown_reason_and_sse_state, shared_m3_state_from_env,
        storage::{
            StorageGcOnceParams, StorageGcOnceResponse, StorageInspectParams,
            StorageInspectResponse, StoragePressureSampleParams, StoragePressureSampleResponse,
            StoragePutProbeRowsParams, StoragePutProbeRowsResponse, inspect_storage,
            put_probe_rows,
        },
        subscribe::{
            SubscribeCancelParams, SubscribeCancelResponse, SubscribeFacadeParams,
            SubscribeFacadeResponse, SubscribeOperation, cancel_owned_subscription,
            cancel_subscription, subscribe_to_events,
        },
        timeline::{
            TimelinePurgeParams, TimelinePurgeResponse, TimelineSearchParams,
            TimelineSearchResponse, purge_timeline, search_timeline,
        },
        timeline_control::{
            TimelineExclusionsParams, TimelineExclusionsResponse, TimelinePauseParams,
            TimelinePauseResponse, TimelineResumeParams, TimelineResumeResponse, pause_timeline,
            resume_timeline, update_timeline_exclusions,
        },
    },
    m4::{
        ActComboParams, ActComboResponse, ActLaunchParams, ActLaunchResponse,
        ActRunShellCancelResponse, ActRunShellJobIdParams, ActRunShellParams, ActRunShellResponse,
        ActRunShellStartParams, ActRunShellStartResponse, ActRunShellStatusParams,
        ActRunShellStatusResponse, ActSpawnAgentCli, ActSpawnAgentLogPaths, ActSpawnAgentParams,
        ActSpawnAgentRequest, ActSpawnAgentResponse, ActSpawnAgentTarget,
        AgentSpawnTaskStartedParams, AgentSpawnTaskStartedResponse, LaunchWindowState,
        M4ServiceConfig, MAX_AGENT_SPAWN_WAIT_TIMEOUT_MS, RUN_SHELL_IDEMPOTENCY_LEGACY_SCAN_LIMIT,
        RUN_SHELL_IDEMPOTENCY_PREFIX, RunShellAuthorization, ShellExecutionContext,
        assign_owned_process_job, authorize_run_shell, authorize_run_shell_start, cancel_shell_job,
        execute_combo, launch_for_session, launch_process_history_row,
        launch_process_history_row_key, launch_request_details,
        prepare_run_shell_params_for_context, prepare_run_shell_start_params_for_context,
        required_combo_permissions, run_authorized_shell, run_shell_idempotency_completed_row,
        run_shell_idempotency_recover_after_restart, run_shell_idempotency_replay,
        run_shell_idempotency_reservation_row, run_shell_idempotency_row_key,
        run_shell_idempotency_select_legacy_row, run_shell_request_details,
        run_shell_start_request_details, shell_execution_context_for_session, shell_job_status,
        start_authorized_shell_job, validate_agent_spawn_params, validate_run_shell_execution_plan,
    },
};

mod action_audit;
mod action_preflight;
pub(crate) mod agent_control;
pub(crate) mod agent_cost;
pub(crate) mod agent_event_ingress;
pub(crate) mod agent_events;
mod agent_facades;
mod agent_mailbox;
pub(crate) mod agent_query;
mod agent_scope;
pub(crate) mod agent_spawn_switch;
pub(crate) mod agent_state;
pub(crate) mod agent_stats;
pub(crate) mod agent_tasks;
pub(crate) mod agent_templates;
pub(crate) mod agent_transcripts;
pub(crate) mod ambient_agents;
mod approval_facades;
mod audit_context;
mod audit_replay_facades;
pub(crate) mod codex_app_server_bridge;
pub(crate) mod command_audit;
#[cfg(windows)]
pub(crate) mod console_type;
mod context;
pub(crate) use context::AgentTranscriptSnapshotRow;
pub(crate) use context::{
    APPROVAL_DECISION_EVENT_KIND, APPROVAL_REQUEST_EVENT_KIND, APPROVAL_TIMEOUT_EVENT_KIND,
};
pub(crate) use lease_tools::DASHBOARD_LEASE_SOURCE_OF_TRUTH;
mod act_foreground_deadline;
mod act_foreground_focus;
mod act_foreground_slot;
mod act_foreground_unwind;
mod act_outcome;
mod act_request_audit;
mod background_router;
/// #189: the real target_act status oracle, shared with m2 click tests outside `server`.
#[cfg(test)]
pub(crate) use background_router::dispatch_status_189_tests;
mod action_history;
mod browser_assert;
mod browser_batch;
mod browser_clock_events;
mod browser_dialog;
mod browser_dnd;
mod browser_emulate;
mod browser_facades;
mod browser_field;
mod browser_files;
mod browser_frames;
mod browser_network;
mod browser_storage;
mod browser_window_guard;
mod capture_gif;
mod data_cleaning;
mod diagnostic_rate_limit_reset;
pub(crate) mod drain;
pub(crate) mod escalation;
mod everquest_autocombat;
mod everquest_contextgraph;
mod everquest_domain;
mod everquest_episode_export;
mod everquest_guard;
mod everquest_log;
mod everquest_map_sensor;
mod everquest_memory;
mod everquest_outcome;
mod everquest_predictive_model;
mod everquest_route;
mod everquest_scorecard;
mod everquest_state;
mod everquest_surprise;
mod everquest_tools;
mod everquest_trajectory;
mod everquest_ui_context;
mod everquest_world_model;
mod everquest_world_summary;
mod handler;
pub(crate) mod health;
mod param_hints;
pub(crate) use health::HttpHealthTelemetry;
mod hygiene_report;
mod intent_tools;
mod lease_tools;
mod livecapture;
mod livecapture_ownership;
#[cfg(feature = "livefeed")]
mod livefeed;
mod m1_tools;
mod m2_tools;
pub(crate) use m2_tools::foreground_click_actionability;
mod m3_tools;
pub(crate) mod m4_tools;
mod notify_tools;
mod operational_facades;
pub(crate) mod output_path_policy;
mod permission_gate;
pub(crate) mod permission_policy;
mod plan_tools;
mod reality;
mod routine_assist_facades;
mod routine_feedback;
mod routine_labeling;
mod schema_sanitize;
mod seat_owner_idle;
mod seat_tools;
mod session_ambient_scan;
pub(crate) mod session_continuity;
pub(crate) mod session_lifecycle;
pub(crate) mod session_registry;
mod session_tools;
pub(crate) mod shell_profile_policy;
pub(crate) mod suggestions;
pub(crate) mod target_claims;
mod target_policy;
pub(crate) mod terminal_capture;
pub(crate) mod terminated_sessions;
#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "server/tests/g4_unit_support.rs"]
pub(crate) mod g4_unit_support;
#[cfg(all(test, windows))]
#[path = "server/tests/own_window_fixture.rs"]
pub(crate) mod own_window_fixture;
pub(crate) mod timeline_digest;
mod timeline_facades;
mod timeline_query;
mod tool_profiles;
mod tools_list_cache;
#[cfg(test)]
#[path = "server/tests/user_active_258_support.rs"]
pub(crate) mod user_active_258_support;
pub(crate) use crate::chrome_debugger_bridge::url_redaction;
mod browser_evaluate_budget;
mod delivery_claim;
// #107 part d: the point click route (`m2::click::point_post`) reports through it too.
pub(crate) mod native_click_evidence;
// 300f / #107d: `target_act` and the point click route refuse an element of another window.
pub(crate) mod element_root;
mod owner_override;
pub(crate) mod user_active_gate;
mod user_activity;
mod verification;
mod videoanalyze;
mod workspace_blackboard;
mod workspace_wait;

use session_registry::{SessionRegistry, SharedSessionRegistry};
use target_claims::SharedTargetClaims;

/// A single MCP session's active perception target (epic #720). When set,
/// `observe`/`find`/`read_text`/`capture_screenshot` perceive this target
/// instead of the global foreground, so many agents observe different windows
/// or browser tabs concurrently.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum SessionTarget {
    Window {
        hwnd: i64,
    },
    Cdp {
        window_hwnd: i64,
        cdp_target_id: String,
    },
}

/// Per-session active-target registry keyed by `Mcp-Session-Id`. A small mutex
/// held only to clone an entry out — never across a perception `.await` — so
/// target reads never serialize behind another session's snapshot.
pub(crate) type SharedSessionTargets = Arc<Mutex<HashMap<String, SessionTarget>>>;
/// Serializes lease ownership transitions across public tools, session
/// teardown, and dashboard recovery. Physical lease mutation and its durable
/// lifecycle edge must be one process-local critical section.
pub(crate) type SharedLeaseTransitionGate = Arc<AsyncMutex<()>>;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct CdpTargetOwner {
    pub session_id: String,
    pub window_hwnd: i64,
    pub endpoint: String,
    #[serde(default)]
    pub chrome_window_id: Option<i64>,
    #[serde(default)]
    pub capture_window_hwnd: Option<i64>,
    pub cdp_target_id: String,
    pub requested_url: String,
    pub target_url: String,
    pub created_at_unix_ms: u64,
}

/// Per-CDP-target ownership registry keyed by browser surface + `TargetID`.
/// Only the creating MCP session may close a registered target; unowned targets
/// may be observed by explicit `set_target` but are never closed by Synapse.
pub(crate) type SharedCdpTargetOwners = Arc<Mutex<HashMap<String, CdpTargetOwner>>>;

#[derive(Debug, Clone)]
pub struct SynapseService {
    started_at: Instant,
    initialized_peers: Arc<Mutex<Vec<rmcp::service::Peer<rmcp::RoleServer>>>>,
    tool_router: ToolRouter<Self>,
    m1_state: SharedM1State,
    m2_state: SharedM2State,
    m3_state: SharedM3State,
    m4_config: M4ServiceConfig,
    drain_state: drain::DaemonDrainState,
    session_targets: SharedSessionTargets,
    cdp_target_owners: SharedCdpTargetOwners,
    session_clipboards: SharedSessionClipboardBuffers,
    session_registry: SharedSessionRegistry,
    mailbox_notify: Arc<Notify>,
    target_claims: SharedTargetClaims,
    session_processes: session_lifecycle::SharedSessionProcessResources,
    terminated_sessions: session_lifecycle::SharedTerminatedSessions,
    lease_transition_gate: SharedLeaseTransitionGate,
    seat_supervisor: Arc<synapse_seats::SeatSupervisor>,
    browser_seat_backend_available: bool,
    browser_seat_backend_detail: String,
    #[cfg(feature = "livefeed")]
    pub(super) livefeed_camera: Arc<synapse_livefeed::Camera>,
}

const SYNAPSE_SEATS_CHROMIUM_ENV: &str = "SYNAPSE_SEATS_CHROMIUM";
const SYNAPSE_SEATS_ROOT_ENV: &str = "SYNAPSE_SEATS_ROOT";
const SEAT_MAX_COUNT: usize = 4;
const SEAT_QUEUE_CAPACITY: usize = 16;
const SEAT_REQUEST_TIMEOUT_MS: u64 = 15_000;
const SEAT_BROWSER_STARTUP_TIMEOUT_MS: u64 = 10_000;

struct SeatSupervisorRuntime {
    supervisor: Arc<synapse_seats::SeatSupervisor>,
    browser_available: bool,
    browser_detail: String,
}

fn shared_seat_supervisor() -> anyhow::Result<SeatSupervisorRuntime> {
    let runtime_root = if let Some(configured) = std::env::var_os(SYNAPSE_SEATS_ROOT_ENV) {
        let root = PathBuf::from(configured);
        if root.as_os_str().is_empty() {
            anyhow::bail!("{SYNAPSE_SEATS_ROOT_ENV} must not be empty");
        }
        root
    } else {
        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        local_app_data.join("synapse").join("seats")
    };
    let chromium = seat_chromium_executable()?;
    let supervisor = synapse_seats::SeatSupervisor::new(synapse_seats::SeatSupervisorConfig {
        browser: synapse_seats::BrowserRuntimeConfig {
            chromium_executable: chromium.path,
            runtime_root,
            startup_timeout_ms: SEAT_BROWSER_STARTUP_TIMEOUT_MS,
        },
        max_seats: SEAT_MAX_COUNT,
        per_seat_queue_capacity: SEAT_QUEUE_CAPACITY,
        request_timeout_ms: SEAT_REQUEST_TIMEOUT_MS,
    })
    .map(Arc::new)
    .map_err(|error| anyhow::anyhow!("initialize isolated browser-seat supervisor: {error}"))?;
    Ok(SeatSupervisorRuntime {
        supervisor,
        browser_available: chromium.available,
        browser_detail: chromium.detail,
    })
}

struct SeatChromiumRuntime {
    path: PathBuf,
    available: bool,
    detail: String,
}

fn seat_chromium_executable() -> anyhow::Result<SeatChromiumRuntime> {
    if let Some(configured) = std::env::var_os(SYNAPSE_SEATS_CHROMIUM_ENV) {
        let path = PathBuf::from(configured);
        if !path.is_file() {
            anyhow::bail!(
                "{SYNAPSE_SEATS_CHROMIUM_ENV} must name an existing Chromium executable: {}",
                path.display()
            );
        }
        return Ok(SeatChromiumRuntime {
            detail: format!(
                "configured isolated Chromium executable: {}",
                path.display()
            ),
            path,
            available: true,
        });
    }
    // Iron Law 9: candidates come from the Chromium rows of browsers/*.json
    // (App Paths, then `exe_candidates`), in row order.
    let registry = synapse_browsers::BrowserRegistry::global();
    let is_file = |path: &std::path::Path| path.is_file();
    let locator = synapse_browsers::BrowserExecutableLocator::new(
        &crate::chrome_debugger_bridge::registry_reader::WindowsRegistryReader,
        &synapse_browsers::process_env_var,
        &is_file,
    );
    let chromium_rows = || {
        registry
            .rows()
            .iter()
            .filter(|row| row.family == synapse_browsers::BrowserFamily::Chromium)
    };
    if let Some(located) = chromium_rows().find_map(|row| locator.locate(row)) {
        return Ok(SeatChromiumRuntime {
            detail: format!(
                "installed isolated Chromium executable: {} source_of_truth={}",
                located.path.display(),
                located.source_of_truth
            ),
            path: located.path,
            available: true,
        });
    }
    let path = chromium_rows()
        .find_map(|row| locator.first_candidate(row))
        .unwrap_or_default();
    Ok(SeatChromiumRuntime {
        detail: format!(
            "no Chromium executable found; set {SYNAPSE_SEATS_CHROMIUM_ENV} (checked {})",
            path.display()
        ),
        path,
        available: false,
    })
}

fn install_chrome_browser_navigation_sink(m3_state: &SharedM3State) {
    let m3_state = Arc::clone(m3_state);
    crate::chrome_debugger_bridge::set_browser_navigation_sink(Arc::new(move |event| {
        let recorder = match m3_state.lock() {
            Ok(state) => state.activity_recorder.clone(),
            Err(_error) => {
                tracing::error!(
                    code = "TIMELINE_BROWSER_NAV_M3_LOCK_POISONED",
                    "M3 service state lock poisoned while recording Chrome browser navigation"
                );
                return;
            }
        };
        if let Some(recorder) = recorder {
            let _ = recorder.record_browser_navigation(BrowserNavigationEvent {
                actor: TimelineActor::Human,
                // Iron Law 9: the image name comes from the reporting host's row.
                app: synapse_browsers::BrowserRegistry::global()
                    .by_id(event.browser_kind.as_str())
                    .and_then(|row| row.process_names.first().cloned()),
                source: event.source,
                event: event.event,
                action: None,
                url: event.url,
                title: event.title,
                tab_id: event.tab_id,
                chrome_window_id: event.chrome_window_id,
                window_hwnd: None,
                cdp_target_id: event.cdp_target_id,
                endpoint: event.endpoint,
                transport: event.transport,
                requested_url: None,
                before_url: None,
                before_title: None,
                ready_state: event.ready_state,
                observed_at_unix_ms: event.observed_at_unix_ms,
                active: event.active,
                highlighted: event.highlighted,
                pinned: event.pinned,
            });
        } else {
            tracing::error!(
                code = "TIMELINE_BROWSER_NAV_RECORDER_MISSING",
                "Chrome browser navigation event arrived before the activity recorder was available"
            );
        }
    }));
}

impl SynapseService {
    #[must_use]
    pub fn new() -> Self {
        match Self::try_new() {
            Ok(service) => service,
            Err(error) => panic!("M3 state should initialize from environment: {error:#}"),
        }
    }

    pub fn try_new() -> anyhow::Result<Self> {
        let seat_runtime = shared_seat_supervisor()?;
        let m3_state = shared_m3_state_from_env()?;
        install_chrome_browser_navigation_sink(&m3_state);
        let service = Self {
            started_at: Instant::now(),
            initialized_peers: Arc::default(),
            tool_router: Self::tool_router(),
            m1_state: SharedM1State::default(),
            m2_state: shared_m2_state_from_env()?,
            m3_state,
            m4_config: M4ServiceConfig::from_env()?,
            drain_state: drain::DaemonDrainState::default(),
            session_targets: Arc::new(Mutex::new(HashMap::new())),
            cdp_target_owners: Arc::new(Mutex::new(HashMap::new())),
            session_clipboards: new_session_clipboards(),
            session_registry: Arc::new(Mutex::new(SessionRegistry::default())),
            mailbox_notify: Arc::new(Notify::new()),
            target_claims: Arc::new(Mutex::new(target_claims::TargetClaimRegistry::default())),
            session_processes: Arc::new(Mutex::new(BTreeMap::new())),
            terminated_sessions: Arc::default(),
            lease_transition_gate: Arc::new(AsyncMutex::new(())),
            seat_supervisor: seat_runtime.supervisor,
            browser_seat_backend_available: seat_runtime.browser_available,
            browser_seat_backend_detail: seat_runtime.browser_detail,
            #[cfg(feature = "livefeed")]
            livefeed_camera: Arc::default(),
        };
        service
            .reconcile_persisted_session_leases_on_startup()
            .map_err(|error| {
                anyhow::anyhow!(
                    "reconcile persisted input leases at daemon startup: {}",
                    error.message
                )
            })?;
        Ok(service)
    }

    /// Test-only constructor rooted at an isolated directory (#143).
    ///
    /// `new()`/`try_new()` read `M3ServiceConfig::from_env()`, which falls back
    /// to the production `%LOCALAPPDATA%\synapse\db` when `SYNAPSE_DB` is
    /// unset, and startup lease reconciliation then opens that RocksDB. Tests
    /// must never touch (or contend for the lock on) the live daemon's DB.
    #[cfg(test)]
    pub(crate) fn try_new_isolated_for_tests(root: &std::path::Path) -> anyhow::Result<Self> {
        let max_subscriptions =
            std::num::NonZeroUsize::new(synapse_reflex::DEFAULT_MAX_SUBSCRIPTIONS_NONZERO.get())
                .ok_or_else(|| anyhow::anyhow!("max subscriptions must be nonzero"))?;
        Self::try_with_m2_shutdown_reason_and_m3_config(
            CancellationToken::new(),
            "test",
            CancellationToken::new(),
            &M2ServiceConfig::default(),
            M3ServiceConfig::from_cli_parts(
                Some(root.join("db")),
                Some(root.to_path_buf()),
                false,
                "127.0.0.1:0".to_owned(),
                max_subscriptions,
                false,
                true,
                None,
                false,
                None,
            ),
            M4ServiceConfig::default(),
        )
    }

    pub fn try_with_m2_shutdown_reason_and_m3_config(
        shutdown_cancel: CancellationToken,
        shutdown_reason: &'static str,
        connection_closed_cancel: CancellationToken,
        m2_config: &M2ServiceConfig,
        m3_config: M3ServiceConfig,
        m4_config: M4ServiceConfig,
    ) -> anyhow::Result<Self> {
        let seat_runtime = shared_seat_supervisor()?;
        let sse_state = SseState::with_max_subscriptions(m3_config.max_subscriptions);
        let m3_state = shared_m3_state_from_config_with_shutdown_reason_and_sse_state(
            m3_config,
            shutdown_cancel.clone(),
            shutdown_reason,
            Some(connection_closed_cancel.clone()),
            sse_state,
        )?;
        install_chrome_browser_navigation_sink(&m3_state);
        let service = Self {
            started_at: Instant::now(),
            initialized_peers: Arc::default(),
            tool_router: Self::tool_router(),
            m1_state: SharedM1State::default(),
            m2_state: shared_m2_state_from_config_with_shutdown_reason(
                m2_config,
                shutdown_cancel,
                shutdown_reason,
                Some(connection_closed_cancel),
            )?,
            m3_state,
            m4_config,
            drain_state: drain::DaemonDrainState::default(),
            session_targets: Arc::new(Mutex::new(HashMap::new())),
            cdp_target_owners: Arc::new(Mutex::new(HashMap::new())),
            session_clipboards: new_session_clipboards(),
            session_registry: Arc::new(Mutex::new(SessionRegistry::default())),
            mailbox_notify: Arc::new(Notify::new()),
            target_claims: Arc::new(Mutex::new(target_claims::TargetClaimRegistry::default())),
            session_processes: Arc::new(Mutex::new(BTreeMap::new())),
            terminated_sessions: Arc::default(),
            lease_transition_gate: Arc::new(AsyncMutex::new(())),
            seat_supervisor: seat_runtime.supervisor,
            browser_seat_backend_available: seat_runtime.browser_available,
            browser_seat_backend_detail: seat_runtime.browser_detail,
            #[cfg(feature = "livefeed")]
            livefeed_camera: Arc::default(),
        };
        service
            .reconcile_persisted_session_leases_on_startup()
            .map_err(|error| {
                anyhow::anyhow!(
                    "reconcile persisted input leases at daemon startup: {}",
                    error.message
                )
            })?;
        Ok(service)
    }

    pub fn try_with_m2_shutdown_reason_and_sse_state_and_m3_config(
        shutdown_cancel: CancellationToken,
        shutdown_reason: &'static str,
        connection_closed_cancel: CancellationToken,
        sse_state: SseState,
        m2_config: &M2ServiceConfig,
        m3_config: M3ServiceConfig,
        m4_config: M4ServiceConfig,
    ) -> anyhow::Result<Self> {
        let seat_runtime = shared_seat_supervisor()?;
        let m3_state = shared_m3_state_from_config_with_shutdown_reason_and_sse_state(
            m3_config,
            shutdown_cancel.clone(),
            shutdown_reason,
            Some(connection_closed_cancel.clone()),
            sse_state,
        )?;
        install_chrome_browser_navigation_sink(&m3_state);
        let service = Self {
            started_at: Instant::now(),
            initialized_peers: Arc::default(),
            tool_router: Self::tool_router(),
            m1_state: SharedM1State::default(),
            m2_state: shared_m2_state_from_config_with_shutdown_reason(
                m2_config,
                shutdown_cancel,
                shutdown_reason,
                Some(connection_closed_cancel),
            )?,
            m3_state,
            m4_config,
            drain_state: drain::DaemonDrainState::default(),
            session_targets: Arc::new(Mutex::new(HashMap::new())),
            cdp_target_owners: Arc::new(Mutex::new(HashMap::new())),
            session_clipboards: new_session_clipboards(),
            session_registry: Arc::new(Mutex::new(SessionRegistry::default())),
            mailbox_notify: Arc::new(Notify::new()),
            target_claims: Arc::new(Mutex::new(target_claims::TargetClaimRegistry::default())),
            session_processes: Arc::new(Mutex::new(BTreeMap::new())),
            terminated_sessions: Arc::default(),
            lease_transition_gate: Arc::new(AsyncMutex::new(())),
            seat_supervisor: seat_runtime.supervisor,
            browser_seat_backend_available: seat_runtime.browser_available,
            browser_seat_backend_detail: seat_runtime.browser_detail,
            #[cfg(feature = "livefeed")]
            livefeed_camera: Arc::default(),
        };
        service
            .reconcile_persisted_session_leases_on_startup()
            .map_err(|error| {
                anyhow::anyhow!(
                    "reconcile persisted input leases at daemon startup: {}",
                    error.message
                )
            })?;
        Ok(service)
    }

    pub fn m2_emitter_done_receiver(&self) -> Option<watch::Receiver<Option<ActionStateSnapshot>>> {
        self.m2_state
            .lock()
            .ok()
            .and_then(|state| state.emitter_done_receiver())
    }

    pub(crate) fn m3_state_handle(&self) -> SharedM3State {
        Arc::clone(&self.m3_state)
    }

    pub(crate) fn lease_transition_gate(&self) -> SharedLeaseTransitionGate {
        Arc::clone(&self.lease_transition_gate)
    }

    pub(crate) fn drain_state_handle(&self) -> drain::DaemonDrainState {
        self.drain_state.clone()
    }

    pub(crate) fn shutdown_cancel_token(&self) -> Result<CancellationToken, ErrorData> {
        self.m3_state
            .lock()
            .map(|state| state.shutdown_cancel.clone())
            .map_err(|_err| {
                mcp_error(
                    error_codes::TOOL_INTERNAL_ERROR,
                    "M3 service state lock poisoned while reading daemon shutdown token",
                )
            })
    }

    pub(crate) const fn session_targets_ref(&self) -> &SharedSessionTargets {
        &self.session_targets
    }

    pub(crate) const fn cdp_target_owners_ref(&self) -> &SharedCdpTargetOwners {
        &self.cdp_target_owners
    }

    pub(crate) const fn session_clipboards_ref(&self) -> &SharedSessionClipboardBuffers {
        &self.session_clipboards
    }

    pub(crate) fn session_registry_handle(&self) -> SharedSessionRegistry {
        Arc::clone(&self.session_registry)
    }

    pub(crate) const fn session_registry_ref(&self) -> &SharedSessionRegistry {
        &self.session_registry
    }

    pub(crate) const fn seat_supervisor_ref(&self) -> &Arc<synapse_seats::SeatSupervisor> {
        &self.seat_supervisor
    }

    pub(crate) fn seat_supervisor_handle(&self) -> Arc<synapse_seats::SeatSupervisor> {
        Arc::clone(&self.seat_supervisor)
    }

    pub(crate) fn browser_seat_backend_status(&self) -> (bool, &str) {
        (
            self.browser_seat_backend_available,
            self.browser_seat_backend_detail.as_str(),
        )
    }

    pub(crate) fn mailbox_notify_handle(&self) -> Arc<Notify> {
        Arc::clone(&self.mailbox_notify)
    }

    /// Resolves the session's active target, if any. The cloned value is
    /// returned after the map guard is dropped.
    pub(crate) fn session_target(
        &self,
        session_id: Option<&str>,
    ) -> Result<Option<SessionTarget>, ErrorData> {
        let Some(session_id) = session_id else {
            return Ok(None);
        };
        // #299: a call that names an agent sees only that agent's target; while
        // agents of this session hold targets, a call that names none sees none.
        match agent_scope::lookup(session_id)? {
            agent_scope::TargetLookup::Agent(target) => Ok(target),
            agent_scope::TargetLookup::AgentsActive => Ok(None),
            agent_scope::TargetLookup::Session => self.restore_session_target_if_needed(session_id),
        }
    }

    /// Resolves the foreground-equivalent target owned by one MCP session.
    /// This never falls back to the human OS foreground window; callers that
    /// need the real OS foreground must ask for it explicitly.
    pub(crate) fn agent_logical_foreground(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionTarget>, ErrorData> {
        self.session_target(Some(session_id))
    }

    /// Resolves the effective target for an ACTION call (#984): an explicit
    /// per-call `window_hwnd` / `cdp_target_id` override wins over the session's
    /// bound target, mirroring how `observe` / `capture_screenshot` accept an
    /// explicit `window_hwnd`. This makes multi-window / multi-agent action
    /// routing deterministic instead of depending on per-session `set_target`
    /// state that may not persist across reconnects. `cdp_target_id` requires
    /// `window_hwnd` (the browser window whose CDP endpoint owns the target) and
    /// gives a stable routing handle that survives window-handle recycling.
    pub(crate) fn action_session_target_override(
        &self,
        explicit_window_hwnd: Option<i64>,
        explicit_cdp_target_id: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<Option<SessionTarget>, ErrorData> {
        match explicit_action_target(explicit_window_hwnd, explicit_cdp_target_id)? {
            Some(target) => {
                if let Some(session_id) = session_id {
                    agent_scope::ensure_access(session_id, &target)?;
                }
                Ok(Some(target))
            }
            None => self.session_target(session_id),
        }
    }

    pub(crate) fn unscoped_action_handle(&self) -> anyhow::Result<ActionHandle> {
        self.m2_state
            .lock()
            .map(|state| state.emitter_handle.clone().with_session_id(None))
            .map_err(|_err| anyhow::anyhow!("M2 service state lock poisoned"))
    }

    fn tool_router() -> ToolRouter<Self> {
        let mut router = Self::m1_tool_router()
            + Self::m2_tool_router()
            + Self::lease_tool_router()
            + Self::session_tool_router()
            + Self::user_activity_tool_router()
            + Self::agent_mailbox_tool_router()
            + Self::agent_cost_tool_router()
            + Self::cost_facade_tool_router()
            + Self::agent_stats_tool_router()
            + Self::agent_query_tool_router()
            + Self::agent_control_tool_router()
            + Self::agent_template_tool_router()
            + Self::agent_task_tool_router()
            + Self::agent_facade_tool_router()
            + Self::approval_facade_tool_router()
            + Self::audit_replay_facade_tool_router()
            + Self::timeline_facade_tool_router()
            + Self::routine_assist_facade_tool_router()
            + Self::operational_facade_tool_router()
            + Self::workspace_blackboard_tool_router()
            + Self::target_claim_tool_router()
            + Self::reality_tool_router()
            + Self::m3_tool_router()
            + Self::intent_tool_router()
            + Self::routine_labeling_tool_router()
            + Self::routine_feedback_tool_router()
            + Self::plan_tool_router()
            + Self::suggestions_tool_router()
            + Self::timeline_query_tool_router()
            + Self::timeline_digest_tool_router()
            + Self::m4_tool_router()
            + Self::notify_tool_router()
            + Self::hygiene_report_tool_router()
            + Self::data_cleaning_tool_router()
            + Self::permission_gate_tool_router()
            + Self::escalation_tool_router()
            + Self::background_router_tool_router()
            + Self::browser_batch_tool_router()
            + Self::capture_gif_tool_router()
            + Self::livecapture_tool_router()
            + Self::videoanalyze_tool_router()
            + Self::verification_tool_router()
            + Self::browser_assert_tool_router()
            + Self::browser_clock_events_tool_router()
            + Self::browser_dialog_tool_router()
            + Self::browser_dnd_tool_router()
            + Self::browser_emulate_tool_router()
            + Self::browser_facade_tool_router()
            + Self::browser_field_tool_router()
            + Self::browser_files_tool_router()
            + Self::browser_frames_tool_router()
            + Self::browser_network_tool_router()
            + Self::browser_storage_tool_router()
            + Self::tool_profile_tool_router();
        #[cfg(feature = "livefeed")]
        {
            router += Self::livefeed_tool_router();
        }
        // The EverQuest domain pack (25 tools) is off the general-agent surface
        // unless the operator opts in (SYNAPSE_ENABLE_EVERQUEST). No capability
        // is lost — visibility is gated. rmcp builds the tool list once per
        // service, so a startup opt-in flag is the gating mechanism (dynamic
        // per-profile re-listing would require tools/list_changed plumbing).
        if everquest_enabled() {
            router = router
                + Self::everquest_tool_router()
                + Self::everquest_autocombat_tool_router()
                + Self::everquest_contextgraph_tool_router()
                + Self::everquest_domain_tool_router()
                + Self::everquest_episode_export_tool_router()
                + Self::everquest_guard_tool_router()
                + Self::everquest_state_tool_router()
                + Self::everquest_map_sensor_tool_router()
                + Self::everquest_memory_tool_router()
                + Self::everquest_outcome_tool_router()
                + Self::everquest_predictive_model_tool_router()
                + Self::everquest_route_tool_router()
                + Self::everquest_scorecard_tool_router()
                + Self::everquest_surprise_tool_router()
                + Self::everquest_trajectory_tool_router()
                + Self::everquest_world_model_tool_router()
                + Self::everquest_world_summary_tool_router();
        }
        // Gate the synthetic fault-injectors off the default agent surface;
        // they remain available only when SYNAPSE_DEBUG_TOOLS is set, mirroring
        // the EverQuest opt-in model — no capability is lost, visibility is gated.
        // `storage_pressure_sample` simulates disk pressure, and the two
        // `action_diagnostic_*` tools force ACTION_RATE_LIMITED / ACTION_QUEUE_FULL
        // to exercise the action emitter's backpressure paths (#1348). They have
        // no production callers and every test that drives them sets
        // SYNAPSE_DEBUG_TOOLS=1.
        //
        // NOTE: `storage_put_probe_rows` is intentionally NOT gated here — it is a
        // default-granted real diagnostic write tool (normal_agent surface, see
        // tool_profiles tests) used by storage-GC FSV to seed bounded probe rows.
        if !debug_tools_enabled() {
            router.remove_route("storage_pressure_sample");
            router.remove_route("action_diagnostic_rate_limit_override");
            router.remove_route("action_diagnostic_queue_full_setup");
        }
        router
    }
}

/// Pure decision for [`SynapseService::action_session_target_override`] (#984):
/// maps explicit per-call routing params to a [`SessionTarget`], or `Ok(None)`
/// when neither is supplied (caller should fall back to the bound session
/// target). A `cdp_target_id` without a `window_hwnd` is rejected because the
/// browser window's HWND is needed to reach the CDP endpoint that owns the
/// target. Extracted as a free function so the routing precedence is unit-tested
/// without standing up a full service.
pub(crate) fn explicit_action_target(
    explicit_window_hwnd: Option<i64>,
    explicit_cdp_target_id: Option<&str>,
) -> Result<Option<SessionTarget>, ErrorData> {
    let explicit_cdp_target_id = explicit_cdp_target_id
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match (explicit_window_hwnd, explicit_cdp_target_id) {
        (Some(window_hwnd), Some(cdp_target_id)) => Ok(Some(SessionTarget::Cdp {
            window_hwnd,
            cdp_target_id: cdp_target_id.to_owned(),
        })),
        (Some(hwnd), None) => Ok(Some(SessionTarget::Window { hwnd })),
        (None, Some(_)) => Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "cdp_target_id requires window_hwnd (the browser window whose CDP endpoint owns the target)",
        )),
        (None, None) => Ok(None),
    }
}

/// Whether test-only/debug MCP tools should be exposed on the surface.
fn debug_tools_enabled() -> bool {
    std::env::var("SYNAPSE_DEBUG_TOOLS")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Whether the EverQuest domain tool pack should be advertised.
fn everquest_enabled() -> bool {
    std::env::var("SYNAPSE_ENABLE_EVERQUEST")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

impl Default for SynapseService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "server/tests/emergency_handlers_94.rs"]
mod emergency_handlers_94;
#[cfg(test)]
#[path = "server/tests/g7a_support.rs"]
mod g7a_support;

#[cfg(test)]
#[path = "server/g7b_support.rs"]
pub(crate) mod g7b_support;

#[cfg(test)]
#[path = "server/storage_maintenance_observer.rs"]
mod storage_maintenance_observer;

#[cfg(test)]
#[path = "server/tests/emergency_reservation_identity_95.rs"]
mod emergency_reservation_identity_95;
