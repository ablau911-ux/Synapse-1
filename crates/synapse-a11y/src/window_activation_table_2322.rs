//! #2322 frozen behaviour table (manager-owned, work method v3). Codex and
//! Opus never edit this file. One case per L row of the table in the #2322
//! report: a foreground restore never injects input; an operator focus keeps
//! the input nudge only as the last resort; a locked workstation tries nothing.

use super::{ActivationStep, ForegroundActivationIntent, activation_plan};

const RESTORE: ForegroundActivationIntent = ForegroundActivationIntent::LeaseContextRestore {
    caller: "act_click",
};
const RESTORE_NO_CALLER: ForegroundActivationIntent =
    ForegroundActivationIntent::LeaseContextRestore { caller: "" };
const OPERATOR: ForegroundActivationIntent = ForegroundActivationIntent::OperatorRequested {
    caller: "focus_window",
};

#[test]
fn window_activation_table_2322() {
    use ActivationStep::{AttachAndRaise, InputNudge, Plain};
    // (row, intent, locked, expected ordered steps)
    let rows: [(&str, ForegroundActivationIntent, bool, &[ActivationStep]); 5] = [
        (
            "2322a_restore_unlocked_is_plain_then_attach_no_nudge",
            RESTORE,
            false,
            &[Plain, AttachAndRaise],
        ),
        (
            "2322b_restore_hostile_empty_caller_still_no_nudge",
            RESTORE_NO_CALLER,
            false,
            &[Plain, AttachAndRaise],
        ),
        (
            "2322c_operator_unlocked_tries_input_free_steps_before_nudge",
            OPERATOR,
            false,
            &[Plain, AttachAndRaise, InputNudge],
        ),
        (
            "2322d_neighbour_restore_locked_tries_nothing",
            RESTORE,
            true,
            &[],
        ),
        (
            "2322e_neighbour_operator_locked_tries_nothing",
            OPERATOR,
            true,
            &[],
        ),
    ];
    let mut failures = Vec::new();
    for (row, intent, locked, expected) in rows {
        let got = activation_plan(intent, locked);
        if got != expected {
            failures.push(format!("row {row}: steps {got:?}, expected {expected:?}"));
        }
    }
    // A restore plan never contains a step that injects input.
    for (row, intent) in [
        ("2322f_restore_plan_injects_no_input", RESTORE),
        (
            "2322f_restore_plan_injects_no_input_empty_caller",
            RESTORE_NO_CALLER,
        ),
    ] {
        let injecting: Vec<_> = activation_plan(intent, false)
            .iter()
            .filter(|step| step.injects_input())
            .collect();
        if !injecting.is_empty() {
            failures.push(format!("row {row}: input-injecting steps {injecting:?}"));
        }
    }
    // Only the nudge injects input.
    for (row, step, expected) in [
        ("2322g_plain_injects_no_input", Plain, false),
        (
            "2322g_attach_and_raise_injects_no_input",
            AttachAndRaise,
            false,
        ),
        ("2322g_input_nudge_injects_input", InputNudge, true),
    ] {
        if step.injects_input() != expected {
            failures.push(format!(
                "row {row}: injects_input {}, expected {expected}",
                step.injects_input()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "#2322 table rows failed:\n{}",
        failures.join("\n")
    );
}
