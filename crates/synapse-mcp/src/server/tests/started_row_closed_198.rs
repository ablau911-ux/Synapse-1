//! Regression test for #198: once a handler has written its "started" audit
//! row, no exit may leave the handler before the closing ok/error row is
//! written - that strands the row and an auditor cannot tell whether the
//! action ran. Failures after the started row go through `audit_try!` (which
//! writes the closing error row), become the handler's `result` (closed by
//! the closing call), or the fallible step runs before the started row.
//!
//! Handlers need a live rmcp `RequestContext`, which cannot be built in a unit
//! test, so this scans the handler sources. In every function that writes a
//! started row, ANY bare `?` or `return Err` between the started row and the
//! function's last closing-row call is an offender, except a `?` on an audit
//! row write itself and a `return Err` directly after a closing-row call.
//! Comments, string/char literals, closure bodies and `async` blocks are
//! ignored (their `?` does not leave the handler).

use std::path::{Path, PathBuf};

/// `src/server`, the root of every handler that can write a started row.
fn server_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("server")
}

/// Test-only sources (their markers are fixtures, not handlers).
fn is_test_source(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    name == "tests.rs"
        || name.ends_with("_tests.rs")
        || path.components().any(|part| part.as_os_str() == "tests")
}

fn collect_rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read_dir {}: {error}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("dir entry in {}: {error}", dir.display()))
            .path();
        if path.is_dir() {
            collect_rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Every non-test `server/**/*.rs` that contains a started-row marker, as
/// `(path relative to src/server, source)`. Discovered at run time so a new
/// handler file is scanned without editing this test (#198 scope miss: a
/// fixed list covered 4 of 18 handler files).
fn handler_sources() -> Vec<(String, String)> {
    let root = server_dir();
    let mut paths = Vec::new();
    collect_rust_sources(&root, &mut paths);
    paths.sort();
    paths
        .into_iter()
        .filter(|path| !is_test_source(path.strip_prefix(&root).unwrap_or(path)))
        .filter_map(|path| {
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            let scanned = sanitize(&source);
            STARTED_ROW_MARKERS
                .iter()
                .any(|marker| scanned.contains(marker))
                .then(|| {
                    let name = path
                        .strip_prefix(&root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    (name, source)
                })
        })
        .collect()
}

/// Calls that write an action's "started" row.
const STARTED_ROW_MARKERS: [&str; 4] = [
    "audit_action_started_for_request(",
    "audit_action_started_with_details_for_request(",
    "audit_action_started_with_details(",
    "audit_action_started_with_details_for_session(",
];

/// Name prefixes of the calls that write the closing ok/error row.
const CLOSING_ROW_PREFIXES: [&str; 5] = [
    "audit_action_result",
    "audit_action_redacted_result",
    "audit_action_ok",
    "audit_action_error",
    "audit_target_claim_denial",
];

const FN_QUALIFIERS: [&str; 6] = [
    "pub(crate) ",
    "pub(super) ",
    "pub ",
    "async ",
    "const ",
    "unsafe ",
];

/// Byte span `(start, end)` of the comment or literal starting at `index`.
fn literal_span(source: &str, index: usize) -> Option<(usize, usize)> {
    let bytes = source.as_bytes();
    let rest = &source[index..];
    if rest.starts_with("//") {
        return Some((
            index,
            rest.find('\n').map_or(source.len(), |end| index + end),
        ));
    }
    if rest.starts_with("/*") {
        return Some((index, block_comment_end(source, index)));
    }
    let after_ident = index > 0 && is_ident_byte(bytes[index - 1]);
    if rest.starts_with('r') && !after_ident {
        let hashes = rest[1..].bytes().take_while(|byte| *byte == b'#').count();
        if rest.as_bytes().get(1 + hashes) == Some(&b'"') {
            let closing = format!("\"{}", "#".repeat(hashes));
            let body = index + 2 + hashes;
            let end = source[body..]
                .find(&closing)
                .map_or(source.len(), |end| body + end + closing.len());
            return Some((index, end));
        }
    }
    match bytes[index] {
        b'"' => Some((index, string_end(bytes, index))),
        b'\'' => char_literal_end(source, index).map(|end| (index, end)),
        _ => None,
    }
}

fn block_comment_end(source: &str, index: usize) -> usize {
    let (mut depth, mut cursor) = (0_usize, index);
    while cursor < source.len() {
        if source[cursor..].starts_with("/*") {
            depth += 1;
            cursor += 2;
        } else if source[cursor..].starts_with("*/") {
            depth -= 1;
            cursor += 2;
            if depth == 0 {
                return cursor;
            }
        } else {
            cursor += 1;
        }
    }
    source.len()
}

fn string_end(bytes: &[u8], index: usize) -> usize {
    let mut cursor = index + 1;
    while cursor < bytes.len() && bytes[cursor] != b'"' {
        cursor += if bytes[cursor] == b'\\' { 2 } else { 1 };
    }
    (cursor + 1).min(bytes.len())
}

/// End of a char literal at `index`, or `None` for a lifetime.
fn char_literal_end(source: &str, index: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(index + 1) == Some(&b'\\') {
        return source[index + 3..]
            .find('\'')
            .map(|end| index + 3 + end + 1);
    }
    let width = source[index + 1..].chars().next()?.len_utf8();
    (bytes.get(index + 1 + width) == Some(&b'\'')).then_some(index + 2 + width)
}

const fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The source with comments and string/char literal contents blanked to
/// spaces (newlines kept), so byte offsets and line structure stay the same.
fn sanitize(source: &str) -> String {
    let mut out = source.as_bytes().to_vec();
    let mut index = 0;
    while index < source.len() {
        match literal_span(source, index) {
            Some((start, end)) => {
                for byte in &mut out[start..end] {
                    if *byte != b'\n' {
                        *byte = b' ';
                    }
                }
                index = end;
            }
            None => index += source[index..].chars().next().map_or(1, char::len_utf8),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_fn_start(line: &str) -> bool {
    let mut rest = line.trim_start();
    while let Some(stripped) = FN_QUALIFIERS
        .iter()
        .find_map(|qualifier| rest.strip_prefix(qualifier))
    {
        rest = stripped;
    }
    rest.starts_with("fn ")
}

/// The sanitized source split into one chunk per `fn` item.
fn function_chunks(source: &str) -> Vec<&str> {
    let mut starts = Vec::new();
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        if is_fn_start(line) {
            starts.push(offset);
        }
        offset += line.len();
    }
    starts.push(source.len());
    starts
        .windows(2)
        .map(|bounds| &source[bounds[0]..bounds[1]])
        .collect()
}

/// End of the brace block opening at `open`.
fn brace_end(text: &str, open: usize) -> usize {
    let mut depth = 0_usize;
    for (offset, byte) in text.bytes().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return offset + 1;
                }
            }
            _ => {}
        }
    }
    text.len()
}

