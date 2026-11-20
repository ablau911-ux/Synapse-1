use super::{M3ServiceConfig, M3State, audio_event_sink};
use crate::http::sse::SseState;
use std::path::PathBuf;
use synapse_audio::{
    detectors::{DetectorProcessor, SharedDetectorState},
    ring::AudioFormat,
};
use synapse_core::{EventFilter, EventSource};
use synapse_reflex::DEFAULT_MAX_SUBSCRIPTIONS_NONZERO;
use tokio_util::sync::CancellationToken;

#[test]
fn must_pass_m3_state_from_config_reads_cli_shape() -> anyhow::Result<()> {
    println!(
        "readback=m3_state scenario=from_config before_db=db before_profile=profiles before_reflex=true"
    );
    let state = M3State::from_config_with_shutdown_reason_and_sse_state(
        M3ServiceConfig {
            db_path: Some(PathBuf::from("db")),
            profile_dir: Some(PathBuf::from("profiles")),
            reflex_disabled: true,
            bind: "127.0.0.1:7701".to_owned(),
            bearer_token: Some("token".to_owned()),
            max_subscriptions: DEFAULT_MAX_SUBSCRIPTIONS_NONZERO,
            enable_audio: false,
            allow_unknown_profile: false,
            allowed_permissions: None,
            reflex_force_degraded: false,
            storage_pressure_free_bytes_sample: None,
        },
        CancellationToken::new(),
        "sigint",
        Some(CancellationToken::new()),
        SseState::from_env(),
    )?;
    println!(
        "readback=m3_state scenario=from_config after_db={:?} after_profile={:?} after_reflex_disabled={} after_bind={} after_token_present={} after_connection_token={}",
        state.db_path,
        state.profile_dir,
        state.reflex_disabled,
        state.bind,
        state.bearer_token.is_some(),
        state.connection_closed_cancel.is_some()
    );
    assert!(state.reflex_disabled);
    assert!(state.scaffold_ready());
    assert_eq!(state.db_path, Some(PathBuf::from("db")));
    assert_eq!(state.profile_dir, Some(PathBuf::from("profiles")));
    assert_eq!(state.bind, "127.0.0.1:7701");
    assert_eq!(state.bearer_token.as_deref(), Some("token"));
    assert!(state.connection_closed_cancel.is_some());
    Ok(())
}

#[test]
fn must_fail_m3_state_rejects_invalid_reflex_disabled() {
    let before_reflex = Some("yes");
    println!("readback=m3_state scenario=invalid_reflex before={before_reflex:?}");
    let after = M3State::from_parts_with_sse_state(
        None,
        None,
        before_reflex,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        CancellationToken::new(),
        "shutdown",
        None,
        SseState::from_env(),
    );
    println!("readback=m3_state scenario=invalid_reflex after={after:?}");
    let Err(error) = after else {
        panic!("SYNAPSE_REFLEX_DISABLED=yes must be refused");
    };
    assert_eq!(
        error.to_string(),
        "SYNAPSE_REFLEX_DISABLED must be one of 1, 0, true, or false; got \"yes\""
    );
}

#[test]
fn must_pass_audio_event_sink_publishes_detector_events_to_shared_bus() -> anyhow::Result<()> {
    let sse_state = SseState::from_env();
    let event_bus = sse_state.event_bus();
    let subscriber = event_bus.subscribe(
        EventFilter::Source {
            source: EventSource::PerceptionAudio,
        },
        Vec::new(),
        false,
    )?;
    println!(
        "readback=audio_event_sink scenario=detector_bridge before_subscriber_len={}",
        subscriber.len()
    );

    let mut detector = DetectorProcessor::new(
        SharedDetectorState::default(),
        audio_event_sink(event_bus.clone()),
    );
    let format = AudioFormat {
        sample_rate_hz: 48_000,
        channels: 2,
    };
    detector.process(&vec![0.0; 480 * 2], format);
    detector.process(&vec![0.9; 480 * 2], format);

    let events = subscriber.drain();
    let kinds = events
        .iter()
        .map(|event| event.kind.as_str())
        .collect::<Vec<_>>();
    println!(
        "readback=audio_event_sink scenario=detector_bridge after_subscriber_len={} after_kinds={:?}",
        events.len(),
        kinds
    );

    assert!(
        events
            .iter()
            .all(|event| event.source == EventSource::PerceptionAudio)
    );
    assert!(kinds.contains(&"loud_transient"));
    assert!(kinds.contains(&"speech_started"));
    Ok(())
}

/// #270 shared helpers for the storage-pressure refusal tests.
#[path = "tests/pressure_shed_support_270.rs"]
pub(crate) mod pressure_shed_support_270;
