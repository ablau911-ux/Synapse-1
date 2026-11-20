//! FROZEN behaviour table of Gitea #2351 (roadmap item #585), rows W1-W6.
//! Written from the issue table before the code. Coders never edit this
//! file; a wrong row is reported to the manager.
//! Place: crates/synapse-mcp/src/m2/wheel_plan_table_2351.rs (registered by
//! the `#[path]` line at the end of the stub block in m2/scroll.rs).

use super::{ActionError, WheelPlan};

/// What one row expects from `WheelPlan::for_notches`.
enum Then {
    /// Planned: exactly `count` messages, every one of them `delta`.
    Messages { count: usize, delta: i16 },
    /// Refused before anything is posted (the existing cap refusal,
    /// `ActionError::TargetInvalid`).
    Refused,
}

struct Row {
    name: &'static str,
    notches: i32,
    then: Then,
}

#[test]
fn wheel_plan_table_2351() {
    let rows = [
        Row {
            name: "W1_three_notches_up",
            notches: 3,
            then: Then::Messages {
                count: 3,
                delta: 120,
            },
        },
        Row {
            // The Firefox case: 80 notches down are 80 real wheel steps, not
            // one message of -9600.
            name: "W2_eighty_notches_down",
            notches: -80,
            then: Then::Messages {
                count: 80,
                delta: -120,
            },
        },
        Row {
            name: "W3_one_notch_up",
            notches: 1,
            then: Then::Messages {
                count: 1,
                delta: 120,
            },
        },
        Row {
            name: "W3_one_notch_down",
            notches: -1,
            then: Then::Messages {
                count: 1,
                delta: -120,
            },
        },
        Row {
            name: "W4_zero_notches_no_message",
            notches: 0,
            then: Then::Messages { count: 0, delta: 0 },
        },
        Row {
            name: "W5_the_cap_1024",
            notches: 1024,
            then: Then::Messages {
                count: 1024,
                delta: 120,
            },
        },
        Row {
            name: "W6_1025_refused",
            notches: 1025,
            then: Then::Refused,
        },
        Row {
            name: "W6_minus_1025_refused",
            notches: -1025,
            then: Then::Refused,
        },
        Row {
            name: "W6_i32_max_refused",
            notches: i32::MAX,
            then: Then::Refused,
        },
        Row {
            name: "W6_i32_min_refused",
            notches: i32::MIN,
            then: Then::Refused,
        },
    ];

    let mut failures = Vec::new();
    for row in &rows {
        match (&row.then, WheelPlan::for_notches(row.notches)) {
            (Then::Messages { count, delta }, Ok(plan)) => {
                let messages = plan.messages();
                if messages.len() != *count || messages.iter().any(|message| message != delta) {
                    failures.push(format!(
                        "{}: expected {count} messages of {delta}, observed {} messages starting {:?}",
                        row.name,
                        messages.len(),
                        messages.iter().take(4).collect::<Vec<_>>()
                    ));
                }
            }
            (Then::Messages { count, delta }, Err(error)) => failures.push(format!(
                "{}: expected {count} messages of {delta}, observed a refusal: {error:?}",
                row.name
            )),
            (Then::Refused, Ok(plan)) => failures.push(format!(
                "{}: expected a refusal, observed a plan of {} messages",
                row.name,
                plan.messages().len()
            )),
            (Then::Refused, Err(ActionError::TargetInvalid { .. })) => {}
            (Then::Refused, Err(error)) => failures.push(format!(
                "{}: expected the cap refusal (TargetInvalid), observed {error:?}",
                row.name
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "{} checks of {} failed:\n{}",
        failures.len(),
        rows.len(),
        failures.join("\n")
    );
}