/// End of an expression-bodied closure starting at `start`.
fn expression_end(text: &str, start: usize) -> usize {
    let mut depth = 0_usize;
    for (offset, byte) in text.bytes().enumerate().skip(start) {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth == 0 => return offset,
            b')' | b']' | b'}' => depth -= 1,
            b',' | b';' if depth == 0 => return offset,
            _ => {}
        }
    }
    text.len()
}

/// Byte ranges of `async { }` blocks in `chunk`.
fn async_block_bodies(chunk: &str) -> Vec<(usize, usize)> {
    chunk
        .match_indices("async ")
        .filter_map(|(at, _)| {
            let rest = chunk[at + 6..].trim_start();
            let rest = rest.strip_prefix("move ").map_or(rest, str::trim_start);
            rest.starts_with('{').then(|| {
                let open = chunk.len() - rest.len();
                (open, brace_end(chunk, open))
            })
        })
        .collect()
}

/// Byte ranges of closure bodies in `chunk` (a `|` that opens a closure
/// follows `(`, `,`, `=`, `{`, `;`, `move` or `return`; `a || b` does not).
fn closure_bodies(chunk: &str) -> Vec<(usize, usize)> {
    let mut bodies = Vec::new();
    for (at, _) in chunk.match_indices('|') {
        let before = chunk[..at].trim_end();
        let opens = before.ends_with(['(', ',', '=', '{', ';'])
            || before.ends_with("move")
            || before.ends_with("return");
        if !opens || chunk[..at].ends_with('|') {
            continue;
        }
        let Some(params_end) = chunk[at + 1..].find('|').map(|end| at + 2 + end) else {
            continue;
        };
        let body = chunk[params_end..].trim_start();
        let body_start = chunk.len() - body.len();
        if body.starts_with("->") || body.starts_with('{') {
            let open = body.find('{').map_or(chunk.len(), |open| body_start + open);
            bodies.push((open, brace_end(chunk, open)));
        } else {
            bodies.push((body_start, expression_end(chunk, body_start)));
        }
    }
    bodies
}

