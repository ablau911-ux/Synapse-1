//! H.264 segment encoder on the Media Foundation Sink Writer
//! (docs/clip-recorder-plan.md, version 4.1, section B, operations B3 to B7).
//!
//! `encode_segment` turns the raw frames of one segment into one H.264
//! (High profile, yuv420) file:
//!
//! - B3: each pixel is duplicated σ × σ and converted to NV12; above the
//!   encoder bound the largest size within it at the same aspect ratio is used
//!   (condition G is then not held for the segment).
//! - B4: hardware transforms are on; on any encoder failure the segment is
//!   encoded once more with them off; a second failure is the caller's error.
//! - B5: every sample carries its instant as the time within the segment and
//!   its duration up to the next new frame; repeated frames are not encoded.
//! - B6: a keyframe every `GOP_FRAMES` encoded frames, no forced keyframe, no
//!   B-frames (set and read back), the screen-content hint tried and recorded.
//! - B7: quality mode at `H264_QUALITY_LEVEL`, read back; if refused, a
//!   bitrate of β · width · height · f, read back; Q gets an honest status.
//!
//! All calls stay on the thread that created the encoder: it initialises COM
//! (MTA) and Media Foundation for that thread and shuts both down on drop.

use std::path::Path;

use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonQuality,
    CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncMPVDefaultBPictureCount,
    CODECAPI_AVEncMPVGOPSize, CODECAPI_AVScenarioInfo, ICodecAPI, IMFAttributes, IMFMediaType,
    IMFSinkWriter, IMFSinkWriterEx, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_YUV_MATRIX,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_TRANSCODE_CONTAINERTYPE, MF_VERSION,
    MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFMediaType_Video, MFNominalRange_16_235, MFSTARTUP_LITE,
    MFShutdown, MFStartup, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_HARDWARE_URL_Attribute,
    MFT_ENUM_HARDWARE_VENDOR_ID_Attribute, MFT_FRIENDLY_NAME_Attribute,
    MFTranscodeContainerType_FMPEG4, MFTranscodeContainerType_MPEG4, MFVideoFormat_H264,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFVideoTransferMatrix_BT709,
    eAVEncCommonRateControlMode_CBR, eAVEncCommonRateControlMode_Quality, eAVEncH264VProfile_High,
    eAVScenarioInfo_DisplayRemoting,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Variant::{VARIANT, VT_UI4};
use windows::core::{GUID, HSTRING, Interface};

use super::capture_clip::RECORD_FPS;
use super::clip_folder::QualityReport;

/// q: `CODECAPI_AVEncCommonQuality` level (0..=100). Condition Q stays
/// `unverified` until q_min is calibrated for the encoder used (C6).
const H264_QUALITY_LEVEL: u32 = 70;
/// β: bits per pixel and frame for the bitrate fallback (equation section 6).
pub(super) const FALLBACK_BITS_PER_PIXEL_FRAME: f64 = 0.10;
/// τ: the keyframe interval in recorded time.
const KEYFRAME_INTERVAL_MS: u32 = 1_000;
/// GOP: τ · f encoded frames between keyframes (S5). Repeats are not encoded,
/// so keyframes are at least τ apart in time.
pub(super) const GOP_FRAMES: u32 = KEYFRAME_INTERVAL_MS * RECORD_FPS / 1_000;
/// The chosen encoder bound (C16): at most this many 16 × 16 macroblocks.
const MAX_ENCODE_MACROBLOCKS: u64 = 36_864;
/// The chosen encoder bound (C16): each side at most this many pixels.
const MAX_ENCODE_SIDE: u32 = 4_096;
const HNS_PER_MS: i64 = 10_000;
/// Upper bound on transforms walked when looking for the encoder in the chain.
const MAX_TRANSFORMS_PER_STREAM: u32 = 8;

/// One frame of a segment: its raw file (captured BGRA rows), its captured
/// size, and its time within the segment (B5).
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
    pub(super) quality: QualityReport,
}

