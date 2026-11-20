//! Shared child-process environment policy (#22). Every process Synapse
//! spawns goes through exactly one of these entry points:
//!
//! - user-directed commands (`act_launch`, `act_run_shell`, durable shell jobs):
//!   [`untrusted_environment`] / [`child_base_environment`] — a reconstructed
//!   base environment with no Synapse credentials and no profile-path hints;
//! - Synapse-owned helpers that inherit the daemon environment (ssh tail,
//!   video decoders, context-graph server, Codex control helpers, desktop
//!   worker, kill/taskkill, shell cleanup): [`scrub_inherited_credentials`] /
//!   [`inherited_environment_block`] — Synapse credentials removed;
//! - the explicit [`TrustedBootstrap`] exceptions listed below.
//!
//! This reduces credential exposure; it is not a filesystem or same-user
//! process isolation boundary.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

use rmcp::ErrorData;

#[cfg(windows)]
use super::validate_launch_environment_entry;
use super::{
    ActLaunchParams, add_windows_profile_environment, add_windows_registry_environment,
    add_windows_standard_environment, ensure_child_temp_environment,
    validate_child_base_environment,
};

pub(super) const PROCESS_BASE_ENV_KEYS: [&str; 20] = [
    "PATH",
    "PATHEXT",
    "COMSPEC",
    "SystemDrive",
    "SystemRoot",
    "WINDIR",
    "TEMP",
    "TMP",
    "USERDOMAIN",
    "USERNAME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "CommonProgramW6432",
];

/// Provider/gateway variable prefixes the daemon forwards to a spawned agent
/// CLI (its own model credentials, base URLs, config homes). Every other
/// daemon variable is dropped — the spawn starts from a cleared environment.
pub(super) const SPAWNED_AGENT_FORWARD_PREFIXES: [&str; 4] =
    ["ANTHROPIC_", "OPENAI_", "CODEX_", "CLAUDE_"];

/// Exact daemon variables forwarded to a spawned agent CLI (network egress and
/// TLS trust required to reach its provider, cloud-provider selection).
pub(super) const SPAWNED_AGENT_FORWARD_KEYS: [&str; 9] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
    "AWS_REGION",
    "AWS_PROFILE",
];

/// The only spawn sites allowed to pass Synapse credentials or profile paths
/// to a child. Adding a variant is a security decision; keep this list short.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrustedBootstrap {
    /// `connect` starting the shared daemon: the daemon itself must load the
    /// master token (env or token.txt) to serve operator clients.
    SharedDaemon,
    /// `setup` repair launcher: operator-run `synapse-setup.ps1` manages
    /// token.txt and operator client configuration.
    SetupRepair,
    /// `act_spawn_agent`: profile paths (CLI login/config discovery) plus the
    /// per-spawn scoped credential from `crate::spawn_credentials` — never the
    /// master token.
    SpawnedAgent,
}

impl TrustedBootstrap {
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::SharedDaemon => "shared daemon loads the master token",
            Self::SetupRepair => "operator setup manages token.txt",
            Self::SpawnedAgent => "spawned agent receives profile paths and a scoped credential",
        }
    }

    /// Marks a spawn site that deliberately keeps the inherited environment.
    pub(crate) fn keep_inherited_environment(self) {
        tracing::debug!(
            code = "CHILD_ENV_TRUSTED_BOOTSTRAP",
            site = ?self,
            reason = self.reason(),
            "child keeps the inherited environment"
        );
    }
}

/// Synapse credential variables (bearer tokens, token-file hints, scoped
/// credentials, capabilities). Windows environment names are case
/// insensitive, so matching is too.
pub(crate) fn is_credential_env_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    key.starts_with("SYNAPSE_")
        && ["TOKEN", "CREDENTIAL", "CAPABILITY"]
            .iter()
            .any(|marker| key.contains(marker))
}