/// Start of the identifier (or macro name) ending at `end`.
fn ident_start(bytes: &[u8], mut end: usize) -> usize {
    while end > 0 && (is_ident_byte(bytes[end - 1]) || bytes[end - 1] == b'!') {
        end -= 1;
    }
    end
}

/// Index of the opener matching the closer at `close`.
fn matching_open(bytes: &[u8], close: usize, open_byte: u8, close_byte: u8) -> usize {
    let mut depth = 0_usize;
    let mut cursor = close + 1;
    while cursor > 0 {
        cursor -= 1;
        if bytes[cursor] == close_byte {
            depth += 1;
        } else if bytes[cursor] == open_byte {
            depth -= 1;
            if depth == 0 {
                return cursor;
            }
        }
    }
    0
}

/// Name of the call (or binding) whose value ends at `text[..end]`, e.g.
/// `command_audit_final` for `self.command_audit_final(..)`, seeing through
/// `.await` and turbofish.
fn callee_before(text: &str, end: usize) -> &str {
    let bytes = text.as_bytes();
    let mut end = text[..end].trim_end().len();
    loop {
        if end > 0 && bytes[end - 1] == b')' {
            let open = matching_open(bytes, end - 1, b'(', b')');
            let mut name_end = text[..open].trim_end().len();
            if name_end > 0 && bytes[name_end - 1] == b'>' {
                name_end = matching_open(bytes, name_end - 1, b'<', b'>').saturating_sub(2);
            }
            return &text[ident_start(bytes, name_end)..name_end];
        }
        let start = ident_start(bytes, end);
        if &text[start..end] == "await" && start > 0 && bytes[start - 1] == b'.' {
            end = text[..start - 1].trim_end().len();
            continue;
        }
        return &text[start..end];
    }
}

fn is_closing_call(callee: &str) -> bool {
    CLOSING_ROW_PREFIXES
        .iter()
        .any(|prefix| callee.starts_with(prefix))
}

fn is_audit_row_call(callee: &str) -> bool {
    is_closing_call(callee)
        || STARTED_ROW_MARKERS
            .iter()
            .any(|marker| marker.strip_suffix('(') == Some(callee))
}

/// `return Err` directly after a closing-row statement is a closed exit.
fn follows_closing_call(before: &str) -> bool {
    let before = before.trim_end();
    let Some(statement) = before.strip_suffix(';') else {
        return false;
    };
    let statement = statement.trim_end();
    let statement = statement.strip_suffix('?').unwrap_or(statement);
    is_closing_call(callee_before(statement, statement.len()))
}

