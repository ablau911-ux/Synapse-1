//! #268 command grammar. Inspect evaluator payloads, never ordinary arguments.

#[path = "shell_gui_tokens.rs"]
mod lexer;
use lexer::{Token, command_tokens};
#[path = "shell_gui_wrappers.rs"]
mod wrappers;
use wrappers::shell_command_payload;
#[path = "shell_gui_expression.rs"]
mod expression;
use expression::{expression_command, expression_payload};

const MAX_EVALUATOR_DEPTH: usize = 4;

#[derive(Clone, Copy)]
enum Shell {
    Cmd,
    PowerShellDesktop,
    Pwsh,
}

impl Shell {
    const fn is_powershell(self) -> bool {
        !matches!(self, Self::Cmd)
    }

    const fn escape(self) -> char {
        if self.is_powershell() { '`' } else { '^' }
    }
}

pub(super) fn detected_launch(
    command: &str,
    args: &[String],
    mut gui_executable: impl FnMut(&str) -> bool,
) -> Option<String> {
    let tokens = std::iter::once(command)
        .chain(args.iter().map(String::as_str))
        .map(Token::argument)
        .collect::<Vec<_>>();
    invocation_launch(&tokens, None, 0, &mut gui_executable)
}

fn invocation_launch(
    tokens: &[Token],
    context: Option<Shell>,
    depth: usize,
    gui: &mut impl FnMut(&str) -> bool,
) -> Option<String> {
    let command = tokens.first()?;
    let builtin = context.is_some_and(Shell::is_powershell) && powershell_builtin(&command.name());
    if launch_verb(tokens) || (!builtin && gui(&command.text)) {
        return Some(command.text.clone());
    }
    let args = &tokens[1..];
    if let Some(shell) = shell_kind(&command.text) {
        return match shell_command_payload(shell, args) {
            Ok(Some(script)) => shell_payload_launch(shell, script, depth, gui),
            Ok(None) => None, // Info-only calls or explicit -File / -EncodedCommand modes.
            Err(reason) => Some(indeterminate(reason)),
        };
    }
    if expression_command(&command.name()) {
        if matches!(context, Some(Shell::Cmd)) {
            // IEX owns a PowerShell expression even inside conservative cmd inspection.
            let expression = raw_payload(args);
            return expression_payload_launch(&expression, depth, gui);
        }
        return expression_launch(args, depth, gui);
    }
    None
}

fn shell_payload_launch(
    shell: Shell,
    mut script: String,
    depth: usize,
    gui: &mut impl FnMut(&str) -> bool,
) -> Option<String> {
    if matches!(shell, Shell::Cmd) {
        let tokens = match command_tokens(&script, shell) {
            Ok(tokens) => tokens,
            Err(reason) => return Some(indeterminate(reason)),
        };
        if let [token] = tokens.as_slice()
            && token.literal
            && token.raw.starts_with('"')
        {
            // A quoted executable path remains one command, including spaces.
            if gui(&token.text) {
                return Some(token.text.clone());
            }
            script.clone_from(&token.text);
        }
    }
    evaluate(&script, shell, depth, gui)
}

fn indeterminate(reason: &str) -> String {
    format!("could not determine whether this starts a GUI: {reason}")
}

fn expression_payload_launch(
    expression: &str,
    depth: usize,
    gui: &mut impl FnMut(&str) -> bool,
) -> Option<String> {
    match command_tokens(expression, Shell::PowerShellDesktop) {
        Ok(tokens) => expression_launch(&tokens, depth, gui),
        Err(reason) => Some(indeterminate(reason)),
    }
}

fn expression_launch(
    args: &[Token],
    depth: usize,
    gui: &mut impl FnMut(&str) -> bool,
) -> Option<String> {
    match expression_payload(args) {
        Ok(script) => evaluate(&script, Shell::PowerShellDesktop, depth, gui),
        Err(reason) => Some(indeterminate(reason)),
    }
}

fn evaluate(
    script: &str,
    shell: Shell,
    depth: usize,
    gui: &mut impl FnMut(&str) -> bool,
) -> Option<String> {
    if depth >= MAX_EVALUATOR_DEPTH {
        return Some(indeterminate(&format!(
            "evaluator recursion depth limit ({MAX_EVALUATOR_DEPTH}) exceeded"
        )));
    }
    match command_tokens(script, shell) {
        Ok(tokens) => script_launch(&tokens, shell, depth + 1, gui),
        Err(reason) => Some(indeterminate(reason)),
    }
}

