//! #21: `process launch` of a shell interpreter or script must obey the same
//! profile policy as the `shell` tool; other launches are not restricted.
//! Driven through the real `process` facade over the HTTP daemon.
//!
//! Fixture: an isolated daemon with NO operator allowlist. By design local
//! access is open, so the operator gate reports `allow_launch_patterns=any`
//! and admits every launch (contract: src/m4.rs `from_parts_with_any_env`;
//! rendered by src/server/health.rs `action_health`). Each test reads that
//! precondition back. The gate under test is the per-session PROFILE gate
//! (src/server/shell_profile_policy.rs `allows_process_launch`, called from
//! src/server/m4_tools.rs `require_launch_profile_policy`).
//!
//! Positive launches use only controlled, harmless, self-terminating
//! programs (console tools, `exit 0` scripts). Every launched PID is killed
//! by a drop guard, so no process outlives the test even when it fails.

#[allow(dead_code)]
mod support;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use support::{HttpDaemon, HttpMcpSession};
use synapse_core::error_codes;
use synapse_storage::{Db, cf, decode_json};

const CAPABILITY: &str = "syn21-isolated-local-agent-capability-0123456789";
const SYSTEM32: &str = r"C:\Windows\System32";

/// Refused-only targets: nothing here is ever started (the profile gate
/// refuses them first), so the files need not exist.
const SHELL_INTERPRETERS: &[(&str, &[&str])] = &[
    ("cmd.exe", &["/c", "echo pwned"]),
    ("CMD.EXE", &["/k", "echo pwned"]),
    (r"C:\Windows\System32\cmd.exe", &["/c", "echo pwned"]),
    ("cmd", &["/c", "echo pwned"]),
    ("powershell.exe", &["-Command", "Get-Date"]),
    (
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        &["-File", "x.ps1"],
    ),
    ("pwsh.exe", &["-EncodedCommand", "AAAA"]),
    ("pwsh", &["-NoProfile", "-Command", "ls"]),
    ("bash.exe", &["-c", "id"]),
    ("sh", &["-c", "id"]),
    ("wsl.exe", &["ls"]),
    ("wsl.exe", &[]),
    ("\"C:/Windows/System32/cmd.exe\"", &["/c", "dir"]),
];

/// Refused-only targets (shell_profile_policy.rs SHELL_SCRIPT_EXTENSIONS and
/// the script hosts in SHELL_INTERPRETERS).
const SHELL_SCRIPTS_AND_HOSTS: &[&str] = &[
    "run.bat",
    "RUN.BAT",
    "setup.cmd",
    r"C:\Users\Adam\scripts\deploy.CMD",
    "x.ps1",
    r"C:\tmp\x.Ps1",
    "x.vbs",
    "x.vbe",
    "x.js",
    "x.jse",
    "x.wsf",
    "x.hta",
    "\"C:/tmp/run.bat\"",
    "wscript.exe",
    "cscript.exe",
    r"C:\Windows\System32\cscript.exe",
    "mshta.exe",
    "MSHTA",
];

/// File names that merely LOOK like shell names; is_shell_interpreter must
/// not classify them as shells (stem/extension rules). Each is a copy of the
/// harmless `whoami.exe`, so a positive launch exits on its own.
const LOOKALIKE_EXECUTABLES: &[&str] = &[
    "bashful.exe",
    "cmdline-tool.exe",
    "myshell.exe",
    "bat.exe",
    "ps1.exe",
];

/// Kills every PID the test launched, on every exit path.
struct LaunchedProcesses {
    launched: Vec<(u32, String)>,
}

impl LaunchedProcesses {
    const fn new() -> Self {
        Self {
            launched: Vec::new(),
        }
    }

    fn record(&mut self, pid: u32, target: &str) {
        self.launched.push((pid, target.to_owned()));
    }
}

/// Image name the launched PID runs as: scripts run under `cmd.exe`; other
/// targets run as their own file name.
fn expected_image_name(target: &str) -> String {
    let lowered = target.trim_matches('"').to_ascii_lowercase();
    let file = lowered
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(&lowered)
        .to_owned();
    if file.ends_with(".bat") || file.ends_with(".cmd") {
        "cmd.exe".to_owned()
    } else if file.contains('.') {
        file
    } else {
        format!("{file}.exe")
    }
}

impl Drop for LaunchedProcesses {
    fn drop(&mut self) {
        for (pid, target) in &self.launched {
            // Exact PID AND expected image name, so a PID the OS already
            // reused for an unrelated process is never killed. An
            // already-exited PID is fine (no match).
            let pid_filter = format!("PID eq {pid}");
            let image_filter = format!("IMAGENAME eq {}", expected_image_name(target));
            let _ = std::process::Command::new("taskkill")
                .args([
                    "/F",
                    "/T",
                    "/FI",
                    pid_filter.as_str(),
                    "/FI",
                    image_filter.as_str(),
                ])
                .output();
        }
    }
}

