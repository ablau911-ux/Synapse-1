//! `clip-probe exp-d`: experiments steps 2 and 3, way D
//! (docs/clip-recorder-experiments-plan.md, equation 7, factors D1 to D5,
//! procedure 2 and 3; docs/clip-recorder-ways.md DW1 to DW4).
//!
//! One run is one process on one corpus folder:
//!
//! 1. Activation: the named H.264 encoder is found by name and vendor in the
//!    Media Foundation enumeration and activated from its enumeration object.
//! 2. Live part: every new frame goes through the copied product
//!    `WaitingFrameWriter` (B, G, R, zstd −3) into `scratch/waiting/`. CPU is the
//!    process cycle counter summed around those calls (a_rt, `exp_cpu`, the
//!    same reading as way Z; the `GetProcessTimes` sum is kept as
//!    `a_rt_gpt_ns`), RAM the peak private bytes above the baseline (M_live).
//! 3. Worker: the copied product `encode_once` (via `probe_encode_once`) reads
//!    the waiting frames back, scales by σ, converts with `Nv12Canvas::draw` and
//!    encodes on the Sink Writer with the run's settings and the frames'
//!    instants as time stamps (a_w, M_worker).
//! 4. Read back from the file: frames, keyframes (clean points), B slices.
//! 5. Decode with the copied `ClipDecoder`; PSNR-Y and SSIM-Y per frame against
//!    the corpus frame scaled by σ (the copied `probe_reference_luma`, so Y uses
//!    the product's matrix). SSIM: 8 × 8 uniform windows at a stride of 4.
//! 6. R by Windows.Media.Ocr on 10 evenly spaced new frames (typing, scroll,
//!    ui); flashes by a pixel test in the flash box (flash scenario).
//!
//! It prints one JSON line and deletes the scratch run folder.
//!
//! Differences from the product's encoder path (named in the results):
//! - the encoder is chosen by the run: hardware transforms on and a DXGI
//!   device manager on the adapter of the encoder's vendor (DW1, DW3), or
//!   hardware transforms off (DW2); the product lets the Sink Writer choose and
//!   retries in software on failure (`encode_segment`), which is not used;
//! - DW4 (Microsoft AVC DX12) cannot be chosen through the Sink Writer: it is
//!   activated and driven directly with one system-memory NV12 frame to confirm
//!   whether it takes one; no clip is made for it;
//! - one segment per run: the whole folder, without the segment split.

#[allow(dead_code)]
#[path = "capture_clip.rs"]
mod capture_clip;
#[allow(dead_code)]
#[path = "clip_decoder.rs"]
mod clip_decoder;
#[allow(dead_code)]
#[path = "clip_encoder.rs"]
mod clip_encoder;
#[allow(dead_code)]
#[path = "clip_folder.rs"]
mod clip_folder;

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
    D3D11CreateDevice, ID3D11Device, ID3D11Multithread,
};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFDXGIDeviceManager, IMFMediaEventGenerator, IMFMediaType, IMFSample,
    IMFTransform, METransformHaveOutput, METransformNeedInput, MF_EVENT_FLAG_NO_WAIT,
    MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_MPEG2_PROFILE, MF_MT_SUBTYPE, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR, MF_TRANSFORM_ASYNC_UNLOCK,
    MF_TRANSFORM_FLAGS_Attribute, MFCreateDXGIDeviceManager, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSourceReaderFromURL, MFMediaType_Video,
    MFSampleExtension_CleanPoint, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_ALL,
    MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_HARDWARE_VENDOR_ID_Attribute, MFT_FRIENDLY_NAME_Attribute,
    MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_REGISTER_TYPE_INFO, MFTEnumEx, MFVideoFormat_H264,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive, eAVEncH264VProfile_High,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::core::{HSTRING, Interface};

use self::clip_encoder::{MediaFoundationRuntime, SegmentInput, SegmentSample};
use crate::Result;
use crate::encoder::attribute_string;
use crate::exp_cpu::Spent;
use crate::exp_meter::{Args, MemorySampler, memory_now};
use crate::exp_settings::{Codec, EncodeSettings};

const INSTANT_MS: u64 = 50;
const SSIM_WINDOW: usize = 8;
const SSIM_STRIDE: usize = 4;
const SSIM_C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
const SSIM_C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);
const OCR_FRAMES: usize = 10;
const TEXT_SCENARIOS: [&str; 3] = ["typing", "scroll", "ui"];
const DISTORTION_D: u8 = 4;
/// DW4 confirm: how long to wait for one encoder event.
const EVENT_WAIT: Duration = Duration::from_secs(3);
/// Flash pixel test: per-channel tolerance around the flash colour.
const FLASH_TOLERANCE: i32 = 60;

pub fn run() -> Result<()> {
    let args = Args::from_env();
    let _runtime = MediaFoundationRuntime::start()?;
    if args.text_or("mode", "run") == "meter" {
        return meter_self_check(&args);
    }
    let mut out = Json::default();
    let outcome = run_way_d(&args, &mut out);
    if let Err(error) = outcome {
        if !out.has("status") {
            out.text("status", "error");
        }
        out.text("error", &error);
    }
    println!("{}", out.finish());
    if let Ok(scratch) = args.text("scratch") {
        let _ = std::fs::remove_dir_all(scratch);
    }
    Ok(())
}

