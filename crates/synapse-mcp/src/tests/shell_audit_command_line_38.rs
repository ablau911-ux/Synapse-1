//! #38 re-review regression: the audit/authorization command line must equal the bytes
//! Rust's Windows `Command` hands to `CreateProcessW`.
//!
//! std quotes an argument only when it is empty or contains an ASCII space/tab, always
//! escapes `"` (`n` preceding backslashes become `2n+1`), and doubles trailing
//! backslashes only when it added the closing quote. The earlier serializer also wrapped
//! quote-only and Unicode-whitespace arguments, so the recorded line and its hash named
//! bytes the OS never received.

use super::*;

#[test]
fn must_fail_quote_only_argument_is_escaped_but_not_wrapped_38() {
    assert_eq!(quote_command_part("a\"b"), "a\\\"b");
}

#[test]
fn must_fail_unicode_whitespace_and_newlines_do_not_trigger_wrapping_38() {
    assert_eq!(quote_command_part("a\u{00a0}b"), "a\u{00a0}b");
    assert_eq!(quote_command_part("a\u{2003}b"), "a\u{2003}b");
    assert_eq!(quote_command_part("a\nb"), "a\nb");
}

#[test]
fn must_pass_ascii_space_tab_and_empty_arguments_are_wrapped_38() {
    assert_eq!(quote_command_part("a b"), "\"a b\"");
    assert_eq!(quote_command_part("a\tb"), "\"a\tb\"");
    assert_eq!(quote_command_part(""), "\"\"");
}

#[test]
fn must_fail_trailing_backslashes_double_only_when_wrapped_38() {
    assert_eq!(quote_command_part("dir\\"), "dir\\");
    assert_eq!(quote_command_part("my dir\\"), "\"my dir\\\\\"");
    assert_eq!(quote_command_part("my dir\\\\"), "\"my dir\\\\\\\\\"");
}

#[test]
fn must_fail_backslashes_before_a_quote_are_doubled_plus_one_38() {
    assert_eq!(quote_command_part("a\\\"b"), "a\\\\\\\"b");
    assert_eq!(quote_command_part("a b\\\"c"), "\"a b\\\\\\\"c\"");
}

#[cfg(windows)]
#[test]
fn must_pass_metadata_command_line_for_quote_only_argument_matches_std_bytes_38() {
    let args = vec!["a\"b".to_owned(), "plain".to_owned()];
    let metadata = shell_command_metadata("tool.exe", &args);
    assert_eq!(metadata.command_line, "\"tool.exe\" a\\\"b plain");
}

/// Ground truth: ask a real child for the command line the OS gave it and compare it with
/// what the audit serializer claims.
#[cfg(windows)]
#[tokio::test]
async fn must_pass_audit_command_line_equals_what_the_child_process_received_38() {
    let temp = match tempfile::TempDir::new() {
        Ok(temp) => temp,
        Err(error) => panic!("tempdir: {error}"),
    };
    let script = temp.path().join("cmdline 38.ps1");
    if let Err(error) = std::fs::write(
        &script,
        "[Console]::Out.Write([Environment]::CommandLine)\n",
    ) {
        panic!("write script: {error}");
    }
    let args: Vec<String> = [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        script.to_string_lossy().as_ref(),
        "a\"b",
        "plain",
        "two words",
        "tab\tword",
        "dir\\",
        "my dir\\",
        "q \"uoted\" x",
        "back\\\\\"slash",
        "",
    ]
    .iter()
    .map(|arg| (*arg).to_owned())
    .collect();

    let mut command = TokioCommand::new("powershell.exe");
    command.args(&args);
    let output = match command.output().await {
        Ok(output) => output,
        Err(error) => panic!("spawn powershell.exe: {error}"),
    };
    assert!(
        output.status.success(),
        "powershell failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let received = String::from_utf8_lossy(&output.stdout).into_owned();
    let claimed = shell_command_line_from_parts("powershell.exe", &args);
    assert_eq!(
        received, claimed,
        "audit line must equal the OS command line"
    );
}
