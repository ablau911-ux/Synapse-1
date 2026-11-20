//! #146: process-wide cache of the sanitized `tools/list` payload, keyed by the
//! environment flags that change which routes `SynapseService::tool_router`
//! registers. Sanitizing every schema costs ~100 ms of CPU, so each distinct
//! flag combination is computed once and shared.
use std::sync::{Arc, OnceLock};

use rmcp::model::Tool;

/// The env switches that alter the router's tool set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ToolListFlags {
    /// `SYNAPSE_DEBUG_TOOLS`: keeps the synthetic fault-injector tools.
    pub debug_tools: bool,
    /// `SYNAPSE_ENABLE_EVERQUEST`: adds the EverQuest domain pack.
    pub everquest: bool,
}

impl ToolListFlags {
    pub(crate) fn from_env() -> Self {
        Self {
            debug_tools: super::debug_tools_enabled(),
            everquest: super::everquest_enabled(),
        }
    }

    fn slot(self) -> usize {
        usize::from(self.debug_tools) | (usize::from(self.everquest) << 1)
    }
}

const SLOTS: usize = 4;

pub(crate) struct ToolsListCache {
    slots: [OnceLock<Arc<Vec<Tool>>>; SLOTS],
}

impl ToolsListCache {
    pub(crate) const fn new() -> Self {
        Self {
            slots: [const { OnceLock::new() }; SLOTS],
        }
    }

    /// Return the cached list for `flags`, computing it once with `compute`.
    pub(crate) fn get_or_compute(
        &self,
        flags: ToolListFlags,
        compute: impl FnOnce() -> Vec<Tool>,
    ) -> Arc<Vec<Tool>> {
        Arc::clone(self.slots[flags.slot()].get_or_init(|| Arc::new(compute())))
    }
}
