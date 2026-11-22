//! H.264 clip encoder on the Media Foundation Sink Writer ("Bug clip video"
//! research, section 5.3, product path).
//!
//! `ClipEncoder` owns one Sink Writer that turns constant-rate NV12 frames into
//! an H.264 (High profile, yuv420) stream inside a fragmented MP4. It asks for
//! a hardware encoder, quality-based rate control and a 1 s keyframe interval,
//! and reports what Media Foundation actually granted: the encoder's name,
//! whether it is hardware, the container, and the rate-control mode.
//!
//! All calls must stay on the thread that created the encoder: it initialises
//! COM (MTA) and Media Foundation for that thread and shuts both down on drop.

use std::path::Path;

use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonQuality,
    CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncMPVGOPSize, ICodecAPI, IMFAttributes,
    IMFMediaType, IMFSinkWriter, IMFSinkWriterEx, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_MT_VIDEO_NOMINAL_RANGE,
    MF_MT_YUV_MATRIX, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_TRANSCODE_CONTAINERTYPE,
    MF_VERSION, MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFMediaType_Video, MFNominalRange_16_235, MFSTARTUP_LITE,
    MFShutdown, MFStartup, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_HARDWARE_URL_Attribute,
    MFT_FRIENDLY_NAME_Attribute, MFTranscodeContainerType_FMPEG4, MFTranscodeContainerType_MPEG4,
    MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoInterlace_Progressive,
    MFVideoTransferMatrix_BT709, eAVEncCommonRateControlMode_CBR,
    eAVEncCommonRateControlMode_Quality, eAVEncH264VProfile_High,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Variant::{VARIANT, VT_UI4};
use windows::core::{GUID, HSTRING, Interface};

/// `CODECAPI_AVEncCommonQuality` level (0..=100) for quality-based rate control.
const H264_QUALITY_LEVEL: u32 = 70;
/// β: bits per pixel and frame for the bitrate fallback (equation section 6).
pub(super) const FALLBACK_BITS_PER_PIXEL_FRAME: f64 = 0.10;
/// Bitrate used when the encoder refuses quality mode (research section 9).
const FALLBACK_BITRATE_BPS: u32 = 6_000_000;
const KEYFRAME_INTERVAL_SECONDS: u32 = 1;
const HNS_PER_SECOND: i64 = 10_000_000;
/// Upper bound on transforms walked when looking for the encoder in the chain.
const MAX_TRANSFORMS_PER_STREAM: u32 = 8;

/// What Media Foundation actually granted for this clip.
#[derive(Clone, Debug)]
#[expect(dead_code, reason = "package 3b reports the container")]
pub(super) struct EncoderReadback {
    pub(super) encoder: String,
    pub(super) hardware: bool,
    /// `fragmented_mp4`, or `mp4` when fragmented MP4 was refused.
    pub(super) container: &'static str,
    /// `quality` or `cbr_bitrate`.
    pub(super) rate_control_mode: &'static str,
    /// Quality level (0..=100) or bits per second, matching the mode.
    pub(super) rate_control_value: u32,
    pub(super) keyframe_interval_frames: u32,
}

/// One frame of a segment: its raw file (captured BGRA rows), its captured
/// size, and its time within the segment (B5).
#[expect(dead_code, reason = "the times are written by package 3b")]
pub(super) struct SegmentSample {
    pub(super) raw: std::path::PathBuf,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) instant: u64,
    pub(super) local_ms: u64,
    pub(super) duration_ms: u64,
}

/// What the clip worker hands the encoder for one segment.
pub(super) struct SegmentInput<'a> {
    /// Captured size of the segment's canvas (the largest size during a drag).
    pub(super) canvas_width: u32,
    pub(super) canvas_height: u32,
    /// σ: whole-number pixel duplication (A3).
    pub(super) scale: u32,
    pub(super) fps: u32,
    pub(super) samples: &'a [SegmentSample],
}

/// A finished segment file and what the encoder granted for it.
pub(super) struct EncodedSegment {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) bound_held: bool,
    pub(super) keyframe_instants: Vec<u64>,
    pub(super) encoder: String,
    pub(super) hardware: bool,
    pub(super) quality: super::clip_folder::QualityReport,
}

