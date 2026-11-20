//! Real facade helpers; safe console effects, never an executable GUI launch.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::server::{
    g4_unit_support::{seed_live_session, use_synthetic_notepad_foreground},
    tool_profiles::ToolProfileKind,
    user_active_258_support::{Fixture, fixture},
};
use crate::{
    m4,
    server::{SynapseService, m4_tools::ShellParams},
};
use rmcp::{ErrorData, handler::server::wrapper::Parameters};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub const SESSION: &str = "g268-shell-facade";
pub const REASON: &str = "owner explicitly requested this shell launch while typing";
pub const ROUTES: [&str; 3] = ["inline", "durable", "start"];

pub fn setup(allow_console: bool, work: &Path) -> Fixture {
    let mut fx = fixture(Some("60000"));
    seed_live_session(&fx.service, SESSION);
    use_synthetic_notepad_foreground(&fx.service);
    fx.service
        .set_tool_profile_for_test(SESSION, ToolProfileKind::FullCapability)
        .expect("full capability avoids unrelated profile restrictions");
    // Deny-all for real GUI commands. Allowlist only safe console executables
    // for marker probes; all GUI-looking statements there are unreachable.
    let patterns = if allow_console {
        console_requests(work)
            .into_iter()
            .map(|request| {
                let command = request["command"].as_str().expect("command");
                let args: Vec<String> =
                    serde_json::from_value(request["args"].clone()).expect("command args");
                let line = m4::shell_command_line_from_parts(command, &args);
                format!("^{}$", regex::escape(&line))
            })
            .collect()
    } else {
        Vec::new()
    };
    fx.service.m4_config =
        m4::M4ServiceConfig::from_cli_parts(patterns, Vec::new(), 60_000).expect("shell policy");
    fx
}

fn console_requests(work: &Path) -> Vec<Value> {
    let mut requests = Vec::new();
    for route in ROUTES {
        for (index, syntax) in [
            "Start-Process firefox",
            "saps firefox",
            "cmd /c start firefox",
            "explorer https://example.com",
            "explorer.exe https://example.com",
            "Invoke-Item .",
            "ii .",
            "rundll32.exe url.dll,FileProtocolHandler https://example.com",
            "sTaRt-PrOcEsS firefox",
        ]
        .into_iter()
        .enumerate()
        {
            requests.push(safe_gui_probe(
                route,
                syntax,
                &work.join(format!("{route}-{index}.pid")),
            ));
        }
        requests.push(request(
            route,
            "cmd.exe",
            vec![
                "/d".into(),
                "/c".into(),
                format!(
                    "echo started>\"{}\" & if 1==2 start firefox",
                    work.join(format!("cmd-{route}.started")).display()
                ),
            ],
            work,
        ));
        let mut markers = vec![format!("unknown-{route}.pid")];
        for known in [true, false] {
            markers.push(format!("approved-{route}-{known}.pid"));
            markers.push(format!("unapproved-{route}-{known}.pid"));
        }
        for index in 0..3 {
            markers.push(format!("bad-reason-{route}-{index}.pid"));
        }
        for age in [Some(60_000), Some(60_001), None] {
            markers.push(format!("idle-{route}-{age:?}.pid"));
        }
        for marker in markers {
            requests.push(safe_gui_probe(
                route,
                "Start-Process firefox",
                &work.join(marker),
            ));
        }
    }
    requests.push(safe_gui_probe(
        "inline",
        "Start-Process firefox",
        &work.join("active-after-preservation.pid"),
    ));
    for (command, args) in [
        ("rtk", vec!["git", "status"]),
        ("cargo", vec!["--version"]),
        (
            "powershell.exe",
            vec!["-NoProfile", "-Command", "Get-ChildItem"],
        ),
        ("python", vec!["-c", "print('g268-console-ok')"]),
    ] {
        requests.push(request(
            "inline",
            command,
            args.into_iter().map(str::to_owned).collect(),
            work,
        ));
    }
    requests.push(request(
        "inline",
        "powershell.exe",
        vec![
            "-NoProfile".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
            work.join("console-script.ps1").display().to_string(),
        ],
        work,
    ));
    requests
}

pub fn request(route: &str, command: &str, args: Vec<String>, cwd: &Path) -> Value {
    let mut value = json!({"operation": if route == "start" {"start"} else {"run"},
        "command":command, "args":args, "working_dir":cwd,
        "timeout_ms":15_000});
    if route != "start" {
        value["execution_mode"] = json!(route);
    }
    value
}

pub fn ps_request(route: &str, script: &str, cwd: &Path) -> Value {
    request(
        route,
        "powershell.exe",
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            script.into(),
        ],
        cwd,
    )
}

pub async fn call(service: &SynapseService, request: Value) -> Result<Value, ErrorData> {
    let params: ShellParams = serde_json::from_value(request.clone())
        .unwrap_or_else(|error| panic!("#268 facade must accept {request}: {error}"));
    service
        .shell(
            Parameters(params),
            crate::test_support::request_context_for_session(SESSION).await,
        )
        .await
        .map(|response| serde_json::to_value(response.0).expect("shell response"))
}

/// The first instruction records this shell's PID. The GUI syntax can never
/// execute, even when the gate is absent. Detection is lexical, per criteria L6.
pub fn safe_gui_probe(route: &str, gui_syntax: &str, marker: &Path) -> Value {
    let path = marker.display().to_string().replace('\'', "''");
    ps_request(
        route,
        &format!("[IO.File]::WriteAllText('{path}', [string]$PID); if ($false) {{ {gui_syntax} }}"),
        marker.parent().expect("marker directory"),
    )
}

