//! M8: the Media Foundation H.264 encoder settings the worker will need
//! (plan B6, O4): GOP of `GOP_FRAMES` = 20 encoded frames, B-picture count 0,
//! the screen-content hint (`CODECAPI_AVScenarioInfo` = display remoting),
//! quality mode at 70. Each is handed to the encoder at `SetInputMediaType`
//! (as the product does), then set again through `ICodecAPI` and read back.
//!
//! Runs: the software encoder (hardware transforms off) and the encoder Media
//! Foundation grants with hardware transforms on; each with and without the
//! hint. 100 frames of 1280 x 720 changing text-like content with uneven
//! durations (50, 50, 150, 50, 100 ms ...), as when repeated frames are not
//! encoded. The keyframe positions are read from the output files with
//! ffprobe afterwards (see results-package0.md).

use std::ffi::c_void;
use std::path::Path;

use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonQuality, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncMPVGOPSize, CODECAPI_AVScenarioInfo,
    ICodecAPI, IMFActivate, IMFAttributes, IMFMediaType, IMFSinkWriter, IMFSinkWriterEx,
    MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO,
    MF_MT_SUBTYPE, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_TRANSCODE_CONTAINERTYPE, MF_VERSION,
    MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFMediaType_Video, MFSTARTUP_FULL, MFShutdown, MFStartup,
    MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER,
    MFT_ENUM_FLAG_SYNCMFT, MFT_ENUM_HARDWARE_URL_Attribute, MFT_ENUM_HARDWARE_VENDOR_ID_Attribute,
    MFT_FRIENDLY_NAME_Attribute, MFT_REGISTER_TYPE_INFO, MFTEnumEx,
    MFTranscodeContainerType_FMPEG4, MFVideoFormat_H264, MFVideoFormat_NV12,
    MFVideoInterlace_Progressive, eAVEncCommonRateControlMode_Quality, eAVEncH264VProfile_High,
    eAVScenarioInfo_DisplayRemoting,
};
use windows::Win32::System::Com::{
    COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::System::Variant::{VARIANT, VT_UI4};
use windows::core::{GUID, HSTRING, Interface};

use crate::clock::{self, CycleRate};
use crate::{Result, win};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const FRAMES: usize = 100;
/// GOP_FRAMES = tau * f = 1 s * 20 frames per second.
const GOP_FRAMES: u32 = 20;
const QUALITY: u32 = 70;
const NOMINAL_FPS: u32 = 20;
const DURATIONS_MS: [i64; 5] = [50, 50, 150, 50, 100];
const FALLBACK_BITRATE_BPS: u32 = 6_000_000;
const MAX_TRANSFORMS_PER_STREAM: u32 = 8;

struct Setting {
    name: &'static str,
    key: GUID,
    wanted: u32,
}

fn settings(screen_hint: bool) -> Vec<Setting> {
    let mut list = vec![
        Setting {
            name: "gop_size",
            key: CODECAPI_AVEncMPVGOPSize,
            wanted: GOP_FRAMES,
        },
        Setting {
            name: "b_picture_count",
            key: CODECAPI_AVEncMPVDefaultBPictureCount,
            wanted: 0,
        },
        Setting {
            name: "rate_control_quality",
            key: CODECAPI_AVEncCommonRateControlMode,
            wanted: eAVEncCommonRateControlMode_Quality.0 as u32,
        },
        Setting {
            name: "quality",
            key: CODECAPI_AVEncCommonQuality,
            wanted: QUALITY,
        },
    ];
    if screen_hint {
        list.push(Setting {
            name: "scenario_display_remoting",
            key: CODECAPI_AVScenarioInfo,
            wanted: eAVScenarioInfo_DisplayRemoting.0 as u32,
        });
    }
    list
}

pub fn run() -> Result<()> {
    let _runtime = MediaFoundation::start()?;
    let rate = CycleRate::calibrate();
    list_encoders(
        "hardware",
        MFT_ENUM_FLAG(MFT_ENUM_FLAG_HARDWARE.0 | MFT_ENUM_FLAG_SORTANDFILTER.0),
    );
    list_encoders(
        "software",
        MFT_ENUM_FLAG(MFT_ENUM_FLAG_SYNCMFT.0 | MFT_ENUM_FLAG_SORTANDFILTER.0),
    );
    let folder = std::env::temp_dir().join("clip-probe-encoder");
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("create {}: {error}", folder.display()))?;
    println!("M8 output_folder={}", folder.display());
    for hardware in [false, true] {
        for screen_hint in [false, true] {
            let name = format!(
                "{}-{}",
                if hardware { "hw" } else { "sw" },
                if screen_hint { "hint" } else { "nohint" }
            );
            if let Err(error) = encode(
                &folder.join(format!("{name}.mp4")),
                &name,
                hardware,
                screen_hint,
                rate,
            ) {
                println!("M8 run={name} error=\"{error}\"");
            }
        }
    }
    Ok(())
}