/// Byte range scanned in `chunk`: first started row to last closing-row call.
fn scanned_region(chunk: &str) -> Option<(usize, usize)> {
    let start = STARTED_ROW_MARKERS
        .iter()
        .filter_map(|marker| chunk.find(marker))
        .min()?;
    let last_close = CLOSING_ROW_PREFIXES
        .iter()
        .filter_map(|prefix| chunk.rfind(prefix))
        .max()
        .filter(|close| *close > start)
        .unwrap_or(chunk.len());
    Some((start, last_close))
}

/// Exits in one function chunk that leave after the started row unclosed.
fn chunk_offenders(chunk: &str) -> Vec<String> {
    let Some((start, end)) = scanned_region(chunk) else {
        return Vec::new();
    };
    let name = chunk
        .split("fn ")
        .nth(1)
        .and_then(|rest| rest.split(['(', '<']).next())
        .unwrap_or_default()
        .trim();
    // The started-row writers themselves (`action_audit.rs`) are not handlers.
    if STARTED_ROW_MARKERS
        .iter()
        .any(|marker| marker.strip_suffix('(') == Some(name))
    {
        return Vec::new();
    }
    let nested: Vec<(usize, usize)> = async_block_bodies(chunk)
        .into_iter()
        .chain(closure_bodies(chunk))
        .collect();
    let in_scope = |at: usize| {
        (start..end).contains(&at) && !nested.iter().any(|(from, to)| (*from..*to).contains(&at))
    };
    let mut offenders = Vec::new();
    for (at, _) in chunk.match_indices('?').filter(|(at, _)| in_scope(*at)) {
        let callee = callee_before(chunk, at);
        if !is_audit_row_call(callee) {
            offenders.push(format!("{name}: `{callee}?`"));
        }
    }
    for (at, _) in chunk
        .match_indices("return Err")
        .filter(|(at, _)| in_scope(*at))
    {
        let bytes = chunk.as_bytes();
        let whole_word = (at == 0 || !is_ident_byte(bytes[at - 1]))
            && !bytes
                .get(at + "return Err".len())
                .copied()
                .is_some_and(is_ident_byte);
        if whole_word && !follows_closing_call(&chunk[..at]) {
            offenders.push(format!("{name}: `return Err`"));
        }
    }
    offenders
}

/// Every unclosed exit after a started row in `source`.
fn stranding_exits(source: &str) -> Vec<String> {
    let sanitized = sanitize(source);
    function_chunks(&sanitized)
        .into_iter()
        .flat_map(chunk_offenders)
        .collect()
}

#[test]
fn must_pass_the_scan_covers_every_started_row_handler_file() {
    let files: Vec<String> = handler_sources()
        .into_iter()
        .map(|(file, _)| file)
        .collect();
    for expected in [
        "m1_tools.rs",
        "m2_tools.rs",
        "m4_tools.rs",
        "notify_tools.rs",
        "background_router.rs",
        "everquest_tools.rs",
        "everquest_autocombat.rs",
        "browser_network/tools.rs",
    ] {
        assert!(
            files.iter().any(|file| file == expected),
            "{expected} writes started rows but is not scanned: {files:?}"
        );
    }
    assert!(
        !files.iter().any(|file| file.starts_with("tests/")),
        "test fixtures must not be scanned as handlers: {files:?}"
    );
}

