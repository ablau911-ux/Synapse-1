//! Shell rules of the tool profiles, and the read-only commands a normal
//! agent may run (#2360, roadmap item #633).

use std::path::{Path, PathBuf};

/// The list when `readonly-shell-commands.json` does not exist: the two
/// commands a normal agent could run before #2360, each without arguments.
const DEFAULT_READ_ONLY_COMMANDS: &[&str] = &["hostname", "whoami"];

/// Interpreters and script hosts whose launch is equivalent to running
/// arbitrary shell commands. `process launch` of one of these is governed by
/// the same profile policy as the `shell` tool (#21).
const SHELL_INTERPRETERS: &[&str] = &[
    "cmd",
    "powershell",
    "pwsh",
    "bash",
    "sh",
    "wsl",
    "wscript",
    "cscript",
    "mshta",
];

/// Script file extensions that Windows executes through a shell or script
/// host, so launching the file is shell execution too.
const SHELL_SCRIPT_EXTENSIONS: &[&str] = &[
    "bat", "cmd", "ps1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "hta",
];

/// True when `target` names a shell interpreter or a shell script, by file
/// name (case-insensitive, any directory, optional quotes).
pub fn is_shell_interpreter(target: &str) -> bool {
    let trimmed = target.trim().trim_matches('"').trim();
    let file_name = trimmed.rsplit(['\\', '/']).next().unwrap_or(trimmed);
    let lowered = file_name.to_ascii_lowercase();
    let (stem, extension) = match lowered.rsplit_once('.') {
        Some((stem, extension)) => (stem, Some(extension)),
        None => (lowered.as_str(), None),
    };
    if extension.is_some_and(|extension| SHELL_SCRIPT_EXTENSIONS.contains(&extension)) {
        return true;
    }
    let interpreter = if extension == Some("exe") {
        stem
    } else {
        lowered.as_str()
    };
    SHELL_INTERPRETERS.contains(&interpreter)
}

/// #2360 (roadmap item #633): the read-only commands a normal agent may run
/// through `shell`, and the shell rule of every tool profile. Built once at
/// daemon start from the outcome of reading `readonly-shell-commands.json`
/// (one exact command line per entry, as a list of words:
/// `[["hostname"], ["whoami", "/all"]]`) and the Windows system folder. A
/// listed program always runs from that folder, never from the working
/// folder or the PATH. `Default` allows nothing.
#[derive(Clone, Debug, Default)]
pub struct ReadOnlyCommands {
    /// The Windows system folder; every listed program is started from it.
    system_folder: PathBuf,
    /// Accepted entries: lower case, the program without `.exe`.
    entries: Vec<Vec<String>>,
    /// Why the file or an entry was not accepted, for the daemon log.
    problems: Vec<String>,
}

impl ReadOnlyCommands {
    /// `read` is the outcome of reading the list file. No file: the default
    /// list (`hostname`, `whoami`). A file that cannot be read or is not a
    /// list of word lists: nothing is allowed. An entry that is empty, names a
    /// path, or names an interpreter or script host is left out. Every
    /// refusal is kept in `problems`. A system folder that is not an absolute
    /// path allows nothing (a missing `SystemRoot` must never turn a plain
    /// name into a relative path).
    #[must_use]
    pub fn from_file(read: std::io::Result<String>, system_folder: &Path) -> Self {
        let mut commands = Self {
            system_folder: system_folder.to_path_buf(),
            entries: Vec::new(),
            problems: Vec::new(),
        };
        if !system_folder.is_absolute() {
            commands.problems.push(format!(
                "the Windows system folder {} is not an absolute path; nothing is allowed",
                system_folder.display()
            ));
            return commands;
        }
        let listed: Vec<Vec<String>> = match read {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(listed) => listed,
                Err(error) => {
                    commands.problems.push(format!(
                        "the file is not a list of word lists ({error}); nothing is allowed"
                    ));
                    return commands;
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                DEFAULT_READ_ONLY_COMMANDS
                    .iter()
                    .map(|program| vec![(*program).to_owned()])
                    .collect()
            }
            Err(error) => {
                commands.problems.push(format!(
                    "the file cannot be read ({error}); nothing is allowed"
                ));
                return commands;
            }
        };
        for entry in listed {
            let Some((program, args)) = entry.split_first() else {
                commands
                    .problems
                    .push("entry [] is empty; it is left out".to_owned());
                continue;
            };
            if let Some(problem) = Self::program_problem(program) {
                commands
                    .problems
                    .push(format!("entry {entry:?} {problem}; it is left out"));
                continue;
            }
            commands.entries.push(Self::words(program, args));
        }
        commands
    }

