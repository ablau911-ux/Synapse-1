use synapse_core::KeystrokeDynamics;

use crate::sample_typing_schedule;

pub(super) const TEXT_VK_RETURN: u16 = 0x0D;
pub(super) const TEXT_VK_TAB: u16 = 0x09;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TextDispatchStep {
    pub(super) iki_ms_before: u32,
    pub(super) inputs: Vec<TextDispatchInput>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub(super) enum TextDispatchInput {
    UnicodeUnit(u16),
    VirtualKey(u16),
}

impl TextDispatchInput {}

pub(super) fn text_dispatch_plan(
    text: &str,
    dynamics: &KeystrokeDynamics,
) -> Vec<TextDispatchStep> {
    let mut previous = None;
    sample_typing_schedule(text, dynamics, None)
        .into_iter()
        .map(|event| {
            let inputs = if is_crlf_line_feed(previous, event.r#char) {
                // "\r\n" is one line break: its '\r' already pressed Enter.
                // The '\n' keeps its step (and its delay) but presses nothing.
                Vec::new()
            } else {
                dispatch_inputs(event.r#char)
            };
            previous = Some(event.r#char);
            TextDispatchStep {
                iki_ms_before: event.iki_ms_before,
                inputs,
            }
        })
        .collect()
}

/// True for the '\n' of a Windows "\r\n" line ending.
const fn is_crlf_line_feed(previous: Option<char>, ch: char) -> bool {
    ch == '\n' && matches!(previous, Some('\r'))
}

fn dispatch_inputs(ch: char) -> Vec<TextDispatchInput> {
    match ch {
        '\n' | '\r' => vec![TextDispatchInput::VirtualKey(TEXT_VK_RETURN)],
        '\t' => vec![TextDispatchInput::VirtualKey(TEXT_VK_TAB)],
        _ => {
            let mut units = [0; 2];
            ch.encode_utf16(&mut units)
                .iter()
                .copied()
                .map(TextDispatchInput::UnicodeUnit)
                .collect()
        }
    }
}

#[cfg(test)]
#[path = "tests/text_dispatch_plan.rs"]
mod plan_tests;
