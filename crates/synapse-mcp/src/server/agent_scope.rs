//! #299: several agents inside ONE MCP session, each with its own target.
//!
//! A Claude Code main session and all its sub-agents share one MCP connection,
//! so they share one `Mcp-Session-Id` and, before this module, one active
//! target: one agent's `target set` replaced another agent's, and `act` (which
//! has no target field) acted on whichever tab was bound last.
//!
//! The optional `agent_id` field on the target-scoped public tools fixes that:
//! - every `(session, agent_id)` pair keeps its own active target in this
//!   module, beside the session's own target;
//! - a call that carries `agent_id` runs inside an [`AgentScope`] task-local,
//!   and every target lookup and bind of that call (`session_target`,
//!   `set_session_target`, `clear_session_target`) uses that agent's target,
//!   never another agent's and never the session's own;
//! - once any agent of a session holds a target, a target-scoped call WITHOUT
//!   `agent_id` is refused, and unscoped lookups see no target, so a call that
//!   forgot its key can never reach another agent's tab;
//! - two agents of one session cannot bind the same window or tab;
//! - a client that never sends `agent_id` sees no change.
//!
//! Agent targets live in memory only: they are not restored after a daemon
//! restart, and they are dropped when the agent clears its target, when the
//! bound tab is closed, and when the session ends.

use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    sync::{Arc, LazyLock, Mutex, MutexGuard},
};

use rmcp::model::Tool;
use serde_json::{Map, Value, json};
use synapse_core::error_codes;

use super::{ErrorData, SessionTarget, mcp_error};

/// The per-call field name, on the wire and in every listed input schema.
pub(crate) const AGENT_ID_FIELD: &str = "agent_id";
const AGENT_ID_MAX_LEN: usize = 64;

/// Public tools whose calls resolve or bind the session target. Each lists
/// `agent_id` in its input schema and is refused without it while agents of
/// the same session hold targets.
pub(crate) const AGENT_SCOPED_TOOLS: &[&str] = &[
    "act",
    "target",
    "observe",
    "find",
    "read_text",
    "screenshot",
    "browser_tabs",
    "browser_nav",
    "browser_dom",
    "browser_form",
    "browser_wait",
    "browser_capture",
    "browser_storage",
    "browser_debugger",
    "reality",
];

tokio::task_local! {
    /// The agent of the running tool call (unset for calls without agent_id).
    static CURRENT_AGENT: AgentScope;
}

static AGENT_TARGETS: LazyLock<Mutex<AgentTargets>> =
    LazyLock::new(|| Mutex::new(AgentTargets::default()));

/// One agent of one MCP session: the key its own target is stored under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentScope {
    session_id: String,
    agent_id: String,
}

impl AgentScope {
    pub(crate) fn new(session_id: &str, agent_id: &str) -> Self {
        Self {
            session_id: session_id.to_owned(),
            agent_id: agent_id.to_owned(),
        }
    }

    /// Runs `future` (one tool call) as this agent; `None` runs it unscoped.
    pub(crate) async fn scope<F: Future>(scope: Option<Self>, future: F) -> F::Output {
        match scope {
            Some(scope) => CURRENT_AGENT.scope(scope, future).await,
            None => future.await,
        }
    }

    /// The scope of the running call, for a task that call spawns.
    pub(crate) fn current() -> Option<Self> {
        CURRENT_AGENT.try_with(Clone::clone).ok()
    }

    /// Carries the call's scope into blocking work that resolves ownership.
    #[cfg(any(windows, test))]
    pub(crate) fn sync_scope<T>(scope: Option<Self>, call: impl FnOnce() -> T) -> T {
        match scope {
            Some(scope) => CURRENT_AGENT.sync_scope(scope, call),
            None => call(),
        }
    }

    /// The agent id of the running call when that call belongs to `session_id`.
    pub(crate) fn agent_for(session_id: &str) -> Option<String> {
        CURRENT_AGENT
            .try_with(|scope| (scope.session_id == session_id).then(|| scope.agent_id.clone()))
            .ok()
            .flatten()
    }
}