    /// Why the file or an entry was not accepted (empty when all is well).
    #[must_use]
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// The system-folder program to start when `command` with `args` is one
    /// of the read-only commands (no environment, no working folder); `None`
    /// when it is not. `command` is a plain name (with or without `.exe`,
    /// any letter case) or a file directly in the system folder.
    #[must_use]
    pub fn program(
        &self,
        command: &str,
        args: &[String],
        has_env_overrides: bool,
        has_working_dir_override: bool,
    ) -> Option<PathBuf> {
        if has_env_overrides || has_working_dir_override {
            return None;
        }
        let words = Self::words(self.program_file_name(command)?, args);
        let program = words.first()?;
        self.entries
            .contains(&words)
            .then(|| self.system_folder.join(format!("{program}.exe")))
    }

    /// The `shell` rule of a tool profile.
    #[must_use]
    pub fn allows_shell_command(
        &self,
        profile: &str,
        command: &str,
        args: &[String],
        has_env_overrides: bool,
        has_working_dir_override: bool,
    ) -> bool {
        match profile {
            "break_glass" | "full_capability" => true,
            "normal_agent" => self
                .program(command, args, has_env_overrides, has_working_dir_override)
                .is_some(),
            _ => false,
        }
    }

    /// Profile gate for `process launch`: non-shell launches are unrestricted;
    /// shell-interpreter launches need the same permission as the `shell` tool.
    #[must_use]
    pub fn allows_process_launch(
        &self,
        profile: &str,
        target: &str,
        args: &[String],
        has_env_overrides: bool,
        has_working_dir_override: bool,
    ) -> bool {
        !is_shell_interpreter(target)
            || self.allows_shell_command(
                profile,
                target,
                args,
                has_env_overrides,
                has_working_dir_override,
            )
    }

    /// Why a list entry's program cannot be a read-only command.
    fn program_problem(program: &str) -> Option<&'static str> {
        if program.trim().is_empty() {
            Some("names no program")
        } else if program.contains(['\\', '/', ':']) {
            Some("names a path, not a plain program name")
        } else if is_shell_interpreter(program) {
            Some("names an interpreter or script host")
        } else {
            None
        }
    }

    /// `command` itself when it is a plain name; its file name when it is a
    /// file directly in the system folder; `None` for any other path.
    fn program_file_name<'a>(&self, command: &'a str) -> Option<&'a str> {
        let Some((folder, file_name)) = command.rsplit_once(['\\', '/']) else {
            return Some(command);
        };
        let system_folder = self.system_folder.to_string_lossy();
        folder
            .eq_ignore_ascii_case(system_folder.trim_end_matches(['\\', '/']))
            .then_some(file_name)
    }

    /// A command line as the list keeps it: lower case, the program without
    /// `.exe`.
    fn words(program: &str, args: &[String]) -> Vec<String> {
        let program = program.to_ascii_lowercase();
        let program = program.strip_suffix(".exe").unwrap_or(&program).to_owned();
        std::iter::once(program)
            .chain(args.iter().map(|arg| arg.to_ascii_lowercase()))
            .collect()
    }
}

#[cfg(all(test, windows))]
#[path = "readonly_commands_table_2360.rs"]
mod readonly_commands_table_2360;
