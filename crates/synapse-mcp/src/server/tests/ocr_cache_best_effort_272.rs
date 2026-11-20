//! #272: exercise the real OCR/cache call site, never the synthetic bypass.
use anyhow::{Context, ensure};
use std::time::{Duration, Instant};
use synapse_core::{OcrBackend, Rect, error_codes};
use synapse_storage::{DiskPressureLevel, cf};

use super::{CapturedOcrBitmap, SynapseService, lock_reflex_runtime};
use crate::m1::{ReadTextCaptureSource, ResolvedReadTextRequest};

// WinRT's shared OCR engine permits only one RecognizeAsync operation at a time.
// Also held by the public-handler test across its recognition and read_text call.
pub(crate) static OCR_TEST_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn bitmap() -> anyhow::Result<CapturedOcrBitmap> {
    let image = image::load_from_memory(include_bytes!("ocr_pressure_272.png"))?.to_rgba8();
    let (width, height) = image.dimensions();
    let region = Rect {
        x: 0,
        y: 0,
        w: i32::try_from(width)?,
        h: i32::try_from(height)?,
    };
    let mut bytes = image.into_raw();
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(CapturedOcrBitmap {
        bitmap: synapse_capture::CapturedBgraBitmap {
            region,
            width,
            height,
            bytes,
        },
        capture_source: "screen",
        capture_backend: "test_fixture",
        capture_hwnd: None,
        capture_region: region,
    })
}

fn request(region: Rect) -> ResolvedReadTextRequest {
    ResolvedReadTextRequest {
        region,
        capture_source: ReadTextCaptureSource::Screen,
        requested_backend: OcrBackend::Winrt,
        effective_backend: OcrBackend::Winrt,
        lang_hint: Some("en-US".to_owned()),
        synthetic: false,
    }
}

fn check_shed(free_bytes: u64, level: DiskPressureLevel) -> anyhow::Result<()> {
    let _ocr = OCR_TEST_GATE.blocking_lock();
    let dir = tempfile::tempdir()?;
    let service: SynapseService = super::tests::service_with_temp_db(dir.path())?;
    let captured = bitmap()?;
    let request = request(captured.capture_region);
    // Prove recognition and sealing work before introducing pressure. Otherwise
    // OCR_NO_TEXT or a DPAPI fallback could mask the cache-write branch.
    let expected = crate::m1::read_text_request_from_bgra(&request, &captured)
        .map_err(|error| anyhow::anyhow!("fixture OCR precondition: {error:?}"))?;
    ensure!(
        expected.full_text == "CACHE PRESSURE TEST",
        "fixture OCR: {expected:?}"
    );
    crate::ocr_cache_seal::seal(&serde_json::to_vec(&expected)?)
        .map_err(|error| anyhow::anyhow!("DPAPI precondition: {error}"))?;
    let runtime = service
        .reflex_runtime()
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let db = {
        let runtime =
            lock_reflex_runtime(&runtime).map_err(|error| anyhow::anyhow!("{error:?}"))?;
        let db = runtime.storage_db();
        ensure!(
            db.scan_cf(cf::CF_OCR_CACHE)?.is_empty(),
            "cache must start empty"
        );
        // Force the sample and observe the actual level, rather than assuming
        // a delay is enough for pressure setup to finish.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let report = runtime.storage_run_pressure_sample(free_bytes)?;
            if report.current_level == level && runtime.storage_pressure_level() == level {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "pressure precondition: expected {level:?}, observed {:?}",
                runtime.storage_pressure_level()
            );
            std::thread::yield_now();
        }
        ensure!(!runtime.storage_pressure_permits_write(cf::CF_OCR_CACHE));
        db
    };

    let actual = service.read_text_request_with_captured_bitmap(request, captured);
    // Separate physical readback, including when the call returns an error.
    ensure!(
        db.scan_cf(cf::CF_OCR_CACHE)?.is_empty(),
        "shed cache write persisted a row"
    );
    let actual = actual.map_err(|error| {
        anyhow::anyhow!("#272: successful OCR must survive cache shedding, got {error:?}")
    })?;
    ensure!(actual.full_text == expected.full_text && actual.words == expected.words);
    ensure!(actual.region == expected.region && actual.lang == expected.lang);
    // Do not prescribe a new response field: accept the code in any non-error
    // metadata field while requiring the successful OCR payload above.
    let mut metadata = serde_json::to_value(&actual)?;
    let fields = metadata.as_object_mut().context("OCR response object")?;
    fields.remove("full_text");
    fields.remove("words");
    fields.remove("error");
    fields.remove("error_code");
    ensure!(
        metadata
            .to_string()
            .contains(error_codes::STORAGE_WRITE_SHED),
        "#272: successful result must explain skipped cache write: {metadata}"
    );
    Ok(())
}

#[test]
fn must_pass_ocr_text_survives_level3_cache_shed_272() -> anyhow::Result<()> {
    check_shed(300_000_000, DiskPressureLevel::Level3)
}

#[test]
fn must_pass_ocr_text_survives_level4_cache_shed_272() -> anyhow::Result<()> {
    check_shed(100_000_000, DiskPressureLevel::Level4)
}