fn script_launch(
    tokens: &[Token],
    shell: Shell,
    depth: usize,
    gui: &mut impl FnMut(&str) -> bool,
) -> Option<String> {
    for script in tokens.iter().flat_map(|token| &token.subexpressions) {
        if let Some(launch) = evaluate(script, Shell::PowerShellDesktop, depth, gui) {
            return Some(launch);
        }
    }
    let (mut index, mut command, mut literal_call) = (0, true, false);
    while let Some(token) = tokens.get(index) {
        if token.boundary {
            literal_call = token.text == "&" || (literal_call && token.text == "(");
            command = true;
        } else if command {
            let end = command_end(tokens, index + 1);
            if !(shell.is_powershell() && token.literal && !literal_call)
                && let Some(launch) =
                    invocation_launch(&tokens[index..end], Some(shell), depth, gui)
            {
                return Some(launch);
            }
            if literal_call
                && let Some(launch) = literal_launch(&tokens[index..end], shell, depth, gui)
            {
                return Some(launch);
            }
            literal_call = token.text == ".";
            if let Some(offset) = grammar_offset(&token.name(), &tokens[index + 1..]) {
                index += offset + 1;
                continue;
            }
            command = false;
            if shell_kind(&token.text).is_some() {
                index = end;
                continue;
            }
        }
        index += 1;
    }
    None
}

fn literal_launch(
    tokens: &[Token],
    shell: Shell,
    depth: usize,
    gui: &mut impl FnMut(&str) -> bool,
) -> Option<String> {
    let first = tokens.first()?;
    if shell.is_powershell() {
        return first
            .literal
            .then(|| evaluate(&first.text, shell, depth, gui))
            .flatten();
    }
    // Preserve conservative & / dot-source inspection without changing cmd quoting.
    if first.raw.starts_with(['\'', '"']) {
        let expression = raw_payload(tokens);
        return expression_payload_launch(&expression, depth, gui);
    }
    None
}

/// A nested wrapper owns its balanced blocks, but not the caller's next command.
fn command_end(tokens: &[Token], start: usize) -> usize {
    let mut nesting = 0_usize;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        if !token.boundary {
            continue;
        }
        match token.text.as_str() {
            "(" | "{" => nesting += 1,
            ")" | "}" if nesting > 0 => nesting -= 1,
            _ if nesting == 0 => return index,
            _ => {}
        }
    }
    tokens.len()
}

