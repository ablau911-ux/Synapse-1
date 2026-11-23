//! Live hardware H.264 for the moving layer of DYN1.
//! The moving thread owns Media Foundation; each interval owns one encoder.
//! Every instant is written, including repeats. Settings must read back exactly.

use std::path::Path;

use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonQuality, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncMPVGOPSize, CODECAPI_AVScenarioInfo,
    ICodecAPI, IMFAttributes, IMFMediaType, IMFSinkWriter, IMFSinkWriterEx, MF_MT_AVG_BITRATE,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_YUV_MATRIX, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
    MF_TRANSCODE_CONTAINERTYPE, MF_VERSION, MFCreateAttributes, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Video,
    MFNominalRange_16_235, MFSTARTUP_LITE, MFShutdown, MFStartup, MFT_CATEGORY_VIDEO_ENCODER,
    MFT_ENUM_HARDWARE_URL_Attribute, MFT_ENUM_HARDWARE_VENDOR_ID_Attribute,
    MFT_FRIENDLY_NAME_Attribute, MFTranscodeContainerType_MPEG4, MFVideoFormat_H264,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFVideoTransferMatrix_BT709,
    eAVEncCommonRateControlMode_Quality, eAVEncH264VProfile_High, eAVScenarioInfo_DisplayRemoting,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Variant::{VARIANT, VT_UI4};
use windows::core::{GUID, HSTRING, Interface};

pub(super) const H264_QUALITY_LEVEL: u32 = 90;
pub(super) const GOP_FRAMES: u32 = 20;
const NOMINAL_BITS_PER_PIXEL_FRAME: f64 = 0.10;
const GOP_REFERENCE_FPS: u32 = 20;
const MAX_ENCODE_MACROBLOCKS: u64 = 36_864;
const MAX_ENCODE_SIDE: u32 = 4_096;
const MACROBLOCK_SIDE: u32 = 16;
const MAX_TRANSFORMS_PER_STREAM: u32 = 8;
const HARDWARE_TRANSFORMS_ENABLED: u32 = 1;
const NO_B_FRAMES: u32 = 0;
const BGR_BYTES: usize = 3;
const BLUE: usize = 0;
const GREEN: usize = 1;
const RED: usize = 2;
const CHROMA_SIDE: usize = 2;
const CHROMA_PIXELS: i32 = 4;
const NV12_SIZE_NUMERATOR: usize = 3;
const NV12_SIZE_DENOMINATOR: usize = 2;
const UV_V_OFFSET: usize = 1;
const COLOR_SHIFT: u32 = 8;
const COLOR_ROUNDING: i32 = 128;
const Y_RED: i32 = 47;
const Y_GREEN: i32 = 157;
const Y_BLUE: i32 = 16;
const Y_OFFSET: i32 = 16;
const U_RED: i32 = -26;
const U_GREEN: i32 = -87;
const U_BLUE: i32 = 112;
const V_RED: i32 = 112;
const V_GREEN: i32 = -102;
const V_BLUE: i32 = -10;
const UV_OFFSET: i32 = 128;

/// One source rectangle in BT.709 limited-range NV12, with repeated edge padding.
pub(super) struct Nv12Box {
    width: usize,
    height: usize,
    source_width: usize,
    source_height: usize,
    nv12: Vec<u8>,
}

impl Nv12Box {
    pub(super) fn new(width: u32, height: u32) -> Self {
        let coded_width = even_up(width) as usize;
        let coded_height = even_up(height) as usize;
        Self {
            width: coded_width,
            height: coded_height,
            source_width: width as usize,
            source_height: height as usize,
            nv12: vec![0; coded_width * coded_height * NV12_SIZE_NUMERATOR / NV12_SIZE_DENOMINATOR],
        }
    }

    pub(super) fn coded_size(&self) -> (u32, u32) {
        (self.width as u32, self.height as u32)
    }

    pub(super) fn bytes(&self) -> &[u8] {
        &self.nv12
    }