/// One corpus folder: its size, σ and new frames.
struct Folder {
    dir: PathBuf,
    scenario: String,
    width: u32,
    height: u32,
    sigma: u32,
    /// (instant, corpus frame index) of every new frame, in order.
    new_frames: Vec<(u64, u64)>,
    instants: u64,
}

impl Folder {
    fn open(dir: &Path) -> Result<Self> {
        let meta = std::fs::read_to_string(dir.join("meta.json")).map_err(|e| format!("meta.json: {e}"))?;
        let scenario = json_str(&meta, "scenario").ok_or("meta.json has no scenario")?;
        let window = json_array(&meta, "window").ok_or("meta.json has no window")?;
        let sigma = json_num(&meta, "sigma").unwrap_or(1.0) as u32;
        let instants = json_num(&meta, "instants").unwrap_or(400.0) as u64;
        let lines = std::fs::read_to_string(dir.join("frames.jsonl")).map_err(|e| format!("frames.jsonl: {e}"))?;
        let new_frames = lines
            .lines()
            .filter(|l| l.contains("\"new\":true"))
            .filter_map(|l| Some((json_num(l, "instant")? as u64, json_num(l, "frame")? as u64)))
            .collect();
        Ok(Self {
            dir: dir.to_owned(),
            scenario,
            width: window[0] as u32,
            height: window[1] as u32,
            sigma: sigma.max(1),
            new_frames,
            instants,
        })
    }

    fn frame_bgr(&self, frame: u64) -> Result<Vec<u8>> {
        let path = self.dir.join(format!("frame-{frame:04}.zst"));
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let size = (self.width * self.height * 3) as usize;
        zstd::bulk::decompress(&bytes, size).map_err(|e| format!("{}: {e}", path.display()))
    }
}

