//! Interpreter argument parsing distinguishes opaque payloads from parse failures.

use super::{Shell, lexer::Token, payload, raw_payload};

/// Info-only calls and explicit script-file/encoded-command modes return `Ok(None)`.
pub(super) fn shell_command_payload(
    shell: Shell,
    args: &[Token],
) -> Result<Option<String>, &'static str> {
    if info_only(shell, args) {
        return Ok(None);
    }
    if matches!(shell, Shell::Cmd) {
        return cmd_payload(args).map(Some);
    }
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        let flag = powershell_flag(arg);
        if !flag.starts_with('-') {
            return required_payload(&args[index..]).map(Some);
        }
        if flag == "-ec" || prefix(&flag, "-file") || prefix(&flag, "-encodedcommand") {
            required_payload(&args[index + 1..])?;
            return Ok(None);
        }
        if !flag.contains(':') && prefix(&flag, "-command") {
            return required_payload(&args[index + 1..]).map(Some);
        }
        let width = powershell_switch_width(&flag, shell);
        if width == 2 && args.get(index + 1).is_none() {
            return Err("missing PowerShell switch value");
        }
        index += width;
    }
    Err("missing PowerShell command payload")
}

fn info_only(shell: Shell, args: &[Token]) -> bool {
    let [arg] = args else {
        return false;
    };
    let flag = powershell_flag(arg);
    match shell {
        Shell::Cmd => arg.text.eq_ignore_ascii_case("/?"),
        Shell::PowerShellDesktop => matches!(flag.as_str(), "-?" | "-help"),
        Shell::Pwsh => matches!(
            flag.as_str(),
            "-v" | "-version" | "--version" | "-?" | "-help"
        ),
    }
}

fn required_payload(args: &[Token]) -> Result<String, &'static str> {
    let script = payload(args);
    if script.trim().is_empty() {
        Err("missing interpreter command payload")
    } else {
        Ok(script)
    }
}

fn cmd_payload(args: &[Token]) -> Result<String, &'static str> {
    for (index, arg) in args.iter().enumerate() {
        let name = arg.text.to_ascii_lowercase();
        if !["/c", "/k", "/r"]
            .iter()
            .any(|&flag| name.starts_with(flag))
        {
            continue;
        }
        if name.len() == 2 {
            return required_payload(&args[index + 1..]);
        }
        // Keep glued payload quoting so cmd, not the caller's lexer, decodes it.
        let source = if arg.raw.starts_with('/') {
            &arg.raw
        } else {
            &arg.text
        };
        let mut script = source[2..].to_owned();
        if index + 1 < args.len() {
            script.push(' ');
            script.push_str(&raw_payload(&args[index + 1..]));
        }
        return Ok(script);
    }
    Err("could not locate cmd /c, /k, or /r command payload")
}

fn powershell_flag(arg: &Token) -> String {
    let mut flag = arg.name();
    if let Some(ch @ ('/' | '\u{2013}' | '\u{2014}' | '\u{2015}')) = flag.chars().next() {
        flag.replace_range(..ch.len_utf8(), "-");
    }
    flag
}

pub(super) fn prefix(flag: &str, name: &str) -> bool {
    flag.len() > 1 && name.starts_with(flag)
}

fn powershell_switch_width(flag: &str, shell: Shell) -> usize {
    if flag.contains(':')
        || (matches!(shell, Shell::Pwsh)
            && (prefix(flag, "-interactive") || prefix(flag, "-version")))
    {
        return 1;
    }
    let takes_value = matches!(flag, "-ep" | "-if" | "-of" | "-ea")
        || [
            ("-executionpolicy", "-ex"),
            ("-windowstyle", "-w"),
            ("-inputformat", "-inp"),
            ("-outputformat", "-o"),
            ("-configurationname", "-config"),
            ("-encodedarguments", "-encodeda"),
        ]
        .iter()
        .any(|(name, minimum)| flag.len() >= minimum.len() && prefix(flag, name))
        || shell_specific_value_switch(flag, shell);
    // Unknown switches cannot discard the remaining command.
    if takes_value { 2 } else { 1 }
}

fn shell_specific_value_switch(flag: &str, shell: Shell) -> bool {
    if matches!(shell, Shell::PowerShellDesktop) {
        return flag == "-psconsolefile"
            || prefix(flag, "-version")
            || prefix(flag, "-inputformat");
    }
    flag == "-wd"
        || [
            ("-configurationfile", "-configurationfile"),
            ("-workingdirectory", "-wo"),
            ("-settingsfile", "-settings"),
            ("-custompipename", "-cus"),
        ]
        .iter()
        .any(|(name, minimum)| flag.len() >= minimum.len() && prefix(flag, name))
}
