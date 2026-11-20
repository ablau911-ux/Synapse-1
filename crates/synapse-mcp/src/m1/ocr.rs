use rmcp::ErrorData;
use synapse_core::{ConfidenceSource, OcrBackend, OcrResult, OcrWord, Rect, error_codes};
use synapse_perception::{TextRegion, read_text as platform_read_text, read_text_with_provider};

use crate::m1::{M1State, ReadTextParams, current_input, element_window_crop, mcp_error};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadTextCaptureSource {
    Screen,
    Window {
        hwnd: i64,
        window_region: Rect,
    },
    /// OCR the entire window using the captured WGC frame's native dimensions.
    /// Used when `window_hwnd` is supplied with no region/element target.
    WholeWindow {
        hwnd: i64,
    },
}

#[derive(Clone, Debug)]
pub struct ResolvedReadTextRequest {
    pub region: Rect,
    pub capture_source: ReadTextCaptureSource,
    pub requested_backend: OcrBackend,
    pub effective_backend: OcrBackend,
    pub lang_hint: Option<String>,
    pub synthetic: bool,
}

impl ResolvedReadTextRequest {
    #[must_use]
    pub fn lang(&self) -> String {
        #[cfg(windows)]
        let engine_language = if self.synthetic {
            None
        } else {
            synapse_perception::ocr_engine_language()
        };
        #[cfg(not(windows))]
        let engine_language: Option<String> = None;
        self.reported_lang(engine_language.as_deref())
    }

    /// #314: the language the reply names. The OCR engine's own language when
    /// it is known (the words were read in that language, whatever the hint
    /// said); otherwise the caller's hint; otherwise `und`.
    #[must_use]
    pub fn reported_lang(&self, engine_language: Option<&str>) -> String {
        engine_language
            .map(str::trim)
            .filter(|lang| !lang.is_empty())
            .or_else(|| {
                self.lang_hint
                    .as_deref()
                    .map(str::trim)
                    .filter(|lang| !lang.is_empty())
            })
            .unwrap_or("und")
            .to_owned()
    }
}

fn validate_winrt_language(hint: Option<&str>, backend: OcrBackend) -> Result<(), ErrorData> {
    let Some(tag) = hint.map(str::trim).filter(|tag| !tag.is_empty()) else {
        return Ok(());
    };
    if backend != OcrBackend::Winrt {
        return Ok(());
    }
    #[cfg(windows)]
    {
        use windows::{Globalization::Language, Media::Ocr::OcrEngine, core::HSTRING};
        let language = Language::CreateLanguage(&HSTRING::from(tag)).map_err(|error| {
            mcp_error(
                error_codes::OCR_LANGUAGE_UNSUPPORTED,
                format!("invalid WinRT OCR language {tag:?}: {error}"),
            )
        })?;
        let supported = OcrEngine::IsLanguageSupported(&language)
            .map_err(|error| mcp_error(error_codes::OCR_BACKEND_UNAVAILABLE, error.to_string()))?;
        if !supported {
            return Err(mcp_error(
                error_codes::OCR_LANGUAGE_UNSUPPORTED,
                format!("WinRT OCR recognizer is not installed for language {tag:?}"),
            ));
        }
    }
    #[cfg(not(windows))]
    let _ = tag;
    Ok(())
}

pub fn resolve_read_text_request(
    state: &M1State,
    params: &ReadTextParams,
    target_hwnd: Option<i64>,
) -> Result<ResolvedReadTextRequest, ErrorData> {
    validate_winrt_language(
        params.lang_hint.as_deref(),
        effective_ocr_backend(params.backend)?,
    )?;
    let (region, capture_source) = text_region(state, params, target_hwnd)?;
    if !matches!(capture_source, ReadTextCaptureSource::WholeWindow { .. }) {
        validate_ocr_region(region)?;
    }
    Ok(ResolvedReadTextRequest {
        region,
        capture_source,
        requested_backend: params.backend,
        effective_backend: effective_ocr_backend(params.backend)?,
        lang_hint: params.lang_hint.clone(),
        synthetic: state.synthetic.is_some(),
    })
}