fn run_way_d(args: &Args, out: &mut Json) -> Result<()> {
    let folder = Folder::open(Path::new(args.text("folder")?))?;
    let way = args.text("encoder")?.to_owned();
    let name = args.text_or("name", "").to_owned();
    let vendor = args.text_or("vendor", "").to_owned();
    let hardware = args.flag("hw", !vendor.is_empty());
    let rate = args.text_or("rate", "quality").to_owned();
    let q: u32 = args.number("q", 70)?;
    let tau: u32 = args.number("gop", 20)?;
    let hint = args.flag("hint", true);
    let beta: f64 = args.number("beta", 0.10)?;
    let scratch = PathBuf::from(args.text("scratch")?);
    // Cycles per second (`clip-probe exp-rate`), shared with way Z (`exp_cpu`).
    let cycle_rate = args.rate()?;
    out.float("cycle_rate", cycle_rate);
    out.text("folder", &folder.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    out.text("scenario", &folder.scenario);
    out.text("encoder", &way);
    out.num("width", folder.width);
    out.num("height", folder.height);
    out.num("sigma", folder.sigma);
    out.num("q", if rate == "quality" { q } else { 0 });
    out.num("tau_f", tau);
    out.num("hint", u32::from(hint));
    out.text("rate", &rate);
    out.float("beta", if rate == "bitrate" { beta } else { 0.0 });
    out.num("n_new", folder.new_frames.len());

    // 1. Activation from the enumeration object.
    let activate = find_encoder(&name, &vendor, hardware)?;
    let activation = unsafe { activate.ActivateObject::<IMFTransform>() };
    match &activation {
        Ok(_) => {
            out.num("activated", 1);
            out.text("hresult", "0x00000000");
        }
        Err(error) => {
            out.num("activated", 0);
            out.text("hresult", &format!("{:#010x}", error.code().0));
            out.text("status", "not_activated");
            return Err(format!("activation failed: {error}"));
        }
    }
    let transform = activation.map_err(|e| e.to_string())?;
    if vendor == "VEN_1414" {
        // DW4: no Sink Writer path; confirm whether it takes system-memory NV12.
        let (w, h) = recorded_size(&folder);
        let confirm = confirm_system_nv12(&transform, w, h);
        let _ = unsafe { activate.ShutdownObject() };
        out.text("dw4_system_nv12", &confirm.clone().unwrap_or_else(|e| format!("no: {e}")));
        out.text("status", "no_product_path");
        return Err(format!(
            "DW4 activates; system-memory NV12: {}; the product's Sink Writer cannot select it",
            confirm.unwrap_or_else(|e| format!("no ({e})"))
        ));
    }
    drop(transform);
    let _ = unsafe { activate.ShutdownObject() };

    // Settings of the run, read by the copied encoder and waiting-frame writer.
    let manager = if hardware { Some(device_manager(&vendor)?) } else { None };
    crate::exp_settings::set_d3d_manager(manager);
    crate::exp_settings::install(EncodeSettings {
        codec: Codec::H264,
        hardware,
        adapter_luid: None,
        quality: (rate == "quality").then_some(q),
        bits_per_pixel_frame: beta,
        gop_frames: tau,
        screen_hint: hint,
        low_latency: false,
        zstd_level: -3,
        raw_waiting: false,
    });

    // 2. Live part: zstd −3 of every new frame through the product's writer.
    std::fs::create_dir_all(scratch.join(clip_folder::WAITING_DIR)).map_err(|e| format!("scratch: {e}"))?;
    let sampler = MemorySampler::start();
    let mut writer = clip_folder::WaitingFrameWriter::new()?;
    let px = (folder.width * folder.height) as usize;
    let mut bgra = vec![255_u8; px * 4];
    let live_base = memory_now().1;
    sampler.take_peaks();
    let live_wall = Instant::now();
    let (mut live, mut waiting_bytes) = (Spent::default(), 0_u64);
    for &(instant, frame) in &folder.new_frames {
        let bgr = folder.frame_bgr(frame)?;
        for (pixel, source) in bgra.chunks_exact_mut(4).zip(bgr.chunks_exact(3)) {
            pixel[..3].copy_from_slice(source);
        }
        waiting_bytes += live.time(|| writer.write(&scratch, instant, &bgra))?;
    }
    let t_live = live_wall.elapsed().as_nanos();
    let m_live = sampler.take_peaks().1.saturating_sub(live_base);
    drop(writer);
    drop(bgra);
    out.num("waiting_bytes", waiting_bytes);
    out.num("t_live_ns", t_live);
    out.num("a_rt_ns", live.cpu_from_cycles(cycle_rate));
    out.num("a_rt_gpt_ns", live.cpu_ns);
    out.num("m_live", m_live);

    // 3. Worker: the copied product encoder on the waiting frames.
    let samples = segment_samples(&folder, &scratch);
    let input = SegmentInput {
        canvas_width: folder.width,
        canvas_height: folder.height,
        scale: folder.sigma,
        fps: capture_clip::RECORD_FPS,
        samples: &samples,
        seed: None,
    };
    let clip = scratch.join("clip.mp4");
    let worker_base = memory_now().1;
    sampler.take_peaks();
    let mut worker = Spent::default();
    let encoded = worker.time(|| clip_encoder::probe_encode_once(&clip, &input, hardware))?;
    let (a_w, t_worker) = (worker.cpu_from_cycles(cycle_rate), worker.wall_ns);
    out.num("a_w_gpt_ns", worker.cpu_ns);
    let m_worker = sampler.take_peaks().1.saturating_sub(worker_base);
    out.num("out_w", encoded.width);
    out.num("out_h", encoded.height);
    out.text("encoder_used", &encoded.encoder);
    out.num("hardware_used", u32::from(encoded.hardware));
    out.num("t_worker_ns", t_worker);
    out.num("a_w_ns", a_w);
    out.num("m_worker", m_worker);
    let quality = &encoded.quality;
    out.num("gop_read", quality.gop_frames);
    out.num("b_frames_read", quality.b_frames);
    out.num("hint_accepted", u32::from(quality.screen_content_hint));
    out.text("rate_read", &quality.mode);
    out.num("rate_value", quality.value);
    out.num("keyframes_segment", encoded.keyframe_instants.len());
    // With hardware transforms off the Sink Writer can load only a software
    // encoder, and the software H.264 MFT reports no friendly name.
    let used_matches = encoded.encoder.contains(&name)
        || (!vendor.is_empty() && encoded.encoder.contains(&vendor))
        || (!hardware && !encoded.hardware);
    out.num("encoder_matches", u32::from(used_matches));

    // 4. Read back from the file.
    let bytes = std::fs::metadata(&clip).map_err(|e| format!("clip: {e}"))?.len();
    out.num("bytes", bytes);
    out.text("clip_hash", &format!("{:016x}", fnv1a(&std::fs::read(&clip).unwrap_or_default())));
    let back = read_back(&clip)?;
    out.num("frames", back.frames);
    out.num("keyframes", back.keyframes);
    out.num("b_frames", back.b_slices);
    out.num("max_gop", back.max_gop);

    // 5 and 6. Decode, quality, R and flashes.
    let quality = measure_quality(&folder, &input, &clip, &sampler, cycle_rate)?;
    quality.put(out);
    out.text("status", if used_matches { "ok" } else { "wrong_encoder" });
    Ok(())
}

/// σ · window, even, as the copied `RecordedSize` gives within the bound.
fn recorded_size(folder: &Folder) -> (u32, u32) {
    let even = |v: u32| (v + 1) & !1;
    (even(folder.width * folder.sigma), even(folder.height * folder.sigma))
}

/// The segment samples of the whole folder: instants within the clip and the
/// duration until the next new frame (B5).
fn segment_samples(folder: &Folder, scratch: &Path) -> Vec<SegmentSample> {
    let first = folder.new_frames.first().map_or(0, |f| f.0);
    folder
        .new_frames
        .iter()
        .enumerate()
        .map(|(order, &(instant, _))| {
            let next = folder.new_frames.get(order + 1).map_or(folder.instants, |f| f.0);
            SegmentSample {
                raw: clip_folder::raw_frame_path(scratch, instant),
                width: folder.width,
                height: folder.height,
                instant,
                local_ms: (instant - first) * INSTANT_MS,
                duration_ms: (next - instant) * INSTANT_MS,
            }
        })
        .collect()
}

/// The H.264 encoder whose friendly name contains `name` (ASCII letters only
/// compared) and whose vendor id is `vendor` (empty: a software encoder).
fn find_encoder(name: &str, vendor: &str, hardware: bool) -> Result<IMFActivate> {
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0_u32;
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_ALL,
            None,
            Some(&raw const output),
            &raw mut activates,
            &raw mut count,
        )
    }
    .map_err(|e| format!("MFTEnumEx failed: {e}"))?;
    if activates.is_null() {
        return Err("no H.264 encoder enumerated".to_owned());
    }
    let list = unsafe { std::slice::from_raw_parts_mut(activates, count as usize) };
    let ascii = |text: &str| text.chars().filter(char::is_ascii).collect::<String>();
    let wanted = ascii(name);
    let mut found = None;
    for activate in list.iter_mut().filter_map(Option::take) {
        let friendly = attribute_string(&activate, &MFT_FRIENDLY_NAME_Attribute).unwrap_or_default();
        let id = attribute_string(&activate, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute).unwrap_or_default();
        let flags = unsafe { activate.GetUINT32(&MF_TRANSFORM_FLAGS_Attribute) }.unwrap_or(0);
        let is_hardware = flags & MFT_ENUM_FLAG_HARDWARE.0 as u32 != 0;
        if found.is_none() && ascii(&friendly).contains(&wanted) && id == vendor && is_hardware == hardware {
            found = Some(activate);
        }
    }
    unsafe { CoTaskMemFree(Some(activates.cast::<c_void>().cast_const())) };
    found.ok_or_else(|| format!("no H.264 encoder named '{name}' with vendor '{vendor}' (hardware {hardware})"))
}