/// Decode the script argument once; retain quoting in subsequent arguments.
fn payload(args: &[Token]) -> String {
    args.iter()
        .enumerate()
        .map(|(index, token)| {
            if index == 0 {
                token.text.as_str()
            } else {
                token.raw.as_str()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn raw_payload(args: &[Token]) -> String {
    args.iter()
        .map(|token| token.raw.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn grammar_offset(name: &str, args: &[Token]) -> Option<usize> {
    match name {
        "if" => Some(cmd_if_offset(args).unwrap_or(0)),
        "for" => Some(cmd_for_offset(args).unwrap_or(0)),
        "call" | "." | "elseif" | "else" | "foreach" | "while" | "do" | "until" | "switch"
        | "try" | "catch" | "finally" | "trap" | "return" => Some(0),
        _ => None,
    }
}

/// Consume cmd conditions as data; PowerShell conditions retain their boundaries.
fn cmd_if_offset(args: &[Token]) -> Option<usize> {
    let mut index = 0;
    while args
        .get(index)
        .is_some_and(|arg| matches!(arg.name().as_str(), "/i" | "not"))
    {
        index += 1;
    }
    let condition = args.get(index)?.raw.to_ascii_lowercase();
    if matches!(
        condition.as_str(),
        "exist" | "defined" | "errorlevel" | "cmdextversion"
    ) {
        return Some(index + 2);
    }
    if condition.contains("==") {
        return Some(index + 1 + usize::from(condition.ends_with("==")));
    }
    let comparison = args.get(index + 1)?.raw.to_ascii_lowercase();
    if comparison.starts_with("==") {
        return Some(index + 2 + usize::from(comparison == "=="));
    }
    matches!(
        comparison.as_str(),
        "equ" | "neq" | "lss" | "leq" | "gtr" | "geq"
    )
    .then_some(index + 3)
}

/// The cmd FOR set is data. Only the command after DO is a command position.
fn cmd_for_offset(args: &[Token]) -> Option<usize> {
    if args.first()?.text == "(" {
        return None;
    }
    let mut nesting = 0_usize;
    for (index, token) in args.iter().enumerate() {
        if token.boundary {
            match token.text.as_str() {
                "(" => nesting += 1,
                ")" if nesting > 0 => nesting -= 1,
                _ if nesting == 0 => return None,
                _ => {}
            }
        } else if nesting == 0 && !token.literal && token.text.eq_ignore_ascii_case("do") {
            return Some(index + 1);
        }
    }
    None
}

fn shell_kind(command: &str) -> Option<Shell> {
    // Basenames identify real interpreter executables, never launch verbs.
    let name = command.trim_start_matches('@').rsplit(['/', '\\']).next()?;
    match name.to_ascii_lowercase().as_str() {
        "cmd" | "cmd.exe" => Some(Shell::Cmd),
        "powershell" | "powershell.exe" => Some(Shell::PowerShellDesktop),
        "pwsh" | "pwsh.exe" => Some(Shell::Pwsh),
        _ => None,
    }
}

fn launch_verb(tokens: &[Token]) -> bool {
    let Some(command) = tokens.first() else {
        return false;
    };
    let name = command.name();
    if cmd_start(&name) {
        return true;
    }
    match name.as_str() {
        "microsoft.powershell.management\\start-process"
        | "microsoft.powershell.management\\invoke-item" => true,
        "start-process" | "saps" | "start" | "explorer" | "explorer.exe" | "invoke-item" | "ii" => {
            true
        }
        "rundll32" | "rundll32.exe" => tokens[1..].iter().any(|token| {
            let value = token.name();
            value.contains("url.dll")
                || value.contains("fileprotocolhandler")
                || value.contains("openurl")
                || value.contains("shellexec_rundll")
        }),
        _ => false,
    }
}

/// Standard console aliases/cmdlets take precedence over same-name PATH PEs.
/// Extensions and paths deliberately do not match these command names.
fn powershell_builtin(name: &str) -> bool {
    const COMMANDS: &str = concat!(
        "write echo sleep cat type % foreach ? where ",
        "write-output write-host start-sleep get-content foreach-object where-object ",
        "get-childitem gci ls dir get-item gi get-location gl pwd set-location sl cd chdir ",
        "push-location pushd pop-location popd get-process gps ps stop-process spps kill ",
        "get-service gsv get-command gcm get-help help get-alias gal set-alias sal new-alias nal ",
        "get-date get-random get-variable gv set-variable sv set clear-host cls clear ",
        "get-history history h ghy select-object select sort-object sort group-object group ",
        "measure-object measure compare-object compare diff tee-object tee ",
        "format-table ft format-list fl format-wide fw out-string out-file out-null out-host oh ",
        "set-content sc add-content ac clear-content clc new-item ni copy-item copy cp cpi ",
        "move-item move mv mi remove-item rm rmdir del erase ri rename-item ren rni ",
        "test-path resolve-path rvpa join-path split-path select-string sls get-member gm read-host ",
        "write-debug write-error write-information write-progress write-verbose write-warning ",
        "start-job get-job gjb wait-job wjb receive-job rcjb"
    );
    COMMANDS
        .split_ascii_whitespace()
        .any(|builtin| name == builtin)
}

fn cmd_start(name: &str) -> bool {
    let Some(switches) = name.strip_prefix("start/") else {
        return false;
    };
    switches.split('/').all(|flag| {
        matches!(
            flag,
            "b" | "i"
                | "min"
                | "max"
                | "wait"
                | "separate"
                | "shared"
                | "low"
                | "normal"
                | "high"
                | "realtime"
                | "abovenormal"
                | "belownormal"
                | "d"
                | "node"
                | "affinity"
                | "machine"
                | "?"
        )
    })
}