/// Encodes one segment into `path` (the `.part` file) and finalizes it.
pub(super) fn encode_segment(path: &Path, input: &SegmentInput) -> Result<EncodedSegment, String> {
    let width = (input.canvas_width * input.scale).max(2) & !1;
    let height = (input.canvas_height * input.scale).max(2) & !1;
    let mut encoder = ClipEncoder::create(path, width, height, input.fps)?;
    let mut canvas = Nv12Canvas::new(width, height);
    for sample in input.samples {
        let bgra = std::fs::read(&sample.raw)
            .map_err(|error| format!("could not read {}: {error}", sample.raw.display()))?;
        canvas.draw(&bgra, sample.width, sample.height, input.scale);
        encoder.write_nv12(&canvas.nv12)?;
    }
    let readback = encoder.finish()?;
    Ok(EncodedSegment {
        width,
        height,
        bound_held: true,
        keyframe_instants: Vec::new(),
        encoder: readback.encoder,
        hardware: readback.hardware,
        quality: super::clip_folder::QualityReport {
            mode: readback.rate_control_mode.to_owned(),
            value: readback.rate_control_value,
            gop_frames: readback.keyframe_interval_frames,
            b_frames: 0,
            screen_content_hint: false,
            status: "unverified".to_owned(),
        },
    })
}

/// The picture the encoder sees, kept as NV12 (BT.709 limited range). Frames
/// are placed at the top left, each pixel duplicated σ × σ, never scaled
/// otherwise (B3, E4); the rest of a larger canvas is black.
struct Nv12Canvas {
    width: u32,
    height: u32,
    nv12: Vec<u8>,
}

impl Nv12Canvas {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            nv12: vec![0; width as usize * height as usize * 3 / 2],
        }
    }

    /// Draws a BGRA frame of `source_width` × `source_height` duplicated by
    /// `scale`, converting to NV12 with chroma averaged over each 2 × 2 block.
    fn draw(&mut self, bgra: &[u8], source_width: u32, source_height: u32, scale: u32) {
        let (width, height) = (self.width as usize, self.height as usize);
        let (source_width, source_height) = (source_width as usize, source_height as usize);
        let scale = scale.max(1) as usize;
        let rgb_at = |col: usize, row: usize| -> (i32, i32, i32) {
            let (x, y) = (col / scale, row / scale);
            if x >= source_width || y >= source_height {
                return (0, 0, 0);
            }
            let offset = (y * source_width + x) * 4;
            bgra.get(offset..offset + 3).map_or((0, 0, 0), |pixel| {
                (
                    i32::from(pixel[2]),
                    i32::from(pixel[1]),
                    i32::from(pixel[0]),
                )
            })
        };
        let (luma, chroma) = self.nv12.split_at_mut(width * height);
        for row in 0..height {
            for col in 0..width {
                let (red, green, blue) = rgb_at(col, row);
                luma[row * width + col] =
                    clamp_u8(((47 * red + 157 * green + 16 * blue + 128) >> 8) + 16);
            }
        }
        for row in (0..height).step_by(2) {
            for col in (0..width).step_by(2) {
                let mut sum = (0, 0, 0);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (red, green, blue) = rgb_at(col + dx, row + dy);
                    sum = (sum.0 + red, sum.1 + green, sum.2 + blue);
                }
                let (red, green, blue) = (sum.0 / 4, sum.1 / 4, sum.2 / 4);
                let at = (row / 2) * width + col;
                chroma[at] = clamp_u8(((-26 * red - 87 * green + 112 * blue + 128) >> 8) + 128);
                chroma[at + 1] = clamp_u8(((112 * red - 102 * green - 10 * blue + 128) >> 8) + 128);
            }
        }
    }
}

fn clamp_u8(value: i32) -> u8 {
    u8::try_from(value.clamp(0, 255)).unwrap_or(u8::MAX)
}

pub(super) struct ClipEncoder {
    // Field order matters: the writer must be released before the runtime
    // guard shuts Media Foundation down.
    writer: IMFSinkWriter,
    stream: u32,
    fps: u32,
    frames_written: u64,
    readback: EncoderReadback,
    /// `true` once `Finalize` ran; otherwise drop finalizes (error exits).
    finalized: bool,
    _runtime: MediaFoundationRuntime,
}