/// A DXGI device manager on the first adapter of `vendor` (`VEN_8086`).
fn device_manager(vendor: &str) -> Result<IMFDXGIDeviceManager> {
    let id = u32::from_str_radix(vendor.trim_start_matches("VEN_"), 16).map_err(|_| format!("vendor {vendor}"))?;
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(|e| format!("CreateDXGIFactory1: {e}"))?;
    for index in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        let desc = unsafe { adapter.GetDesc1() }.map_err(|e| format!("GetDesc1: {e}"))?;
        if desc.VendorId != id {
            continue;
        }
        let adapter: IDXGIAdapter = adapter.cast().map_err(|e| format!("IDXGIAdapter: {e}"))?;
        let mut device: Option<ID3D11Device> = None;
        unsafe {
            D3D11CreateDevice(
                Some(&adapter),
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&raw mut device),
                None,
                None,
            )
        }
        .map_err(|e| format!("D3D11CreateDevice: {e}"))?;
        let device = device.ok_or("D3D11CreateDevice returned no device")?;
        if let Ok(multithread) = device.cast::<ID3D11Multithread>() {
            let _ = unsafe { multithread.SetMultithreadProtected(true) };
        }
        let (mut token, mut manager) = (0_u32, None);
        unsafe { MFCreateDXGIDeviceManager(&raw mut token, &raw mut manager) }
            .map_err(|e| format!("MFCreateDXGIDeviceManager: {e}"))?;
        let manager: IMFDXGIDeviceManager = manager.ok_or("no device manager")?;
        unsafe { manager.ResetDevice(&device, token) }.map_err(|e| format!("ResetDevice: {e}"))?;
        return Ok(manager);
    }
    Err(format!("no adapter of {vendor}"))
}

/// DW4: does the activated encoder take one system-memory NV12 frame without
/// a D3D device manager? Returns the step reached, or the failing step and
/// its HRESULT.
fn confirm_system_nv12(transform: &IMFTransform, width: u32, height: u32) -> Result<String> {
    let step = |name: &'static str| move |e: windows::core::Error| format!("{name} {:#010x}", e.code().0);
    unsafe {
        let attributes = transform.GetAttributes().map_err(step("GetAttributes"))?;
        attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1).map_err(step("MF_TRANSFORM_ASYNC_UNLOCK"))?;
        let output = media_type(&MFVideoFormat_H264, width, height)?;
        output.SetUINT32(&MF_MT_AVG_BITRATE, (0.10 * f64::from(width * height) * 20.0) as u32).map_err(step("AVG_BITRATE"))?;
        output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32).map_err(step("PROFILE"))?;
        transform.SetOutputType(0, &output, 0).map_err(step("SetOutputType"))?;
        let input = media_type(&MFVideoFormat_NV12, width, height)?;
        transform.SetInputType(0, &input, 0).map_err(step("SetInputType(NV12)"))?;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).map_err(step("BEGIN_STREAMING"))?;
        transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).map_err(step("START_OF_STREAM"))?;
        let events: IMFMediaEventGenerator = transform.cast().map_err(step("IMFMediaEventGenerator"))?;
        wait_event(&events, METransformNeedInput.0 as u32)?;
        let length = width * height * 3 / 2;
        let buffer = MFCreateMemoryBuffer(length).map_err(step("MFCreateMemoryBuffer"))?;
        buffer.SetCurrentLength(length).map_err(step("SetCurrentLength"))?;
        let sample: IMFSample = MFCreateSample().map_err(step("MFCreateSample"))?;
        sample.AddBuffer(&buffer).map_err(step("AddBuffer"))?;
        sample.SetSampleTime(0).map_err(step("SetSampleTime"))?;
        sample.SetSampleDuration(500_000).map_err(step("SetSampleDuration"))?;
        transform.ProcessInput(0, &sample, 0).map_err(step("ProcessInput(system NV12)"))?;
        transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0).map_err(step("DRAIN"))?;
        wait_event(&events, METransformHaveOutput.0 as u32)?;
    }
    Ok("yes: ProcessInput accepted a system-memory NV12 sample and the encoder signalled output".to_owned())
}

fn media_type(subtype: &windows::core::GUID, width: u32, height: u32) -> Result<IMFMediaType> {
    let step = |name: &'static str| move |e: windows::core::Error| format!("{name} {:#010x}", e.code().0);
    unsafe {
        let media = MFCreateMediaType().map_err(step("MFCreateMediaType"))?;
        media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(step("MAJOR_TYPE"))?;
        media.SetGUID(&MF_MT_SUBTYPE, subtype).map_err(step("SUBTYPE"))?;
        media.SetUINT64(&MF_MT_FRAME_SIZE, (u64::from(width) << 32) | u64::from(height)).map_err(step("FRAME_SIZE"))?;
        media.SetUINT64(&MF_MT_FRAME_RATE, (20_u64 << 32) | 1).map_err(step("FRAME_RATE"))?;
        media
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(step("INTERLACE_MODE"))?;
        Ok(media)
    }
}

