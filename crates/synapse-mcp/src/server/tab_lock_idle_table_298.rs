//! #298 / D6 frozen behaviour table (manager-owned, work method v3). Codex and
//! Opus never edit this file. One case per L row of the tab-lock idle-release
//! table in the tab-locks lane report.

use super::{
    DEFAULT_IDLE_RELEASE_MS, HolderActivity, KeepReason, TabLockHolder, TabLockPlan,
    TabLockRequest, idle_release_ms_from, plan_tab_lock, tab_lock_refusal_message,
};

const NOW: u64 = 1_800_000_000_000;
const WINDOW: u64 = DEFAULT_IDLE_RELEASE_MS;
const TOOL: &str = "cdp_navigate_tab";
const TAB: &str = "TAB0298";

fn seen(session: &str, idle_ms: u64) -> TabLockHolder {
    TabLockHolder {
        session_id: session.to_owned(),
        activity: HolderActivity::Seen {
            last_seen_unix_ms: NOW - idle_ms,
            in_flight: false,
        },
    }
}

fn holder(session: &str, activity: HolderActivity) -> TabLockHolder {
    TabLockHolder {
        session_id: session.to_owned(),
        activity,
    }
}

fn refuse(session: &str, reason: KeepReason) -> TabLockPlan {
    TabLockPlan::Refuse {
        holder: session.to_owned(),
        reason,
    }
}

fn transfer(sessions: &[&str]) -> TabLockPlan {
    TabLockPlan::Transfer {
        from: sessions
            .iter()
            .map(|session| (*session).to_owned())
            .collect(),
    }
}

struct Row {
    name: &'static str,
    requester: &'static str,
    explicit: bool,
    holders: Vec<TabLockHolder>,
    window: u64,
    expected: TabLockPlan,
    /// Extra text a refusal must carry, besides the owner, the window and the
    /// own-tab path that every refusal carries.
    fragments: &'static [&'static str],
}

fn row(
    name: &'static str,
    requester: &'static str,
    holders: Vec<TabLockHolder>,
    expected: TabLockPlan,
) -> Row {
    Row {
        name,
        requester,
        explicit: true,
        holders,
        window: WINDOW,
        expected,
        fragments: &[],
    }
}

fn window_text(window: u64) -> String {
    if window == WINDOW {
        "10 minutes without MCP calls".to_owned()
    } else {
        format!("{} seconds without MCP calls", window / 1000)
    }
}

