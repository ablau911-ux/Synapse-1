//! FROZEN behaviour table of roadmap #598 (D8: agent spawn is opt-in),
//! rows P1-P8 (`AgentSpawnSwitch`). Written from the table before the code.
//! Coders never edit this file; a wrong row is reported to the manager.
//! Place: crates/synapse-mcp/src/server/agent_spawn_switch_table_598.rs
//! (registered by the `#[path]` lines at the top of `agent_spawn_switch.rs`).
//! One test per row, named after the row. `None` = the variable is not set.
//! CONTROL cases stop a stub that answers one constant (always on, or
//! always off) from passing a row.

use super::AgentSpawnSwitch;

const DISABLED: &str = "AGENT_SPAWN_DISABLED";

fn from(raw: &str) -> AgentSpawnSwitch {
    AgentSpawnSwitch::from_env_value(Some(raw))
}

fn unset() -> AgentSpawnSwitch {
    AgentSpawnSwitch::from_env_value(None)
}

fn allowed(switch: AgentSpawnSwitch) -> bool {
    switch.ensure_allowed().is_ok()
}

/// The error code of the refusal, `None` when the spawn is allowed.
fn refusal_code(switch: AgentSpawnSwitch) -> Option<String> {
    switch.ensure_allowed().err().and_then(|error| {
        error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    })
}

fn refusal_message(switch: AgentSpawnSwitch) -> String {
    switch
        .ensure_allowed()
        .err()
        .map(|error| error.message.to_string())
        .unwrap_or_default()
}

#[test]
fn p1_unset_is_off_and_spawn_is_refused() {
    assert_eq!(refusal_code(unset()).as_deref(), Some(DISABLED));
    // CONTROL: "1" is allowed (a stub that always refuses fails here).
    assert!(allowed(from("1")));
}

#[test]
fn p2_one_is_on_and_spawn_is_allowed() {
    assert!(allowed(from("1")));
    // CONTROL: "0" is refused (a stub that always allows fails here).
    assert_eq!(refusal_code(from("0")).as_deref(), Some(DISABLED));
}

#[test]
fn p3_one_with_spaces_or_a_line_end_is_on() {
    for raw in [" 1", "1 ", "\t1\r\n", "1\n"] {
        assert!(allowed(from(raw)), "{raw:?} must turn spawn on");
    }
    // CONTROL: unset is refused.
    assert_eq!(refusal_code(unset()).as_deref(), Some(DISABLED));
}

#[test]
fn p4_empty_and_off_words_are_off() {
    for raw in ["", "   ", "0", "false", "no", "off", "OFF"] {
        assert_eq!(
            refusal_code(from(raw)).as_deref(),
            Some(DISABLED),
            "{raw:?} must keep spawn off"
        );
    }
    // CONTROL: "1" is allowed.
    assert!(allowed(from("1")));
}

#[test]
fn p5_any_other_value_is_off() {
    for raw in [
        "true", "yes", "on", "ON", "enabled", "2", "01", "-1", "1.0", "1 1", "1;calc", "\u{ff11}",
    ] {
        assert_eq!(
            refusal_code(from(raw)).as_deref(),
            Some(DISABLED),
            "{raw:?} must keep spawn off"
        );
    }
    // CONTROL: "1" is allowed.
    assert!(allowed(from("1")));
}

#[test]
fn p6_a_config_that_never_read_the_variable_is_off() {
    assert_eq!(
        refusal_code(AgentSpawnSwitch::default()).as_deref(),
        Some(DISABLED)
    );
    // CONTROL: "1" is allowed.
    assert!(allowed(from("1")));
}

#[test]
fn p7_the_refusal_says_only_the_owner_turns_it_on_and_how() {
    let message = refusal_message(unset());
    assert!(
        message.contains("SYNAPSE_AGENT_SPAWN_ENABLED=1"),
        "message must name the setting: {message}"
    );
    assert!(
        message.contains("owner"),
        "message must name the owner: {message}"
    );
    // CONTROL: "1" is allowed (a stub that always refuses fails here).
    assert!(allowed(from("1")));
}

#[test]
fn p8_health_shows_the_switch() {
    assert_eq!(from("1").health_detail(), "agent_spawn=on");
    assert_eq!(unset().health_detail(), "agent_spawn=off");
    assert_eq!(from("true").health_detail(), "agent_spawn=off");
    assert_eq!(
        AgentSpawnSwitch::default().health_detail(),
        "agent_spawn=off"
    );
}