async fn start_daemon() -> anyhow::Result<HttpDaemon> {
    // Synthetic notepad foreground: the action scope gate that precedes the
    // launch profile gate must not depend on the host's real foreground.
    HttpDaemon::start(&[
        ("SYNAPSE_LOCAL_AGENT_CAPABILITY", CAPABILITY),
        ("SYNAPSE_MCP_SYNTHETIC_FIXTURE", "notepad"),
    ])
    .await
}

async fn effective_profile(session: &mut HttpMcpSession) -> anyhow::Result<Value> {
    let status = session
        .tools_call("profile", json!({"operation": "status"}))
        .await?;
    Ok(status["structuredContent"]["status"]["snapshot"]["profile"].clone())
}

/// Fixture precondition readback: the operator gate is open (`any`) because
/// no allowlist is configured (health.rs `action_health`).
async fn assert_operator_gate_open(session: &mut HttpMcpSession) -> anyhow::Result<()> {
    let health = session.tools_call("health", json!({})).await?;
    let detail = health["structuredContent"]["subsystems"]["action"]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        detail.contains("allow_shell_patterns=any allow_launch_patterns=any"),
        "fixture precondition: isolated daemon must have the open operator gate: {detail}"
    );
    Ok(())
}

async fn launch_refused(
    session: &mut HttpMcpSession,
    target: &str,
    args: &[&str],
) -> anyhow::Result<Value> {
    session
        .tools_call_error(
            "process",
            json!({"operation": "launch", "target": target, "args": args}),
        )
        .await
}

/// A successful launch; records the PID for cleanup and returns it
/// (src/m4.rs `ActLaunchResponse.pid`). cmd/powershell get a hidden console
/// (m4.rs `launch_target_needs_new_console`).
async fn launch_ok(
    session: &mut HttpMcpSession,
    launched: &mut LaunchedProcesses,
    target: &str,
    args: &[&str],
) -> anyhow::Result<u32> {
    let mut call = json!({"operation": "launch", "target": target, "args": args});
    let file = target.to_ascii_lowercase();
    if (file.ends_with("cmd.exe") || file.ends_with("powershell.exe"))
        && let Some(call) = call.as_object_mut()
    {
        call.insert("windows_console_window_state".into(), json!("hidden"));
    }
    let response = session.tools_call("process", call).await?;
    let pid = response["structuredContent"]["launch"]["pid"]
        .as_u64()
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0);
    let Some(pid) = pid else {
        anyhow::bail!("{target}: launch readback has no pid: {response}");
    };
    launched.record(pid, target);
    Ok(pid)
}

fn assert_profile_denied(error: &Value, target: &str, profile: &str) {
    // m4_tools.rs require_launch_profile_policy message.
    assert_eq!(
        error["data"]["code"],
        error_codes::TOOL_PROFILE_POLICY_DENIED,
        "{target}: {error}"
    );
    let message = error["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("launching shell interpreter")
            && message.contains(&format!("profile {profile}")),
        "{target}: denial must come from the #21 launch profile gate: {error}"
    );
}

/// `(pid, target)` of every CF_PROCESS_HISTORY `process_start` row
/// (m4.rs `launch_process_history_row`).
fn process_history(db_path: &Path) -> anyhow::Result<Vec<(u64, String)>> {
    let db = Db::open(db_path, synapse_core::SCHEMA_VERSION)?;
    let mut rows = Vec::new();
    for (_key, value) in db.scan_cf_prefix(cf::CF_PROCESS_HISTORY, b"")? {
        let row: Value = decode_json(&value)?;
        if row["row_kind"] == "process_start" {
            rows.push((
                row["pid"].as_u64().unwrap_or_default(),
                row["target"].as_str().unwrap_or_default().to_owned(),
            ));
        }
    }
    Ok(rows)
}

/// Every launched `(pid, target)` has its own process-history row.
fn assert_history_matches(db_path: &Path, launched: &LaunchedProcesses) -> anyhow::Result<()> {
    let history = process_history(db_path)?;
    for (pid, target) in &launched.launched {
        assert!(
            history
                .iter()
                .any(|(row_pid, row_target)| row_pid == &u64::from(*pid) && row_target == target),
            "launch {target} pid {pid} has no CF_PROCESS_HISTORY row: {history:?}"
        );
    }
    assert_eq!(history.len(), launched.launched.len(), "{history:?}");
    Ok(())
}

/// Harmless fixtures in the test's own directory, read back before use.
struct ScriptFixtures {
    dir: tempfile::TempDir,
}

impl ScriptFixtures {
    fn create() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("ok.bat"), "@exit /b 0\r\n")?;
        std::fs::write(dir.path().join("ok.cmd"), "@exit /b 0\r\n")?;
        std::fs::write(dir.path().join("quit.vbs"), "WScript.Quit 0\r\n")?;
        let whoami = Path::new(SYSTEM32).join("whoami.exe");
        for name in LOOKALIKE_EXECUTABLES {
            std::fs::copy(&whoami, dir.path().join(name))?;
        }
        let fixtures = Self { dir };
        for name in ["ok.bat", "ok.cmd", "quit.vbs"]
            .iter()
            .chain(LOOKALIKE_EXECUTABLES)
        {
            anyhow::ensure!(
                fixtures.path(name).is_file(),
                "fixture precondition: {name} must exist"
            );
        }
        Ok(fixtures)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn arg(&self, name: &str) -> String {
        self.path(name).to_string_lossy().into_owned()
    }
}

