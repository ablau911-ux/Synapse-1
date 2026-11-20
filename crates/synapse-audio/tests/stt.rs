use std::path::PathBuf;

use synapse_audio::{
    AudioConfig, AudioFormat, AudioRuntime, AudioWindow, WhisperTinyStt,
    stt::WHISPER_TINY_INT8_SHA256,
};
use synapse_core::error_codes;

#[test]
fn must_fail_stt_missing_model_maps_audio_code() -> TestResult {
    let runtime = AudioRuntime::spawn(AudioConfig {
        stt_model_path: Some(PathBuf::from("missing-whisper-tiny-int8.onnx")),
        ..AudioConfig::default()
    })?;

    let err = match runtime.transcribe_file(fixture_path(), "en") {
        Ok(transcription) => {
            return Err(format!("missing model returned {transcription:?}").into());
        }
        Err(err) => err,
    };

    assert_eq!(err.code(), error_codes::AUDIO_STT_MODEL_NOT_LOADED);
    Ok(())
}

#[test]
fn must_fail_stt_corrupt_model_maps_hash_mismatch() -> TestResult {
    let temp = tempfile::tempdir()?;
    let model = temp.path().join("whisper-tiny-int8.onnx");
    std::fs::write(&model, b"not an onnx model")?;
    let runtime = AudioRuntime::spawn(AudioConfig {
        stt_model_path: Some(model),
        ..AudioConfig::default()
    })?;

    let err = match runtime.transcribe_file(fixture_path(), "en") {
        Ok(transcription) => {
            return Err(format!("corrupt model returned {transcription:?}").into());
        }
        Err(err) => err,
    };

    assert_eq!(err.code(), error_codes::MODEL_HASH_MISMATCH);
    assert!(err.to_string().contains(WHISPER_TINY_INT8_SHA256));
    Ok(())
}

#[test]
fn must_fail_empty_and_silence_windows_return_blank_without_model_load() -> TestResult {
    let stt = WhisperTinyStt::new(Some(PathBuf::from("missing-whisper-tiny-int8.onnx")));

    // Silence is covered by must_pass_silence_window_returns_blank_*; only the
    // empty-window edge is asserted here.
    let empty = stt.transcribe_window(&window(Vec::new(), 0), "en")?;

    assert_eq!(empty.text, "");
    assert!(empty.confidence.abs() <= f32::EPSILON);
    assert!(empty.audio_seconds.abs() <= f32::EPSILON);
    assert!(!stt.is_loaded());
    Ok(())
}

#[test]
fn must_pass_silence_window_returns_blank_transcription_with_window_metadata() -> TestResult {
    let model = PathBuf::from("missing-whisper-tiny-int8.onnx");
    let stt = WhisperTinyStt::new(Some(model.clone()));

    let silence = stt.transcribe_window(&window(vec![0.0; 16_000], 16_000), "en")?;

    assert_eq!(silence.text, "");
    assert_eq!(silence.language, "en");
    assert!((silence.audio_seconds - 1.0).abs() <= f32::EPSILON);
    assert_eq!(silence.elapsed_ms, 0);
    assert_eq!(silence.model_path, model);
    assert_eq!(stt.model_path(), model.as_path());
    assert!(silence.backend.is_none());
    assert!(silence.session_id.is_none());
    assert!(!stt.is_loaded());
    Ok(())
}

#[test]
#[ignore = "requires side-loaded whisper-tiny-int8.onnx in %LOCALAPPDATA%\\synapse\\models"]
fn must_pass_stt_side_loaded_model_transcribes_hello_world_fixture() -> TestResult {
    let runtime = AudioRuntime::spawn(AudioConfig::default())?;

    let transcription = runtime.transcribe_file(fixture_path(), "en")?;

    let text = transcription.text.to_ascii_lowercase();
    assert!(text.contains("hello"), "text={:?}", transcription.text);
    assert!(text.contains("world"), "text={:?}", transcription.text);
    assert_eq!(transcription.language, "en");
    assert!(
        (transcription.confidence - 0.86).abs() <= f32::EPSILON,
        "confidence={}",
        transcription.confidence
    );
    assert_eq!(
        transcription.model_path,
        synapse_audio::stt::default_model_path()
    );
    assert!(transcription.backend.is_some());
    assert!(transcription.session_id.is_some());
    Ok(())
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("fixtures")
        .join("audio")
        .join("hello_world_5s.wav")
}

#[allow(clippy::missing_const_for_fn)]
fn window(samples: Vec<f32>, frames: usize) -> AudioWindow {
    AudioWindow {
        format: AudioFormat {
            sample_rate_hz: 16_000,
            channels: 1,
        },
        frames,
        samples,
        rms_db: -120.0,
    }
}

#[test]
fn must_fail_silence_window_with_unsupported_language_is_refused_like_speech() {
    let stt = WhisperTinyStt::new(Some(PathBuf::from("missing-whisper-tiny-int8.onnx")));

    let refused = stt.transcribe_window(&window(vec![0.0; 16_000], 16_000), "fr");

    let error = match refused {
        Ok(transcription) => {
            panic!("silence must not bypass language validation: {transcription:?}")
        }
        Err(error) => error,
    };
    // Same refusal the speech path gives (validate_language): the crate's
    // invalid-input code, naming the language.
    assert_eq!(error.code(), error_codes::AUDIO_LOOPBACK_INIT_FAILED);
    assert!(
        error.to_string().contains("unsupported STT language `fr`"),
        "detail must name the refused language: {error}"
    );
    assert!(
        !stt.is_loaded(),
        "a refused request must not load the model"
    );
}

#[test]
fn must_pass_silence_window_with_blank_language_reports_the_default() -> TestResult {
    let stt = WhisperTinyStt::new(Some(PathBuf::from("missing-whisper-tiny-int8.onnx")));

    let silence = stt.transcribe_window(&window(vec![0.0; 16_000], 16_000), "  ")?;

    assert_eq!(silence.language, "en");
    Ok(())
}
