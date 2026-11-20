use std::{process::Stdio, sync::OnceLock};

use synapse_test_utils::stdio_mcp_client::StdioMcpClient;
use tempfile::TempDir;
use tokio::process::Command;

#[tokio::test]
async fn must_fail_allow_shell_rejects_broad_patterns_at_startup() -> anyhow::Result<()> {
    let _guard = cli_allow_test_lock().lock().await;
    let cases = [
        (".*", "unbounded_any_character_repetition"),
        ("^.+$", "unbounded_any_character_repetition"),
        ("", "empty_pattern"),
        ("git status", "shell_pattern_must_match_full_command_line"),
        ("^$", "matches_empty"),
    ];

    for (pattern, reason) in cases {
        let dir = TempDir::new()?;
        let output = Command::new(env!("CARGO_BIN_EXE_synapse-mcp"))
            .args(["--mode", "stdio", "--db"])
            .arg(isolated_db(dir.path()))
            .args(["--allow-shell", pattern])
            .env("SYNAPSE_LOG_DIR", dir.path())
            .env("LOCALAPPDATA", dir.path())
            .env_remove("SYNAPSE_ALLOW_SHELL")
            .env_remove("SYNAPSE_ALLOW_LAUNCH")
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .output()
            .await?;

        assert_eq!(
            output.status.code(),
            Some(2),
            "pattern {pattern:?} should fail startup"
        );
        let stderr = String::from_utf8(output.stderr)?;
        let logs = read_logs(dir.path())?;
        let combined = format!("{stderr}\n{logs}");
        assert!(combined.contains("CONFIG_INVALID"), "{combined}");
        assert!(combined.contains("SHELL_PATTERN_TOO_BROAD"), "{combined}");
        assert!(combined.contains("SYNAPSE_ALLOW_SHELL"), "{combined}");
        assert!(combined.contains(reason), "{combined}");
        assert!(!combined.contains("MCP_STDIO_STARTED"), "{combined}");
    }

    Ok(())
}

#[tokio::test]
async fn must_fail_allow_launch_rejects_broad_patterns_at_startup() -> anyhow::Result<()> {
    let _guard = cli_allow_test_lock().lock().await;
    let cases = [
        (".*", "unbounded_any_character_repetition"),
        ("^.+$", "unbounded_any_character_repetition"),
        ("", "empty_pattern"),
        ("notepad.exe", "launch_pattern_must_match_full_command_line"),
        ("^$", "matches_empty"),
    ];

    for (pattern, reason) in cases {
        let dir = TempDir::new()?;
        let output = Command::new(env!("CARGO_BIN_EXE_synapse-mcp"))
            .args(["--mode", "stdio", "--db"])
            .arg(isolated_db(dir.path()))
            .args(["--allow-launch", pattern])
            .env("SYNAPSE_LOG_DIR", dir.path())
            .env("LOCALAPPDATA", dir.path())
            .env_remove("SYNAPSE_ALLOW_SHELL")
            .env_remove("SYNAPSE_ALLOW_LAUNCH")
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .output()
            .await?;

        assert_eq!(
            output.status.code(),
            Some(2),
            "pattern {pattern:?} should fail startup"
        );
        let stderr = String::from_utf8(output.stderr)?;
        let logs = read_logs(dir.path())?;
        let combined = format!("{stderr}\n{logs}");
        assert!(combined.contains("CONFIG_INVALID"), "{combined}");
        assert!(combined.contains("LAUNCH_PATTERN_TOO_BROAD"), "{combined}");
        assert!(combined.contains("SYNAPSE_ALLOW_LAUNCH"), "{combined}");
        assert!(combined.contains(reason), "{combined}");
        assert!(!combined.contains("MCP_STDIO_STARTED"), "{combined}");
    }

    Ok(())
}

