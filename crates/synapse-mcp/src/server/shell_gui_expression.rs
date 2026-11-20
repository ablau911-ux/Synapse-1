//! Bind only statically known Invoke-Expression string literals; refuse uncertainty.

use super::{Shell, lexer::Token, lexer::command_tokens, wrappers::prefix};

pub(super) fn expression_command(name: &str) -> bool {
    matches!(
        name,
        "iex" | "invoke-expression" | "microsoft.powershell.utility\\invoke-expression"
    )
}

pub(super) fn expression_payload(args: &[Token]) -> Result<String, &'static str> {
    let (mut index, mut script) = (0, None);
    while let Some(arg) = args.get(index) {
        let name = arg.name();
        let (flag, inline) = name
            .split_once(':')
            .map_or((name.as_str(), None), |(k, v)| (k, Some(v)));
        if !arg.literal && prefix(flag, "-command") {
            let (value, width) = command_value(args, index)?;
            set_payload(&mut script, value)?;
            index += width;
        } else if !arg.literal && flag.starts_with('-') {
            let width =
                common_parameter_width(flag).ok_or("unknown Invoke-Expression parameter")?;
            let width = if inline.is_some_and(|value| !value.is_empty()) {
                1
            } else {
                width
            };
            if width == 2 {
                let value = args
                    .get(index + 1)
                    .ok_or("missing evaluator parameter value")?;
                if value.boundary || (!value.literal && value.text.starts_with('-')) {
                    return Err("undeterminable evaluator parameter value");
                }
            }
            index += width;
        } else {
            set_payload(&mut script, literal_payload(&arg.raw)?)?;
            index += 1;
        }
    }
    script.ok_or("missing evaluator expression")
}

fn command_value(args: &[Token], index: usize) -> Result<(String, usize), &'static str> {
    let arg = args.get(index).ok_or("missing evaluator expression")?;
    if let Some((_, value)) = arg.raw.split_once(':')
        && !value.is_empty()
    {
        return literal_payload(value).map(|script| (script, 1));
    }
    let value = args.get(index + 1).ok_or("missing evaluator expression")?;
    literal_payload(&value.raw).map(|script| (script, 2))
}

fn set_payload(script: &mut Option<String>, value: String) -> Result<(), &'static str> {
    if script.is_some() {
        return Err("evaluator payload must be a single quoted string literal");
    }
    *script = Some(value);
    Ok(())
}

fn literal_payload(raw: &str) -> Result<String, &'static str> {
    if !single_literal(raw) {
        return Err(
            "evaluator payload must be a single quoted string literal without interpolation",
        );
    }
    let tokens = command_tokens(raw, Shell::PowerShellDesktop)?;
    match tokens.as_slice() {
        [token] if token.literal => Ok(token.text.clone()),
        _ => Err("undeterminable evaluator string literal"),
    }
}

/// Preserve doubled quotes and escaped dollars, but reject interpolation and joins.
fn single_literal(raw: &str) -> bool {
    let mut chars = raw.chars().peekable();
    let Some(quote @ ('\'' | '"')) = chars.next() else {
        return false;
    };
    while let Some(ch) = chars.next() {
        if quote == '"' && ch == '`' {
            if chars.next().is_none() {
                return false;
            }
        } else if quote == '"' && ch == '$' {
            return false;
        } else if ch == quote {
            if chars.peek() == Some(&quote) {
                chars.next();
            } else {
                return chars.next().is_none();
            }
        }
    }
    false
}

fn common_parameter_width(flag: &str) -> Option<usize> {
    match flag {
        "-verbose" | "-debug" => Some(1),
        "-erroraction"
        | "-ea"
        | "-warningaction"
        | "-wa"
        | "-informationaction"
        | "-ia"
        | "-errorvariable"
        | "-ev"
        | "-warningvariable"
        | "-wv"
        | "-informationvariable"
        | "-iv"
        | "-outvariable"
        | "-ov"
        | "-outbuffer"
        | "-ob"
        | "-pipelinevariable"
        | "-pv" => Some(2),
        _ => None,
    }
}