pub fn read_text_request_uncached(
    request: &ResolvedReadTextRequest,
) -> Result<OcrResult, ErrorData> {
    validate_winrt_language(request.lang_hint.as_deref(), request.effective_backend)?;
    if request.synthetic {
        let provider = SyntheticOcrProvider {
            region: request.region,
        };
        let words = read_text_with_provider(&provider, request.region)
            .map_err(|err| mcp_error(err.code(), err.to_string()))?;
        return Ok(ocr_result_from_text_regions(words, request));
    }
    match request.effective_backend {
        OcrBackend::Winrt => {
            let words = platform_read_text(request.region)
                .map_err(|err| mcp_error(err.code(), err.to_string()))?;
            Ok(ocr_result_from_text_regions(words, request))
        }
        OcrBackend::Crnn => Err(crnn_unavailable_error()),
        OcrBackend::Auto => Err(mcp_error(
            error_codes::OCR_BACKEND_UNAVAILABLE,
            "internal OCR backend resolution left backend=auto after request validation",
        )),
    }
}

#[cfg(windows)]
pub fn read_text_request_from_bgra(
    request: &ResolvedReadTextRequest,
    captured: &synapse_capture::CapturedBgraBitmap,
) -> Result<OcrResult, ErrorData> {
    validate_winrt_language(request.lang_hint.as_deref(), request.effective_backend)?;
    if request.synthetic {
        return read_text_request_uncached(request);
    }
    let request = read_text_request_for_captured_bitmap(request.clone(), captured)?;
    match request.effective_backend {
        OcrBackend::Winrt => {
            let words = synapse_perception::read_text_from_bgra_bitmap(
                request.region,
                captured.width,
                captured.height,
                &captured.bytes,
            )
            .map_err(|err| mcp_error(err.code(), err.to_string()))?;
            Ok(ocr_result_from_text_regions(words, &request))
        }
        OcrBackend::Crnn => Err(crnn_unavailable_error()),
        OcrBackend::Auto => Err(mcp_error(
            error_codes::OCR_BACKEND_UNAVAILABLE,
            "internal OCR backend resolution left backend=auto after request validation",
        )),
    }
}

#[cfg(windows)]
pub fn read_text_request_for_captured_bitmap(
    mut request: ResolvedReadTextRequest,
    captured: &synapse_capture::CapturedBgraBitmap,
) -> Result<ResolvedReadTextRequest, ErrorData> {
    if matches!(
        request.capture_source,
        ReadTextCaptureSource::WholeWindow { .. }
    ) {
        request.region = Rect {
            x: 0,
            y: 0,
            w: i32::try_from(captured.width).map_err(|_| {
                mcp_error(
                    error_codes::OCR_NO_TEXT,
                    format!(
                        "whole-window OCR bitmap width {} exceeds i32",
                        captured.width
                    ),
                )
            })?,
            h: i32::try_from(captured.height).map_err(|_| {
                mcp_error(
                    error_codes::OCR_NO_TEXT,
                    format!(
                        "whole-window OCR bitmap height {} exceeds i32",
                        captured.height
                    ),
                )
            })?,
        };
        validate_ocr_region(request.region)?;
    }
    Ok(request)
}

