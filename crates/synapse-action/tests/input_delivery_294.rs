//! #294 public error contract and raw OS input call-site inventory.
use std::{fs, path::Path};

use synapse_action::ActionError;
use synapse_core::error_codes;

#[test]
fn must_pass_input_target_required_code_literal_exists() {
    assert_eq!(
        error_codes::ACTION_INPUT_TARGET_REQUIRED,
        "ACTION_INPUT_TARGET_REQUIRED"
    );
}

#[test]
fn must_pass_input_target_required_maps_to_public_code() {
    let error = ActionError::InputTargetRequired {
        detail: "no explicit or session target".into(),
    };
    assert_eq!(error.code(), error_codes::ACTION_INPUT_TARGET_REQUIRED);
}

fn strip_non_code(source: &str) -> String {
    let mut code: Vec<char> = source.chars().collect();
    let mut index = 0;
    while index < code.len() {
        let start = index;
        if code[index..].starts_with(&['/', '/']) {
            index += 2;
            while index < code.len() && code[index] != '\n' {
                index += 1;
            }
        } else if code[index..].starts_with(&['/', '*']) {
            index += 2;
            let mut depth = 1;
            while index < code.len() && depth > 0 {
                if code[index..].starts_with(&['/', '*']) {
                    depth += 1;
                    index += 2;
                } else if code[index..].starts_with(&['*', '/']) {
                    depth -= 1;
                    index += 2;
                } else {
                    index += 1;
                }
            }
        } else {
            // Raw strings may contain quotes, escapes, and comment delimiters.
            let mut quote = index;
            if code[index] == 'r' {
                quote += 1;
                while code.get(quote) == Some(&'#') {
                    quote += 1;
                }
            }
            if code[index] == 'r' && code.get(quote) == Some(&'"') {
                let hashes = quote - index - 1;
                index = quote + 1;
                while index < code.len() {
                    let end = index + 1 + hashes;
                    if code[index] == '"'
                        && end <= code.len()
                        && code[index + 1..end].iter().all(|ch| *ch == '#')
                    {
                        index = end;
                        break;
                    }
                    index += 1;
                }
            } else if code[index] == '"'
                || (code[index] == '\''
                    && (code.get(index + 1) == Some(&'\\') || code.get(index + 2) == Some(&'\'')))
            {
                // Recognize character literals too, without consuming lifetimes.
                let delimiter = code[index];
                index += 1;
                while index < code.len() {
                    if code[index] == '\\' {
                        index = (index + 2).min(code.len());
                    } else if code[index] == delimiter {
                        index += 1;
                        break;
                    } else {
                        index += 1;
                    }
                }
            } else {
                index += 1;
                continue;
            }
        }
        // Leave a separator so removing text cannot manufacture a call.
        code[start..index].fill(' ');
    }
    code.into_iter().collect()
}

fn raw_call_offsets<'a>(code: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    code.match_indices(needle).filter_map(move |(offset, _)| {
        let inside_identifier = code[..offset]
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
        (!inside_identifier).then_some(offset)
    })
}