    /// The caller supplies the tightly packed BGR rectangle, without a canvas.
    pub(super) fn draw(&mut self, bgr: &[u8]) {
        let (luma, chroma) = self.nv12.split_at_mut(self.width * self.height);
        for row in 0..self.height {
            let source_row = row.min(self.source_height - 1);
            for col in 0..self.width {
                let source_col = col.min(self.source_width - 1);
                let at = (source_row * self.source_width + source_col) * BGR_BYTES;
                let blue = i32::from(bgr[at + BLUE]);
                let green = i32::from(bgr[at + GREEN]);
                let red = i32::from(bgr[at + RED]);
                luma[row * self.width + col] = clamp_u8(
                    ((Y_RED * red + Y_GREEN * green + Y_BLUE * blue + COLOR_ROUNDING)
                        >> COLOR_SHIFT)
                        + Y_OFFSET,
                );
            }
        }
        for row in (0..self.height).step_by(CHROMA_SIDE) {
            for col in (0..self.width).step_by(CHROMA_SIDE) {
                let mut red = 0;
                let mut green = 0;
                let mut blue = 0;
                for dy in 0..CHROMA_SIDE {
                    let source_row = (row + dy).min(self.source_height - 1);
                    for dx in 0..CHROMA_SIDE {
                        let source_col = (col + dx).min(self.source_width - 1);
                        let at = (source_row * self.source_width + source_col) * BGR_BYTES;
                        blue += i32::from(bgr[at + BLUE]);
                        green += i32::from(bgr[at + GREEN]);
                        red += i32::from(bgr[at + RED]);
                    }
                }
                red /= CHROMA_PIXELS;
                green /= CHROMA_PIXELS;
                blue /= CHROMA_PIXELS;
                let at = (row / CHROMA_SIDE) * self.width + col;
                chroma[at] = clamp_u8(
                    ((U_RED * red + U_GREEN * green + U_BLUE * blue + COLOR_ROUNDING)
                        >> COLOR_SHIFT)
                        + UV_OFFSET,
                );
                chroma[at + UV_V_OFFSET] = clamp_u8(
                    ((V_RED * red + V_GREEN * green + V_BLUE * blue + COLOR_ROUNDING)
                        >> COLOR_SHIFT)
                        + UV_OFFSET,
                );
            }
        }
    }
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, i32::from(u8::MAX)) as u8
}

const fn even_up(value: u32) -> u32 {
    value.div_ceil(CHROMA_SIDE as u32) * CHROMA_SIDE as u32
}

const fn gop_frames(fps: u32) -> u32 {
    GOP_FRAMES * fps / GOP_REFERENCE_FPS
}

/// The grant read from the actual hardware transform.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct EncoderGrant {
    pub(super) name: String,
    pub(super) hardware: bool,
    pub(super) quality_level: u32,
    pub(super) gop_frames: u32,
    pub(super) b_frames: u32,
    pub(super) screen_content_hint: bool,
}

/// One live H.264 segment. The caller keeps MediaFoundationRuntime alive on this thread.
pub(super) struct ClipEncoder {
    writer: IMFSinkWriter,
    stream: u32,
    grant: EncoderGrant,
    finalized: bool,
}

impl ClipEncoder {
    pub(super) fn accepts(width: u32, height: u32) -> bool {
        width > 0
            && height > 0
            && width <= MAX_ENCODE_SIDE
            && height <= MAX_ENCODE_SIDE
            && width.is_multiple_of(CHROMA_SIDE as u32)
            && height.is_multiple_of(CHROMA_SIDE as u32)
            && u64::from(width.div_ceil(MACROBLOCK_SIDE))
                * u64::from(height.div_ceil(MACROBLOCK_SIDE))
                <= MAX_ENCODE_MACROBLOCKS
    }

    pub(super) fn create(path: &Path, width: u32, height: u32, fps: u32) -> Result<Self, String> {
        if !Self::accepts(width, height) || fps == 0 {
            return Err(format!(
                "unsupported H.264 size or rate: {width}x{height} at {fps} fps"
            ));
        }
        let (writer, stream) =
            open_writer(path, MFTranscodeContainerType_MPEG4, width, height, fps)?;
        let grant = confirm_settings(&writer, stream, fps)?;
        // SAFETY: the writer is configured; this starts the session.
        unsafe { writer.BeginWriting() }.map_err(hr("IMFSinkWriter::BeginWriting"))?;
        Ok(Self {
            writer,
            stream,
            grant,
            finalized: false,
        })
    }