/// Runs WinRT OCR over a web element's captured BGRA bitmap and returns an
/// `OcrResult` whose word boxes are relative to the captured element (#703).
///
/// Used by the `read_text` handler when `element_id` is a CDP/web node, which
/// the UIA element-bounds path cannot resolve. The bitmap comes from a CDP
/// element-clipped screenshot, so the OCR region is the whole bitmap.
///
/// # Errors
///
/// `OCR_NO_TEXT` if the bitmap dimensions exceed `i32` or OCR finds no text;
/// any `WinRT` OCR backend error from `read_text_from_bgra_bitmap`.
#[cfg(windows)]
pub fn ocr_result_from_web_bitmap(
    width: u32,
    height: u32,
    bgra: &[u8],
    lang_hint: Option<&str>,
) -> Result<OcrResult, ErrorData> {
    validate_winrt_language(lang_hint, OcrBackend::Winrt)?;
    let w = i32::try_from(width).map_err(|_| {
        mcp_error(
            error_codes::OCR_NO_TEXT,
            format!("web element OCR bitmap width {width} exceeds i32"),
        )
    })?;
    let h = i32::try_from(height).map_err(|_| {
        mcp_error(
            error_codes::OCR_NO_TEXT,
            format!("web element OCR bitmap height {height} exceeds i32"),
        )
    })?;
    let region = Rect { x: 0, y: 0, w, h };
    let words = synapse_perception::read_text_from_bgra_bitmap(region, width, height, bgra)
        .map_err(|err| mcp_error(err.code(), err.to_string()))?;
    let request = ResolvedReadTextRequest {
        region,
        capture_source: ReadTextCaptureSource::Screen,
        requested_backend: OcrBackend::Auto,
        effective_backend: OcrBackend::Winrt,
        lang_hint: lang_hint.map(str::to_owned),
        synthetic: false,
    };
    Ok(ocr_result_from_text_regions(words, &request))
}

fn text_region(
    state: &M1State,
    params: &ReadTextParams,
    target_hwnd: Option<i64>,
) -> Result<(Rect, ReadTextCaptureSource), ErrorData> {
    let target_hwnd = params.window_hwnd.or(target_hwnd);
    if let Some(region) = params.region {
        return match target_hwnd {
            Some(hwnd) => target_window_region_capture_source(hwnd, region),
            None => Ok((region, ReadTextCaptureSource::Screen)),
        };
    }
    if let Some(element_id) = &params.element_id {
        if state.synthetic.is_none() {
            let region = synapse_a11y::element_bounding_rect(element_id).map_err(|err| {
                mcp_error(
                    error_codes::OCR_NO_TEXT,
                    format!("element_id has no live visible OCR region: {err}"),
                )
            })?;
            // #178: capture the element's own window and crop, never the screen pixels
            // at the element rect (a covering window would be read instead).
            return element_window_capture_source(element_id, region);
        }
        let input = current_input(state, 2)?;
        let region = input
            .elements
            .iter()
            .find(|node| &node.element_id == element_id)
            .map(|node| node.bbox)
            .ok_or_else(|| {
                mcp_error(
                    error_codes::OCR_NO_TEXT,
                    "element_id has no visible OCR region",
                )
            })?;
        return Ok((region, ReadTextCaptureSource::Screen));
    }

    default_text_region(state, target_hwnd)
}

fn default_text_region(
    state: &M1State,
    target_hwnd: Option<i64>,
) -> Result<(Rect, ReadTextCaptureSource), ErrorData> {
    let Some(hwnd) = target_hwnd else {
        let input = current_input(state, 2)?;
        let focused = input.focused.ok_or_else(|| {
            mcp_error(
                error_codes::OCR_NO_TEXT,
                "read_text requires region, element_id, or a focused element with a visible OCR region",
            )
        })?;
        // #255: focus does not authorize reading pixels from a covering window.
        return element_window_capture_source(&focused.element_id, focused.bbox);
    };

    fail_if_minimized_target_needs_window_capture(hwnd)?;
    Ok((
        Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        },
        ReadTextCaptureSource::WholeWindow { hwnd },
    ))
}