fn wait_event(events: &IMFMediaEventGenerator, wanted: u32) -> Result<()> {
    let start = Instant::now();
    let mut seen = Vec::new();
    while start.elapsed() < EVENT_WAIT {
        match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
            Ok(event) => {
                let kind = unsafe { event.GetType() }.unwrap_or(0);
                if kind == wanted {
                    return Ok(());
                }
                seen.push(kind);
            }
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    Err(format!("event {wanted} not signalled within {EVENT_WAIT:?} (seen {seen:?})"))
}

/// What the clip file holds, read from its compressed samples.
struct ReadBack {
    frames: u64,
    keyframes: u64,
    b_slices: u64,
    max_gop: u64,
}

fn read_back(clip: &Path) -> Result<ReadBack> {
    let url = HSTRING::from(clip.as_os_str());
    let reader = unsafe { MFCreateSourceReaderFromURL(&url, None) }.map_err(|e| format!("source reader: {e}"))?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let mut back = ReadBack { frames: 0, keyframes: 0, b_slices: 0, max_gop: 0 };
    let mut since_key = 0_u64;
    loop {
        let (mut flags, mut time) = (0_u32, 0_i64);
        let mut sample: Option<IMFSample> = None;
        unsafe {
            reader.ReadSample(stream, 0, None, Some(&raw mut flags), Some(&raw mut time), Some(&raw mut sample))
        }
        .map_err(|e| format!("ReadSample: {e}"))?;
        if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
            return Err("read back: stream error".to_owned());
        }
        if let Some(sample) = sample {
            back.frames += 1;
            if unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint) }.unwrap_or(0) != 0 {
                back.keyframes += 1;
                since_key = 0;
            }
            since_key += 1;
            back.max_gop = back.max_gop.max(since_key);
            let data = sample_bytes(&sample)?;
            back.b_slices += slice_types(&data).iter().filter(|&&t| t == 1).count() as u64;
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            return Ok(back);
        }
    }
}

fn sample_bytes(sample: &IMFSample) -> Result<Vec<u8>> {
    unsafe {
        let buffer = sample.ConvertToContiguousBuffer().map_err(|e| format!("buffer: {e}"))?;
        let (mut data, mut length) = (std::ptr::null_mut::<u8>(), 0_u32);
        buffer.Lock(&raw mut data, None, Some(&raw mut length)).map_err(|e| format!("Lock: {e}"))?;
        let bytes = if data.is_null() { Vec::new() } else { std::slice::from_raw_parts(data, length as usize).to_vec() };
        let _ = buffer.Unlock();
        Ok(bytes)
    }
}

/// slice_type mod 5 (0 P, 1 B, 2 I) of every slice NAL unit of one access
/// unit, in Annex B or 4-byte length-prefixed form.
fn slice_types(data: &[u8]) -> Vec<u32> {
    let mut nals: Vec<&[u8]> = Vec::new();
    if data.starts_with(&[0, 0, 1]) || data.starts_with(&[0, 0, 0, 1]) {
        let mut starts = Vec::new();
        let mut i = 0;
        while i + 3 <= data.len() {
            if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
                starts.push(i + 3);
                i += 3;
            } else {
                i += 1;
            }
        }
        for (k, &s) in starts.iter().enumerate() {
            let end = starts.get(k + 1).map_or(data.len(), |&n| n - 3);
            nals.push(&data[s..end.max(s)]);
        }
    } else {
        let mut i = 0;
        while i + 4 <= data.len() {
            let len = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
            let end = (i + 4 + len).min(data.len());
            nals.push(&data[i + 4..end]);
            i = end;
        }
    }
    nals.iter()
        .filter(|nal| !nal.is_empty() && matches!(nal[0] & 0x1F, 1 | 5))
        .filter_map(|nal| {
            let head: Vec<u8> = nal[1..nal.len().min(24)].to_vec();
            let mut rbsp = Vec::with_capacity(head.len());
            for (i, &b) in head.iter().enumerate() {
                if b == 3 && i >= 2 && head[i - 1] == 0 && head[i - 2] == 0 {
                    continue;
                }
                rbsp.push(b);
            }
            let mut bits = Bits { data: &rbsp, at: 0 };
            bits.ue()?;
            Some(bits.ue()? % 5)
        })
        .collect()
}

struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.at / 8)?;
        let bit = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Some(u32::from(bit))
    }

    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let mut value = 0_u32;
        for _ in 0..zeros {
            value = (value << 1) | self.bit()?;
        }
        Some((1 << zeros) - 1 + value)
    }
}

/// Quality of one decoded clip against the corpus.
#[derive(Default)]
struct Quality {
    decoded: u64,
    /// The decoder's `next_frame` calls; CPU from the cycle counter at `rate`.
    dec: Spent,
    rate: f64,
    m_decode: u64,
    t_quality: u128,
    t_ocr: u128,
    ssim: Vec<f64>,
    psnr: Vec<f64>,
    mse_sum: f64,
    r: Option<f64>,
    r_note: String,
    flashes: Option<u64>,
    time_mismatch: u64,
}

