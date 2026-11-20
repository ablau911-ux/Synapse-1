//! #39c: inline shell output (and every other public readback built on
//! `redact_shell_output_for_public_readback`) must mask an unterminated
//! `BEGIN ... PRIVATE KEY` block from the header to the end of the text.

use super::redact_shell_output_for_public_readback as redact;

const BODY: &str = "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7SECRETBODY";

#[test]
fn must_fail_unterminated_private_key_body_is_masked_to_end_of_text() {
    let out = redact(&format!(
        "before\n-----BEGIN PRIVATE KEY-----\n{BODY}\nline2-secret\n"
    ));
    assert!(out.contains("before"), "{out}");
    assert!(out.contains("-----BEGIN PRIVATE KEY-----"), "{out}");
    assert!(out.contains("[redacted-shell-output]"), "{out}");
    assert!(!out.contains(BODY), "leaked body: {out}");
    assert!(!out.contains("line2-secret"), "leaked tail: {out}");
}

#[test]
fn must_fail_unterminated_rsa_key_without_trailing_newline_is_masked() {
    let out = redact(&format!("-----BEGIN RSA PRIVATE KEY-----\n{BODY}"));
    assert!(!out.contains(BODY), "{out}");
    assert!(out.contains("[redacted-shell-output]"), "{out}");
}

#[test]
fn must_fail_terminated_block_then_unterminated_block_masks_both() {
    let out = redact(&format!(
        "-----BEGIN PRIVATE KEY-----\nAAA-first\n-----END PRIVATE KEY-----\nkeep-me\n-----BEGIN EC PRIVATE KEY-----\n{BODY}"
    ));
    assert!(!out.contains("AAA-first"), "{out}");
    assert!(!out.contains(BODY), "{out}");
    assert!(out.contains("keep-me"), "{out}");
    assert!(out.contains("-----END PRIVATE KEY-----"), "{out}");
}

#[test]
fn must_pass_terminated_block_still_keeps_text_after_end() {
    let out = redact("-----BEGIN PRIVATE KEY-----\nzzz\n-----END PRIVATE KEY-----\nafter");
    assert!(!out.contains("zzz"), "{out}");
    assert!(out.contains("after"), "{out}");
}

#[test]
fn must_fail_public_tail_masks_unterminated_key_in_artifact_tail() {
    let temp = tempfile::TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let path = temp.path().join("stdout.log");
    std::fs::write(&path, format!("ok\n-----BEGIN PRIVATE KEY-----\n{BODY}\n"))
        .unwrap_or_else(|e| panic!("write: {e}"));
    let tail = super::public_shell_output_tail(&path, 4096).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(!tail.contains(BODY), "{tail}");
    assert!(tail.contains("ok"), "{tail}");
    assert!(tail.contains("-----BEGIN PRIVATE KEY-----"), "{tail}");
    assert!(tail.contains("[redacted-shell-output]"), "{tail}");
    assert_eq!(
        tail, "ok\n-----BEGIN PRIVATE KEY-----\n[redacted-shell-output]\n",
        "tail must keep the prefix and armor and mask the unterminated body"
    );
}

// Windowed tails: the BEGIN header can fall before the window start.
use super::redact_log_window as window;

fn letters_line(seed: usize) -> String {
    // 64 base64-alphabet chars, no digits: the opaque-token rule cannot match.
    (0..64)
        .map(|i| char::from(b'A' + u8::try_from((seed + i) % 26).unwrap_or(0)))
        .collect()
}

fn body_lines(count: usize) -> String {
    (0..count).map(|n| letters_line(n) + "\n").collect()
}

#[test]
fn must_fail_window_with_end_but_no_begin_masks_from_start_through_end() {
    let text = format!("{}-----END PRIVATE KEY-----\nafter-ok\n", body_lines(5));
    let out = window(&text, true);
    for n in 0..5 {
        assert!(!out.contains(&letters_line(n)), "leaked line {n}: {out}");
    }
    assert!(out.contains("[redacted-shell-output]"), "{out}");
    assert!(out.contains("-----END PRIVATE KEY-----"), "{out}");
    assert!(out.contains("after-ok"), "{out}");
    assert_eq!(
        out,
        "[redacted-shell-output]\n-----END PRIVATE KEY-----\nafter-ok\n"
    );
}

#[test]
fn must_fail_window_starting_mid_body_without_any_armor_masks_base64_run() {
    let cut = &letters_line(0)[17..];
    let text = format!(
        "{cut}\n{}SHORT+Tail/AwEAAQ==\nnormal log line\n",
        body_lines(5)
    );
    let out = window(&text, true);
    for n in 0..5 {
        assert!(!out.contains(&letters_line(n)), "leaked line {n}: {out}");
    }
    assert!(!out.contains("SHORT+Tail"), "{out}");
    assert!(out.contains("normal log line"), "{out}");
}

#[test]
fn must_pass_window_that_is_the_whole_file_or_plain_text_is_untouched() {
    let text = format!("{}done\n", body_lines(3));
    assert!(window(&text, false).contains(&letters_line(1)));
    let plain = "partial line of text\nsecond line\n";
    assert_eq!(window(plain, true), plain);
    let hex = format!("{}\nnext\n", "ab12".repeat(16));
    assert_eq!(window(&hex, true), hex);
}

#[test]
fn must_pass_window_with_begin_before_end_keeps_normal_block_handling() {
    let text = format!(
        "x\n-----BEGIN PRIVATE KEY-----\n{}-----END PRIVATE KEY-----\ny\n",
        body_lines(2)
    );
    let out = window(&text, true);
    for n in 0..2 {
        assert!(!out.contains(&letters_line(n)), "leaked line {n}: {out}");
    }
    assert!(out.starts_with("x\n"), "{out}");
    assert!(out.ends_with("-----END PRIVATE KEY-----\ny\n"), "{out}");
    assert_eq!(
        out,
        "x\n-----BEGIN PRIVATE KEY-----\n[redacted-shell-output]\n-----END PRIVATE KEY-----\ny\n"
    );
}
