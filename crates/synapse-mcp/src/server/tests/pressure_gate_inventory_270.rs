//! #270 inventory: every storage-pressure gate in the daemon crate (found by a
//! source search for the pressure check, not by a hand-kept list) must refuse
//! with `STORAGE_WRITE_SHED`. This catches a gate nobody listed or a new one;
//! each known gate also has a behavioural test next to its module.
//!
//! SOURCE GUARD: the first `STORAGE_WRITE_*` code after a pressure check is the
//! code of its refusal.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

/// These two ingesters defer the cycle (`Ok`, cursor not advanced, counter and
/// warning) instead of returning an error, so they have no refusal code.
const DEFERRING_INGESTERS: [&str; 2] = ["agent_transcripts.rs", "ambient_agents.rs"];
/// Lower bound that proves the search matched the real gates, not nothing.
const MIN_GATE_SITES: usize = 14;
const REFUSAL_WINDOW: usize = 500;

/// Find a call's closing parenthesis without confusing nested calls or quoted
/// messages with the error mapper belonging to this storage operation.
fn call_end(source: &str, open: usize) -> usize {
    delimited_end(source, open, '(', ')')
}

fn delimited_end(source: &str, open: usize, opening: char, closing: char) -> usize {
    let mut depth = 0_usize;
    let mut quoted = false;
    let mut escaped = false;
    for (offset, ch) in source[open..].char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
            continue;
        }
        match ch {
            '"' => quoted = true,
            ch if ch == opening => depth += 1,
            ch if ch == closing => {
                depth -= 1;
                if depth == 0 {
                    return open + offset + 1;
                }
            }
            _ => {}
        }
    }
    panic!("unterminated source call at byte {open}");
}

/// Inline closures and plain function mappers must preserve the storage code.
/// Resolve the function in this file, irrespective of its declaration order.
fn mapper_body<'a>(source: &'a str, mapper: &'a str) -> Option<(&'a str, Option<&'a str>)> {
    let mapper = mapper.trim().trim_end_matches(',').trim_end();
    if mapper.starts_with('|') || mapper.starts_with("move ") {
        return Some((mapper, None));
    }
    if mapper.is_empty()
        || !mapper
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    let declaration = regex::Regex::new(&format!(r"\bfn\s+{}\s*\(", regex::escape(mapper)))
        .unwrap_or_else(|error| panic!("invalid mapper declaration pattern: {error}"));
    let function = declaration.find(source)?;
    let after_params = call_end(source, function.end() - 1);
    let params = &source[function.end()..after_params - 1];
    let parameter = params.split_once(':').map(|(name, _)| name.trim());
    let open = after_params + source[after_params..].find('{')?;
    Some((
        &source[open..delimited_end(source, open, '{', '}')],
        parameter.filter(|_| !params.contains(',')),
    ))
}

/// Accept only a single wrapping expression using the original parameter.
/// Merely mentioning the parameter (or wrapping a replacement) is insufficient.
fn wraps_original_error(body: &str, parameter: Option<&str>) -> bool {
    let Some(parameter) = parameter.filter(|name| {
        !name.is_empty()
            && name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    }) else {
        return false;
    };
    let expression = body
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}')
        .trim();
    let expression = expression
        .strip_prefix("return ")
        .map_or(expression, |value| value.trim_end_matches(';').trim());
    let parameter = regex::escape(parameter);
    let wrapper = regex::Regex::new(&format!(
        r"^(?:(?:Box\s*::\s*new|Error\s*::\s*from|Self\s*::\s*Storage)\s*\(\s*{parameter}\s*\)|{parameter}\s*\.\s*into\s*\(\s*\))$"
    ))
    .unwrap_or_else(|error| panic!("invalid pass-through mapper pattern: {error}"));
    wrapper.is_match(expression)
}

// Shared with synthetic mutation tests: exercise the inventory's actual scanner.
fn write_mapper_inventory(source: &str) -> (usize, Vec<usize>) {
    let writes = regex::Regex::new(r"\.(?:storage_put_[A-Za-z0-9_]+|put|put_batch)\s*\(")
        .expect("storage write call pattern");
    let preserves_code = regex::Regex::new(r"\.\s*code\s*\(\s*\)")
        .unwrap_or_else(|error| panic!("invalid storage code pattern: {error}"));
    let mut sites = 0;
    let mut wrong = Vec::new();
    for write in writes.find_iter(source) {
        // Explicit pressure-bypass writes cannot shed. Match the Db writer
        // scope here too: put_batch_pressure_bypass is already excluded above.
        if write.as_str().contains("_pressure_bypass") {
            continue;
        }
        let after_call = call_end(source, write.end() - 1);
        let Some(mapper) = source[after_call..].trim_start().strip_prefix(".map_err") else {
            continue;
        };
        let mapper = mapper.trim_start();
        if !mapper.starts_with('(') {
            continue;
        }
        sites += 1;
        let argument = &mapper[1..call_end(mapper, 0) - 1];
        if !mapper_body(source, argument).is_some_and(|(body, parameter)| {
            preserves_code.is_match(body) || wraps_original_error(body, parameter)
        }) {
            wrong.push(source[..write.start()].matches('\n').count() + 1);
        }
    }
    (sites, wrong)
}

#[test]
fn must_fail_storage_put_error_mappers_hardcoding_write_failed_270() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    production_sources(&root, &mut files);
    let mut sites = 0_usize;
    let mut wrong = Vec::new();
    for file in files {
        let source = std::fs::read_to_string(&file).expect("read source");
        let (matched, lines) = write_mapper_inventory(&source);
        sites += matched;
        for line in lines {
            wrong.push(format!("{}:{line}", file.display()));
        }
    }
    assert!(
        sites >= 17,
        "source search matched only {sites} write mappers"
    );
    assert!(
        wrong.is_empty(),
        "#270: storage write map_err must preserve StorageError::code() (WriteShed -> STORAGE_WRITE_SHED); hardcoded STORAGE_WRITE_FAILED at {} sites: {wrong:#?}",
        wrong.len()
    );
}

#[path = "named_mapper_inventory_274.rs"]
mod named_mapper_inventory_274;

fn production_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "tests") {
                continue;
            }
            production_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if stem != "tests" && !stem.ends_with("_tests") {
                out.push(path);
            }
        }
    }
}

#[test]
fn must_fail_any_storage_pressure_gate_refusing_with_a_code_other_than_shed_270() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    production_sources(&root, &mut files);
    let mut sites = 0_usize;
    let mut wrong = Vec::new();
    for file in files {
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if DEFERRING_INGESTERS.contains(&name) {
            continue;
        }
        let source = std::fs::read_to_string(&file)
            .expect("read source")
            .replace("\r\n", "\n");
        for (at, _) in source.match_indices("pressure_permits_write(") {
            let window = &source[at..source.len().min(at + REFUSAL_WINDOW)];
            let Some(code_at) = window.find("error_codes::STORAGE_WRITE_") else {
                continue;
            };
            sites += 1;
            if !window[code_at..].starts_with("error_codes::STORAGE_WRITE_SHED") {
                let line = source[..at].matches('\n').count() + 1;
                wrong.push(format!("{}:{line}", file.display()));
            }
        }
    }
    assert!(
        sites >= MIN_GATE_SITES,
        "the source search matched only {sites} pressure gates"
    );
    assert!(
        wrong.is_empty(),
        "#270: these pressure gates refuse with a code other than STORAGE_WRITE_SHED: {wrong:#?}"
    );
}
