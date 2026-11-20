//! #24 (reopened): the `act_launch` route still CRT-escapes an explicit `cmd.exe /c` tail.
//!
//! The shell routes plan cmd tails with `command_line::plan_spawn_args`, but
//! `launch_command_line` (the string handed to `CreateProcessW` by `spawn_windows_child`
//! and the line recorded for policy/audit) joins `quote_command_part` per argument, so
//! `echo "quoted-text"` reaches cmd as `echo \"quoted-text\"`. Children are harmless
//! (`echo` into a temp dir) and run inside a kill-on-close job dropped by the test.

#![cfg(windows)]

use std::time::{Duration, Instant};

use super::*;

const OUTPUT_DEADLINE: Duration = Duration::from_secs(15);
const QUOTED_TAIL: &str = "echo \"quoted-text\">out.txt";

fn params(target: &str, args: &[&str], dir: Option<&Path>) -> ActLaunchParams {
    ActLaunchParams {
        target: target.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        working_dir: dir.map(|dir| dir.to_string_lossy().into_owned()),
        env: BTreeMap::new(),
        wait_for_window_title_regex: None,
        timeout_ms: 10_000,
        idempotency_key: None,
        cdp_debug: Some(false),
        force_renderer_accessibility: None,
        windows_console_window_state: Some(LaunchWindowState::Hidden),
        desktop: None,
    }
}

fn line(params: &ActLaunchParams) -> String {
    launch_command_line(params).unwrap_or_else(|error| panic!("launch line: {error:?}"))
}

/// Waits until `path` holds a complete line (cmd writes it in one go) and returns it.
fn read_child_output(path: &Path) -> Option<String> {
    let started = Instant::now();
    while started.elapsed() < OUTPUT_DEADLINE {
        if let Ok(text) = std::fs::read_to_string(path)
            && text.ends_with("\r\n")
        {
            return Some(text);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

#[test]
fn must_fail_launch_line_keeps_cmd_tail_quotes_verbatim_24() {
    let rendered = line(&params("cmd.exe", &["/d", "/c", QUOTED_TAIL], None));
    assert_eq!(rendered, "cmd.exe /d /c \"echo \"quoted-text\">out.txt\"");
    assert!(
        !rendered.contains("\\\""),
        "no CRT backslash-quote may reach cmd: {rendered}"
    );
}

#[test]
fn must_fail_launched_cmd_child_receives_the_quotes_as_typed_24() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("temp dir: {error}"));
    let out = dir.path().join("out.txt");
    assert!(!out.exists(), "precondition: no output yet");
    let launch = params("cmd.exe", &["/d", "/c", QUOTED_TAIL], Some(dir.path()));

    let spawned = spawn_launch_child_with(&launch, None, true, assign_owned_process_job)
        .unwrap_or_else(|error| panic!("launch cmd: {error:?}"));
    let text = read_child_output(&out);
    drop(spawned);

    assert_eq!(text.as_deref(), Some("\"quoted-text\"\r\n"));
}

#[test]
fn must_pass_non_cmd_target_keeps_crt_encoding_24() {
    let rendered = line(&params("python.exe", &["a \"b\"", "", "tail\\"], None));
    assert_eq!(rendered, "python.exe \"a \\\"b\\\"\" \"\" tail\\");
}

#[test]
fn must_pass_quote_free_cmd_tail_keeps_regular_encoding_24() {
    let rendered = line(&params("cmd.exe", &["/d", "/c", "echo plain"], None));
    assert_eq!(rendered, "cmd.exe /d /c \"echo plain\"");
}

#[test]
fn must_pass_launched_quote_free_cmd_child_runs_24() {
    let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("temp dir: {error}"));
    let out = dir.path().join("out.txt");
    let launch = params(
        "cmd.exe",
        &["/d", "/c", "echo plain>out.txt"],
        Some(dir.path()),
    );

    let spawned = spawn_launch_child_with(&launch, None, true, assign_owned_process_job)
        .unwrap_or_else(|error| panic!("launch cmd: {error:?}"));
    let text = read_child_output(&out);
    drop(spawned);

    assert_eq!(text.as_deref(), Some("plain\r\n"));
}
