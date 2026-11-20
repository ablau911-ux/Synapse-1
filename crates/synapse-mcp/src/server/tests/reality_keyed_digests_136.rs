//! Regression for #136: persisted `reality` compact state must not carry plain
//! (brute-forceable) SHA-256 digests of user-derived text (focused field value,
//! selected text, window title, element names, clipboard excerpt).

use std::fs;

use synapse_core::{AccessibleNode, ClipboardSummary, ElementId, FocusedElement, Rect, UiaPattern};
use synapse_perception::{ObservationInput, ObserveInclude};
use tempfile::TempDir;

use super::tests::{
    install_synthetic_input, service_with_profile_dir, synthetic_input, write_profile,
};
use super::*;

const SECRET_VALUE: &str = "4821";
const SECRET_SELECTION: &str = "hunter2";
const SECRET_TITLE: &str = "Bank - account 99887766";
const SECRET_NAME: &str = "Password field";
const SECRET_ELEMENT_NAME: &str = "Savings account 55443322";
const SECRET_CLIPBOARD: &str = "clipboard-secret-ZZ9";

fn plain_sha256(text: &str) -> String {
    hash_bytes(text.as_bytes())
}

fn input_with(value: &str) -> anyhow::Result<ObservationInput> {
    let mut input = synthetic_input(SECRET_TITLE);
    input.focused = Some(FocusedElement {
        element_id: ElementId::parse("0x1234:0001")?,
        name: SECRET_NAME.to_owned(),
        role: "Edit".to_owned(),
        automation_id: None,
        bbox: Rect {
            x: 1,
            y: 2,
            w: 3,
            h: 4,
        },
        enabled: true,
        patterns: vec![UiaPattern::Value],
        value: Some(value.to_owned()),
        selected_text: Some(SECRET_SELECTION.to_owned()),
    });
    Ok(input)
}

fn compact(value: &str) -> anyhow::Result<CompactRealityState> {
    let observation =
        ObservationAssembler::new().assemble(ObserveInclude::default(), input_with(value)?)?;
    Ok(compact_state(&observation)?)
}

/// `element_id` of the first `focused` object (a `CompactFocused`) found in a
/// stored row, wherever the row nests its compact state.
fn focused_element_id(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => map
            .get("focused")
            .and_then(|focused| focused.get("element_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .or_else(|| map.values().find_map(focused_element_id)),
        serde_json::Value::Array(items) => items.iter().find_map(focused_element_id),
        _ => None,
    }
}

fn keyed(text: &str) -> String {
    format!(
        "hmac-sha256:{}",
        crate::content_digest::keyed_text_digest_hex(text)
    )
}

#[test]
fn must_fail_compact_state_digests_are_keyed_not_plain_sha256() -> anyhow::Result<()> {
    let state = compact(SECRET_VALUE)?;
    let focused = state
        .focused
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("focused slot missing"))?;
    assert_eq!(
        focused.value_sha256.as_deref(),
        Some(keyed(SECRET_VALUE).as_str())
    );
    assert_eq!(
        focused.selected_text_sha256.as_deref(),
        Some(keyed(SECRET_SELECTION).as_str())
    );
    assert_eq!(
        focused.name_sha256.as_deref(),
        Some(keyed(SECRET_NAME).as_str())
    );
    assert_eq!(
        state.foreground.window_title_sha256.as_deref(),
        Some(keyed(SECRET_TITLE).as_str())
    );

    let persisted = serde_json::to_string(&state)?;
    for secret in [SECRET_VALUE, SECRET_SELECTION, SECRET_TITLE, SECRET_NAME] {
        assert!(
            !persisted.contains(&plain_sha256(secret)),
            "plain sha256 of {secret:?} must not be persisted"
        );
    }
    Ok(())
}

