//! #135 raid: `shell_terminal_pre_marker_parse_failure_is_classified_without_cleanup_unverified`
//! (one of the eight #135 tests) was deleted with m4's inline tests in 8081a84c and never
//! replaced. The classifier decides whether an SSH job that died before printing its
//! remote pid marker is a remote-shell parse failure (no remote process to clean up) or a
//! job whose remote cleanup must stay "unverified". These tests pin that classification.

use super::{RemotePreMarkerTerminalEvidence, remote_pre_marker_terminal_evidence};

#[test]
fn must_pass_remote_shell_parse_failures_are_classified_with_reason_and_pattern_135() {
    let cases = [
        (
            "bash: -c: line 1: unexpected EOF while looking for matching `''",
            "remote_shell_unmatched_quote",
            "unexpected eof while looking for matching",
        ),
        (
            "sh: 1: Syntax error: Unterminated quoted string",
            "remote_shell_unterminated_quote",
            "syntax error: unterminated quoted string",
        ),
        (
            "bash: -c: line 2: syntax error: unexpected end of file",
            "remote_shell_unexpected_end",
            "syntax error: unexpected end of file",
        ),
        (
            "zsh:1: parse error near `)'",
            "remote_shell_parse_error",
            "parse error near",
        ),
    ];
    for (stderr, reason, pattern) in cases {
        assert_eq!(
            remote_pre_marker_terminal_evidence(stderr),
            Some(RemotePreMarkerTerminalEvidence { reason, pattern }),
            "stderr {stderr:?}"
        );
    }
}

#[test]
fn must_fail_non_parse_stderr_is_not_classified_as_pre_marker_terminal_135() {
    for stderr in [
        "",
        "ssh: connect to host 10.0.0.9 port 22: Connection timed out",
        "Permission denied (publickey).",
        "bash: line 1: cargo: command not found",
        "EOF while reading the prompt",
    ] {
        assert_eq!(
            remote_pre_marker_terminal_evidence(stderr),
            None,
            "stderr {stderr:?} must leave remote cleanup unverified, not pre-marker terminal"
        );
    }
}