    pub(super) fn grant(&self) -> &EncoderGrant {
        &self.grant
    }

    pub(super) fn write_nv12(
        &mut self,
        nv12: &[u8],
        time_hns: i64,
        duration_hns: i64,
    ) -> Result<(), String> {
        let length = u32::try_from(nv12.len()).map_err(|_| "NV12 frame too large".to_owned())?;
        // SAFETY: the buffer is locked for exactly length bytes while copied.
        unsafe {
            let buffer = MFCreateMemoryBuffer(length).map_err(hr("MFCreateMemoryBuffer"))?;
            let mut data: *mut u8 = std::ptr::null_mut();
            buffer
                .Lock(&raw mut data, None, None)
                .map_err(hr("IMFMediaBuffer::Lock"))?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), data, nv12.len());
            buffer.Unlock().map_err(hr("IMFMediaBuffer::Unlock"))?;
            buffer
                .SetCurrentLength(length)
                .map_err(hr("IMFMediaBuffer::SetCurrentLength"))?;
            let sample = MFCreateSample().map_err(hr("MFCreateSample"))?;
            sample
                .AddBuffer(&buffer)
                .map_err(hr("IMFSample::AddBuffer"))?;
            sample
                .SetSampleTime(time_hns)
                .map_err(hr("IMFSample::SetSampleTime"))?;
            sample
                .SetSampleDuration(duration_hns)
                .map_err(hr("IMFSample::SetSampleDuration"))?;
            self.writer
                .WriteSample(self.stream, &sample)
                .map_err(hr("IMFSinkWriter::WriteSample"))?;
        }
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<(), String> {
        self.finalized = true;
        // SAFETY: the writer is open; Finalize flushes and closes it.
        unsafe { self.writer.Finalize() }.map_err(hr("IMFSinkWriter::Finalize"))
    }
}

impl Drop for ClipEncoder {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        self.finalized = true;
        // SAFETY: an unfinished writer is finalized once before release.
        if let Err(error) = unsafe { self.writer.Finalize() } {
            tracing::debug!(code = "CLIP_ENCODER_FINALIZE_ON_DROP_FAILED", error = %error,
                "finalize after an earlier clip error failed");
        }
    }
}

/// Creates the writer and configures the H.264 stream. Returns the writer and
/// its stream index.
fn open_writer(
    path: &Path,
    container: GUID,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(IMFSinkWriter, u32), String> {
    // SAFETY: Media Foundation calls on attribute stores and media types this
    // function owns; every result is checked.
    unsafe {
        let attributes = new_attributes(2)?;
        attributes
            .SetUINT32(
                &MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
                HARDWARE_TRANSFORMS_ENABLED,
            )
            .map_err(hr("set MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS"))?;
        attributes
            .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &container)
            .map_err(hr("set MF_TRANSCODE_CONTAINERTYPE"))?;
        let url = HSTRING::from(path.as_os_str());
        let writer = MFCreateSinkWriterFromURL(&url, None, &attributes)
            .map_err(hr("MFCreateSinkWriterFromURL"))?;

        let output = video_type(&MFVideoFormat_H264, width, height, fps)?;
        output
            .SetUINT32(&MF_MT_AVG_BITRATE, nominal_bitrate(width, height, fps))
            .map_err(hr("set MF_MT_AVG_BITRATE"))?;
        output
            .SetUINT32(
                &MF_MT_MPEG2_PROFILE,
                eAVEncH264VProfile_High.0.cast_unsigned(),
            )
            .map_err(hr("set MF_MT_MPEG2_PROFILE"))?;
        let stream = writer
            .AddStream(&output)
            .map_err(hr("IMFSinkWriter::AddStream"))?;

        let input = video_type(&MFVideoFormat_NV12, width, height, fps)?;
        input
            .SetUINT32(&MF_MT_DEFAULT_STRIDE, width)
            .map_err(hr("set MF_MT_DEFAULT_STRIDE"))?;
        // The encoder sees the settings before its media types are set; a
        // refusal retries with the keyframe interval and B-frame count only.
        if writer
            .SetInputMediaType(stream, &input, &encoder_params(fps, true)?)
            .is_err()
        {
            writer
                .SetInputMediaType(stream, &input, &encoder_params(fps, false)?)
                .map_err(hr("IMFSinkWriter::SetInputMediaType"))?;
        }
        Ok((writer, stream))
    }
}

