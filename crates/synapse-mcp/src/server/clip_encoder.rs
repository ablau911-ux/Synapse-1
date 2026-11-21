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
/// Bitrate used when the encoder refuses quality mode (research section 9).
const FALLBACK_BITRATE_BPS: u32 = 6_000_000;
const KEYFRAME_INTERVAL_SECONDS: u32 = 1;
const HNS_PER_SECOND: i64 = 10_000_000;
/// Upper bound on transforms walked when looking for the encoder in the chain.
const MAX_TRANSFORMS_PER_STREAM: u32 = 8;

/// What Media Foundation actually granted for this clip.
#[derive(Clone, Debug)]
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

pub(super) struct ClipEncoder {
    // Field order matters: the writer must be released before the runtime
    // guard shuts Media Foundation down.
    writer: IMFSinkWriter,
    stream: u32,
    fps: u32,
    frames_written: u64,
    readback: EncoderReadback,
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
            _runtime: runtime,
        })
    }

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
    pub(super) fn finish(self) -> Result<EncoderReadback, String> {
        unsafe { self.writer.Finalize() }.map_err(hr("IMFSinkWriter::Finalize"))?;
        Ok(self.readback)
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
        let (mode, value) = confirm_rate_control(&writer, stream);
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
/// readback. On refusal it switches to the 6 Mbps constant-bitrate fallback.
fn confirm_rate_control(writer: &IMFSinkWriter, stream: u32) -> (&'static str, u32) {
    let Some(codec) = codec_api(writer, stream) else {
        return ("cbr_bitrate", FALLBACK_BITRATE_BPS);
    };
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
            return ("quality", H264_QUALITY_LEVEL);
        }
        // The output type already carries MF_MT_AVG_BITRATE; these make the
        // fallback explicit on encoders that honour ICodecAPI.
        let _ = codec.SetValue(
            &CODECAPI_AVEncCommonRateControlMode,
            &variant_u32(eAVEncCommonRateControlMode_CBR.0.cast_unsigned()),
        );
        let _ = codec.SetValue(
            &CODECAPI_AVEncCommonMeanBitRate,
            &variant_u32(FALLBACK_BITRATE_BPS),
        );
    }
    ("cbr_bitrate", FALLBACK_BITRATE_BPS)
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