fn encode(
    path: &Path,
    name: &str,
    hardware: bool,
    screen_hint: bool,
    rate: CycleRate,
) -> Result<()> {
    let _ = std::fs::remove_file(path);
    let wanted = settings(screen_hint);
    // SAFETY: Media Foundation calls on this thread, which started the runtime.
    unsafe {
        let attributes = new_attributes()?;
        attributes
            .SetUINT32(
                &MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
                u32::from(hardware),
            )
            .map_err(win("set hardware transforms"))?;
        attributes
            .SetGUID(
                &MF_TRANSCODE_CONTAINERTYPE,
                &MFTranscodeContainerType_FMPEG4,
            )
            .map_err(win("set container"))?;
        let writer = MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attributes)
            .map_err(win("MFCreateSinkWriterFromURL"))?;
        let output = video_type(&MFVideoFormat_H264)?;
        output
            .SetUINT32(&MF_MT_AVG_BITRATE, FALLBACK_BITRATE_BPS)
            .map_err(win("set bitrate"))?;
        output
            .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)
            .map_err(win("set profile"))?;
        let stream = writer.AddStream(&output).map_err(win("AddStream"))?;
        let input = video_type(&MFVideoFormat_NV12)?;
        input
            .SetUINT32(&MF_MT_DEFAULT_STRIDE, WIDTH)
            .map_err(win("set stride"))?;

        let params = new_attributes()?;
        for setting in &wanted {
            params
                .SetUINT32(&setting.key, setting.wanted)
                .map_err(win("set encoder parameter"))?;
        }
        let accepted_at_open = writer.SetInputMediaType(stream, &input, &params).is_ok();
        if !accepted_at_open {
            writer
                .SetInputMediaType(stream, &input, None)
                .map_err(win("SetInputMediaType without parameters"))?;
        }
        let (encoder, is_hardware) = encoder_identity(&writer, stream);
        println!(
            "M8 run={name} encoder=\"{encoder}\" hardware_mft={is_hardware} parameters_accepted_at_open={accepted_at_open}"
        );
        let codec = codec_api(&writer, stream)
            .ok_or_else(|| "the encoder exposes no ICodecAPI".to_owned())?;
        for setting in &wanted {
            let supported = codec.IsSupported(&setting.key).is_ok();
            let set = codec.SetValue(&setting.key, &variant_u32(setting.wanted));
            let read = codec
                .GetValue(&setting.key)
                .ok()
                .as_ref()
                .and_then(variant_as_u32);
            println!(
                "M8 run={name} setting={} wanted={} supported={supported} set_through_codecapi={} read_back={}",
                setting.name,
                setting.wanted,
                set.map_or_else(
                    |error| format!("refused({})", error.code()),
                    |()| "ok".to_owned()
                ),
                read.map_or_else(|| "none".to_owned(), |value| value.to_string())
            );
        }
        writer.BeginWriting().map_err(win("BeginWriting"))?;
        // Frames are made before the clock starts, so the timing holds only the encoder path.
        let frames: Vec<Vec<u8>> = (0..FRAMES).map(nv12_frame).collect();
        let (wall_start, process_start) = (clock::now_hns(), clock::process_cycles());
        let mut time_hns = 0_i64;
        for (index, frame) in frames.iter().enumerate() {
            let duration_hns = DURATIONS_MS[index % DURATIONS_MS.len()] * clock::HNS_PER_MS;
            write_frame(&writer, stream, frame, time_hns, duration_hns)?;
            time_hns += duration_hns;
        }
        writer.Finalize().map_err(win("Finalize"))?;
        let wall_ms = clock::hns_to_ms(clock::now_hns() - wall_start);
        let process_ms = rate.millis(clock::process_cycles() - process_start);
        let bytes = std::fs::metadata(path).map_or(0, |meta| meta.len());
        println!(
            "M8 run={name} frames={FRAMES} bytes={bytes} wall_ms={wall_ms:.1} process_cpu_ms_per_frame={:.3} file={}",
            process_ms / FRAMES as f64,
            path.display()
        );
    }
    Ok(())
}