/// The stored rows (head + baseline) written by `reality_baseline` must carry
/// the keyed digest of every user-derived text and never its plain SHA-256:
/// focused value / selection / name, window title, an element-list name and the
/// clipboard excerpt.
#[tokio::test]
async fn must_fail_persisted_baseline_rows_carry_no_plain_sha256_of_user_text() -> anyhow::Result<()>
{
    let temp = TempDir::new()?;
    let profile_dir = temp.path().join("profiles");
    fs::create_dir_all(&profile_dir)?;
    write_profile(
        &profile_dir.join("synthetic.toml"),
        "synthetic",
        "single_player",
    )?;
    let service = service_with_profile_dir(temp.path(), &profile_dir)?;
    let mut input = input_with(SECRET_VALUE)?;
    // The capture keeps the focused element only if it is also in the element
    // list (`m1.rs` `input_limited_to_depth`: otherwise focus moves to the first
    // retained element and the secret value/selection are never observed). So
    // the focused node 0001 is listed alongside the named element 0002.
    input.elements = vec![
        AccessibleNode {
            element_id: ElementId::parse("0x1234:0001")?,
            parent: None,
            name: SECRET_NAME.to_owned(),
            role: "Edit".to_owned(),
            automation_id: None,
            value: Some(SECRET_VALUE.to_owned()),
            bbox: Rect {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
            },
            enabled: true,
            focused: true,
            patterns: vec![UiaPattern::Value],
            children_count: 0,
            depth: 1,
            control_state: None,
        },
        AccessibleNode {
            element_id: ElementId::parse("0x1234:0002")?,
            parent: None,
            name: SECRET_ELEMENT_NAME.to_owned(),
            role: "Button".to_owned(),
            automation_id: None,
            value: None,
            bbox: Rect {
                x: 5,
                y: 6,
                w: 7,
                h: 8,
            },
            enabled: true,
            focused: false,
            patterns: Vec::new(),
            children_count: 0,
            depth: 1,
            control_state: None,
        },
    ];
    input.clipboard_summary = Some(ClipboardSummary {
        formats: vec!["text".to_owned()],
        text_len: Some(20),
        text_excerpt: Some(SECRET_CLIPBOARD.to_owned()),
        redacted: false,
    });
    install_synthetic_input(&service, input)?;

    let response = service
        .reality_baseline(Parameters(RealityBaselineParams {
            profile_id: Some("synthetic".to_owned()),
            epoch_id: Some("epoch-136".to_owned()),
            force_new_epoch: true,
            include: vec![
                crate::m1::ObserveSlot::Focused,
                crate::m1::ObserveSlot::Elements,
                crate::m1::ObserveSlot::Clipboard,
            ],
            depth: DEFAULT_DEPTH,
            max_elements: DEFAULT_MAX_ELEMENTS,
        }))
        .await?
        .0;
    assert_eq!(response.profile_key, "synthetic");

    let mut rows = Vec::new();
    {
        let runtime = service.reflex_runtime()?;
        let runtime = runtime
            .lock()
            .map_err(|_| anyhow::anyhow!("reflex runtime lock poisoned"))?;
        for key in [
            head_key("synthetic"),
            baseline_row_key("synthetic", "epoch-136"),
        ] {
            let bytes = runtime
                .storage_kv_row(key.as_bytes())?
                .ok_or_else(|| anyhow::anyhow!("stored row {key} missing"))?;
            rows.push((key, String::from_utf8(bytes)?));
        }
    }

    let head = &rows[0].1;
    // Fixture precondition readback: the stored capture kept focus on 0001,
    // the node carrying the secret value and selection.
    let head_json: serde_json::Value = serde_json::from_str(head)?;
    assert_eq!(
        focused_element_id(&head_json).as_deref(),
        Some("0x1234:0001"),
        "precondition: the persisted capture is focused on the secret field"
    );
    // Contract: `content_digest.rs` module doc + #136 — persisted digests of
    // observed user text are keyed (`hmac-sha256:`), never plain SHA-256.
    for secret in [
        SECRET_VALUE,
        SECRET_SELECTION,
        SECRET_TITLE,
        SECRET_NAME,
        SECRET_ELEMENT_NAME,
        SECRET_CLIPBOARD,
    ] {
        assert!(
            head.contains(&keyed(secret)),
            "head row must carry the keyed digest of {secret:?}"
        );
        for (key, row) in &rows {
            assert!(
                !row.contains(&plain_sha256(secret)),
                "row {key} carries the plain sha256 of {secret:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn must_pass_keyed_digests_keep_equality_semantics() -> anyhow::Result<()> {
    let first = compact(SECRET_VALUE)?;
    let same = compact(SECRET_VALUE)?;
    let other = compact("4822")?;
    let value = |state: &CompactRealityState| {
        state
            .focused
            .as_ref()
            .and_then(|focused| focused.value_sha256.clone())
    };
    assert_eq!(value(&first), value(&same));
    assert_ne!(value(&first), value(&other));
    assert_eq!(hash_json(&first)?, hash_json(&same)?);
    assert_ne!(hash_json(&first)?, hash_json(&other)?);
    Ok(())
}

#[test]
fn must_fail_empty_and_whitespace_values_have_no_digest() -> anyhow::Result<()> {
    let state = compact("   ")?;
    let focused = state
        .focused
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("focused slot missing"))?;
    assert_eq!(focused.value_sha256, None);
    let padded = compact(" 4821 ")?;
    assert_eq!(
        padded
            .focused
            .as_ref()
            .and_then(|focused| focused.value_sha256.as_deref()),
        Some(keyed(SECRET_VALUE).as_str())
    );
    Ok(())
}

#[test]
fn must_fail_source_ref_summary_does_not_leak_plain_title_hash() -> anyhow::Result<()> {
    let observation =
        ObservationAssembler::new().assemble(ObserveInclude::default(), input_with("x")?)?;
    let refs = source_refs_for_observation(&observation)?;
    let plain = plain_sha256(SECRET_TITLE);
    assert!(refs.iter().all(|source| !source.summary.contains(&plain)));
    assert!(
        refs.iter()
            .any(|source| source.summary.contains(&keyed(SECRET_TITLE)))
    );
    Ok(())
}