#[test]
fn must_pass_raw_call_scan_ignores_non_code() {
    let mentions = r####"
        // SendInput (#50), SendInput(
        /// SetCursorPos(
        //! SendInput(
        /* SendInput( /* nested */ SetCursorPos( */
        let text = "SendInput(\"SetCursorPos(";
        let raw = r##"embedded " SendInput( /* SetCursorPos( */"##;
        let quote = '"';
        let byte_quote = b'"';
        mySendInput(); _SetCursorPos(); SendInput (); SetCursorPos ();
    "####;
    let code = strip_non_code(mentions);
    for needle in ["SendInput(", "SetCursorPos("] {
        assert_eq!(raw_call_offsets(&code, needle).count(), 0);
        let code = strip_non_code(&format!("{mentions}\nunsafe {{ {needle}0); }}"));
        assert_eq!(raw_call_offsets(&code, needle).count(), 1);
    }
}

fn scan_sources(
    directory: &Path,
    root: &Path,
    violations: &mut Vec<String>,
    calls: &mut [usize; 2],
) -> std::io::Result<()> {
    let needles = ["SendInput(", "SetCursorPos("];
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            scan_sources(&path, root, violations, calls)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = fs::read_to_string(&path)?;
            let relative = path.strip_prefix(root).map_err(std::io::Error::other)?;
            let relative = relative.to_string_lossy().replace('\\', "/");
            let code = strip_non_code(&source);
            for (index, needle) in needles.iter().enumerate() {
                for offset in raw_call_offsets(&code, needle) {
                    calls[index] += 1;
                    let before_call: String = code[..offset]
                        .chars()
                        .filter(|ch| !ch.is_whitespace())
                        .collect();
                    let function = if index == 0 {
                        "send_input_batch("
                    } else {
                        "set_cursor_pos("
                    };
                    let allowed_path = if index == 0 {
                        "synapse-action/src/backend/software/input.rs"
                    } else {
                        "synapse-action/src/backend/software/mouse.rs"
                    };
                    let a11y_exception = relative == "synapse-a11y/src/platform/windows/window.rs";
                    // Require the exact owner function, not merely its allowed source file.
                    let correct_owner = before_call
                        .rsplit("fn")
                        .next()
                        .is_some_and(|owner| owner.starts_with(function));
                    let is_approved_owner = relative == allowed_path && correct_owner;
                    if !(a11y_exception || is_approved_owner) {
                        violations.push(format!(
                            "{relative}: raw call {needle} outside approved owner"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

#[test]
fn must_fail_raw_input_calls_outside_guarded_owners() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates = manifest.parent().ok_or("manifest has no crates parent")?;
    let mut violations = Vec::new();
    let mut calls = [0, 0];
    for entry in fs::read_dir(crates)? {
        let source = entry?.path().join("src");
        if source.is_dir() {
            scan_sources(&source, crates, &mut violations, &mut calls)?;
        }
    }
    assert!(calls[0] > 0, "scan must find the existing batch sender");
    assert!(calls[1] > 0, "scan must find the existing cursor helper");
    assert!(violations.is_empty(), "raw input bypasses: {violations:#?}");
    Ok(())
}

/// Collects `(relative path, owner fn)` for every raw call of `needle`.
fn raw_call_owners(
    directory: &Path,
    root: &Path,
    needle: &str,
    owners: &mut Vec<(String, String)>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            raw_call_owners(&path, root, needle, owners)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let code = strip_non_code(&fs::read_to_string(&path)?);
            let relative = path.strip_prefix(root).map_err(std::io::Error::other)?;
            let relative = relative.to_string_lossy().replace('\\', "/");
            for offset in raw_call_offsets(&code, needle) {
                let before_call: String = code[..offset]
                    .chars()
                    .filter(|ch| !ch.is_whitespace())
                    .collect();
                let owner = before_call
                    .rsplit("fn")
                    .next()
                    .and_then(|owner| owner.split('(').next())
                    .unwrap_or_default()
                    .to_owned();
                owners.push((relative.clone(), owner));
            }
        }
    }
    Ok(())
}

/// #294: `SetPhysicalCursorPos` moves the real cursor like `SetCursorPos`; it
/// may only run inside the guarded helper that checks the delivery target.
#[test]
fn must_fail_physical_cursor_moves_outside_guarded_helper() -> Result<(), Box<dyn std::error::Error>>
{
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let crates = manifest.parent().ok_or("manifest has no crates parent")?;
    let mut owners = Vec::new();
    for entry in fs::read_dir(crates)? {
        let source = entry?.path().join("src");
        if source.is_dir() {
            raw_call_owners(&source, crates, "SetPhysicalCursorPos(", &mut owners)?;
        }
    }
    assert_eq!(
        owners,
        [(
            "synapse-action/src/backend/software/mouse.rs".to_owned(),
            "set_physical_cursor_pos".to_owned()
        )],
        "SetPhysicalCursorPos must have exactly one guarded owner"
    );
    Ok(())
}