/// How the active-target lookup of one session resolves for the running call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TargetLookup {
    /// The call names an agent: that agent's own target, or none.
    Agent(Option<SessionTarget>),
    /// The call names no agent, but agents of this session hold targets: the
    /// call gets no target at all (never an agent's, never the session's own).
    AgentsActive,
    /// No agent in play: the session's own target, exactly as before #299.
    Session,
}

pub(crate) fn lookup(session_id: &str) -> Result<TargetLookup, ErrorData> {
    let agent = AgentScope::agent_for(session_id);
    let registry = registry()?;
    Ok(match agent {
        Some(agent_id) => TargetLookup::Agent(registry.get(session_id, &agent_id)),
        None if registry.holds_any(session_id) => TargetLookup::AgentsActive,
        None => TargetLookup::Session,
    })
}

/// Binds `target` for `agent_id`; returns the agent's previous target.
pub(crate) fn bind(
    session_id: &str,
    agent_id: &str,
    target: SessionTarget,
) -> Result<Option<SessionTarget>, ErrorData> {
    registry()?.bind(session_id, agent_id, target)
}

/// Refuses access to a surface held by another agent of this session.
pub(crate) fn ensure_access(session_id: &str, target: &SessionTarget) -> Result<(), ErrorData> {
    let agent_id = AgentScope::agent_for(session_id);
    let registry = registry()?;
    if let Some(agents) = registry.by_session.get(session_id)
        && let Some((holder, _)) = agents.iter().find(|(holder, held)| {
            agent_id.as_deref() != Some(holder.as_str()) && same_surface(held, target)
        })
    {
        return Err(mcp_error(
            error_codes::ACTION_TARGET_INVALID,
            format!(
                "target {target:?} is held by agent {holder:?} of this MCP session; clear that agent's target before transferring it"
            ),
        ));
    }
    Ok(())
}

/// Clears `agent_id`'s target; returns it.
pub(crate) fn unbind(session_id: &str, agent_id: &str) -> Result<Option<SessionTarget>, ErrorData> {
    Ok(registry()?.unbind(session_id, agent_id))
}

/// A closed or vanished tab: clears it from every agent of the session that
/// had it bound; returns `(agent_id, target)` for each.
pub(crate) fn unbind_cdp_target(
    session_id: &str,
    cdp_target_id: &str,
) -> Result<Vec<(String, SessionTarget)>, ErrorData> {
    Ok(registry()?.unbind_cdp_target(session_id, cdp_target_id))
}

/// Session teardown: drops every agent target of the session; returns how many.
pub(crate) fn forget_session(session_id: &str) -> usize {
    // Teardown must not fail on a poisoned lock: the registry holds plain data,
    // so the inner value is still consistent.
    let mut registry = AGENT_TARGETS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    registry.forget_session(session_id)
}