/// β · width · height · f, in bits per second (B7).
fn nominal_bitrate(width: u32, height: u32, fps: u32) -> u32 {
    let bits = NOMINAL_BITS_PER_PIXEL_FRAME * f64::from(width) * f64::from(height) * f64::from(fps);
    bits.round().clamp(1.0, f64::from(u32::MAX)) as u32
}

/// Builds a progressive, square-pixel, BT.709 limited-range video media type.
fn video_type(subtype: &GUID, width: u32, height: u32, fps: u32) -> Result<IMFMediaType, String> {
    // SAFETY: setters on a media type this function owns.
    unsafe {
        let media_type = MFCreateMediaType().map_err(hr("MFCreateMediaType"))?;
        media_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(hr("set MF_MT_MAJOR_TYPE"))?;
        media_type
            .SetGUID(&MF_MT_SUBTYPE, subtype)
            .map_err(hr("set MF_MT_SUBTYPE"))?;
        media_type
            .SetUINT32(
                &MF_MT_INTERLACE_MODE,
                MFVideoInterlace_Progressive.0.cast_unsigned(),
            )
            .map_err(hr("set MF_MT_INTERLACE_MODE"))?;
        media_type
            .SetUINT64(&MF_MT_FRAME_SIZE, pack_pair(width, height))
            .map_err(hr("set MF_MT_FRAME_SIZE"))?;
        media_type
            .SetUINT64(&MF_MT_FRAME_RATE, pack_pair(fps, 1))
            .map_err(hr("set MF_MT_FRAME_RATE"))?;
        media_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_pair(1, 1))
            .map_err(hr("set MF_MT_PIXEL_ASPECT_RATIO"))?;
        media_type
            .SetUINT32(
                &MF_MT_YUV_MATRIX,
                MFVideoTransferMatrix_BT709.0.cast_unsigned(),
            )
            .map_err(hr("set MF_MT_YUV_MATRIX"))?;
        media_type
            .SetUINT32(
                &MF_MT_VIDEO_NOMINAL_RANGE,
                MFNominalRange_16_235.0.cast_unsigned(),
            )
            .map_err(hr("set MF_MT_VIDEO_NOMINAL_RANGE"))?;
        Ok(media_type)
    }
}

/// Encoder parameters handed to `SetInputMediaType` (applied through
/// `ICodecAPI` by the Sink Writer): GOP in frames and no B-frames always; the
/// screen-content hint and quality mode when `full`.
fn encoder_params(fps: u32, full: bool) -> Result<IMFAttributes, String> {
    // SAFETY: setters on an attribute store this function owns.
    unsafe {
        let params = new_attributes(5)?;
        params
            .SetUINT32(&CODECAPI_AVEncMPVGOPSize, gop_frames(fps))
            .map_err(hr("set CODECAPI_AVEncMPVGOPSize"))?;
        params
            .SetUINT32(&CODECAPI_AVEncMPVDefaultBPictureCount, 0)
            .map_err(hr("set CODECAPI_AVEncMPVDefaultBPictureCount"))?;
        if full {
            params
                .SetUINT32(
                    &CODECAPI_AVScenarioInfo,
                    eAVScenarioInfo_DisplayRemoting.0.cast_unsigned(),
                )
                .map_err(hr("set CODECAPI_AVScenarioInfo"))?;
            params
                .SetUINT32(
                    &CODECAPI_AVEncCommonRateControlMode,
                    eAVEncCommonRateControlMode_Quality.0.cast_unsigned(),
                )
                .map_err(hr("set CODECAPI_AVEncCommonRateControlMode"))?;
            params
                .SetUINT32(&CODECAPI_AVEncCommonQuality, H264_QUALITY_LEVEL)
                .map_err(hr("set CODECAPI_AVEncCommonQuality"))?;
        }
        Ok(params)
    }
}