fn validate_ocr_region(region: Rect) -> Result<(), ErrorData> {
    if region.w <= 0 || region.h <= 0 {
        return Err(mcp_error(
            error_codes::OCR_NO_TEXT,
            format!(
                "read_text OCR region must be non-empty: bbox=({}, {}, {}, {})",
                region.x, region.y, region.w, region.h
            ),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn element_capture_root_hwnd(hwnd: i64) -> Result<i64, ErrorData> {
    synapse_a11y::top_level_root_hwnd(hwnd).map_err(|error| {
        mcp_error(
            error_codes::TARGET_WINDOW_NOT_FOUND,
            format!("read_text could not resolve root of element window {hwnd:#x}: {error}"),
        )
    })
}

/// Resolves a UIA element's screen rectangle into a capture of its top-level root window,
/// cropped relative to the root frame (#178, #256). Screen-pixel capture would read any window covering the
/// element, so there is no screen fallback: when the window cannot be captured or does not
/// contain the element, the request fails with a precise error.
#[cfg(windows)]
fn element_window_capture_source(
    element_id: &synapse_core::ElementId,
    element_rect: Rect,
) -> Result<(Rect, ReadTextCaptureSource), ErrorData> {
    let hwnd = element_id
        .parts()
        .map_err(|error| {
            mcp_error(
                error_codes::OCR_NO_TEXT,
                format!("read_text element_id {element_id} has no owning window: {error}"),
            )
        })?
        .hwnd;
    let hwnd = element_capture_root_hwnd(hwnd)?;
    fail_if_minimized_target_needs_window_capture(hwnd)?;
    let frame = synapse_capture::window_capture_bounds(hwnd).map_err(|error| {
        mcp_error(
            error.code(),
            format!(
                "read_text could not resolve capture bounds of element window {hwnd:#x}: {error}"
            ),
        )
    })?;
    let crop = element_window_crop::crop_element_to_window_frame(element_rect, frame).ok_or_else(
        || {
            mcp_error(
                error_codes::OCR_NO_TEXT,
                format!(
                    "read_text element {element_id} rect {element_rect:?} lies outside its window {hwnd:#x} frame {frame:?}; screen capture is refused because a covering window would be read instead"
                ),
            )
        },
    )?;
    Ok((
        crop.screen_region,
        ReadTextCaptureSource::Window {
            hwnd,
            window_region: crop.window_region,
        },
    ))
}

#[cfg(not(windows))]
fn element_window_capture_source(
    _element_id: &synapse_core::ElementId,
    _element_rect: Rect,
) -> Result<(Rect, ReadTextCaptureSource), ErrorData> {
    Err(mcp_error(
        error_codes::OBSERVE_NO_PERCEPTION_AVAILABLE,
        "read_text element OCR requires Windows window capture",
    ))
}

#[cfg(windows)]
/// Resolves a client-relative OCR region against the target window bitmap and
/// returns the region actually read (clipped to the client area and the
/// bitmap, #233), so the reported `region` and the word boxes describe the
/// captured pixels.
fn target_window_region_capture_source(
    hwnd: i64,
    client_region: Rect,
) -> Result<(Rect, ReadTextCaptureSource), ErrorData> {
    synapse_capture::validate_hwnd(hwnd).map_err(|error| {
        mcp_error(
            error_codes::TARGET_WINDOW_NOT_FOUND,
            format!("read_text window_hwnd {hwnd:#x} is not a live window: {error}"),
        )
    })?;
    let resolved =
        synapse_capture::client_region_to_window_regions(hwnd, client_region).map_err(|error| {
            mcp_error(
                error.code(),
                format!(
                    "read_text could not convert client-relative region {client_region:?} for hwnd {hwnd:#x} into target-window bitmap coordinates: {error}"
                ),
            )
        })?;
    Ok((
        resolved.client,
        ReadTextCaptureSource::Window {
            hwnd,
            window_region: resolved.window,
        },
    ))
}

#[cfg(not(windows))]
fn target_window_region_capture_source(
    _hwnd: i64,
    _window_region: Rect,
) -> Result<(Rect, ReadTextCaptureSource), ErrorData> {
    Err(mcp_error(
        error_codes::OBSERVE_NO_PERCEPTION_AVAILABLE,
        "read_text target-window OCR requires Windows window capture",
    ))
}

#[cfg(windows)]
fn fail_if_minimized_target_needs_window_capture(hwnd: i64) -> Result<(), ErrorData> {
    synapse_capture::validate_hwnd(hwnd).map_err(|error| {
        mcp_error(
            error_codes::TARGET_WINDOW_NOT_FOUND,
            format!("read_text window_hwnd {hwnd:#x} is not a live window: {error}"),
        )
    })?;
    if synapse_a11y::is_window_minimized(hwnd).map_err(|error| {
        mcp_error(
            error.code(),
            format!("read_text could not determine minimized state for hwnd {hwnd:#x}: {error}"),
        )
    })? {
        return Err(mcp_error(
            error_codes::A11Y_TARGET_WINDOW_MINIMIZED_UIA_UNAVAILABLE,
            format!(
                "read_text target hwnd {hwnd:#x} is minimized and no explicit window-relative OCR region was supplied; whole-window WGC OCR requires a live non-minimized target window"
            ),
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn fail_if_minimized_target_needs_window_capture(_hwnd: i64) -> Result<(), ErrorData> {
    Err(mcp_error(
        error_codes::OBSERVE_NO_PERCEPTION_AVAILABLE,
        "read_text target-window OCR requires Windows window capture",
    ))
}

pub fn effective_ocr_backend(backend: OcrBackend) -> Result<OcrBackend, ErrorData> {
    match backend {
        OcrBackend::Winrt | OcrBackend::Auto => Ok(OcrBackend::Winrt),
        OcrBackend::Crnn => Err(crnn_unavailable_error()),
    }
}

fn crnn_unavailable_error() -> ErrorData {
    mcp_error(
        error_codes::OCR_BACKEND_UNAVAILABLE,
        "CRNN OCR backend is declared in schema but no CRNN runtime/model provider is wired on this host; request backend=winrt or backend=auto",
    )
}

fn ocr_result_from_text_regions(
    regions: Vec<TextRegion>,
    request: &ResolvedReadTextRequest,
) -> OcrResult {
    let full_text = regions
        .iter()
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let confidence = aggregate_confidence(&regions);
    let confidence_source = aggregate_confidence_source(&regions);
    OcrResult {
        full_text,
        words: regions
            .into_iter()
            .map(|word| OcrWord {
                confidence: word_confidence(&word.text, word.confidence, word.confidence_source),
                confidence_source: word_confidence_source(
                    &word.text,
                    word.confidence,
                    word.confidence_source,
                ),
                text: word.text,
                bbox: word.bbox,
            })
            .collect(),
        confidence,
        confidence_source,
        region: request.region,
        lang: request.lang(),
        cache_write: None,
        perceived_text_notice: None,
        suspected_injection: Vec::new(),
    }
}

fn aggregate_confidence(regions: &[TextRegion]) -> f32 {
    if regions.is_empty() {
        return 0.0;
    }
    let sum = regions
        .iter()
        .map(|word| word_confidence(&word.text, word.confidence, word.confidence_source))
        .sum::<f32>();
    let count = u16::try_from(regions.len()).unwrap_or(u16::MAX);
    sum / f32::from(count)
}

fn aggregate_confidence_source(regions: &[TextRegion]) -> ConfidenceSource {
    regions
        .iter()
        .map(|word| word_confidence_source(&word.text, word.confidence, word.confidence_source))
        .reduce(ConfidenceSource::least_trusted)
        .unwrap_or_default()
}

fn word_confidence(text: &str, confidence: f32, source: ConfidenceSource) -> f32 {
    if source == ConfidenceSource::Unsupported {
        0.0
    } else {
        identifier_aware_confidence(text, confidence)
    }
}

fn word_confidence_source(
    text: &str,
    confidence: f32,
    source: ConfidenceSource,
) -> ConfidenceSource {
    if !matches!(
        source,
        ConfidenceSource::Unsupported | ConfidenceSource::NotApplicable
    ) && normalize_confidence(confidence) > AMBIGUOUS_IDENTIFIER_CONFIDENCE_CAP
        && is_ambiguous_identifier_token(text)
    {
        ConfidenceSource::Heuristic
    } else {
        source
    }
}

fn identifier_aware_confidence(text: &str, confidence: f32) -> f32 {
    let confidence = normalize_confidence(confidence);
    if is_ambiguous_identifier_token(text) {
        confidence.min(AMBIGUOUS_IDENTIFIER_CONFIDENCE_CAP)
    } else {
        confidence
    }
}

const AMBIGUOUS_IDENTIFIER_CONFIDENCE_CAP: f32 = 0.74;

fn is_ambiguous_identifier_token(text: &str) -> bool {
    let token = text.trim_matches(|ch: char| !identifier_char(ch));
    if token.len() < 2 || !token.chars().any(ambiguous_identifier_char) {
        return false;
    }
    let has_digit = token.chars().any(|ch| ch.is_ascii_digit());
    let has_separator = token.chars().any(identifier_separator);
    if looks_like_ordinary_lowercase_word(token) && !has_digit && !has_separator {
        return false;
    }
    let lower = token.to_ascii_lowercase();
    let short_all_ambiguous =
        token.chars().count() <= 4 && token.chars().all(ambiguous_identifier_char);
    let has_ambiguous_pair = [
        "v1", "vl", "vi", "i1", "l1", "1l", "1i", "o0", "0o", "ol", "lo",
    ]
    .iter()
    .any(|pair| lower.contains(pair));
    has_digit
        || has_separator
        || short_all_ambiguous
        || has_ambiguous_pair
        || lower == "vl"
        || lower == "v1"
}

fn looks_like_ordinary_lowercase_word(token: &str) -> bool {
    token.len() >= 3
        && token
            .chars()
            .all(|ch| ch.is_ascii_lowercase() && ch.is_ascii_alphabetic())
}

const fn identifier_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '-' | '_' | '.' | ':' | '/' | '\\' | '|' | '[' | ']' | '(' | ')'
        )
}

const fn identifier_separator(ch: char) -> bool {
    matches!(ch, '-' | '_' | '.' | ':' | '/' | '\\' | '|')
}

const fn ambiguous_identifier_char(ch: char) -> bool {
    matches!(ch, '0' | 'O' | 'o' | '1' | 'I' | 'l' | '|' | 'v' | 'V')
}

const fn normalize_confidence(confidence: f32) -> f32 {
    if confidence.is_finite() {
        confidence.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

struct SyntheticOcrProvider {
    region: Rect,
}

impl synapse_perception::OcrProvider for SyntheticOcrProvider {
    fn read_text(&self, _region: Rect) -> synapse_perception::PerceptionResult<Vec<TextRegion>> {
        Ok(vec![TextRegion {
            text: "Synapse".to_owned(),
            bbox: Rect {
                x: self.region.x.saturating_add(4),
                y: self.region.y.saturating_add(4),
                w: 72,
                h: 18,
            },
            confidence: 0.99,
            confidence_source: ConfidenceSource::Synthetic,
        }])
    }
}

#[cfg(test)]
#[path = "ocr_confidence_table.rs"]
mod ocr_confidence_table;

#[cfg(all(test, windows))]
#[path = "tests/whole_window_ocr_133.rs"]
mod whole_window_ocr_133;

#[cfg(all(test, windows))]
#[path = "tests/element_window_capture_source_178.rs"]
mod element_window_capture_source_178;

#[cfg(all(test, windows))]
#[path = "tests/focused_window_capture_source_255.rs"]
mod focused_window_capture_source_255;