pub fn assert_no_job_rows() {
    let snapshot = m4::shell_jobs_dashboard_snapshot(100).expect("job file readback");
    assert_eq!(
        snapshot.job_count, 0,
        "refusal created job rows: {snapshot:?}"
    );
    let root = snapshot.job_root.expect("job root");
    if Path::new(&root).exists() {
        assert_eq!(
            std::fs::read_dir(root).expect("job root readback").count(),
            0,
            "refusal must not leave even a partial job reservation"
        );
    }
}

pub fn gate_refusal(result: Result<Value, ErrorData>, code: &str, launch: &str) {
    let error = result.expect_err("#268 GUI launch must be refused");
    assert_eq!(
        crate::server::g4_unit_support::error_code(&error).as_deref(),
        Some(code),
        "#268 expected activity gate, not shell policy/spawn failure: {error:?}"
    );
    assert_eq!(error.code.0, -32099, "same #258 refusal shape");
    let data = error.data.as_ref().expect("typed gate data");
    let gate = if data["delivery_state"].is_string() {
        data
    } else {
        &data["cause"]
    };
    assert_eq!(
        gate["delivery_state"], "refused_before_delivery",
        "{error:?}"
    );
    assert_eq!(gate["quiet_window_ms"], 60_000, "{error:?}");
    assert_eq!(
        gate["physical_input_monitoring"],
        code != "USER_ACTIVITY_UNKNOWN",
        "{error:?}"
    );
    assert_eq!(
        gate["quiet_window_config"], "SYNAPSE_USER_ACTIVE_QUIET_MS",
        "{error:?}"
    );
    assert!(
        gate["source_of_truth"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "{error:?}"
    );
    let text = format!("{} {data}", error.message).to_lowercase();
    for required in [launch, "process", "launch", "owner_approved", "quiet"] {
        assert!(
            text.contains(&required.to_lowercase()),
            "missing {required}: {text}"
        );
    }
    let compact = text.replace([' ', '"'], "");
    assert!(
        compact.contains("operation=launch") || compact.contains("operation:launch"),
        "refusal must name process operation=launch remediation: {text}"
    );
}

pub async fn completed(service: &SynapseService, response: Value) -> Value {
    let job_id = response["run"]["job_id"]
        .as_str()
        .or_else(|| response["start"]["job"]["job_id"].as_str());
    let Some(job_id) = job_id else {
        assert_eq!(response["run"]["exit_code"], 0, "{response}");
        return response;
    };
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        let status = call(service, json!({"operation":"status", "job_id":job_id}))
            .await
            .expect("status facade");
        let state = status["status"]["job"]["status"]
            .as_str()
            .expect("job state");
        if !matches!(state, "running" | "cancel_requested" | "finalizing") {
            assert_eq!(status["status"]["job"]["exit_code"], 0, "{status}");
            // Read the persisted status independently of the facade response.
            let path = status["status"]["job"]["status_path"]
                .as_str()
                .expect("status path");
            let disk: Value = serde_json::from_slice(&std::fs::read(path).expect("status file"))
                .expect("status JSON");
            assert_eq!(disk["exit_code"], 0, "{disk}");
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "console child never finished: {status}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Copy a genuine console PE, changing only its subsystem. Deny-all policy
/// ALWAYS protects these files: they are classification fixtures, never run.
pub fn gui_pe(dir: &Path) -> std::path::PathBuf {
    let system = std::env::var_os("SystemRoot").expect("SystemRoot");
    let mut bytes = std::fs::read(Path::new(&system).join("System32/cmd.exe")).expect("console PE");
    assert_eq!(&bytes[..2], b"MZ");
    let pe = u32::from_le_bytes(bytes[0x3c..0x40].try_into().expect("PE offset")) as usize;
    assert_eq!(&bytes[pe..pe + 4], b"PE\0\0");
    let subsystem = pe + 24 + 68; // IMAGE_OPTIONAL_HEADER32/64.Subsystem
    assert_eq!(
        u16::from_le_bytes(
            bytes[subsystem..subsystem + 2]
                .try_into()
                .expect("subsystem")
        ),
        3
    );
    bytes[subsystem..subsystem + 2].copy_from_slice(&2u16.to_le_bytes());
    let path = dir.join("g268-gui-probe.exe");
    std::fs::write(&path, bytes).expect("GUI PE fixture");
    let readback = std::fs::read(&path).expect("PE readback");
    assert_eq!(&readback[subsystem..subsystem + 2], &2u16.to_le_bytes());
    path
}

/// Temporary HKCU App Paths registration with a unique name; removed on drop.
pub struct AppPath {
    key: Vec<u16>,
    pub name: String,
}
impl AppPath {
    pub fn new(exe: &Path) -> Self {
        use windows::{Win32::System::Registry::*, core::PCWSTR};
        let name = format!("g268-{}.exe", uuid::Uuid::new_v4());
        let key: Vec<u16> =
            format!("Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{name}")
                .encode_utf16()
                .chain(Some(0))
                .collect();
        let guard = Self { key, name };
        let mut handle = HKEY::default();
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(guard.key.as_ptr()),
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut handle,
                None,
            )
        }
        .ok()
        .expect("App Paths key");
        let value: Vec<u8> = exe
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        let written = unsafe { RegSetValueExW(handle, PCWSTR::null(), None, REG_SZ, Some(&value)) };
        unsafe { RegCloseKey(handle) }
            .ok()
            .expect("close App Paths key");
        written.ok().expect("App Paths value");
        guard
    }
}
impl Drop for AppPath {
    fn drop(&mut self) {
        use windows::{Win32::System::Registry::*, core::PCWSTR};
        let result = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(self.key.as_ptr())) };
        assert!(
            result.is_ok(),
            "remove test App Paths registration: {result:?}"
        );
    }
}
