//! D8 (roadmap #598): agent spawn is opt-in.
//!
//! A daemon starts agents only when the owner started it with
//! `SYNAPSE_AGENT_SPAWN_ENABLED=1`. Every spawn route (the `agent` facade,
//! task dispatch, respawn, the dashboard) asks this switch before anything
//! is launched, and `health` shows it.

use crate::m1::mcp_error;
use crate::server::ErrorData;

#[cfg(test)]
#[path = "agent_spawn_switch_table_598.rs"]
mod agent_spawn_switch_table_598;

/// Whether this daemon may start agents.
///
/// Read once at daemon start from [`Self::ENV`]: only the value `1` (spaces
/// and line ends around it ignored) turns it on; unset, empty or any other
/// value is off, so a wrong value allows less, never more. Nothing changes it while the daemon runs.
/// `Default` (a config that did not read the variable) is off.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AgentSpawnSwitch {
    enabled: bool,
}

impl AgentSpawnSwitch {
    /// The variable the owner sets on the daemon to allow agent spawn.
    pub const ENV: &'static str = "SYNAPSE_AGENT_SPAWN_ENABLED";
    /// Error code of a spawn refused because the switch is off.
    pub const DISABLED_CODE: &'static str = "AGENT_SPAWN_DISABLED";

    /// The switch for the raw value of [`Self::ENV`] (`None` = not set).
    #[must_use]
    pub fn from_env_value(raw: Option<&str>) -> Self {
        Self {
            enabled: raw.is_some_and(|value| value.trim() == "1"),
        }
    }

    /// Reads [`Self::ENV`] once, at daemon start (`main.rs`), and logs the
    /// result. A value that is not valid text counts as not set.
    #[must_use]
    pub fn from_env() -> Self {
        let switch = Self::from_env_value(std::env::var(Self::ENV).ok().as_deref());
        tracing::info!(
            code = "AGENT_SPAWN_SWITCH",
            env = Self::ENV,
            detail = switch.health_detail(),
            "agent spawn switch read at daemon start"
        );
        switch
    }

    /// Every spawn route calls this before any side effect.
    ///
    /// # Errors
    /// `AGENT_SPAWN_DISABLED` when the switch is off; the message tells the
    /// caller that only the owner turns it on, and how.
    pub fn ensure_allowed(&self) -> Result<(), ErrorData> {
        if self.enabled {
            return Ok(());
        }
        Err(mcp_error(
            Self::DISABLED_CODE,
            format!(
                "agent spawn is off on this daemon (D8): {} is not 1, so nothing was launched. Only the owner turns it on, by starting the daemon with {}=1",
                Self::ENV,
                Self::ENV
            ),
        ))
    }

    /// The token `health` appends to the action subsystem's detail.
    #[must_use]
    pub const fn health_detail(&self) -> &'static str {
        if self.enabled {
            "agent_spawn=on"
        } else {
            "agent_spawn=off"
        }
    }
}
