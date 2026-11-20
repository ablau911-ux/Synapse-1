use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use synapse_core::{
    AccessibleNode, AudioContext, DetectedEntity, ElementId, Event, EventFilter, EventRef,
    EventSource, EventSummary, FocusedElement, ForegroundContext, Health, HudReading, HudReadings,
    HudValue, Observation, ObservationDiagnostics, PerceptionMode, Point, Rect, SensorStatus, Size,
    UiaPattern, element_id, entity_id,
};

#[test]
fn must_pass_geometry_json_shape() {
    insta::assert_json_snapshot!(Point { x: -10, y: 20 });
    insta::assert_json_snapshot!(Rect {
        x: -10,
        y: 20,
        w: 30,
        h: 40,
    });
    insta::assert_json_snapshot!(Size { w: 1920, h: 1080 });
}

#[test]
fn must_pass_observation_json_shape() {
    insta::assert_json_snapshot!(sample_observation());
}

#[test]
fn must_pass_event_json_shape() {
    insta::assert_json_snapshot!(sample_event());
    insta::assert_json_snapshot!(sample_event().summary());
}

#[test]
fn must_fail_geometry_json_with_unknown_missing_or_out_of_range_fields_is_rejected() {
    for (raw, reason) in [
        (r#"{"x":1,"y":2,"z":3}"#, "unknown field `z`"),
        (r#"{"x":1}"#, "missing field `y`"),
        (r#"{"x":1.5,"y":2}"#, "invalid type: floating point `1.5`"),
        (r#"{"x":"1","y":2}"#, "invalid type: string \"1\""),
    ] {
        assert_rejected(serde_json::from_str::<Point>(raw), reason, raw);
    }
    for (raw, reason) in [
        (
            r#"{"x":0,"y":0,"w":1,"h":1,"extra":true}"#,
            "unknown field `extra`",
        ),
        (r#"{"x":0,"y":0,"w":1}"#, "missing field `h`"),
        (r"{}", "missing field `x`"),
    ] {
        assert_rejected(serde_json::from_str::<Rect>(raw), reason, raw);
    }
    for (raw, reason) in [
        (r#"{"w":-1,"h":1}"#, "invalid value: integer `-1`"),
        (r#"{"w":1,"h":1,"d":1}"#, "unknown field `d`"),
        (
            r#"{"w":4294967296,"h":1}"#,
            "invalid value: integer `4294967296`",
        ),
    ] {
        assert_rejected(serde_json::from_str::<Size>(raw), reason, raw);
    }
}

/// A rejection must carry the named serde reason, so a different decode
/// failure cannot satisfy the case under test.
fn assert_rejected<T: std::fmt::Debug>(
    result: Result<T, serde_json::Error>,
    reason: &str,
    case: &str,
) {
    match result {
        Ok(decoded) => panic!("{case}: expected rejection `{reason}`, decoded {decoded:?}"),
        Err(err) => assert!(
            err.to_string().contains(reason),
            "{case}: expected `{reason}`, got `{err}`"
        ),
    }
}

#[test]
fn must_fail_health_json_with_unknown_or_missing_fields_is_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let valid = serde_json::to_value(Health {
        ok: true,
        version: "0.1.0".to_owned(),
        build: "dev".to_owned(),
        pid: 4321,
        uptime_s: 0,
        tool_count: 2,
        tool_surface_sha256: "0123456789abcdef".to_owned(),
        tool_names: vec!["act_run_shell".to_owned(), "health".to_owned()],
        subsystems: BTreeMap::new(),
    })?;
    assert!(serde_json::from_value::<Health>(valid.clone()).is_ok());

    let mut unknown = valid.clone();
    unknown["unexpected"] = serde_json::json!(true);
    assert_rejected(
        serde_json::from_value::<Health>(unknown),
        "unknown field `unexpected`",
        "health unknown",
    );

    let mut missing_pid = valid.clone();
    missing_pid
        .as_object_mut()
        .ok_or("health must serialize as an object")?
        .remove("pid");
    assert_rejected(
        serde_json::from_value::<Health>(missing_pid),
        "missing field `pid`",
        "health missing pid",
    );

    let mut negative_pid = valid;
    negative_pid["pid"] = serde_json::json!(-1);
    assert_rejected(
        serde_json::from_value::<Health>(negative_pid),
        "invalid value: integer `-1`",
        "health negative pid",
    );
    Ok(())
}

#[test]
fn must_fail_event_json_with_unknown_field_source_or_correlation_is_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let valid = serde_json::to_value(sample_event())?;
    assert_eq!(
        serde_json::from_value::<Event>(valid.clone())?,
        sample_event()
    );

    let mut unknown = valid.clone();
    unknown["unexpected"] = serde_json::json!(1);
    assert_rejected(
        serde_json::from_value::<Event>(unknown),
        "unknown field `unexpected`",
        "event unknown",
    );

    let mut miscased_source = valid.clone();
    miscased_source["source"] = serde_json::json!("PerceptionHud");
    assert_rejected(
        serde_json::from_value::<Event>(miscased_source),
        "unknown variant `PerceptionHud`",
        "event miscased source",
    );

    let mut unknown_source = valid.clone();
    unknown_source["source"] = serde_json::json!("not_a_source");
    assert_rejected(
        serde_json::from_value::<Event>(unknown_source),
        "unknown variant `not_a_source`",
        "event unknown source",
    );

    let mut bad_correlation = valid;
    bad_correlation["correlations"] =
        serde_json::json!([{"seq": 9, "relation": "supersedes", "extra": true}]);
    assert_rejected(
        serde_json::from_value::<Event>(bad_correlation),
        "unknown field `extra`",
        "event bad correlation",
    );
    Ok(())
}

#[test]
fn must_fail_event_filter_json_with_unknown_op_or_missing_field_is_rejected() {
    assert_eq!(
        serde_json::from_str::<EventFilter>(r#"{"op":"kind","kind":"x"}"#).ok(),
        Some(EventFilter::Kind {
            kind: "x".to_owned()
        })
    );
    for (raw, reason) in [
        (r#"{"op":"xor","args":[]}"#, "unknown variant `xor`"),
        (r#"{"op":"Kind","kind":"x"}"#, "unknown variant `Kind`"),
        (r#"{"op":"kind"}"#, "missing field `kind`"),
        (r#"{"kind":"x"}"#, "missing field `op`"),
        (
            r#"{"op":"data","path":"/x","predicate":{"op":"approx","value":1}}"#,
            "unknown variant `approx`",
        ),
        (
            r#"{"op":"source","source":"not_a_source"}"#,
            "unknown variant `not_a_source`",
        ),
    ] {
        assert_rejected(serde_json::from_str::<EventFilter>(raw), reason, raw);
    }
}

fn fixed_time() -> DateTime<Utc> {
    DateTime::<Utc>::from(std::time::UNIX_EPOCH + std::time::Duration::from_hours(494_304))
}

fn sample_event() -> Event {
    Event {
        seq: 10,
        at: fixed_time(),
        source: EventSource::PerceptionHud,
        kind: "hud-value-changed".to_owned(),
        data: serde_json::json!({
            "field": "hp",
            "old": 25,
            "new": 15,
            "confidence": 0.98
        }),
        correlations: vec![EventRef {
            seq: 9,
            relation: "supersedes".to_owned(),
        }],
    }
}

#[allow(clippy::too_many_lines)]
fn sample_observation() -> Observation {
    let at = fixed_time();
    let focused_id = element_id(0x12ab, "0a1b2c3d");
    let parent_id = ElementId::parse("0x12ab:ff")
        .unwrap_or_else(|err| panic!("literal element id should parse: {err}"));
    let mut hud = HudReadings::default();
    hud.by_name.insert(
        "hp".to_owned(),
        HudReading {
            raw_text: "15".to_owned(),
            parsed: HudValue::Number(15.0),
            confidence: 0.98,
            confidence_source: synapse_core::ConfidenceSource::Synthetic,
            stale_ms: 16,
        },
    );

    Observation {
        seq: 123,
        at,
        mode: PerceptionMode::Hybrid,
        foreground: ForegroundContext {
            hwnd: 0x12ab,
            pid: 4242,
            process_name: "notepad.exe".to_owned(),
            process_path: "C:\\Windows\\System32\\notepad.exe".to_owned(),
            window_title: "Untitled - Notepad".to_owned(),
            window_bounds: Rect {
                x: 0,
                y: 0,
                w: 800,
                h: 600,
            },
            monitor_index: 0,
            dpi_scale: 1.25,
            profile_id: Some("windows.notepad".to_owned()),
            steam_appid: None,
            is_fullscreen: false,
            is_dwm_composed: true,
        },
        perceived_text_notice: None,
        suspected_injection: Vec::new(),
        focused: Some(FocusedElement {
            element_id: focused_id.clone(),
            name: "Text Editor".to_owned(),
            role: "Edit".to_owned(),
            automation_id: Some("15".to_owned()),
            bbox: Rect {
                x: 10,
                y: 20,
                w: 600,
                h: 400,
            },
            enabled: true,
            patterns: vec![UiaPattern::Value, UiaPattern::Text],
            value: Some("hello".to_owned()),
            selected_text: None,
        }),
        elements: vec![AccessibleNode {
            element_id: focused_id,
            parent: Some(parent_id),
            name: "Text Editor".to_owned(),
            role: "Edit".to_owned(),
            automation_id: Some("15".to_owned()),
            value: None,
            bbox: Rect {
                x: 10,
                y: 20,
                w: 600,
                h: 400,
            },
            enabled: true,
            focused: true,
            patterns: vec![UiaPattern::Value, UiaPattern::Text],
            children_count: 0,
            depth: 1,
            control_state: None,
        }],
        entities: vec![DetectedEntity {
            entity_id: entity_id(7),
            track_id: 7,
            class_label: "cursor".to_owned(),
            bbox: Rect {
                x: 120,
                y: 130,
                w: 16,
                h: 24,
            },
            confidence: 0.75,
            first_seen_at: at,
            last_seen_at: at,
            velocity_px_per_s: Some((0.0, 0.0)),
        }],
        hud,
        audio: AudioContext::default(),
        recent_events: vec![EventSummary {
            seq: 10,
            at,
            source: EventSource::PerceptionHud,
            kind: "hud-value-changed".to_owned(),
            data_excerpt: serde_json::json!({"field": "hp", "new": 15}),
        }],
        clipboard_summary: None,
        fs_recent: Vec::new(),
        diagnostics: ObservationDiagnostics {
            assembled_in_ms: 4.5,
            sensor_latency_ms: BTreeMap::new(),
            a11y_enabled: true,
            pixel_enabled: true,
            audio_enabled: false,
            a11y_status: SensorStatus::Healthy,
            capture_status: SensorStatus::Healthy,
            detection_status: SensorStatus::Unavailable,
            audio_status: SensorStatus::Disabled,
            is_minimized: false,
            capture_config: None,
            capture_runtime: None,
            input_backends: None,
            cdp: None,
            web_path: None,
            elements_truncated: false,
            elements_page: None,
            entities_truncated: false,
            size_bytes: 2048,
            size_estimate_tokens: 512,
        },
    }
}