impl Quality {
    fn put(&self, out: &mut Json) {
        out.num("decoded_frames", self.decoded);
        out.num("time_mismatch", self.time_mismatch);
        out.num("t_decode_ns", self.dec.wall_ns);
        out.num("a_dec_ns", self.dec.cpu_from_cycles(self.rate));
        out.num("a_dec_gpt_ns", self.dec.cpu_ns);
        out.num("m_decode", self.m_decode);
        out.num("t_quality_ns", self.t_quality);
        out.num("t_ocr_ns", self.t_ocr);
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        let finite: Vec<f64> = self.psnr.iter().copied().filter(|p| p.is_finite()).collect();
        out.float("ssim_mean", mean(&self.ssim));
        out.float("ssim_min", self.ssim.iter().copied().fold(f64::INFINITY, f64::min));
        out.float("psnr_mean", mean(&finite));
        out.float("psnr_min", self.psnr.iter().copied().fold(f64::INFINITY, f64::min));
        out.num("identical_frames", self.psnr.len() - finite.len());
        let mse = self.mse_sum / self.psnr.len().max(1) as f64;
        out.float("psnr_global", if mse > 0.0 { 10.0 * (255.0 * 255.0 / mse).log10() } else { f64::INFINITY });
        match self.r {
            Some(r) => out.float("r", r),
            None => out.text("r", "n/a"),
        }
        out.text("r_note", &self.r_note);
        match self.flashes {
            Some(n) => out.num("flashes", n),
            None => out.text("flashes", "n/a"),
        }
    }
}

/// Decodes the clip with the copied product decoder and measures every frame.
fn measure_quality(folder: &Folder, input: &SegmentInput, clip: &Path, sampler: &MemorySampler, rate: f64) -> Result<Quality> {
    let mut quality = Quality { rate, ..Quality::default() };
    let text = TEXT_SCENARIOS.contains(&folder.scenario.as_str());
    let n = folder.new_frames.len();
    let ocr_orders: Vec<usize> = if text { (0..OCR_FRAMES.min(n)).map(|i| i * n / OCR_FRAMES.min(n)).collect() } else { Vec::new() };
    let mut ocr_decoded: Vec<(usize, Vec<u8>, u32, u32)> = Vec::new();
    let flash = (folder.scenario == "flash").then(FlashBox::from_env);
    let (mut flash_on, mut flashes) = (false, 0_u64);
    let decode_base = memory_now().1;
    sampler.take_peaks();
    let mut decoder = clip_decoder::ClipDecoder::open(clip)?;
    loop {
        let frame = quality.dec.time(|| decoder.next_frame())?;
        let Some(frame) = frame else { break };
        quality.decoded += 1;
        let wall = Instant::now();
        // No B-frames: decode order is the order of the new frames. A frame
        // whose time is not its sample's instant is counted.
        let order = (quality.decoded as usize - 1).min(input.samples.len() - 1);
        if frame.t_ms().abs_diff(input.samples[order].local_ms) > INSTANT_MS / 2 {
            quality.time_mismatch += 1;
        }
        let (_, corpus_frame) = folder.new_frames[order];
        let bgr = folder.frame_bgr(corpus_frame)?;
        let (rw, rh, reference) = clip_encoder::probe_reference_luma(input, &bgr, folder.width, folder.height);
        let (rw, rh) = (rw as usize, rh as usize);
        if (frame.width as usize) < rw || (frame.height as usize) < rh {
            return Err(format!("decoded {}x{} is smaller than {rw}x{rh}", frame.width, frame.height));
        }
        let decoded = luma_of_bgra(&frame.bgra, frame.width as usize, rw, rh);
        let mse = mse(&reference, &decoded);
        quality.mse_sum += mse;
        quality.psnr.push(if mse > 0.0 { 10.0 * (255.0 * 255.0 / mse).log10() } else { f64::INFINITY });
        quality.ssim.push(ssim(&reference, &decoded, rw, rh));
        if let Some(flash) = &flash {
            let on = flash.lit(&frame.bgra, frame.width as usize, folder.sigma as usize);
            if on && !flash_on {
                flashes += 1;
            }
            flash_on = on;
        }
        if ocr_orders.contains(&order) && !ocr_decoded.iter().any(|d| d.0 == order) {
            ocr_decoded.push((order, crop_bgra(&frame.bgra, frame.width as usize, rw, rh), rw as u32, rh as u32));
        }
        quality.t_quality += wall.elapsed().as_nanos();
    }
    quality.m_decode = sampler.take_peaks().1.saturating_sub(decode_base);
    drop(decoder);
    if flash.is_some() {
        quality.flashes = Some(flashes);
    }
    if text {
        let wall = Instant::now();
        match ocr_share(folder, &ocr_decoded) {
            Ok((r, note)) => {
                quality.r = Some(r);
                quality.r_note = note;
            }
            Err(error) => quality.r_note = format!("OCR failed: {error}"),
        }
        quality.t_ocr = wall.elapsed().as_nanos();
    } else {
        quality.r_note = "not a text scenario".to_owned();
    }
    Ok(quality)
}

