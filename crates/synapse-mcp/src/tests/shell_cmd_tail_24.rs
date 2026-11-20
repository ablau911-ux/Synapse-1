//! #24 re-review regression: an explicit `cmd.exe /c <payload>` must reach cmd with the
//! payload's quotes intact.
//!
//! Rust's regular argument encoder backslash-escapes embedded quotes (`\"`), which is the
//! CRT convention and means nothing to cmd.exe, so a quoted `echo ... | find ...` payload
//! was mangled. cmd gets a dedicated command-tail serializer: when the tail contains a
//! quote it is passed verbatim inside one extra outer quote pair (cmd strips the first and
//! last quote), and the audit renderer emits the same bytes. Ordinary executables keep the
//! CRT encoding.

use super::command_line::{SpawnArg, plan_spawn_args};
use super::*;

fn owned(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

#[test]
fn must_pass_quoted_cmd_payload_is_passed_verbatim_inside_one_outer_quote_pair_24() {
    let planned = plan_spawn_args(
        "cmd.exe",
        &owned(&["/c", "echo \"hello world\" | find \"hello\""]),
    );
    assert_eq!(
        planned,
        vec![
            SpawnArg::Regular("/c".to_owned()),
            SpawnArg::Raw("\"echo \"hello world\" | find \"hello\"\"".to_owned()),
        ]
    );
}

#[test]
fn must_fail_cmd_switches_before_the_run_switch_stay_regular_24() {
    let planned = plan_spawn_args("cmd.exe", &owned(&["/d", "/s", "/C", "dir \"C:\\a b\""]));
    assert_eq!(
        planned,
        vec![
            SpawnArg::Regular("/d".to_owned()),
            SpawnArg::Regular("/s".to_owned()),
            SpawnArg::Regular("/C".to_owned()),
            SpawnArg::Raw("\"dir \"C:\\a b\"\"".to_owned()),
        ]
    );
}

#[test]
fn must_pass_multiple_tail_arguments_are_joined_and_only_quote_free_ones_are_wrapped_24() {
    let planned = plan_spawn_args(
        "C:\\Windows\\System32\\CMD.EXE",
        &owned(&["/k", "echo", "\"a b\"", "plain tail", ""]),
    );
    assert_eq!(
        planned,
        vec![
            SpawnArg::Regular("/k".to_owned()),
            SpawnArg::Raw("\"echo \"a b\" \"plain tail\" \"\"\"".to_owned()),
        ]
    );
}

#[test]
fn must_pass_quote_free_cmd_tail_keeps_the_regular_encoding_24() {
    let args = owned(&["/c", "echo hello", ""]);
    let planned = plan_spawn_args("cmd.exe", &args);
    assert_eq!(
        planned,
        args.iter()
            .cloned()
            .map(SpawnArg::Regular)
            .collect::<Vec<_>>()
    );
}

#[test]
fn must_fail_cmd_without_a_run_switch_keeps_the_regular_encoding_24() {
    let args = owned(&["/d", "echo \"x\""]);
    let planned = plan_spawn_args("cmd.exe", &args);
    assert_eq!(
        planned,
        args.iter()
            .cloned()
            .map(SpawnArg::Regular)
            .collect::<Vec<_>>()
    );
}

#[test]
fn must_fail_ordinary_executables_keep_the_crt_encoding_even_with_quotes_and_c_switch_24() {
    let args = owned(&["/c", "say \"hi\"", "C:\\dir with space\\", "a\"b"]);
    for program in ["powershell.exe", "ssh", "notcmd.exe", "cmd.exe.bak"] {
        let planned = plan_spawn_args(program, &args);
        assert_eq!(
            planned,
            args.iter()
                .cloned()
                .map(SpawnArg::Regular)
                .collect::<Vec<_>>(),
            "{program}"
        );
    }
}

#[cfg(windows)]
#[test]
fn must_pass_audit_line_for_a_quoted_cmd_payload_is_the_raw_tail_24() {
    let line = shell_command_line_from_parts(
        "cmd.exe",
        &owned(&["/c", "echo \"hello world\" | find \"hello\""]),
    );
    assert_eq!(
        line,
        "\"cmd.exe\" /c \"echo \"hello world\" | find \"hello\"\""
    );
}

#[cfg(windows)]
async fn run_cmd(args: &[String]) -> (Option<i32>, String) {
    let mut command = TokioCommand::new("cmd.exe");
    super::command_line::apply_args(&mut command, "cmd.exe", args);
    let output = match command.output().await {
        Ok(output) => output,
        Err(error) => panic!("spawn cmd.exe: {error}"),
    };
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

/// The original report: a quoted echo piped into find must run as typed.
#[cfg(windows)]
#[tokio::test]
async fn must_pass_cmd_runs_the_quoted_echo_find_payload_as_typed_24() {
    // Name Windows find.exe explicitly: Git-for-Windows puts GNU find first on PATH.
    let (code, stdout) = run_cmd(&owned(&[
        "/c",
        "echo \"hello world\" | \"%SystemRoot%\\System32\\find.exe\" \"hello\"",
    ]))
    .await;
    assert_eq!(code, Some(0), "stdout: {stdout:?}");
    assert!(
        stdout.contains("\"hello world\""),
        "quotes must survive: {stdout:?}"
    );
    assert!(
        !stdout.contains('\\'),
        "no CRT backslash escapes: {stdout:?}"
    );
}

/// Ground truth: cmd reports the exact command line it was created with.
#[cfg(windows)]
#[tokio::test]
async fn must_pass_cmd_receives_exactly_the_line_the_audit_renderer_reports_24() {
    let args = owned(&["/c", "echo \"x y\" & echo %CMDCMDLINE%"]);
    let (code, stdout) = run_cmd(&args).await;
    assert_eq!(code, Some(0), "stdout: {stdout:?}");
    let claimed = shell_command_line_from_parts("cmd.exe", &args);
    let received = stdout.lines().last().unwrap_or_default().trim_end();
    assert_eq!(received, claimed);
}
