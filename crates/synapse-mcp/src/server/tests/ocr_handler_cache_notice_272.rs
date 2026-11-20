//! criteria_b2.md:2. Replace only pixels; run the public read_text handler,
//! actual WinRT recognition, cache gate, serialization, and hygiene annotations.
use super::*;
use std::sync::Mutex;

static CAPTURE: Mutex<Option<CapturedOcrBitmap>> = Mutex::new(None);

pub(super) fn take_capture(region: Rect) -> Option<CapturedOcrBitmap> {
    let mut slot = CAPTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot
        .as_ref()
        .is_some_and(|bitmap| bitmap.capture_region == region)
    {
        slot.take()
    } else {
        None
    }
}

struct ResetCapture;
impl Drop for ResetCapture {
    fn drop(&mut self) {
        CAPTURE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

#[tokio::test]
async fn must_pass_read_text_handler_retains_dedicated_cache_write_notice_272() -> anyhow::Result<()>
{
    let _ocr = super::ocr_cache_best_effort_272::OCR_TEST_GATE.lock().await;
    let dir = tempfile::tempdir()?;
    let service = crate::test_support::service_in(dir.path())?;
    service.m1_state()?.synthetic = None;
    let image = image::load_from_memory(include_bytes!("ocr_pressure_272.png"))?.to_rgba8();
    let (width, height) = image.dimensions();
    let region = Rect {
        x: 272,
        y: 272,
        w: i32::try_from(width)?,
        h: i32::try_from(height)?,
    };
    let mut bytes = image.into_raw();
    for pixel in bytes.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let bitmap = CapturedOcrBitmap {
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
    };
    // Recognition precondition: a missing language pack cannot impersonate RED.
    let request = crate::m1::ResolvedReadTextRequest {
        region,
        capture_source: crate::m1::ReadTextCaptureSource::Screen,
        requested_backend: OcrBackend::Winrt,
        effective_backend: OcrBackend::Winrt,
        lang_hint: Some("en-US".into()),
        synthetic: false,
    };
    let expected = crate::m1::read_text_request_from_bgra(&request, &bitmap)?;
    assert_eq!(expected.full_text, "CACHE PRESSURE TEST");
    crate::ocr_cache_seal::seal(&serde_json::to_vec(&expected)?)
        .map_err(|error| anyhow::anyhow!("DPAPI prerequisite: {error}"))?;
    let runtime = service.reflex_runtime()?;
    let db = {
        let runtime = lock_reflex_runtime(&runtime)?;
        let db = runtime.storage_db();
        assert!(db.scan_cf(cf::CF_OCR_CACHE)?.is_empty());
        let report = runtime.storage_run_pressure_sample(0)?;
        assert_eq!(
            report.current_level,
            synapse_storage::DiskPressureLevel::Level4
        );
        assert!(!runtime.storage_pressure_permits_write(cf::CF_OCR_CACHE));
        db
    };
    let _reset = ResetCapture;
    *CAPTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(bitmap);
    let result = service
        .read_text(
            Parameters(serde_json::from_value(json!({
                "region":region, "backend":"winrt", "lang_hint":"en-US"
            }))?),
            crate::test_support::request_context_for_session("batch2-ocr").await,
        )
        .await;
    assert!(
        CAPTURE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none(),
        "handler must consume the capture seam"
    );
    assert!(
        db.scan_cf(cf::CF_OCR_CACHE)?.is_empty(),
        "shed write persisted a row"
    );
    let result = result?.0;
    assert_eq!(result.full_text, expected.full_text);
    assert_eq!(result.words, expected.words);
    let wire = serde_json::to_value(result)?;
    let dedicated = wire
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("OCR object"))?
        .iter()
        .filter(|(key, _)| {
            !matches!(
                key.as_str(),
                "full_text"
                    | "words"
                    | "perceived_text_notice"
                    | "suspected_injection"
                    | "error"
                    | "error_code"
            )
        })
        .any(|(_, value)| {
            value.get("code").and_then(Value::as_str) == Some(error_codes::STORAGE_WRITE_SHED)
                && value
                    .get("detail")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
        });
    assert!(
        dedicated,
        "dedicated non-error cache-write code/detail lost through hygiene: {wire}"
    );
    Ok(())
}