/// R = mean over the OCR frames of 1 − edit distance / source length.
fn ocr_share(folder: &Folder, decoded: &[(usize, Vec<u8>, u32, u32)]) -> Result<(f64, String)> {
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| format!("OcrEngine: {e}"))?;
    let mut shares = Vec::new();
    let mut source_chars = 0;
    for (order, bgra, w, h) in decoded {
        let bgr = folder.frame_bgr(folder.new_frames[*order].1)?;
        let source = scaled_bgra(&bgr, folder.width as usize, folder.height as usize, folder.sigma as usize, *w as usize, *h as usize);
        let source_text = ocr_text(&engine, &source, *w, *h)?;
        let decoded_text = ocr_text(&engine, bgra, *w, *h)?;
        source_chars += source_text.chars().count();
        shares.push(share_read(&source_text, &decoded_text));
    }
    let r = shares.iter().sum::<f64>() / shares.len().max(1) as f64;
    Ok((r, format!("{} frames, {source_chars} source chars", shares.len())))
}

fn share_read(source: &str, decoded: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (source.chars().collect(), decoded.chars().collect());
    if a.is_empty() {
        return if b.is_empty() { 1.0 } else { 0.0 };
    }
    1.0 - edit_distance(&a, &b) as f64 / a.len() as f64
}

fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous + usize::from(ca != cb);
            previous = row[j + 1];
            row[j + 1] = substitute.min(row[j] + 1).min(previous + 1);
        }
    }
    row[b.len()]
}

fn ocr_text(engine: &OcrEngine, bgra: &[u8], width: u32, height: u32) -> Result<String> {
    let buffer = CryptographicBuffer::CreateFromByteArray(bgra).map_err(|e| format!("buffer: {e}"))?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, width as i32, height as i32)
        .map_err(|e| format!("SoftwareBitmap: {e}"))?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| format!("RecognizeAsync: {e}"))?
        .join()
        .map_err(|e| format!("OCR: {e}"))?;
    Ok(result.Text().map_err(|e| format!("Text: {e}"))?.to_string_lossy())
}

/// The corpus frame duplicated σ × σ into a `width` × `height` BGRA image
/// (black outside), as the encoder's canvas places it.
fn scaled_bgra(bgr: &[u8], sw: usize, sh: usize, sigma: usize, width: usize, height: usize) -> Vec<u8> {
    let mut out = vec![0_u8; width * height * 4];
    for row in 0..height {
        for col in 0..width {
            let at = (row * width + col) * 4;
            out[at + 3] = 255;
            let (x, y) = (col / sigma, row / sigma);
            if x < sw && y < sh {
                let from = (y * sw + x) * 3;
                out[at..at + 3].copy_from_slice(&bgr[from..from + 3]);
            }
        }
    }
    out
}

fn crop_bgra(bgra: &[u8], stride_px: usize, width: usize, height: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        let start = row * stride_px * 4;
        out.extend_from_slice(&bgra[start..start + width * 4]);
    }
    for pixel in out.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    out
}

/// Y of a BGRA image with the product's matrix (`Nv12Canvas::draw`).
fn luma_of_bgra(bgra: &[u8], stride_px: usize, width: usize, height: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(width * height);
    for row in 0..height {
        let line = &bgra[row * stride_px * 4..row * stride_px * 4 + width * 4];
        out.extend(line.chunks_exact(4).map(|p| luma(i32::from(p[2]), i32::from(p[1]), i32::from(p[0]))));
    }
    out
}

fn luma(red: i32, green: i32, blue: i32) -> u8 {
    (((47 * red + 157 * green + 16 * blue + 128) >> 8) + 16).clamp(0, 255) as u8
}

fn mse(a: &[u8], b: &[u8]) -> f64 {
    let sum: u64 = a.iter().zip(b).map(|(&x, &y)| u64::from(x.abs_diff(y)).pow(2)).sum();
    sum as f64 / a.len().max(1) as f64
}

/// Mean SSIM over 8 × 8 uniform windows at a stride of 4.
fn ssim(a: &[u8], b: &[u8], width: usize, height: usize) -> f64 {
    let n = (SSIM_WINDOW * SSIM_WINDOW) as f64;
    let (mut total, mut count) = (0.0, 0_u64);
    let mut y = 0;
    while y + SSIM_WINDOW <= height {
        let mut x = 0;
        while x + SSIM_WINDOW <= width {
            let (mut sa, mut sb, mut saa, mut sbb, mut sab) = (0_u64, 0_u64, 0_u64, 0_u64, 0_u64);
            for row in y..y + SSIM_WINDOW {
                let base = row * width + x;
                for (&p, &q) in a[base..base + SSIM_WINDOW].iter().zip(&b[base..base + SSIM_WINDOW]) {
                    let (p, q) = (u64::from(p), u64::from(q));
                    sa += p;
                    sb += q;
                    saa += p * p;
                    sbb += q * q;
                    sab += p * q;
                }
            }
            let (ma, mb) = (sa as f64 / n, sb as f64 / n);
            let va = saa as f64 / n - ma * ma;
            let vb = sbb as f64 / n - mb * mb;
            let cov = sab as f64 / n - ma * mb;
            total += ((2.0 * ma * mb + SSIM_C1) * (2.0 * cov + SSIM_C2)) / ((ma * ma + mb * mb + SSIM_C1) * (va + vb + SSIM_C2));
            count += 1;
            x += SSIM_STRIDE;
        }
        y += SSIM_STRIDE;
    }
    if count == 0 { 1.0 } else { total / count as f64 }
}

/// The flash box of the flash page (corpus.json "flash"), passed as
/// `flash=B:G:R:left_fraction:top:width:height:min_pixels`.
struct FlashBox {
    bgr: [i32; 3],
    left_fraction: f64,
    top: usize,
    width: usize,
    height: usize,
    min_pixels: usize,
    window_width: usize,
}