fn is_profile_hint_key(key: &str) -> bool {
    matches!(
        key.to_ascii_uppercase().as_str(),
        "APPDATA" | "LOCALAPPDATA" | "USERPROFILE" | "HOME" | "HOMEDRIVE" | "HOMEPATH"
    )
}

/// Untrusted-command policy for a single variable.
pub(super) fn child_environment_entry_allowed(key: &str) -> bool {
    !(is_profile_hint_key(key) || is_credential_env_key(key))
}

fn reconstructed_base_environment() -> BTreeMap<String, (String, String)> {
    reconstructed_base_environment_with(std::env::var_os)
}

/// [`reconstructed_base_environment`] with the daemon-variable lookup for
/// `PROCESS_BASE_ENV_KEYS` injected (test seam). The registry, standard,
/// profile and TEMP/TMP additions still read the live host.
pub(crate) fn reconstructed_base_environment_with(
    var: impl Fn(&'static str) -> Option<OsString>,
) -> BTreeMap<String, (String, String)> {
    let mut env = BTreeMap::new();
    for key in PROCESS_BASE_ENV_KEYS {
        if let Some(value) = var(key) {
            env.insert(
                key.to_ascii_uppercase(),
                (key.to_owned(), value.to_string_lossy().into_owned()),
            );
        }
    }
    add_windows_registry_environment(&mut env);
    add_windows_standard_environment(&mut env);
    add_windows_profile_environment(&mut env);
    // Resolve TEMP/TMP before any profile paths used to derive them are dropped.
    ensure_child_temp_environment(&mut env);
    // The registry can hold a persisted user SYNAPSE_BEARER_TOKEN.
    env.retain(|_, (key, _)| !is_credential_env_key(key));
    env
}

/// Base environment for untrusted commands.
pub(super) fn child_base_environment() -> BTreeMap<String, (String, String)> {
    let mut env = reconstructed_base_environment();
    env.retain(|_, (key, _)| child_environment_entry_allowed(key));
    env
}

/// `act_launch` environment: untrusted base plus allowed caller overrides,
/// keyed by upper-case name (Windows block order).
pub(super) fn untrusted_environment(
    params: &ActLaunchParams,
    surface: &'static str,
) -> Result<BTreeMap<String, (String, String)>, ErrorData> {
    let mut env = child_base_environment();
    validate_child_base_environment(&env, surface)?;
    for (key, value) in &params.env {
        #[cfg(windows)]
        validate_launch_environment_entry(key, value)?;
        if child_environment_entry_allowed(key) {
            env.insert(key.to_ascii_uppercase(), (key.clone(), value.clone()));
        }
    }
    Ok(env)
}

/// [`TrustedBootstrap::SpawnedAgent`] environment: the reconstructed base plus
/// the allow-listed provider/gateway daemon variables, so the agent CLI finds
/// its login/config. The spawn runs with a cleared environment, so nothing
/// else leaks from the daemon; the only Synapse credentials admitted are a
/// live scoped spawn credential and the local-agent capability.
pub(crate) fn spawned_agent_environment(
    params: &ActLaunchParams,
) -> Result<BTreeMap<String, String>, ErrorData> {
    TrustedBootstrap::SpawnedAgent.keep_inherited_environment();
    let mut env = reconstructed_base_environment();
    env.extend(forwarded_agent_environment(daemon_environment()));
    validate_child_base_environment(&env, "act_spawn_agent")?;
    for (key, value) in &params.env {
        #[cfg(windows)]
        validate_launch_environment_entry(key, value)?;
        if spawned_agent_entry_allowed(key, value) {
            env.insert(key.to_ascii_uppercase(), (key.clone(), value.clone()));
        }
    }
    Ok(env.into_values().collect())
}

fn daemon_environment() -> impl Iterator<Item = (String, String)> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
}