/// Text-like stripes that move sideways each frame, plus a moving block.
fn nv12_frame(index: usize) -> Vec<u8> {
    let (width, height) = (WIDTH as usize, HEIGHT as usize);
    let mut data = vec![128_u8; width * height * 3 / 2];
    let shift = index * 7;
    for (row, line) in data[..width * height].chunks_exact_mut(width).enumerate() {
        for (column, luma) in line.iter_mut().enumerate() {
            let glyph = ((column + shift) / 5 + row / 9).is_multiple_of(3) && (row / 18) % 2 == 0;
            let block = (column / 64 == index % 20) && (row / 64 == index % 11);
            *luma = if glyph || block { 30 } else { 220 };
        }
    }
    data
}

/// # Safety
/// Media Foundation must be started on this thread.
unsafe fn write_frame(
    writer: &IMFSinkWriter,
    stream: u32,
    nv12: &[u8],
    time_hns: i64,
    duration_hns: i64,
) -> Result<()> {
    let length = u32::try_from(nv12.len()).map_err(|_| "frame too large".to_owned())?;
    // SAFETY: the buffer is locked while the frame is copied into it.
    unsafe {
        let buffer = MFCreateMemoryBuffer(length).map_err(win("MFCreateMemoryBuffer"))?;
        let mut data: *mut u8 = std::ptr::null_mut();
        buffer
            .Lock(&raw mut data, None, None)
            .map_err(win("IMFMediaBuffer::Lock"))?;
        std::ptr::copy_nonoverlapping(nv12.as_ptr(), data, nv12.len());
        buffer.Unlock().map_err(win("IMFMediaBuffer::Unlock"))?;
        buffer
            .SetCurrentLength(length)
            .map_err(win("SetCurrentLength"))?;
        let sample = MFCreateSample().map_err(win("MFCreateSample"))?;
        sample.AddBuffer(&buffer).map_err(win("AddBuffer"))?;
        sample
            .SetSampleTime(time_hns)
            .map_err(win("SetSampleTime"))?;
        sample
            .SetSampleDuration(duration_hns)
            .map_err(win("SetSampleDuration"))?;
        writer
            .WriteSample(stream, &sample)
            .map_err(win("WriteSample"))
    }
}

fn video_type(subtype: &GUID) -> Result<IMFMediaType> {
    // SAFETY: plain attribute setters on a new media type.
    unsafe {
        let media_type = MFCreateMediaType().map_err(win("MFCreateMediaType"))?;
        media_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(win("set major type"))?;
        media_type
            .SetGUID(&MF_MT_SUBTYPE, subtype)
            .map_err(win("set subtype"))?;
        media_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(win("set interlace"))?;
        media_type
            .SetUINT64(
                &MF_MT_FRAME_SIZE,
                (u64::from(WIDTH) << 32) | u64::from(HEIGHT),
            )
            .map_err(win("set frame size"))?;
        media_type
            .SetUINT64(&MF_MT_FRAME_RATE, (u64::from(NOMINAL_FPS) << 32) | 1)
            .map_err(win("set frame rate"))?;
        media_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1_u64 << 32) | 1)
            .map_err(win("set aspect"))?;
        Ok(media_type)
    }
}

fn new_attributes() -> Result<IMFAttributes> {
    let mut attributes = None;
    // SAFETY: one out-parameter.
    unsafe { MFCreateAttributes(&raw mut attributes, 4) }.map_err(win("MFCreateAttributes"))?;
    attributes.ok_or_else(|| "MFCreateAttributes returned nothing".to_owned())
}

fn codec_api(writer: &IMFSinkWriter, stream: u32) -> Option<ICodecAPI> {
    let mut raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: asks the stream's encoder for ICodecAPI; a non-null result is an owned reference.
    unsafe {
        writer
            .GetServiceForStream(stream, &GUID::zeroed(), &ICodecAPI::IID, &raw mut raw)
            .ok()?;
        (!raw.is_null()).then(|| ICodecAPI::from_raw(raw))
    }
}