impl FlashBox {
    fn from_env() -> Self {
        let args = Args::from_env();
        let parts: Vec<f64> = args
            .text_or("flash", "80:32:255:0.6:40:160:120:1000")
            .split(':')
            .filter_map(|p| p.parse().ok())
            .collect();
        let get = |i: usize, d: f64| parts.get(i).copied().unwrap_or(d);
        Self {
            bgr: [get(0, 80.0) as i32, get(1, 32.0) as i32, get(2, 255.0) as i32],
            left_fraction: get(3, 0.6),
            top: get(4, 40.0) as usize,
            width: get(5, 160.0) as usize,
            height: get(6, 120.0) as usize,
            min_pixels: get(7, 1000.0) as usize,
            window_width: args.number("window_width", 0).unwrap_or(0),
        }
    }

    /// Whether the box holds at least `min_pixels · σ²` flash-coloured pixels.
    fn lit(&self, bgra: &[u8], stride_px: usize, sigma: usize) -> bool {
        let window = if self.window_width > 0 { self.window_width } else { stride_px / sigma };
        let left = (self.left_fraction * window as f64).round() as usize * sigma;
        let (top, w, h) = (self.top * sigma, self.width * sigma, self.height * sigma);
        let mut count = 0;
        for row in top..top + h {
            for col in left..(left + w).min(stride_px) {
                let at = (row * stride_px + col) * 4;
                let Some(p) = bgra.get(at..at + 3) else { continue };
                if (0..3).all(|c| (i32::from(p[c]) - self.bgr[c]).abs() <= FLASH_TOLERANCE) {
                    count += 1;
                }
            }
        }
        count >= self.min_pixels * sigma * sigma
    }
}

/// `exp-d mode=meter`: SSIM-Y(x, x), PSNR-Y(x, x), R(x, x) and the known
/// distortion Y + 4 on one corpus frame (the middle new frame).
fn meter_self_check(args: &Args) -> Result<()> {
    let folder = Folder::open(Path::new(args.text("folder")?))?;
    let (_, frame) = folder.new_frames[folder.new_frames.len() / 2];
    let bgr = folder.frame_bgr(frame)?;
    let (w, h) = (folder.width as usize, folder.height as usize);
    let y: Vec<u8> = bgr.chunks_exact(3).map(|p| luma(i32::from(p[2]), i32::from(p[1]), i32::from(p[0]))).collect();
    let ssim_xx = ssim(&y, &y, w, h);
    let mse_xx = mse(&y, &y);
    let clipped = y.iter().filter(|&&v| v > 255 - DISTORTION_D).count();
    let shifted: Vec<u8> = y.iter().map(|&v| v.saturating_add(DISTORTION_D)).collect();
    let mse_d4 = mse(&y, &shifted);
    let bgra = scaled_bgra(&bgr, w, h, 1, w, h);
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| format!("OcrEngine: {e}"))?;
    let (first, second) = (ocr_text(&engine, &bgra, w as u32, h as u32)?, ocr_text(&engine, &bgra, w as u32, h as u32)?);
    let mut out = Json::default();
    out.text("mode", "meter");
    out.float("ssim_xx", ssim_xx);
    if mse_xx == 0.0 {
        out.text("psnr_xx", "inf");
    } else {
        out.float("psnr_xx", 10.0 * (255.0 * 255.0 / mse_xx).log10());
    }
    out.float("r_xx", share_read(&first, &second));
    out.num("ocr_chars", first.chars().count());
    out.float("psnr_d4", 10.0 * (255.0 * 255.0 / mse_d4).log10());
    out.num("clipped_d4", clipped);
    out.text("ssim_window", "8x8 uniform, stride 4");
    println!("{}", out.finish());
    Ok(())
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// The one JSON line a run prints (no serde in this probe).
#[derive(Default)]
struct Json(Vec<(String, String)>);

impl Json {
    fn has(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }

    fn set(&mut self, key: &str, value: String) {
        self.0.retain(|(k, _)| k != key);
        self.0.push((key.to_owned(), value));
    }

    fn num(&mut self, key: &str, value: impl std::fmt::Display) {
        self.set(key, value.to_string());
    }

    fn float(&mut self, key: &str, value: f64) {
        let text = if value.is_finite() { format!("{value}") } else if value > 0.0 { "\"inf\"".to_owned() } else { "null".to_owned() };
        self.set(key, text);
    }

    fn text(&mut self, key: &str, value: &str) {
        let escaped: String = value
            .chars()
            .flat_map(|c| match c {
                '"' => vec!['\\', '"'],
                '\\' => vec!['\\', '\\'],
                c if c.is_control() => vec![' '],
                c => vec![c],
            })
            .collect();
        self.set(key, format!("\"{escaped}\""));
    }

    fn finish(&self) -> String {
        let body: Vec<String> = self.0.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
        format!("{{{}}}", body.join(","))
    }
}

fn json_field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let at = text.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = text[at..].trim_start().strip_prefix(':')?.trim_start();
    Some(rest)
}

fn json_num(text: &str, key: &str) -> Option<f64> {
    let rest = json_field(text, key)?;
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn json_str(text: &str, key: &str) -> Option<String> {
    let rest = json_field(text, key)?.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_owned())
}

fn json_array(text: &str, key: &str) -> Option<Vec<f64>> {
    let rest = json_field(text, key)?.strip_prefix('[')?;
    let body = &rest[..rest.find(']')?];
    Some(body.split(',').filter_map(|p| p.trim().parse().ok()).collect())
}