/// Allow-listed provider/gateway subset of `vars` (case-insensitive names),
/// never including Synapse credentials.
pub(super) fn forwarded_agent_environment(
    vars: impl IntoIterator<Item = (String, String)>,
) -> BTreeMap<String, (String, String)> {
    vars.into_iter()
        .filter(|(key, _)| is_spawned_agent_forwarded_key(key))
        .map(|(key, value)| (key.to_ascii_uppercase(), (key, value)))
        .collect()
}

fn is_spawned_agent_forwarded_key(key: &str) -> bool {
    if is_credential_env_key(key) {
        return false;
    }
    let upper = key.to_ascii_uppercase();
    SPAWNED_AGENT_FORWARD_KEYS
        .iter()
        .any(|name| upper == name.to_ascii_uppercase())
        || SPAWNED_AGENT_FORWARD_PREFIXES
            .iter()
            .any(|prefix| upper.starts_with(prefix))
}

fn spawned_agent_entry_allowed(key: &str, value: &str) -> bool {
    if !is_credential_env_key(key) {
        return true;
    }
    if key.eq_ignore_ascii_case(crate::spawn_credentials::SPAWN_CREDENTIAL_ENV) {
        return crate::spawn_credentials::registry().is_live_credential(value);
    }
    key.eq_ignore_ascii_case(crate::local_agent_capability::ENV)
        && crate::local_agent_capability::matches(value)
}

/// Encodes an upper-case-keyed environment as a double-NUL-terminated
/// UTF-16 block for `CreateProcessW` with `CREATE_UNICODE_ENVIRONMENT`.
#[cfg(windows)]
pub(crate) fn environment_block(env: BTreeMap<String, (String, String)>) -> Vec<u16> {
    let mut block = Vec::new();
    for (_sort_key, (key, value)) in env {
        block.extend(format!("{key}={value}").encode_utf16());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

/// Daemon environment minus Synapse credentials, as a `CreateProcessW` block.
#[cfg(windows)]
pub(crate) fn inherited_environment_block() -> Vec<u16> {
    inherited_environment_block_from(std::env::vars_os())
}

/// [`inherited_environment_block`] over injected daemon variables (test seam).
#[cfg(windows)]
pub(crate) fn inherited_environment_block_from(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<u16> {
    let env = vars
        .into_iter()
        .filter_map(|(key, value)| {
            let key = key.into_string().ok()?;
            let value = value.into_string().ok()?;
            (!is_credential_env_key(&key)).then(|| (key.to_ascii_uppercase(), (key, value)))
        })
        .collect();
    environment_block(env)
}

/// Process builders that inherit the daemon environment.
pub(crate) trait InheritedEnvCommand {
    fn remove_inherited_env(&mut self, key: &OsStr);
}

impl InheritedEnvCommand for std::process::Command {
    fn remove_inherited_env(&mut self, key: &OsStr) {
        self.env_remove(key);
    }
}

impl InheritedEnvCommand for tokio::process::Command {
    fn remove_inherited_env(&mut self, key: &OsStr) {
        self.env_remove(key);
    }
}

/// `std::process::Command` for an internal helper (credentials scrubbed).
pub(crate) fn internal_helper_command(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    scrub_inherited_credentials(&mut command);
    command
}

/// `tokio::process::Command` for an internal helper (credentials scrubbed).
pub(crate) fn internal_helper_tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    scrub_inherited_credentials(&mut command);
    command
}

/// Internal-helper policy: the child inherits the daemon environment except
/// Synapse credentials.
pub(crate) fn scrub_inherited_credentials<C: InheritedEnvCommand>(command: &mut C) {
    scrub_credentials_from(command, std::env::vars_os().map(|(key, _)| key));
}

pub(super) fn scrub_credentials_from<C, I>(command: &mut C, keys: I)
where
    C: InheritedEnvCommand,
    I: IntoIterator<Item = OsString>,
{
    for key in keys {
        if key.to_str().is_some_and(is_credential_env_key) {
            command.remove_inherited_env(&key);
        }
    }
}