#[tokio::test]
async fn must_fail_normal_agent_cannot_launch_shell_interpreters() -> anyhow::Result<()> {
    let daemon = start_daemon().await?;
    let db_path = daemon.db_path().to_path_buf();
    let mut session = daemon.session("syn21-normal-agent", &[]).await?;
    assert_eq!(effective_profile(&mut session).await?, "normal_agent");
    assert_operator_gate_open(&mut session).await?;
    for (target, args) in SHELL_INTERPRETERS {
        let error = launch_refused(&mut session, target, args).await?;
        assert_profile_denied(&error, target, "normal_agent");
    }
    let _home = daemon.stop().await?;
    assert!(process_history(&db_path)?.is_empty(), "nothing may launch");
    Ok(())
}

#[tokio::test]
async fn must_fail_normal_agent_cannot_launch_shell_scripts_or_script_hosts() -> anyhow::Result<()>
{
    let daemon = start_daemon().await?;
    let db_path = daemon.db_path().to_path_buf();
    let mut session = daemon.session("syn21-normal-agent", &[]).await?;
    assert_eq!(effective_profile(&mut session).await?, "normal_agent");
    assert_operator_gate_open(&mut session).await?;
    for target in SHELL_SCRIPTS_AND_HOSTS {
        let error = launch_refused(&mut session, target, &[]).await?;
        assert_profile_denied(&error, target, "normal_agent");
    }
    let _home = daemon.stop().await?;
    assert!(process_history(&db_path)?.is_empty(), "nothing may launch");
    Ok(())
}

#[tokio::test]
async fn must_pass_full_capability_keeps_launching_shell_interpreters_and_scripts()
-> anyhow::Result<()> {
    let fixtures = ScriptFixtures::create()?;
    let mut launched = LaunchedProcesses::new();
    let daemon = start_daemon().await?;
    let db_path = daemon.db_path().to_path_buf();
    let mut session = daemon
        .session(
            "synapse-local-model-agent",
            &[("X-Synapse-Local-Agent-Capability", CAPABILITY)],
        )
        .await?;
    assert_eq!(effective_profile(&mut session).await?, "full_capability");
    assert_operator_gate_open(&mut session).await?;

    // full_capability admits shell launches (shell_profile_policy.rs
    // allows_shell_command: "break_glass" | "full_capability" => true), and
    // the open operator gate admits them too. Every program exits by itself.
    let cases: Vec<(String, Vec<String>)> = vec![
        ("cmd.exe".into(), vec!["/c".into(), "exit 0".into()]),
        (
            format!(r"{SYSTEM32}\cmd.exe"),
            vec!["/c".into(), "exit 0".into()],
        ),
        (
            "powershell.exe".into(),
            vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "exit 0".into(),
            ],
        ),
        (
            format!(r"{SYSTEM32}\cscript.exe"),
            vec!["//Nologo".into(), "//B".into(), fixtures.arg("quit.vbs")],
        ),
        (fixtures.arg("ok.bat"), Vec::new()),
        (fixtures.arg("ok.cmd"), Vec::new()),
    ];
    for (target, args) in &cases {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        launch_ok(&mut session, &mut launched, target, &args).await?;
    }

    let _home = daemon.stop().await?;
    assert_history_matches(&db_path, &launched)
}

#[tokio::test]
async fn must_pass_normal_agent_non_shell_and_allowlisted_launches_pass_profile_gate()
-> anyhow::Result<()> {
    let fixtures = ScriptFixtures::create()?;
    let mut launched = LaunchedProcesses::new();
    let daemon = start_daemon().await?;
    let db_path = daemon.db_path().to_path_buf();
    let mut session = daemon.session("syn21-normal-agent", &[]).await?;
    assert_eq!(effective_profile(&mut session).await?, "normal_agent");
    assert_operator_gate_open(&mut session).await?;

    // Non-shell launches are never profile-restricted (shell_profile_policy.rs
    // allows_process_launch: `!is_shell_interpreter(target) || ...`), even
    // when the name merely resembles a shell. The allowlisted whoami/hostname
    // pass too (NORMAL_AGENT_SHELL_ALLOWLIST).
    let mut targets: Vec<String> = LOOKALIKE_EXECUTABLES
        .iter()
        .map(|name| fixtures.arg(name))
        .collect();
    targets.push(format!(r"{SYSTEM32}\whoami.exe"));
    targets.push(format!(r"{SYSTEM32}\hostname.exe"));
    for target in &targets {
        launch_ok(&mut session, &mut launched, target, &[]).await?;
    }

    let _home = daemon.stop().await?;
    assert_history_matches(&db_path, &launched)
}