/// EXPECT FAIL until the m1_tools.rs / background_router.rs (and any other
/// newly scanned) handlers close their started rows on every exit (#198).
#[test]
fn must_fail_started_handlers_do_not_leave_before_the_closing_row() {
    let offenders: Vec<String> = handler_sources()
        .into_iter()
        .flat_map(|(file, source)| {
            stranding_exits(&source)
                .into_iter()
                .map(move |offender| format!("{file}: {offender}"))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "a bare `?` / `return Err` after the started audit row strands it; use audit_try!, fold the failure into the result that the closing row records, or move the call above the started row: {offenders:?}"
    );
}

#[test]
fn must_fail_the_scan_flags_any_bare_question_mark_after_the_started_row() {
    let stranded = "\n    pub async fn act_x(&self) {\n        self.audit_action_started_for_request(\"act_x\", &request_context)?;\n        let runtime = self.reflex_runtime()?;\n        let ids = self.current_session_ids()?;\n        self.audit_action_result_for_request(\"act_x\", &result, &request_context)?;\n    }";
    assert_eq!(
        stranding_exits(stranded),
        vec![
            "act_x: `reflex_runtime?`".to_owned(),
            "act_x: `current_session_ids?`".to_owned()
        ]
    );
    let ordered = "\n    pub async fn act_x(&self) {\n        let runtime = self.reflex_runtime()?;\n        self.audit_action_started_for_request(\"act_x\", &request_context)?;\n        self.audit_action_result_for_request(\"act_x\", &result, &request_context)?;\n    }";
    assert!(stranding_exits(ordered).is_empty());
}

#[test]
fn must_fail_the_scan_flags_return_err_and_the_for_session_marker() {
    let stranded = "\n    pub(super) async fn act_launch_x(&self) -> R {\n        self.audit_action_started_with_details_for_session(\"act_launch\", &payload, id)?;\n        let result = match launch().await {\n            Ok(outcome) => {\n                if failed {\n                    return Err(lifecycle_error(\"spawned; cleanup\"));\n                }\n                Ok(outcome)\n            }\n            Err(error) => Err(error),\n        };\n        match &result {\n            Ok(response) => self.command_audit_final(input)?,\n            Err(error) => self.command_audit_final(input)?,\n        };\n        self.audit_action_result_for_session(\"act_launch\", &result, id)?;\n        result\n    }";
    assert_eq!(
        stranding_exits(stranded),
        vec![
            "act_launch_x: `command_audit_final?`".to_owned(),
            "act_launch_x: `command_audit_final?`".to_owned(),
            "act_launch_x: `return Err`".to_owned()
        ]
    );
}

#[test]
fn must_pass_closed_exits_closures_and_literals_are_not_flagged() {
    let closed = "\n    async fn act_y(&self) -> R {\n        // a comment? return Err(x)\n        self.audit_action_started_with_details(\"act_y\", &d)?;\n        let note = \"what? return Err\";\n        let c = '?';\n        let load = || -> Result<_, E> { let r = self.profile_runtime()?; Ok(r) };\n        let value = audit_try!(self, \"act_y\", &request_context, load());\n        let inner = async { self.execute_literal_command().await?; Ok(()) }.await;\n        if let Err(error) = gate {\n            self.audit_action_result::<T>(\"act_y\", &Err(error.clone()))?;\n            return Err(error);\n        }\n        let command_final = self.command_audit_final(input);\n        self.audit_action_result(\"act_y\", &result)?;\n        command_final?;\n        result\n    }";
    assert_eq!(stranding_exits(closed), Vec::<String>::new());
}

/// The macro needs a live `SynapseService` + rmcp `RequestContext`, so this is
/// a source pin: its `Err` arm must return through a closing-row writer.
#[test]
fn must_pass_audit_try_macro_is_in_use_and_returns_through_a_closing_row() {
    let m2_tools = include_str!("../m2_tools.rs");
    let start = m2_tools
        .find("macro_rules! audit_try")
        .expect("audit_try macro is defined in m2_tools.rs");
    let body = &m2_tools[start..brace_end(m2_tools, start)];
    let err_arm = body
        .split("Err(error) =>")
        .nth(1)
        .expect("audit_try has an Err arm");
    let returned = err_arm
        .split("return ")
        .nth(1)
        .and_then(|rest| rest.split('(').next())
        .expect("audit_try Err arm returns a call");
    assert!(
        is_closing_call(returned.trim()),
        "audit_try Err arm returns `{returned}`, not a closing-row writer"
    );
    assert!(m2_tools.matches("audit_try!(").count() >= 10);
}
