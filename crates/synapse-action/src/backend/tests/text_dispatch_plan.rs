//! The production text path (the Windows `SoftwareBackend` `TypeText`) turns
//! text into input steps with `text_dispatch_plan`: one `KEYEVENTF_UNICODE`
//! unit per UTF-16 code unit, and only `VK_RETURN` / `VK_TAB` as virtual keys.
//! It never presses Shift or any other modifier. These tests pin that plan
//! directly, instead of the `RecordingBackend` key expansion (a test double
//! whose Shift-wrapping no production backend performs).

use proptest::{
    collection::vec,
    prelude::*,
    test_runner::{Config, TestCaseError, TestRng, TestRunner},
};
use synapse_core::{KeystrokeDynamics, KeystrokeNaturalParams};

use super::{TEXT_VK_RETURN, TEXT_VK_TAB, TextDispatchInput, TextDispatchStep, text_dispatch_plan};
use crate::sample_typing_schedule;

/// Windows `VK_SHIFT`; the plan must never contain it (or any other modifier).
const VK_SHIFT: u16 = 0x10;

fn unit(ch: char) -> TextDispatchInput {
    let mut units = [0_u16; 2];
    let encoded = ch.encode_utf16(&mut units);
    assert_eq!(encoded.len(), 1, "{ch:?} is not a single BMP unit");
    TextDispatchInput::UnicodeUnit(encoded[0])
}

fn burst_inputs(text: &str) -> Vec<Vec<TextDispatchInput>> {
    let plan = text_dispatch_plan(text, &KeystrokeDynamics::Burst);
    assert!(
        plan.iter().all(|step| step.iki_ms_before == 0),
        "burst dynamics must plan no delay: {plan:?}"
    );
    plan.into_iter().map(|step| step.inputs).collect()
}

/// Decodes a plan back to the text the target receives: unicode units are
/// UTF-16, `VK_RETURN` types a line break, `VK_TAB` a tab.
fn decode_plan(plan: &[TextDispatchStep]) -> Result<String, String> {
    let mut output = String::new();
    let mut pending = Vec::new();
    for input in plan.iter().flat_map(|step| step.inputs.iter()) {
        match *input {
            TextDispatchInput::UnicodeUnit(code_unit) => pending.push(code_unit),
            TextDispatchInput::VirtualKey(vkey) => {
                output.push_str(&String::from_utf16(&pending).map_err(|error| error.to_string())?);
                pending.clear();
                match vkey {
                    TEXT_VK_RETURN => output.push('\n'),
                    TEXT_VK_TAB => output.push('\t'),
                    other => return Err(format!("unexpected virtual key 0x{other:02X}")),
                }
            }
        }
    }
    output.push_str(&String::from_utf16(&pending).map_err(|error| error.to_string())?);
    Ok(output)
}

fn has_virtual_key(plan: &[TextDispatchStep], vkey: u16) -> bool {
    plan.iter()
        .flat_map(|step| step.inputs.iter())
        .any(|input| *input == TextDispatchInput::VirtualKey(vkey))
}

#[test]
fn must_pass_known_mixed_case_plan_is_one_unicode_unit_per_char_without_shift() {
    assert_eq!(
        burst_inputs("aA!"),
        vec![vec![unit('a')], vec![unit('A')], vec![unit('!')]],
        "upper case and shifted symbols are typed as unicode units, not Shift+key"
    );
}

#[test]
fn must_fail_lowercase_plan_never_contains_a_modifier_key() {
    let plan = text_dispatch_plan("abc", &KeystrokeDynamics::Burst);
    assert!(!has_virtual_key(&plan, VK_SHIFT), "plan={plan:?}");
    assert_eq!(
        burst_inputs("abc"),
        vec![vec![unit('a')], vec![unit('b')], vec![unit('c')]]
    );
}

#[test]
fn must_pass_consecutive_uppercase_plan_is_one_unicode_unit_per_char() {
    assert_eq!(
        burst_inputs("AB"),
        vec![vec![unit('A')], vec![unit('B')]],
        "no modifier is held across or between upper-case characters"
    );
}