impl ClipEncoder {
    /// Opens `path` for writing an H.264 clip of fixed `width`x`height` (even)
    /// at a constant `fps`.
    pub(super) fn create(path: &Path, width: u32, height: u32, fps: u32) -> Result<Self, String> {
        let runtime = MediaFoundationRuntime::start()?;
        let opened = open_writer(path, MFTranscodeContainerType_FMPEG4, width, height, fps)
            .map(|opened| (opened, "fragmented_mp4"))
            .or_else(|fragmented_error| {
                tracing::warn!(
                    code = "CLIP_ENCODER_FMP4_REFUSED",
                    error = %fragmented_error,
                    "fragmented MP4 refused; falling back to normal MP4"
                );
                let _ = std::fs::remove_file(path);
                open_writer(path, MFTranscodeContainerType_MPEG4, width, height, fps)
                    .map(|opened| (opened, "mp4"))
                    .map_err(|mp4_error| {
                        format!("fragmented MP4: {fragmented_error}; MP4: {mp4_error}")
                    })
            })?;
        let ((writer, stream, rate_control_mode, rate_control_value), container) = opened;
        let (encoder, hardware) = encoder_identity(&writer, stream);
        Ok(Self {
            writer,
            stream,
            fps,
            frames_written: 0,
            readback: EncoderReadback {
                encoder,
                hardware,
                container,
                rate_control_mode,
                rate_control_value,
                keyframe_interval_frames: fps * KEYFRAME_INTERVAL_SECONDS,
            },
            finalized: false,
            _runtime: runtime,
        })
    }

    #[expect(dead_code, reason = "package 3b reads the encoder back per segment")]
    pub(super) const fn readback(&self) -> &EncoderReadback {
        &self.readback
    }

    /// Encodes one NV12 frame (`width*height*3/2` bytes) as the next frame of
    /// the constant-rate timeline.
    pub(super) fn write_nv12(&mut self, nv12: &[u8]) -> Result<(), String> {
        let length = u32::try_from(nv12.len()).map_err(|_| "NV12 frame too large".to_owned())?;
        let start = frame_time_hns(self.frames_written, self.fps);
        let next = frame_time_hns(self.frames_written + 1, self.fps);
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
                .SetSampleTime(start)
                .map_err(hr("IMFSample::SetSampleTime"))?;
            sample
                .SetSampleDuration(next - start)
                .map_err(hr("IMFSample::SetSampleDuration"))?;
            self.writer
                .WriteSample(self.stream, &sample)
                .map_err(hr("IMFSinkWriter::WriteSample"))?;
        }
        self.frames_written += 1;
        Ok(())
    }

    /// Flushes the encoder and closes the file.
    pub(super) fn finish(mut self) -> Result<EncoderReadback, String> {
        self.finalized = true;
        unsafe { self.writer.Finalize() }.map_err(hr("IMFSinkWriter::Finalize"))?;
        Ok(self.readback.clone())
    }
}

impl Drop for ClipEncoder {
    /// Error exits after `BeginWriting` still finalize the writer; the
    /// caller's original error is kept, so this result is only logged.
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        self.finalized = true;
        if let Err(error) = unsafe { self.writer.Finalize() } {
            tracing::debug!(
                code = "CLIP_ENCODER_FINALIZE_ON_DROP_FAILED",
                error = %error,
                "finalize after an earlier clip error failed"
            );
        }
    }
}

/// Creates the writer, configures the H.264 stream and begins writing.
/// Returns the writer, its stream index and the rate control granted.
fn open_writer(
    path: &Path,
    container: GUID,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<(IMFSinkWriter, u32, &'static str, u32), String> {
    unsafe {
        let attributes = new_attributes(2)?;
        attributes
            .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
            .map_err(hr("set MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS"))?;
        attributes
            .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &container)
            .map_err(hr("set MF_TRANSCODE_CONTAINERTYPE"))?;
        let url = HSTRING::from(path.as_os_str());
        let writer = MFCreateSinkWriterFromURL(&url, None, &attributes)
            .map_err(hr("MFCreateSinkWriterFromURL"))?;

        let output = video_type(&MFVideoFormat_H264, width, height, fps)?;
        output
            .SetUINT32(&MF_MT_AVG_BITRATE, FALLBACK_BITRATE_BPS)
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
        let keyframe_interval = fps * KEYFRAME_INTERVAL_SECONDS;
        // First ask for quality mode up front (the encoder sees it before its
        // media types are set); a refusal retries with the keyframe interval only.
        let quality_params = encoder_params(keyframe_interval, true)?;
        if writer
            .SetInputMediaType(stream, &input, &quality_params)
            .is_err()
        {
            let plain_params = encoder_params(keyframe_interval, false)?;
            writer
                .SetInputMediaType(stream, &input, &plain_params)
                .map_err(hr("IMFSinkWriter::SetInputMediaType"))?;
        }
        let (mode, value) = confirm_rate_control(&writer, stream)?;
        writer
            .BeginWriting()
            .map_err(hr("IMFSinkWriter::BeginWriting"))?;
        Ok((writer, stream, mode, value))
    }
}

