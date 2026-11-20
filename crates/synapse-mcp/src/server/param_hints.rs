//! Parameter hints for refused facade calls (upstream ee274ba0, 3d7d8c34).
//! Facade params are `deny_unknown_fields`: a wrong field still fails the
//! call. These pure helpers only make the refusal say where the field belongs,
//! and fold an envelope-level target alias into the selected operation spec
//! (fail closed on conflict).
// handler.rs calls `param_hint_message`; `merge_target_alias` waits for the
// browser facade envelope aliases (not wired yet). Remove with that wiring.
#![allow(dead_code)]

/// rmcp 1.7 wraps every `Parameters<T>` deserialize failure with this prefix.
pub(crate) const DESERIALIZE_PREFIX: &str = "failed to deserialize parameters:";

/// Longest field name worth matching or echoing; longer names are hostile or
/// garbage and get a truncated echo and no suggestion.
const MAX_HINT_FIELD_CHARS: usize = 64;

/// The refusal text for an rmcp `unknown field` param error when a correction
/// is known; `None` keeps the raw message (other errors, no confident match).
/// The call stays refused either way: nothing is retried or corrected.
#[must_use]
pub(crate) fn param_hint_message(tool: &str, raw_message: &str) -> Option<String> {
    let inner = raw_message.strip_prefix(DESERIALIZE_PREFIX)?.trim_start();
    let (field, accepted) = parse_unknown_field(inner)?;
    let length = field.chars().count();
    if length > MAX_HINT_FIELD_CHARS {
        let shown: String = field.chars().take(MAX_HINT_FIELD_CHARS).collect();
        return Some(format!(
            "{tool}: unknown field `{shown}...` ({length} characters); the call was not run."
        ));
    }
    if field.is_empty() {
        return None;
    }
    let hint = registry_hint(tool, field)
        .or_else(|| near_miss(field, &accepted).map(|name| format!("did you mean `{name}`?")))?;
    Some(format!(
        "{tool}: unknown field `{field}`; {hint} Facade fields are snake_case and unknown fields are refused; the call was not run."
    ))
}

/// Folds an envelope-level target alias (`cdp_target_id`, `window_hwnd`) into
/// the selected operation spec. Equal values are one target; different values,
/// or an alias on an operation that does not act on a single target, are
/// refused without echoing either value.
pub(crate) fn merge_target_alias<T: PartialEq>(
    tool: &str,
    field: &str,
    envelope: Option<T>,
    spec: Option<T>,
    single_target_operation: bool,
) -> Result<Option<T>, String> {
    let Some(envelope) = envelope else {
        return Ok(spec);
    };
    if !single_target_operation {
        return Err(format!(
            "{tool}: top-level {field} is refused for this operation because it does not act on a single target; remove {field}. The call was not run."
        ));
    }
    match spec {
        None => Ok(Some(envelope)),
        Some(spec) if spec == envelope => Ok(Some(spec)),
        Some(_) => Err(format!(
            "{tool}: top-level {field} conflicts with the {field} inside the operation spec; pass it once. The call was not run."
        )),
    }
}

/// serde's `unknown field `X`, expected one of `a`, `b`` (every arity): the
/// first backticked token is the field, the rest are the accepted fields.
fn parse_unknown_field(message: &str) -> Option<(&str, Vec<&str>)> {
    let mut remaining = message.strip_prefix("unknown field ")?;
    let mut tokens = Vec::new();
    while let Some(open) = remaining.find('`') {
        let after = &remaining[open + 1..];
        let close = after.find('`')?;
        tokens.push(&after[..close]);
        remaining = &after[close + 1..];
    }
    let (field, accepted) = tokens.split_first()?;
    Some((field, accepted.to_vec()))
}

/// Mistakes a flat accepted-field list cannot express: a nested field hoisted
/// to the envelope, an operation value passed as a field, and `act`, which has
/// no per-call target.
fn registry_hint(tool: &str, field: &str) -> Option<String> {
    match (tool, field) {
        ("browser_wait", "timeout_ms" | "polling_interval_ms") => Some(format!(
            "it belongs on the nested condition spec: wait.<condition>.{field} (e.g. wait.text.{field})."
        )),
        ("profile", "status" | "set") => Some(format!(
            "`{field}` is a value of `operation`, not a field: pass operation={field}."
        )),
        ("act", "cdp_target_id" | "window_hwnd") => Some(
            "act has no per-call target field: bind the session target first with target operation=set, then call act."
                .to_owned(),
        ),
        _ => None,
    }
}

/// The accepted field `field` most likely meant: camelCase to snake_case, then
/// case/underscore folding, then one unique closest name within a small edit
/// distance. A tie is ambiguous and suggests nothing.
fn near_miss<'a>(field: &str, accepted: &[&'a str]) -> Option<&'a str> {
    let snake = to_snake_case(field);
    if let Some(hit) = accepted.iter().copied().find(|name| *name == snake) {
        return Some(hit);
    }
    let folded = fold_key(field);
    if let Some(hit) = accepted
        .iter()
        .copied()
        .find(|name| fold_key(name) == folded)
    {
        return Some(hit);
    }
    let lower = field.to_ascii_lowercase();
    let threshold = (field.chars().count() / 3).max(2);
    let mut best: Option<(usize, &'a str)> = None;
    let mut tied = false;
    for &name in accepted {
        let distance = levenshtein(&lower, &name.to_ascii_lowercase());
        match best {
            Some((best_distance, _)) if distance > best_distance => {}
            Some((best_distance, _)) if distance == best_distance => tied = true,
            _ => {
                best = Some((distance, name));
                tied = false;
            }
        }
    }
    let (distance, name) = best?;
    (distance <= threshold && !tied).then_some(name)
}

/// `awaitPromise`, `await_promise` and `AwaitPromise` fold equal.
fn fold_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| *character != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

fn to_snake_case(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 4);
    for (index, character) in value.char_indices() {
        if character.is_ascii_uppercase() {
            if index != 0 && !out.ends_with('_') {
                out.push('_');
            }
            out.push(character.to_ascii_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

/// Levenshtein edit distance over chars (two-row dynamic programming).
fn levenshtein(left: &str, right: &str) -> usize {
    let right_chars: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right_chars.len()).collect();
    let mut current = vec![0_usize; right_chars.len() + 1];
    for (row, left_char) in left.chars().enumerate() {
        current[0] = row + 1;
        for (column, right_char) in right_chars.iter().enumerate() {
            let cost = usize::from(left_char != *right_char);
            current[column + 1] = (previous[column + 1] + 1)
                .min(current[column] + 1)
                .min(previous[column] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right_chars.len()]
}

#[cfg(test)]
#[path = "param_hints_table_hints.rs"]
mod param_hints_table_hints;