/// Encodes one segment into `path` (the `.part` file) and finalizes it. B4:
/// on any encoder failure the segment is encoded once more from its raw
/// frames with hardware transforms off.
pub(super) fn encode_segment(path: &Path, input: &SegmentInput) -> Result<EncodedSegment, String> {
    encode_once(path, input, true).or_else(|hardware_error| {
        tracing::warn!(
            code = "CLIP_ENCODER_RETRY_SOFTWARE",
            error = %hardware_error,
            "segment encode failed; retrying once with hardware transforms off"
        );
        let _ = std::fs::remove_file(path);
        encode_once(path, input, false).map_err(|software_error| {
            format!("encoder failed twice: {hardware_error}; without hardware transforms: {software_error}")
        })
    })
}

fn encode_once(
    path: &Path,
    input: &SegmentInput,
    hardware: bool,
) -> Result<EncodedSegment, String> {
    let size = RecordedSize::of(input);
    let mut encoder = ClipEncoder::create(path, size.width, size.height, input.fps, hardware)?;
    let mut canvas = Nv12Canvas::new(&size, input.canvas_width, input.canvas_height, input.scale);
    for sample in input.samples {
        let bgra = std::fs::read(&sample.raw)
            .map_err(|error| format!("could not read {}: {error}", sample.raw.display()))?;
        canvas.draw(&bgra, sample.width, sample.height);
        encoder.write_nv12(
            &canvas.nv12,
            ms_to_hns(sample.local_ms),
            ms_to_hns(sample.duration_ms),
        )?;
    }
    let readback = encoder.finish()?;
    let keyframe_times = super::clip_decoder::keyframe_times_hns(path)?;
    let keyframe_instants = keyframe_times
        .iter()
        .filter_map(|&time| {
            input
                .samples
                .iter()
                .rev()
                .find(|sample| ms_to_hns(sample.local_ms) <= time)
                .map(|sample| sample.instant)
        })
        .collect();
    Ok(EncodedSegment {
        width: size.width,
        height: size.height,
        bound_held: size.bound_held,
        keyframe_instants,
        encoder: readback.encoder,
        hardware: readback.hardware,
        quality: readback.quality,
    })
}

fn ms_to_hns(ms: u64) -> i64 {
    i64::try_from(ms)
        .unwrap_or(i64::MAX / HNS_PER_MS)
        .saturating_mul(HNS_PER_MS)
}

/// B3: the recorded size σ · canvas, or the largest size within the encoder
/// bound at the same aspect ratio; both even.
struct RecordedSize {
    width: u32,
    height: u32,
    bound_held: bool,
}

impl RecordedSize {
    fn of(input: &SegmentInput) -> Self {
        let width = u64::from(input.canvas_width.max(1)) * u64::from(input.scale.max(1));
        let height = u64::from(input.canvas_height.max(1)) * u64::from(input.scale.max(1));
        let macroblocks = width.div_ceil(16) * height.div_ceil(16);
        let side = u64::from(MAX_ENCODE_SIDE);
        if macroblocks <= MAX_ENCODE_MACROBLOCKS && width <= side && height <= side {
            return Self {
                width: even(width),
                height: even(height),
                bound_held: true,
            };
        }
        let area_factor = (MAX_ENCODE_MACROBLOCKS as f64 * 256.0 / (width * height) as f64).sqrt();
        let factor = area_factor
            .min(side as f64 / width as f64)
            .min(side as f64 / height as f64);
        Self {
            width: even((width as f64 * factor).floor() as u64),
            height: even((height as f64 * factor).floor() as u64),
            bound_held: false,
        }
    }
}

fn even(value: u64) -> u32 {
    u32::try_from(value.max(2) & !1).unwrap_or(!1)
}

/// The picture the encoder sees, kept as NV12 (BT.709 limited range). Frames
/// are placed at the top left with each pixel duplicated σ × σ, never scaled
/// otherwise (E4); the rest of a larger canvas is black. Above the encoder
/// bound the same map picks the nearest source pixel.
struct Nv12Canvas {
    width: usize,
    height: usize,
    /// Source column and row of each output column and row.
    column_source: Vec<usize>,
    row_source: Vec<usize>,
    nv12: Vec<u8>,
}

impl Nv12Canvas {
    fn new(size: &RecordedSize, canvas_width: u32, canvas_height: u32, scale: u32) -> Self {
        let (width, height) = (size.width as usize, size.height as usize);
        let map = |out: usize, canvas: u32| -> Vec<usize> {
            (0..out)
                .map(|at| {
                    if size.bound_held {
                        at / scale.max(1) as usize
                    } else {
                        at * canvas as usize / out.max(1)
                    }
                })
                .collect()
        };
        Self {
            width,
            height,
            column_source: map(width, canvas_width),
            row_source: map(height, canvas_height),
            nv12: vec![0; width * height * 3 / 2],
        }
    }