/// Required encoder settings are read back exactly; refusal is an error.
fn confirm_settings(writer: &IMFSinkWriter, stream: u32, fps: u32) -> Result<EncoderGrant, String> {
    let codec = codec_api(writer, stream).ok_or_else(|| {
        "the encoder exposes no ICodecAPI: its settings cannot be set or read back".to_owned()
    })?;
    let expected_gop = gop_frames(fps);
    let gop_frames = set_and_read(&codec, &CODECAPI_AVEncMPVGOPSize, expected_gop)
        .filter(|value| *value == expected_gop)
        .ok_or("the encoder did not read back the required GOP size")?;
    let b_frames = set_and_read(&codec, &CODECAPI_AVEncMPVDefaultBPictureCount, NO_B_FRAMES)
        .filter(|value| *value == NO_B_FRAMES)
        .ok_or("the encoder did not read back a B-frame count of 0")?;
    let hint = eAVScenarioInfo_DisplayRemoting.0.cast_unsigned();
    let screen_content_hint = set_and_read(&codec, &CODECAPI_AVScenarioInfo, hint) == Some(hint);
    let quality_mode = eAVEncCommonRateControlMode_Quality.0.cast_unsigned();
    let quality_level = set_and_read(&codec, &CODECAPI_AVEncCommonRateControlMode, quality_mode)
        .filter(|mode| *mode == quality_mode)
        .and_then(|_| set_and_read(&codec, &CODECAPI_AVEncCommonQuality, H264_QUALITY_LEVEL))
        .filter(|level| *level == H264_QUALITY_LEVEL)
        .ok_or("the encoder did not read back quality mode at level 90")?;
    let (name, hardware) = encoder_identity(writer, stream);
    if !hardware {
        return Err(format!(
            "hardware H.264 is required; Media Foundation granted {name}"
        ));
    }
    Ok(EncoderGrant {
        name,
        hardware,
        quality_level,
        gop_frames,
        b_frames,
        screen_content_hint,
    })
}

/// Sets a `VT_UI4` codec value and returns what the encoder reads back.
fn set_and_read(codec: &ICodecAPI, key: &GUID, value: u32) -> Option<u32> {
    // SAFETY: plain ICodecAPI calls with an owned VARIANT.
    unsafe {
        codec.SetValue(key, &variant_u32(value)).ok()?;
        variant_as_u32(&codec.GetValue(key).ok()?)
    }
}

/// The value of a `VT_UI4` VARIANT.
fn variant_as_u32(variant: &VARIANT) -> Option<u32> {
    // SAFETY: the union member is read only when the tag says it is a UI4.
    unsafe {
        let inner = &*variant.Anonymous.Anonymous;
        (inner.vt == VT_UI4).then(|| inner.Anonymous.ulVal)
    }
}

fn codec_api(writer: &IMFSinkWriter, stream: u32) -> Option<ICodecAPI> {
    let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: an out-parameter for an ICodecAPI pointer the caller then owns.
    unsafe {
        writer
            .GetServiceForStream(stream, &GUID::zeroed(), &ICodecAPI::IID, &raw mut raw)
            .ok()?;
        if raw.is_null() {
            return None;
        }
        Some(ICodecAPI::from_raw(raw))
    }
}