/// Takes `agent_id` out of a target-scoped call's arguments (the tool's own
/// parameter type never sees it) and returns the scope the call runs in.
/// Refuses a target-scoped call without `agent_id` while agents of the same
/// session hold targets. Legacy live-target operations fail closed too.
pub(crate) fn scope_for_call(
    tool: &str,
    session_id: Option<&str>,
    arguments: &mut Option<Map<String, Value>>,
) -> Result<Option<AgentScope>, ErrorData> {
    if !AGENT_SCOPED_TOOLS.contains(&tool) {
        if arguments
            .as_ref()
            .is_some_and(|arguments| arguments.contains_key(AGENT_ID_FIELD))
        {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!("{tool} does not support agent_id"),
            ));
        }
        if unscoped_target_operation(tool, arguments.as_ref())
            && let Some(session_id) = session_id
        {
            let holders = registry()?.agents_with_targets(session_id);
            if !holders.is_empty() {
                return Err(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    format!(
                        "{tool} has no agent-scoped target support and is refused while agents {holders:?} hold targets; clear their targets with target operation=clear and the named agent_id first"
                    ),
                ));
            }
        }
        return Ok(None);
    }
    let agent_id = match arguments
        .as_mut()
        .and_then(|arguments| arguments.remove(AGENT_ID_FIELD))
    {
        None => None,
        Some(value) => Some(parse_agent_id(tool, &value)?),
    };
    match (session_id, agent_id) {
        (Some(session_id), Some(agent_id)) => Ok(Some(AgentScope::new(session_id, &agent_id))),
        (None, Some(_)) => Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{tool} agent_id requires an MCP session id (run the daemon in HTTP mode so each client connection has its own Mcp-Session-Id)"
            ),
        )),
        (None, None) => Ok(None),
        (Some(session_id), None) => {
            let holders = registry()?.agents_with_targets(session_id);
            if holders.is_empty() {
                Ok(None)
            } else {
                Err(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    format!(
                        "{tool} refused a call without agent_id: agents {holders:?} of this MCP session hold their own targets, so this call has no target; pass your agent_id (the one you bound with target operation=set or browser_tabs operation=select)"
                    ),
                ))
            }
        }
    }
}

/// Legacy operations that can perceive or operate on a live target directly.
fn unscoped_target_operation(tool: &str, arguments: Option<&Map<String, Value>>) -> bool {
    let operation = arguments
        .and_then(|arguments| arguments.get("operation"))
        .and_then(Value::as_str);
    match tool {
        "livecapture" | "livefeed" => true,
        "subscribe" => operation == Some("events"),
        "replay" => matches!(operation, Some("record" | "demo_start")),
        "verification" => matches!(operation, Some("inbox" | "poll")),
        "routine" => operation == Some("armed_tick"),
        "assist" => operation == Some("suggestion_accept"),
        _ => false,
    }
}

/// Lists `agent_id` in the input schema of every agent-scoped tool, including
/// each `oneOf`/`anyOf` branch that names its own properties.
pub(crate) fn add_agent_id_parameter(tools: &mut [Tool]) {
    for tool in tools
        .iter_mut()
        .filter(|tool| AGENT_SCOPED_TOOLS.contains(&tool.name.as_ref()))
    {
        let mut schema = (*tool.input_schema).clone();
        add_agent_id_property(&mut schema);
        tool.input_schema = Arc::new(schema);
    }
}

fn add_agent_id_property(schema: &mut Map<String, Value>) {
    let properties = schema
        .entry("properties")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(properties) = properties {
        properties.insert(AGENT_ID_FIELD.to_owned(), agent_id_property_schema());
    }
    for keyword in ["oneOf", "anyOf"] {
        let Some(Value::Array(branches)) = schema.get_mut(keyword) else {
            continue;
        };
        for branch in branches.iter_mut().filter_map(Value::as_object_mut) {
            let closed = branch.get("additionalProperties") == Some(&Value::Bool(false));
            if !closed && !branch.contains_key("properties") {
                continue;
            }
            let properties = branch
                .entry("properties")
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(properties) = properties {
                properties.insert(
                    AGENT_ID_FIELD.to_owned(),
                    json!({ "$ref": format!("#/properties/{AGENT_ID_FIELD}") }),
                );
            }
        }
    }
}

fn agent_id_property_schema() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "maxLength": AGENT_ID_MAX_LEN,
        "pattern": "^[A-Za-z0-9._:-]+$",
        "description": "Optional key of one agent when several agents share this MCP session (letters, digits, . _ : -). Each agent_id has its own active target: bind it with target operation=set or browser_tabs operation=select passing agent_id, then pass the same agent_id on every call. Once any agent_id holds a target, calls to this tool without agent_id are refused."
    })
}

fn parse_agent_id(tool: &str, value: &Value) -> Result<String, ErrorData> {
    let valid = value.as_str().filter(|text| {
        !text.is_empty()
            && text.len() <= AGENT_ID_MAX_LEN
            && text.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
            })
    });
    valid.map(str::to_owned).ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{tool} agent_id must be a string of 1..={AGENT_ID_MAX_LEN} letters, digits or . _ : - characters; got {value}"
            ),
        )
    })
}