#[test]
fn tab_lock_idle_table_298() {
    let rows: [Row; 18] = [
        row(
            "298a_same_session_owner_retains",
            "agent-b",
            vec![seen("agent-b", 5)],
            TabLockPlan::Retain,
        ),
        row(
            "298b_operator_tab_without_owner_rows_grants_nothing",
            "agent-b",
            vec![],
            TabLockPlan::NoOwner,
        ),
        row(
            "298c_boundary_idle_just_under_window_is_refused",
            "agent-b",
            vec![seen("agent-a", WINDOW - 1)],
            refuse(
                "agent-a",
                KeepReason::Active {
                    idle_ms: WINDOW - 1,
                },
            ),
        ),
        row(
            "298c_boundary_idle_exactly_window_transfers",
            "agent-b",
            vec![seen("agent-a", WINDOW)],
            transfer(&["agent-a"]),
        ),
        row(
            "298c_boundary_idle_just_over_window_transfers",
            "agent-b",
            vec![seen("agent-a", WINDOW + 1)],
            transfer(&["agent-a"]),
        ),
        row(
            "298d_closed_owner_transfers",
            "agent-b",
            vec![holder("agent-a", HolderActivity::Closed)],
            transfer(&["agent-a"]),
        ),
        Row {
            fragments: &["cannot be read"],
            ..row(
                "298e_unknown_owner_activity_is_refused",
                "agent-b",
                vec![holder("agent-a", HolderActivity::Unknown)],
                refuse("agent-a", KeepReason::UnknownActivity),
            )
        },
        Row {
            fragments: &["in flight"],
            ..row(
                "298f_owner_with_call_in_flight_is_refused_even_when_long_idle",
                "agent-b",
                vec![holder(
                    "agent-a",
                    HolderActivity::Seen {
                        last_seen_unix_ms: NOW - 2 * 60 * 60 * 1000,
                        in_flight: true,
                    },
                )],
                refuse("agent-a", KeepReason::InFlight),
            )
        },
        row(
            "298g_hostile_future_last_seen_is_refused",
            "agent-b",
            vec![holder(
                "agent-a",
                HolderActivity::Seen {
                    last_seen_unix_ms: NOW + 60_000,
                    in_flight: false,
                },
            )],
            refuse("agent-a", KeepReason::Active { idle_ms: 0 }),
        ),
        Row {
            explicit: false,
            fragments: &["cdp_target_id"],
            ..row(
                "298h_idle_owner_without_explicit_tab_request_is_refused",
                "agent-b",
                vec![seen("agent-a", WINDOW + 1)],
                refuse("agent-a", KeepReason::NotExplicit),
            )
        },
        row(
            "298i_empty_requester_session_is_refused",
            "",
            vec![seen("agent-a", WINDOW + 1)],
            refuse("agent-a", KeepReason::NoRequester),
        ),
        row(
            "298j_second_actor_after_transfer_is_refused",
            "agent-c",
            vec![seen("agent-b", 0)],
            refuse("agent-b", KeepReason::Active { idle_ms: 0 }),
        ),
        row(
            "298k_one_active_co_owner_refuses_without_partial_transfer",
            "agent-b",
            vec![seen("agent-a", WINDOW + 1), seen("agent-d", 1_000)],
            refuse("agent-d", KeepReason::Active { idle_ms: 1_000 }),
        ),
        row(
            "298l_all_idle_or_closed_co_owners_transfer_together",
            "agent-b",
            vec![
                seen("agent-a", WINDOW),
                holder("agent-d", HolderActivity::Closed),
            ],
            transfer(&["agent-a", "agent-d"]),
        ),
        row(
            "298m_neighbour_requester_already_co_owner_retains",
            "agent-b",
            vec![seen("agent-a", 1_000), seen("agent-b", WINDOW + 1)],
            TabLockPlan::Retain,
        ),
        Row {
            window: 60_000,
            ..row(
                "298n_configured_window_not_a_literal_transfers_at_its_boundary",
                "agent-b",
                vec![seen("agent-a", 60_000)],
                transfer(&["agent-a"]),
            )
        },
        Row {
            window: 90_000,
            fragments: &["90 seconds"],
            ..row(
                "298n_configured_window_appears_in_refusal",
                "agent-b",
                vec![seen("agent-a", 30_000)],
                refuse("agent-a", KeepReason::Active { idle_ms: 30_000 }),
            )
        },
        Row {
            fragments: &["599 seconds"],
            ..row(
                "298p_slow_former_owner_gets_clear_error_naming_new_owner",
                "agent-a",
                vec![seen("agent-b", 1_000)],
                refuse("agent-b", KeepReason::Active { idle_ms: 1_000 }),
            )
        },
    ];

    let mut failures = Vec::new();
    for case in &rows {
        let request = TabLockRequest {
            requester_session_id: case.requester,
            explicit_target: case.explicit,
            holders: &case.holders,
            now_unix_ms: NOW,
            idle_release_ms: case.window,
        };
        let got = plan_tab_lock(&request);
        if got != case.expected {
            failures.push(format!(
                "row {}: plan {got:?}, expected {:?}",
                case.name, case.expected
            ));
            continue;
        }
        if let TabLockPlan::Refuse { holder, reason } = &got {
            let message =
                tab_lock_refusal_message(TOOL, TAB, holder, case.requester, *reason, case.window);
            let window = window_text(case.window);
            let mut required = vec![
                holder.as_str(),
                TAB,
                window.as_str(),
                "browser_tabs operation=new",
                "target operation=status",
            ];
            required.extend_from_slice(case.fragments);
            for fragment in required {
                if !message.contains(fragment) {
                    failures.push(format!(
                        "row {}: refusal lacks {fragment:?}: {message}",
                        case.name
                    ));
                }
            }
        }
    }

    // Config rows: the window is a config value, never a literal.
    let config_rows: [(&str, Option<&str>, u64); 6] = [
        ("298o_config_unset_is_ten_minutes", None, 600_000),
        ("298o_config_valid_value_is_used", Some("120000"), 120_000),
        ("298o_config_padded_value_is_used", Some(" 45000 "), 45_000),
        ("298o_config_zero_falls_back_to_default", Some("0"), 600_000),
        ("298o_config_empty_falls_back_to_default", Some(""), 600_000),
        (
            "298o_config_hostile_text_falls_back_to_default",
            Some("-5;ten"),
            600_000,
        ),
    ];
    for (name, raw, expected) in config_rows {
        let got = idle_release_ms_from(raw);
        if got != expected {
            failures.push(format!("row {name}: window {got}, expected {expected}"));
        }
    }

    assert!(
        failures.is_empty(),
        "#298 tab-lock idle table rows failed:\n{}",
        failures.join("\n")
    );
}