/// Finds the video encoder transform the Sink Writer inserted and reports its
/// name and whether it is a hardware MFT. Hardware encoders often report no
/// friendly name; their vendor id is reported instead (F8, package 0).
fn encoder_identity(writer: &IMFSinkWriter, stream: u32) -> (String, bool) {
    let Ok(writer_ex) = writer.cast::<IMFSinkWriterEx>() else {
        return ("unknown".to_owned(), false);
    };
    for index in 0..MAX_TRANSFORMS_PER_STREAM {
        let mut category = GUID::zeroed();
        let mut transform = None;
        // SAFETY: two out-parameters.
        let found = unsafe {
            writer_ex.GetTransformForStream(
                stream,
                index,
                Some(&raw mut category),
                &raw mut transform,
            )
        };
        if found.is_err() {
            break;
        }
        if category != MFT_CATEGORY_VIDEO_ENCODER {
            continue;
        }
        let Some(transform) = transform else {
            continue;
        };
        // SAFETY: a plain query on the transform.
        let attributes = unsafe { transform.GetAttributes() }.ok();
        let hardware = attributes.as_ref().is_some_and(|attributes| {
            // SAFETY: a plain attribute query.
            unsafe { attributes.GetStringLength(&MFT_ENUM_HARDWARE_URL_Attribute) }.is_ok()
        });
        let name = attributes
            .as_ref()
            .and_then(|attributes| attribute_string(attributes, &MFT_FRIENDLY_NAME_Attribute))
            .or_else(|| {
                attributes
                    .as_ref()
                    .and_then(|attributes| {
                        attribute_string(attributes, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute)
                    })
                    .map(|vendor| format!("hardware H.264 encoder ({vendor})"))
            })
            .unwrap_or_else(|| {
                if hardware {
                    "unnamed hardware H.264 encoder".to_owned()
                } else {
                    "unnamed software H.264 encoder".to_owned()
                }
            });
        return (name, hardware);
    }
    ("unknown".to_owned(), false)
}

fn attribute_string(attributes: &IMFAttributes, key: &GUID) -> Option<String> {
    // SAFETY: the buffer is sized from the reported length plus the terminator.
    unsafe {
        let length = attributes.GetStringLength(key).ok()?;
        let mut buffer = vec![0_u16; usize::try_from(length).ok()? + 1];
        attributes.GetString(key, &mut buffer, None).ok()?;
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    }
}

fn new_attributes(initial_size: u32) -> Result<IMFAttributes, String> {
    let mut attributes = None;
    // SAFETY: one out-parameter.
    unsafe { MFCreateAttributes(&raw mut attributes, initial_size) }
        .map_err(hr("MFCreateAttributes"))?;
    attributes.ok_or_else(|| "MFCreateAttributes returned no attribute store".to_owned())
}

fn variant_u32(value: u32) -> VARIANT {
    let mut variant = VARIANT::default();
    // SAFETY: writing the tag and the matching union member of a fresh VARIANT.
    unsafe {
        let inner = &mut *variant.Anonymous.Anonymous;
        inner.vt = VT_UI4;
        inner.Anonymous.ulVal = value;
    }
    variant
}

const fn pack_pair(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | low as u64
}

pub(super) fn hr(step: &'static str) -> impl Fn(windows::core::Error) -> String {
    move |error| format!("{step} failed: {error}")
}

/// COM (MTA) + Media Foundation for the current thread, released on drop.
/// Shared with the clip decoder.
pub(super) struct MediaFoundationRuntime {
    com_initialized: bool,
}

impl MediaFoundationRuntime {
    pub(super) fn start() -> Result<Self, String> {
        // S_OK and S_FALSE both need a matching CoUninitialize; a thread that
        // is already STA (RPC_E_CHANGED_MODE) keeps its apartment.
        // SAFETY: plain COM and Media Foundation start-up.
        let com_initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        // SAFETY: plain Media Foundation start-up.
        if let Err(error) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) } {
            if com_initialized {
                // SAFETY: matches the successful CoInitializeEx above.
                unsafe { CoUninitialize() };
            }
            return Err(format!("MFStartup failed: {error}"));
        }
        Ok(Self { com_initialized })
    }
}

impl Drop for MediaFoundationRuntime {
    fn drop(&mut self) {
        // SAFETY: matches the start-up calls in `start`.
        unsafe {
            let _ = MFShutdown();
            if self.com_initialized {
                CoUninitialize();
            }
        }
    }
}