/// The encoder transform the sink writer inserted: its friendly name (or, when
/// it has none, its hardware vendor id) and whether it is a hardware MFT.
fn encoder_identity(writer: &IMFSinkWriter, stream: u32) -> (String, bool) {
    let Ok(writer) = writer.cast::<IMFSinkWriterEx>() else {
        return ("unknown".to_owned(), false);
    };
    for index in 0..MAX_TRANSFORMS_PER_STREAM {
        let mut category = GUID::zeroed();
        let mut transform = None;
        // SAFETY: out-parameters for the category and the transform.
        if unsafe {
            writer.GetTransformForStream(stream, index, Some(&raw mut category), &raw mut transform)
        }
        .is_err()
        {
            break;
        }
        if category != MFT_CATEGORY_VIDEO_ENCODER {
            continue;
        }
        // SAFETY: plain query.
        let Some(attributes) =
            transform.and_then(|transform| unsafe { transform.GetAttributes() }.ok())
        else {
            continue;
        };
        // SAFETY: plain query.
        let hardware =
            unsafe { attributes.GetStringLength(&MFT_ENUM_HARDWARE_URL_Attribute) }.is_ok();
        let name = attribute_string(&attributes, &MFT_FRIENDLY_NAME_Attribute)
            .or_else(|| {
                attribute_string(&attributes, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute)
                    .map(|vendor| format!("unnamed, vendor {vendor}"))
            })
            .unwrap_or_else(|| "unnamed".to_owned());
        return (name, hardware);
    }
    ("unknown".to_owned(), false)
}

fn list_encoders(label: &str, flags: MFT_ENUM_FLAG) {
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0_u32;
    // SAFETY: out-parameters for an array that is freed below.
    if let Err(error) = unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            None,
            Some(&raw const output),
            &raw mut activates,
            &raw mut count,
        )
    } {
        println!("M8 {label}_encoders error={error}");
        return;
    }
    if activates.is_null() {
        println!("M8 {label}_encoders count=0");
        return;
    }
    // SAFETY: MFTEnumEx returned `count` entries at `activates`.
    let list = unsafe { std::slice::from_raw_parts_mut(activates, count as usize) };
    let names: Vec<String> = list
        .iter_mut()
        .filter_map(Option::take)
        .map(|activate| {
            let name = attribute_string(&activate, &MFT_FRIENDLY_NAME_Attribute)
                .unwrap_or_else(|| "unnamed".to_owned());
            match attribute_string(&activate, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute) {
                Some(vendor) => format!("{name} ({vendor})"),
                None => name,
            }
        })
        .collect();
    // SAFETY: the array MFTEnumEx allocated; its entries were released above.
    unsafe { CoTaskMemFree(Some(activates.cast::<c_void>().cast_const())) };
    println!("M8 {label}_encoders count={count} names={names:?}");
}

pub(crate) fn attribute_string(attributes: &IMFAttributes, key: &GUID) -> Option<String> {
    // SAFETY: the buffer is one longer than the reported length.
    unsafe {
        let length = attributes.GetStringLength(key).ok()?;
        let mut buffer = vec![0_u16; usize::try_from(length).ok()? + 1];
        attributes.GetString(key, &mut buffer, None).ok()?;
        let end = buffer
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    }
}

fn variant_u32(value: u32) -> VARIANT {
    let mut variant = VARIANT::default();
    // SAFETY: writes the tag and the matching union member of a fresh VARIANT.
    unsafe {
        let inner = &mut *variant.Anonymous.Anonymous;
        inner.vt = VT_UI4;
        inner.Anonymous.ulVal = value;
    }
    variant
}

fn variant_as_u32(variant: &VARIANT) -> Option<u32> {
    // SAFETY: the union member is read only when the tag says it is a UI4.
    unsafe {
        let inner = &*variant.Anonymous.Anonymous;
        (inner.vt == VT_UI4).then_some(inner.Anonymous.ulVal)
    }
}

/// COM (multithreaded) and Media Foundation for this thread, released on drop.
pub(crate) struct MediaFoundation;

impl MediaFoundation {
    pub(crate) fn start() -> Result<Self> {
        // SAFETY: paired with CoUninitialize and MFShutdown in Drop.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(win("CoInitializeEx"))?;
            MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(win("MFStartup"))?;
        }
        Ok(Self)
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        // SAFETY: started in `start`.
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}