#[test]
fn must_pass_random_mixed_case_plans_decode_back_without_shift_1000()
-> Result<(), Box<dyn std::error::Error>> {
    let config = Config {
        cases: 1_000,
        failure_persistence: None,
        ..Config::default()
    };
    let algorithm = config.rng_algorithm;
    let mut runner = TestRunner::new_with_rng(config, TestRng::deterministic_rng(algorithm));
    let strategy = vec(
        prop_oneof![
            prop::char::range('a', 'z'),
            prop::char::range('A', 'Z'),
            prop::char::range('!', '/'),
        ],
        1..=50,
    )
    .prop_map(|chars| chars.into_iter().collect::<String>());

    runner.run(&strategy, |text| {
        let plan = text_dispatch_plan(&text, &KeystrokeDynamics::Burst);
        prop_assert_eq!(plan.len(), text.chars().count(), "input={:?}", text);
        prop_assert!(
            plan.iter()
                .flat_map(|step| step.inputs.iter())
                .all(|input| matches!(input, TextDispatchInput::UnicodeUnit(_))),
            "input={:?} plan={:?}",
            text,
            plan
        );
        let decoded = decode_plan(&plan).map_err(TestCaseError::fail)?;
        prop_assert_eq!(&decoded, &text);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn must_fail_empty_text_plans_zero_steps() {
    let plan = text_dispatch_plan("", &KeystrokeDynamics::Burst);
    assert!(plan.is_empty(), "plan={plan:?}");
    assert_eq!(decode_plan(&plan), Ok(String::new()));
}

#[test]
fn must_pass_extended_latin_astral_and_control_keys_plan_exactly() {
    // 'Ā' U+0100, '😀' U+1F600 (surrogate pair D83D DE00), '𝄞' U+1D11E (D834 DD1E).
    let text = "Az \u{00C0}\u{0100}\u{017F}\t\u{1F600}\n\u{1D11E}";
    assert_eq!(
        burst_inputs(text),
        vec![
            vec![unit('A')],
            vec![unit('z')],
            vec![unit(' ')],
            vec![TextDispatchInput::UnicodeUnit(0x00C0)],
            vec![TextDispatchInput::UnicodeUnit(0x0100)],
            vec![TextDispatchInput::UnicodeUnit(0x017F)],
            vec![TextDispatchInput::VirtualKey(TEXT_VK_TAB)],
            vec![
                TextDispatchInput::UnicodeUnit(0xD83D),
                TextDispatchInput::UnicodeUnit(0xDE00),
            ],
            vec![TextDispatchInput::VirtualKey(TEXT_VK_RETURN)],
            vec![
                TextDispatchInput::UnicodeUnit(0xD834),
                TextDispatchInput::UnicodeUnit(0xDD1E),
            ],
        ]
    );
    let plan = text_dispatch_plan(text, &KeystrokeDynamics::Burst);
    assert_eq!(decode_plan(&plan).as_deref(), Ok(text));
}

/// EXPECT FAIL (owner decision): a Windows "\r\n" line ending is ONE line break
/// for the user, so it must press Enter once. The plan maps '\r' and '\n' each
/// to `VK_RETURN`, so pasted CRLF text gets a blank line after every line.
#[test]
fn must_pass_crlf_line_ending_is_one_enter_press() {
    let plan = text_dispatch_plan("a\r\nb", &KeystrokeDynamics::Burst);
    let returns = plan
        .iter()
        .flat_map(|step| step.inputs.iter())
        .filter(|input| **input == TextDispatchInput::VirtualKey(TEXT_VK_RETURN))
        .count();
    assert_eq!(returns, 1, "CRLF must be one Enter press: plan={plan:?}");
    assert_eq!(decode_plan(&plan).as_deref(), Ok("a\nb"));
}

#[test]
fn must_pass_random_strings_round_trip_through_the_dispatch_plan_10k()
-> Result<(), Box<dyn std::error::Error>> {
    let config = Config {
        cases: 10_000,
        failure_persistence: None,
        ..Config::default()
    };
    let algorithm = config.rng_algorithm;
    let mut runner = TestRunner::new_with_rng(config, TestRng::deterministic_rng(algorithm));
    // Every char except '\r' (whose line-ending contract is pinned above).
    let strategy = vec(
        prop_oneof![
            prop::char::range('\u{0000}', '\u{000C}'),
            prop::char::range('\u{000E}', '\u{007F}'),
            prop::char::range('\u{00C0}', '\u{017F}'),
            prop::char::range('\u{1F600}', '\u{1F64F}'),
        ],
        0..=200,
    )
    .prop_map(|chars| chars.into_iter().collect::<String>());
    let dynamics = KeystrokeDynamics::Natural {
        params: KeystrokeNaturalParams::FAST,
    };

    runner.run(&strategy, |text| {
        let plan = text_dispatch_plan(&text, &dynamics);
        let schedule = sample_typing_schedule(&text, &dynamics, None);
        prop_assert_eq!(plan.len(), schedule.len(), "input={:?}", text);
        for (step, event) in plan.iter().zip(&schedule) {
            prop_assert_eq!(step.iki_ms_before, event.iki_ms_before, "input={:?}", text);
            prop_assert_eq!(
                step.inputs.len(),
                if matches!(event.r#char, '\n' | '\t') {
                    1
                } else {
                    event.r#char.len_utf16()
                },
                "input={:?} char={:?}",
                text,
                event.r#char
            );
        }
        prop_assert!(!has_virtual_key(&plan, VK_SHIFT), "input={:?}", text);
        let decoded = decode_plan(&plan).map_err(TestCaseError::fail)?;
        prop_assert_eq!(&decoded, &text);
        Ok(())
    })?;
    Ok(())
}
