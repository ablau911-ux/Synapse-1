//! #268: classify visible GUI launches before shell authorization or spawn.
//! This is command-line inspection, not an interpreter or a script-file scanner.

use std::collections::BTreeMap;

use super::{ErrorData, SynapseService};

#[cfg(windows)]
#[path = "shell_gui_syntax.rs"]
mod syntax;

const DETECTION_LIMITS: &str = "Detection does not inspect script files, EncodedCommand, string concatenation or variable-based invocation outside recognized Invoke-Expression payloads, COM ShellExecute, bash/wsl/python/node wrappers, .cmd wrappers such as code, document associations (.lnk/.url/.html), Out-GridView, or commands inside cmd for /f sets. Runtime aliases/functions and PowerShell here-strings are not fully modeled; builtin precedence uses known console names. Missing or unparseable interpreter payloads, nonliteral Invoke-Expression payloads, and evaluator recursion beyond the depth limit are refused. UNC executables are unknown and are not opened for PE inspection";

impl SynapseService {
    pub(crate) fn guard_shell_gui_launch(
        &self,
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
        env: &BTreeMap<String, String>,
        session_id: &str,
    ) -> Result<(), ErrorData> {
        // Most calls need no filesystem/registry classification at all.
        let Err(mut refusal) = self.ensure_user_idle_for_shell_launch(session_id) else {
            return Ok(());
        };
        let Some(launch) = detected_launch(command, args, working_dir, env) else {
            return Ok(());
        };
        refusal.message = format!(
            "{}; shell GUI launch check: {launch}. {DETECTION_LIMITS}",
            refusal.message
        )
        .into();
        if let Some(data) = refusal
            .data
            .as_mut()
            .and_then(serde_json::Value::as_object_mut)
        {
            data.insert("detected_launch".to_owned(), serde_json::json!(launch));
            data.insert(
                "detection_limits".to_owned(),
                serde_json::json!(DETECTION_LIMITS),
            );
        }
        Err(refusal)
    }
}

#[cfg(windows)]
fn detected_launch(
    command: &str,
    args: &[String],
    working_dir: Option<&str>,
    env: &BTreeMap<String, String>,
) -> Option<String> {
    syntax::detected_launch(command, args, |token| {
        gui_executable(token, working_dir, env)
    })
}

#[cfg(not(windows))]
fn detected_launch(
    _command: &str,
    _args: &[String],
    _working_dir: Option<&str>,
    _env: &BTreeMap<String, String>,
) -> Option<String> {
    None
}

#[cfg(windows)]
fn gui_executable(token: &str, working_dir: Option<&str>, env: &BTreeMap<String, String>) -> bool {
    let expanded = synapse_browsers::expand_env_vars(token.trim_start_matches('@'), &|name| {
        env.iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var(name).ok())
    });
    let Some(program) = expanded else {
        return false;
    };
    if network_or_device_path(std::path::Path::new(&program)) {
        return false;
    }
    crate::m4::resolve_shell_program(&program, working_dir, env)
        .or_else(|| app_path(&program))
        .is_some_and(|path| is_gui_pe(&path))
}

#[cfg(windows)]
fn app_path(program: &str) -> Option<std::path::PathBuf> {
    use synapse_browsers::RegistryHive::{CurrentUser, LocalMachine};

    if program.is_empty() || program.contains(['/', '\\', ':']) {
        return None;
    }
    let name = if std::path::Path::new(program).extension().is_some() {
        program.to_owned()
    } else {
        format!("{program}.exe")
    };
    let key = format!(r"Software\Microsoft\Windows\CurrentVersion\App Paths\{name}");
    [CurrentUser, LocalMachine].into_iter().find_map(|hive| {
        let value =
            crate::chrome_debugger_bridge::registry_reader::read_registry_string(hive, &key, "")?;
        let expanded = synapse_browsers::expand_process_env_vars(value.trim_matches('"'))?;
        let path = std::path::PathBuf::from(expanded);
        (!network_or_device_path(&path) && path.is_file()).then_some(path)
    })
}

/// Reject UNC (including verbatim UNC) and device namespaces before any PE open.
#[cfg(windows)]
fn network_or_device_path(path: &std::path::Path) -> bool {
    use std::path::{Component, Prefix};

    if let Some(Component::Prefix(prefix)) = path.components().next()
        && matches!(prefix.kind(), Prefix::VerbatimDisk(_))
    {
        return false;
    }
    let text = path.to_string_lossy().replace('/', "\\");
    text.starts_with(r"\\") || text.starts_with(r"\??\")
}

/// Both PE32 and PE32+ store Subsystem 68 bytes into the optional header.
/// Fixed-size reads avoid loading executable bodies or trusting header offsets.
#[cfg(windows)]
fn is_gui_pe(path: &std::path::Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};

    if network_or_device_path(path) {
        return false;
    }
    let inspect = || -> std::io::Result<bool> {
        let mut file = std::fs::File::open(path)?;
        let mut dos = [0_u8; 64];
        file.read_exact(&mut dos)?;
        if &dos[..2] != b"MZ" {
            return Ok(false);
        }
        let offset = u32::from_le_bytes([dos[60], dos[61], dos[62], dos[63]]);
        if offset < 64 {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(u64::from(offset)))?;
        let mut pe = [0_u8; 24];
        file.read_exact(&mut pe)?;
        if &pe[..4] != b"PE\0\0" || u16::from_le_bytes([pe[20], pe[21]]) < 70 {
            return Ok(false);
        }
        let mut optional = [0_u8; 70];
        file.read_exact(&mut optional)?;
        Ok(matches!(
            u16::from_le_bytes([optional[0], optional[1]]),
            0x10b | 0x20b
        ) && u16::from_le_bytes([optional[68], optional[69]]) == 2)
    };
    inspect().unwrap_or(false)
}