#[tokio::test]
async fn must_pass_allow_shell_accepts_narrow_patterns_and_reaches_stdio() -> anyhow::Result<()> {
    let _guard = cli_allow_test_lock().lock().await;
    let cases = [
        r"^git \w+$",
        r"^echo .{0,100}$",
        r"^cargo (build|test)( --[\w-]+)*$",
    ];

    // Contract (src/m4.rs `from_parts_with_any_env`): a configured family
    // restricts to its patterns; a family with no patterns defaults to the
    // permissive `any` (local access is open by design). Rendered by
    // src/server/health.rs `action_health`.
    for pattern in cases {
        assert_registered_patterns(
            "--allow-shell",
            pattern,
            "allow_shell_patterns=1 allow_launch_patterns=any",
        )
        .await?;
    }

    Ok(())
}

#[tokio::test]
async fn must_pass_allow_launch_accepts_narrow_patterns_and_reaches_stdio() -> anyhow::Result<()> {
    let _guard = cli_allow_test_lock().lock().await;
    let cases = [
        r"^notepad\.exe$",
        r"^javaw\.exe$",
        r"^C:\\Program Files\\Microsoft VS Code\\Code\.exe$",
    ];

    // Same contract as above, mirrored: launch restricted to its one
    // pattern, shell left at the permissive `any` default.
    for pattern in cases {
        assert_registered_patterns(
            "--allow-launch",
            pattern,
            "allow_shell_patterns=any allow_launch_patterns=1",
        )
        .await?;
    }

    Ok(())
}

/// Starts the stdio daemon with one `flag pattern`, then reads the registered
/// pattern counts back through the real `health` tool (a parser that silently
/// dropped the pattern would report `any`) and checks a clean stdin-EOF exit.
///
/// Policy env isolation: `StdioMcpClient` strips every inherited host
/// `SYNAPSE_*` variable (incl. `SYNAPSE_ALLOW_SHELL`, `SYNAPSE_ALLOW_LAUNCH`,
/// `SYNAPSE_ALLOW_SHELL_ANY`, `SYNAPSE_ALLOW_LAUNCH_ANY`) on this launch path
/// (synapse-test-utils `apply_isolated_env`), so the CLI flag is the only
/// policy input. The precondition is read back below.
async fn assert_registered_patterns(
    flag: &str,
    pattern: &str,
    expected_counts: &str,
) -> anyhow::Result<()> {
    for key in [
        "SYNAPSE_ALLOW_SHELL",
        "SYNAPSE_ALLOW_LAUNCH",
        "SYNAPSE_ALLOW_SHELL_ANY",
        "SYNAPSE_ALLOW_LAUNCH_ANY",
    ] {
        if std::env::var_os(key).is_some() {
            eprintln!("note: host sets {key}; the harness strips it from the daemon env");
        }
    }
    let dir = TempDir::new()?;
    let mut client =
        StdioMcpClient::launch_and_init_with_args(Some(dir.path()), &[flag, pattern], &[]).await?;
    let health = client.tools_call("health", serde_json::json!({})).await?;
    let detail = health["structuredContent"]["subsystems"]["action"]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        detail.contains(expected_counts),
        "{flag} {pattern:?}: expected `{expected_counts}` in action health detail: {detail}"
    );
    let status = client.shutdown().await?;
    assert!(
        status.success(),
        "{flag} {pattern:?}: daemon exit {status:?}"
    );
    let logs = read_logs(dir.path())?;
    assert!(logs.contains("MCP_STDIO_STARTED"), "{logs}");
    Ok(())
}

fn cli_allow_test_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Per-launch DB path under the test's own temp dir (#58): never the shared default DB
/// (and its single-instance lock) under the real %LOCALAPPDATA%\synapse\db.
fn isolated_db(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("db")
}

fn read_logs(path: &std::path::Path) -> anyhow::Result<String> {
    let mut logs = String::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.metadata()?.is_file() {
            logs.push_str(&std::fs::read_to_string(entry.path())?);
        }
    }
    Ok(logs)
}