fn registry() -> Result<MutexGuard<'static, AgentTargets>, ErrorData> {
    AGENT_TARGETS.lock().map_err(|_poisoned| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            "agent target registry lock poisoned",
        )
    })
}

/// The active target of every agent, per MCP session.
#[derive(Debug, Default)]
struct AgentTargets {
    by_session: HashMap<String, BTreeMap<String, SessionTarget>>,
}

impl AgentTargets {
    fn get(&self, session_id: &str, agent_id: &str) -> Option<SessionTarget> {
        self.by_session.get(session_id)?.get(agent_id).cloned()
    }

    fn holds_any(&self, session_id: &str) -> bool {
        self.by_session
            .get(session_id)
            .is_some_and(|agents| !agents.is_empty())
    }

    fn agents_with_targets(&self, session_id: &str) -> Vec<String> {
        self.by_session
            .get(session_id)
            .map(|agents| agents.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn bind(
        &mut self,
        session_id: &str,
        agent_id: &str,
        target: SessionTarget,
    ) -> Result<Option<SessionTarget>, ErrorData> {
        let agents = self.by_session.entry(session_id.to_owned()).or_default();
        if let Some((holder, _)) = agents
            .iter()
            .find(|(holder, held)| holder.as_str() != agent_id && same_surface(held, &target))
        {
            return Err(mcp_error(
                error_codes::ACTION_TARGET_INVALID,
                format!(
                    "agent {agent_id:?} cannot bind {target:?}: agent {holder:?} of this MCP session already holds it; each agent drives its own window or tab"
                ),
            ));
        }
        Ok(agents.insert(agent_id.to_owned(), target))
    }

    fn unbind(&mut self, session_id: &str, agent_id: &str) -> Option<SessionTarget> {
        let agents = self.by_session.get_mut(session_id)?;
        let removed = agents.remove(agent_id);
        if agents.is_empty() {
            self.by_session.remove(session_id);
        }
        removed
    }

    fn unbind_cdp_target(
        &mut self,
        session_id: &str,
        cdp_target_id: &str,
    ) -> Vec<(String, SessionTarget)> {
        let Some(agents) = self.by_session.get_mut(session_id) else {
            return Vec::new();
        };
        let matching = agents
            .iter()
            .filter(|(_, held)| {
                matches!(held, SessionTarget::Cdp { cdp_target_id: id, .. } if id.eq_ignore_ascii_case(cdp_target_id))
            })
            .map(|(agent_id, _)| agent_id.clone())
            .collect::<Vec<_>>();
        let removed = matching
            .into_iter()
            .filter_map(|agent_id| agents.remove(&agent_id).map(|target| (agent_id, target)))
            .collect();
        if agents.is_empty() {
            self.by_session.remove(session_id);
        }
        removed
    }

    fn forget_session(&mut self, session_id: &str) -> usize {
        self.by_session
            .remove(session_id)
            .map_or(0, |agents| agents.len())
    }
}

/// Window targets overlap every tab in that window; two CDP targets overlap
/// only when they name the same tab.
fn same_surface(left: &SessionTarget, right: &SessionTarget) -> bool {
    match (left, right) {
        (SessionTarget::Window { hwnd: left }, SessionTarget::Window { hwnd: right }) => {
            left == right
        }
        (
            SessionTarget::Cdp {
                cdp_target_id: left,
                ..
            },
            SessionTarget::Cdp {
                cdp_target_id: right,
                ..
            },
        ) => left.eq_ignore_ascii_case(right),
        (SessionTarget::Window { hwnd }, SessionTarget::Cdp { window_hwnd, .. })
        | (SessionTarget::Cdp { window_hwnd, .. }, SessionTarget::Window { hwnd }) => {
            hwnd == window_hwnd
        }
    }
}