/// Builds a progressive, square-pixel, BT.709 limited-range video media type.
fn video_type(subtype: &GUID, width: u32, height: u32, fps: u32) -> Result<IMFMediaType, String> {
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
/// `ICodecAPI` by the Sink Writer).
fn encoder_params(keyframe_interval: u32, quality: bool) -> Result<IMFAttributes, String> {
    unsafe {
        let params = new_attributes(3)?;
        params
            .SetUINT32(&CODECAPI_AVEncMPVGOPSize, keyframe_interval)
            .map_err(hr("set CODECAPI_AVEncMPVGOPSize"))?;
        if quality {
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

/// Sets quality mode on the live encoder; the encoder's answer is the
/// readback. On refusal it switches to the 6 Mbps constant-bitrate fallback,
/// checks both setters and reports the mode and bitrate the encoder reads
/// back. A fallback that cannot be set or confirmed is an error.
fn confirm_rate_control(
    writer: &IMFSinkWriter,
    stream: u32,
) -> Result<(&'static str, u32), String> {
    let codec = codec_api(writer, stream).ok_or_else(|| {
        "the encoder exposes no ICodecAPI: rate control cannot be set or read back".to_owned()
    })?;
    let cbr = eAVEncCommonRateControlMode_CBR.0.cast_unsigned();
    unsafe {
        let quality_accepted = codec
            .SetValue(
                &CODECAPI_AVEncCommonRateControlMode,
                &variant_u32(eAVEncCommonRateControlMode_Quality.0.cast_unsigned()),
            )
            .is_ok()
            && codec
                .SetValue(
                    &CODECAPI_AVEncCommonQuality,
                    &variant_u32(H264_QUALITY_LEVEL),
                )
                .is_ok();
        if quality_accepted {
            return Ok(("quality", H264_QUALITY_LEVEL));
        }
        codec
            .SetValue(&CODECAPI_AVEncCommonRateControlMode, &variant_u32(cbr))
            .map_err(hr("set CODECAPI_AVEncCommonRateControlMode (CBR fallback)"))?;
        codec
            .SetValue(
                &CODECAPI_AVEncCommonMeanBitRate,
                &variant_u32(FALLBACK_BITRATE_BPS),
            )
            .map_err(hr("set CODECAPI_AVEncCommonMeanBitRate (CBR fallback)"))?;
        let mode = codec
            .GetValue(&CODECAPI_AVEncCommonRateControlMode)
            .map_err(hr("read back CODECAPI_AVEncCommonRateControlMode"))?;
        let bitrate = codec
            .GetValue(&CODECAPI_AVEncCommonMeanBitRate)
            .map_err(hr("read back CODECAPI_AVEncCommonMeanBitRate"))?;
        match (variant_as_u32(&mode), variant_as_u32(&bitrate)) {
            (Some(mode), Some(bitrate)) if mode == cbr => Ok(("cbr_bitrate", bitrate)),
            (mode, _) => Err(format!(
                "the CBR fallback did not take effect: the encoder reads back rate control mode {mode:?}"
            )),
        }
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
/// friendly name and whether it is a hardware MFT.
fn encoder_identity(writer: &IMFSinkWriter, stream: u32) -> (String, bool) {
    let Ok(writer_ex) = writer.cast::<IMFSinkWriterEx>() else {
        return ("unknown".to_owned(), false);
    };
    for index in 0..MAX_TRANSFORMS_PER_STREAM {
        let mut category = GUID::zeroed();
        let mut transform = None;
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
        let attributes = unsafe { transform.GetAttributes() }.ok();
        let hardware = attributes.as_ref().is_some_and(|attributes| unsafe {
            attributes
                .GetStringLength(&MFT_ENUM_HARDWARE_URL_Attribute)
                .is_ok()
        });
        let name = attributes
            .as_ref()
            .and_then(|attributes| attribute_string(attributes, &MFT_FRIENDLY_NAME_Attribute))
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

fn frame_time_hns(frame: u64, fps: u32) -> i64 {
    i64::try_from(frame)
        .unwrap_or(i64::MAX)
        .saturating_mul(HNS_PER_SECOND)
        / i64::from(fps.max(1))
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
        let com_initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        if let Err(error) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) } {
            if com_initialized {
                unsafe { CoUninitialize() };
            }
            return Err(format!("MFStartup failed: {error}"));
        }
        Ok(Self { com_initialized })
    }
}

impl Drop for MediaFoundationRuntime {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            if self.com_initialized {
                CoUninitialize();
            }
        }
    }
}
