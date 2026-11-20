//! FROZEN behaviour table of Gitea #2401 (review of build 085eafa2) with #2395,
//! rows W7-W11 and P1-P4. Written from the issue table before the code.
//! Coders never edit this file; a wrong row is reported to the manager.
//! Place: crates/synapse-mcp/src/m2/wheel_plan_table_2401.rs (registered by the
//! `#[path]` line at the end of the #2401 stub block in m2/scroll.rs).
//!
//! W rows: `WheelPlan::for_scroll(dy, dx)` plans both directions of one scroll
//! together; more than 1024 messages TOGETHER are refused before anything is
//! posted. P rows: `ScrollPacing::new(smooth_asked, route_pauses, dy, dx)`
//! gives the reply's `smooth`, `smooth_interval_ms` and
//! `scheduled_smooth_total_ms` from what runs, not from the request. The table
//! `wheel_plan_table_2351.rs` (W1-W6, `for_notches`) stays as it is.

use super::{ActionError, ScrollPacing, WheelPlan};

/// What one W row expects from `WheelPlan::for_scroll`.
enum Then {
    /// Planned: (count, delta) of the vertical messages, then of the
    /// horizontal ones; every message of one direction is that delta.
    Plan {
        vertical: (usize, i16),
        horizontal: (usize, i16),
    },
    /// Refused before anything is posted (`ActionError::TargetInvalid`).
    Refused,
}

struct Row {
    name: &'static str,
    dy: i32,
    dx: i32,
    then: Then,
}

/// One P row: the route's facts in, the reply's three values out.
struct PacingRow {
    name: &'static str,
    smooth_asked: bool,
    route_pauses: bool,
    dy: i32,
    dx: i32,
    /// (`smooth`, `smooth_interval_ms`, `scheduled_smooth_total_ms`).
    reply: (bool, u32, u32),
}

fn plan_mismatch(
    direction: &str,
    messages: &[i16],
    (count, delta): (usize, i16),
) -> Option<String> {
    (messages.len() != count || messages.iter().any(|message| *message != delta)).then(|| {
        format!(
            "{direction}: expected {count} messages of {delta}, observed {} messages starting {:?}",
            messages.len(),
            messages.iter().take(4).collect::<Vec<_>>()
        )
    })
}

#[test]
fn wheel_plan_table_2401() {
    let rows = [
        Row {
            name: "W7_600_and_600_together_over_the_limit",
            dy: 600,
            dx: 600,
            then: Then::Refused,
        },
        Row {
            name: "W8_512_up_and_512_right",
            dy: 512,
            dx: 512,
            then: Then::Plan {
                vertical: (512, 120),
                horizontal: (512, 120),
            },
        },
        Row {
            name: "W8_1024_down_only",
            dy: -1024,
            dx: 0,
            then: Then::Plan {
                vertical: (1024, -120),
                horizontal: (0, 0),
            },
        },
        Row {
            name: "W8_1024_right_only",
            dy: 0,
            dx: 1024,
            then: Then::Plan {
                vertical: (0, 0),
                horizontal: (1024, 120),
            },
        },
        Row {
            name: "W9_1024_up_and_1_right",
            dy: 1024,
            dx: 1,
            then: Then::Refused,
        },
        Row {
            name: "W9_1_down_and_1024_left",
            dy: -1,
            dx: -1024,
            then: Then::Refused,
        },
        Row {
            name: "W10_i32_min_both",
            dy: i32::MIN,
            dx: i32::MIN,
            then: Then::Refused,
        },
        Row {
            name: "W10_i32_max_both",
            dy: i32::MAX,
            dx: i32::MAX,
            then: Then::Refused,
        },
        Row {
            name: "W11_nothing_to_scroll",
            dy: 0,
            dx: 0,
            then: Then::Plan {
                vertical: (0, 0),
                horizontal: (0, 0),
            },
        },
    ];
    let pacing_rows = [
        PacingRow {
            // #2395: the posted route never pauses, whatever the caller asked.
            name: "P1_posted_route_smooth_asked_80_down",
            smooth_asked: true,
            route_pauses: false,
            dy: -80,
            dx: 0,
            reply: (false, 0, 0),
        },
        PacingRow {
            name: "P2_foreground_route_smooth_5_steps",
            smooth_asked: true,
            route_pauses: true,
            dy: 5,
            dx: 0,
            reply: (true, 30, 120),
        },
        PacingRow {
            name: "P3_foreground_route_not_smooth_3",
            smooth_asked: false,
            route_pauses: true,
            dy: 3,
            dx: 0,
            reply: (false, 0, 0),
        },
        PacingRow {
            name: "P4_foreground_route_smooth_1_step",
            smooth_asked: true,
            route_pauses: true,
            dy: 1,
            dx: 0,
            reply: (true, 30, 0),
        },
        PacingRow {
            name: "P4_foreground_route_smooth_0_steps",
            smooth_asked: true,
            route_pauses: true,
            dy: 0,
            dx: 0,
            reply: (false, 0, 0),
        },
    ];

    let mut failures = Vec::new();
    for row in &rows {
        match (&row.then, WheelPlan::for_scroll(row.dy, row.dx)) {
            (
                Then::Plan {
                    vertical,
                    horizontal,
                },
                Ok((vertical_plan, horizontal_plan)),
            ) => {
                for mismatch in [
                    plan_mismatch("vertical", vertical_plan.messages(), *vertical),
                    plan_mismatch("horizontal", horizontal_plan.messages(), *horizontal),
                ]
                .into_iter()
                .flatten()
                {
                    failures.push(format!("{}: {mismatch}", row.name));
                }
            }
            (Then::Plan { .. }, Err(error)) => failures.push(format!(
                "{}: expected a plan, observed a refusal: {error:?}",
                row.name
            )),
            (Then::Refused, Ok((vertical_plan, horizontal_plan))) => failures.push(format!(
                "{}: expected a refusal, observed a plan of {} + {} messages",
                row.name,
                vertical_plan.messages().len(),
                horizontal_plan.messages().len()
            )),
            (Then::Refused, Err(ActionError::TargetInvalid { .. })) => {}
            (Then::Refused, Err(error)) => failures.push(format!(
                "{}: expected the limit refusal (TargetInvalid), observed {error:?}",
                row.name
            )),
        }
    }
    for row in &pacing_rows {
        let pacing = ScrollPacing::new(row.smooth_asked, row.route_pauses, row.dy, row.dx);
        let observed = (
            pacing.smooth(),
            pacing.interval_ms(),
            pacing.scheduled_total_ms(),
        );
        if observed != row.reply {
            failures.push(format!(
                "{}: expected (smooth, smooth_interval_ms, scheduled_smooth_total_ms) = {:?}, observed {observed:?}",
                row.name, row.reply
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} checks of {} failed:\n{}",
        failures.len(),
        rows.len() + pacing_rows.len(),
        failures.join("\n")
    );
}