    /// Draws a BGRA frame of `source_width` × `source_height`, converting to
    /// NV12 with chroma averaged over each 2 × 2 block.
    fn draw(&mut self, bgra: &[u8], source_width: u32, source_height: u32) {
        let (width, height) = (self.width, self.height);
        let (source_width, source_height) = (source_width as usize, source_height as usize);
        let (columns, rows) = (&self.column_source, &self.row_source);
        let rgb_at = |col: usize, row: usize| -> (i32, i32, i32) {
            let (x, y) = (columns[col.min(width - 1)], rows[row.min(height - 1)]);
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

/// What Media Foundation granted for a segment, read back.
struct EncoderReadback {
    encoder: String,
    hardware: bool,
    quality: QualityReport,
}

struct ClipEncoder {
    // Field order matters: the writer must be released before the runtime
    // guard shuts Media Foundation down.
    writer: IMFSinkWriter,
    stream: u32,
    readback: EncoderReadback,
    /// `true` once `Finalize` ran; otherwise drop finalizes (error exits).
    finalized: bool,
    _runtime: MediaFoundationRuntime,
}

impl ClipEncoder {
    /// Opens `path` for one H.264 segment of `width` × `height` (even).
    fn create(
        path: &Path,
        width: u32,
        height: u32,
        fps: u32,
        hardware: bool,
    ) -> Result<Self, String> {
        let runtime = MediaFoundationRuntime::start()?;
        let (writer, stream) = open_writer(
            path,
            MFTranscodeContainerType_FMPEG4,
            width,
            height,
            fps,
            hardware,
        )
        .or_else(|fragmented_error| {
            tracing::warn!(
                code = "CLIP_ENCODER_FMP4_REFUSED",
                error = %fragmented_error,
                "fragmented MP4 refused; falling back to normal MP4"
            );
            let _ = std::fs::remove_file(path);
            open_writer(
                path,
                MFTranscodeContainerType_MPEG4,
                width,
                height,
                fps,
                hardware,
            )
            .map_err(|mp4_error| format!("fragmented MP4: {fragmented_error}; MP4: {mp4_error}"))
        })?;
        let quality = confirm_settings(&writer, stream, width, height, fps)?;
        // SAFETY: the writer is configured; this starts the session.
        unsafe { writer.BeginWriting() }.map_err(hr("IMFSinkWriter::BeginWriting"))?;
        let (encoder, is_hardware) = encoder_identity(&writer, stream);
        Ok(Self {
            writer,
            stream,
            readback: EncoderReadback {
                encoder,
                hardware: is_hardware,
                quality,
            },
            finalized: false,
            _runtime: runtime,
        })
    }

    /// Encodes one NV12 frame (`width*height*3/2` bytes) at `time_hns` within
    /// the segment, shown for `duration_hns` (B5).
    fn write_nv12(&mut self, nv12: &[u8], time_hns: i64, duration_hns: i64) -> Result<(), String> {
        let length = u32::try_from(nv12.len()).map_err(|_| "NV12 frame too large".to_owned())?;
        // SAFETY: the buffer is locked for exactly `length` bytes while copied.
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
                .SetSampleDuration(duration_hns.max(1))
                .map_err(hr("IMFSample::SetSampleDuration"))?;
            self.writer
                .WriteSample(self.stream, &sample)
                .map_err(hr("IMFSinkWriter::WriteSample"))?;
        }
        Ok(())
    }

    /// Flushes the encoder and closes the file.
    fn finish(mut self) -> Result<EncoderReadback, String> {
        self.finalized = true;
        // SAFETY: the writer is open; Finalize flushes and closes it.
        unsafe { self.writer.Finalize() }.map_err(hr("IMFSinkWriter::Finalize"))?;
        Ok(EncoderReadback {
            encoder: std::mem::take(&mut self.readback.encoder),
            hardware: self.readback.hardware,
            quality: std::mem::take(&mut self.readback.quality),
        })
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
        // SAFETY: the writer is open; Finalize closes it.
        if let Err(error) = unsafe { self.writer.Finalize() } {
            tracing::debug!(
                code = "CLIP_ENCODER_FINALIZE_ON_DROP_FAILED",
                error = %error,
                "finalize after an earlier clip error failed"
            );
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
    hardware: bool,
) -> Result<(IMFSinkWriter, u32), String> {
    // SAFETY: Media Foundation calls on attribute stores and media types this
    // function owns; every result is checked.
    unsafe {
        let attributes = new_attributes(2)?;
        attributes
            .SetUINT32(
                &MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
                u32::from(hardware),
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
            .SetUINT32(&MF_MT_AVG_BITRATE, fallback_bitrate(width, height, fps))
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
            .SetInputMediaType(stream, &input, &encoder_params(true)?)
            .is_err()
        {
            writer
                .SetInputMediaType(stream, &input, &encoder_params(false)?)
                .map_err(hr("IMFSinkWriter::SetInputMediaType"))?;
        }
        Ok((writer, stream))
    }
}

/// β · width · height · f, in bits per second (B7).
fn fallback_bitrate(width: u32, height: u32, fps: u32) -> u32 {
    let bits =
        FALLBACK_BITS_PER_PIXEL_FRAME * f64::from(width) * f64::from(height) * f64::from(fps);
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
fn encoder_params(full: bool) -> Result<IMFAttributes, String> {
    // SAFETY: setters on an attribute store this function owns.
    unsafe {
        let params = new_attributes(5)?;
        params
            .SetUINT32(&CODECAPI_AVEncMPVGOPSize, GOP_FRAMES)
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

/// B6 and B7 on the live encoder: every setting is set and read back. GOP and
/// zero B-frames must hold (variable time stamps assume no reordering); the
/// screen-content hint is recorded as accepted or not; quality mode, if
/// refused, becomes the bitrate fallback, which must read back.
fn confirm_settings(
    writer: &IMFSinkWriter,
    stream: u32,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<QualityReport, String> {
    let codec = codec_api(writer, stream).ok_or_else(|| {
        "the encoder exposes no ICodecAPI: its settings cannot be set or read back".to_owned()
    })?;
    let gop_frames = set_and_read(&codec, &CODECAPI_AVEncMPVGOPSize, GOP_FRAMES)
        .ok_or("the encoder did not accept or read back the GOP size")?;
    let b_frames = set_and_read(&codec, &CODECAPI_AVEncMPVDefaultBPictureCount, 0)
        .ok_or("the encoder did not accept or read back a B-frame count of 0")?;
    if b_frames != 0 {
        return Err(format!(
            "the encoder reads back {b_frames} B-frames; 0 is required"
        ));
    }
    let hint = eAVScenarioInfo_DisplayRemoting.0.cast_unsigned();
    let screen_content_hint = set_and_read(&codec, &CODECAPI_AVScenarioInfo, hint) == Some(hint);
    let quality_mode = eAVEncCommonRateControlMode_Quality.0.cast_unsigned();
    let quality = set_and_read(&codec, &CODECAPI_AVEncCommonRateControlMode, quality_mode)
        .filter(|mode| *mode == quality_mode)
        .and_then(|_| set_and_read(&codec, &CODECAPI_AVEncCommonQuality, H264_QUALITY_LEVEL));
    if let Some(level) = quality {
        return Ok(QualityReport {
            mode: "quality".to_owned(),
            value: level,
            gop_frames,
            b_frames,
            screen_content_hint,
            // q_min is not calibrated yet (C6).
            status: "unverified".to_owned(),
        });
    }
    let cbr = eAVEncCommonRateControlMode_CBR.0.cast_unsigned();
    let bitrate = fallback_bitrate(width, height, fps);
    let mode = set_and_read(&codec, &CODECAPI_AVEncCommonRateControlMode, cbr);
    let read_bitrate = set_and_read(&codec, &CODECAPI_AVEncCommonMeanBitRate, bitrate);
    match (mode, read_bitrate) {
        (Some(mode), Some(read)) if mode == cbr => Ok(QualityReport {
            mode: "bitrate".to_owned(),
            value: read,
            gop_frames,
            b_frames,
            screen_content_hint,
            status: "not_held".to_owned(),
        }),
        (mode, _) => Err(format!(
            "quality mode was refused and the bitrate fallback did not take effect (mode read back {mode:?})"
        )),
    }
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
