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
use crate::exp_measure::{CaptureFrame, ClipFiles, RamMeter};
use crate::exp_meter::{Args, MemorySampler, memory_now};
use crate::exp_settings::{Codec, EncodeSettings};

const INSTANT_MS: u64 = 50;
/// Diagnostic lists in the JSON line (keyframe orders, time offsets): first items only.
const DIAG_ITEMS: usize = 12;
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
    let phase = args.text_or("phase", "").to_owned();
    // The worker starts Media Foundation inside its measured work (H3).
    let _runtime = if phase == "worker" { None } else { Some(MediaFoundationRuntime::start()?) };
    if args.text_or("mode", "run") == "meter" {
        return meter_self_check(&args);
    }
    // M3: the live part and the worker are two processes, as in the product (E2).
    let mut out = Json::default();
    out.text("phase", &phase);
    let outcome = match phase.as_str() {
        "live" => run_live(&args, &mut out),
        "worker" => run_worker(&args, &mut out),
        other => Err(format!("phase={other}: give phase=live, then phase=worker on the same scratch folder")),
    };
    let failed = outcome.is_err();
    if let Err(error) = outcome {
        if !out.has("status") {
            out.text("status", "error");
        }
        out.text("error", &error);
    }
    println!("{}", out.finish());
    // The live part leaves the waiting folder for the worker; the worker removes the run folder.
    if (phase != "live" || failed)
        && let Ok(scratch) = args.text("scratch")
    {
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
    /// Every line of frames.jsonl: (instant, stored frame shown, new).
    lines: Vec<(u64, u64, bool)>,
    instants: u64,
}

impl Folder {
    fn open(dir: &Path) -> Result<Self> {
        let meta = std::fs::read_to_string(dir.join("meta.json")).map_err(|e| format!("meta.json: {e}"))?;
        let scenario = json_str(&meta, "scenario").ok_or("meta.json has no scenario")?;
        let window = json_array(&meta, "window").ok_or("meta.json has no window")?;
        let sigma = json_num(&meta, "sigma").unwrap_or(1.0) as u32;
        let instants = json_num(&meta, "instants").unwrap_or(400.0) as u64;
        let text = std::fs::read_to_string(dir.join("frames.jsonl")).map_err(|e| format!("frames.jsonl: {e}"))?;
        let lines: Vec<(u64, u64, bool)> = text
            .lines()
            .filter_map(|l| Some((json_num(l, "instant")? as u64, json_num(l, "frame")? as u64, l.contains("\"new\":true"))))
            .collect();
        let new_frames = lines.iter().filter(|l| l.2).map(|&(instant, frame, _)| (instant, frame)).collect();
        Ok(Self {
            dir: dir.to_owned(),
            scenario,
            width: window[0] as u32,
            height: window[1] as u32,
            sigma: sigma.max(1),
            new_frames,
            lines,
            instants,
        })
    }

    fn put(&self, out: &mut Json) {
        out.text("folder", &self.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        out.text("scenario", &self.scenario);
        out.num("width", self.width);
        out.num("height", self.height);
        out.num("sigma", self.sigma);
        out.num("n_new", self.new_frames.len());
    }

    fn frame_bgr(&self, frame: u64) -> Result<Vec<u8>> {
        let path = self.dir.join(format!("frame-{frame:04}.zst"));
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let size = (self.width * self.height * 3) as usize;
        zstd::bulk::decompress(&bytes, size).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Corpus frames decoded into one reused buffer: input preparation, never
/// measured (M1), and allocated before M_0 so no corpus buffer is in a reading (M3).
struct CorpusReader {
    dctx: zstd::bulk::Decompressor<'static>,
    packed: Vec<u8>,
}

impl CorpusReader {
    fn new(px: usize) -> Result<Self> {
        let dctx = zstd::bulk::Decompressor::new().map_err(|e| format!("zstd: {e}"))?;
        // A zstd frame of raw pixels is at most a little larger than the pixels.
        Ok(Self { dctx, packed: Vec::with_capacity(px * 3 + 65536) })
    }

    fn frame(&mut self, folder: &Folder, frame: u64, out: &mut [u8]) -> Result<()> {
        use std::io::Read as _;
        let path = folder.dir.join(format!("frame-{frame:04}.zst"));
        self.packed.clear();
        std::fs::File::open(&path)
            .and_then(|mut f| f.read_to_end(&mut self.packed))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let n = self.dctx.decompress_to_buffer(&self.packed, out).map_err(|e| format!("{}: {e}", path.display()))?;
        if n != out.len() {
            return Err(format!("{}: {n} bytes, expected {}", path.display(), out.len()));
        }
        Ok(())
    }
}

/// A fixed text region of a scenario (corpus.json `text_regions`, M7), in window
/// pixels at σ = 1: insets from the four edges, then an optional cap on the size.
/// Passed as `regions=left:top:right:bottom:max_width:max_height:max_width_fraction;...`.
struct TextRegion {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    max_width: u32,
    max_height: u32,
    max_width_fraction: f64,
}

impl TextRegion {
    fn parse(text: &str) -> Result<Vec<Self>> {
        text.split(';')
            .filter(|r| !r.trim().is_empty())
            .map(|r| {
                let v: Vec<f64> = r.split(':').map(|x| x.trim().parse::<f64>().map_err(|_| format!("region {r}: bad number {x}"))).collect::<Result<_>>()?;
                if v.len() != 7 {
                    return Err(format!("region {r}: 7 numbers expected"));
                }
                let px = |x: f64| x.max(0.0) as u32;
                Ok(Self {
                    left: px(v[0]),
                    top: px(v[1]),
                    right: px(v[2]),
                    bottom: px(v[3]),
                    max_width: px(v[4]),
                    max_height: px(v[5]),
                    max_width_fraction: v[6],
                })
            })
            .collect()
    }

    /// (x, y, w, h) in the recorded frame: window pixels × σ, clipped to `rw` × `rh`.
    fn rect(&self, window_w: u32, window_h: u32, sigma: u32, rw: u32, rh: u32) -> Option<(u32, u32, u32, u32)> {
        let mut w = window_w.saturating_sub(self.left + self.right);
        let h = window_h.saturating_sub(self.top + self.bottom);
        if self.max_width > 0 {
            w = w.min(self.max_width);
        }
        if self.max_width_fraction > 0.0 {
            w = w.min((f64::from(window_w) * self.max_width_fraction) as u32);
        }
        let h = if self.max_height > 0 { h.min(self.max_height) } else { h };
        let (x, y) = (self.left * sigma, self.top * sigma);
        let (w, h) = ((w * sigma).min(rw.saturating_sub(x)), (h * sigma).min(rh.saturating_sub(y)));
        (w >= 8 && h >= 8).then_some((x, y, w, h))
    }
}

/// The live process (M1 to M3): every new frame arrives as B, G, R, A and goes
/// through the copied product `WaitingFrameWriter` (alpha dropped, zstd −3,
/// written to `scratch/waiting/`). CPU = cycle counter of that work (a_rt), RAM =
/// OS peak of private bytes above M_0 (M_live). It leaves the waiting folder.
fn run_live(args: &Args, out: &mut Json) -> Result<()> {
    let folder = Folder::open(Path::new(args.text("folder")?))?;
    let request = Request::from_args(args)?;
    let scratch = PathBuf::from(args.text("scratch")?);
    let cycle_rate = args.rate()?;
    out.float("cycle_rate", cycle_rate);
    folder.put(out);
    out.num("live_pid", std::process::id());
    request.put(out);
    crate::exp_settings::install(request.settings(false));
    std::fs::create_dir_all(scratch.join(clip_folder::WAITING_DIR)).map_err(|e| format!("scratch: {e}"))?;

    // Input preparation before M_0 (M1, M3): the corpus reader, one B,G,R buffer
    // and one capture frame exist and one frame has been read, like the product's slot.
    let px = (folder.width * folder.height) as usize;
    let mut corpus = CorpusReader::new(px)?;
    let mut bgr = vec![0_u8; px * 3];
    let mut capture = CaptureFrame::new(px);
    if let Some(&(_, first)) = folder.new_frames.first() {
        corpus.frame(&folder, first, &mut bgr)?;
        capture.fill(&bgr);
    }
    let ram = RamMeter::start();
    let wall = Instant::now();
    let mut live = Spent::default();
    let mut writer = live.time(clip_folder::WaitingFrameWriter::new)?;
    let mut waiting_bytes = 0_u64;
    for &(instant, frame) in &folder.new_frames {
        corpus.frame(&folder, frame, &mut bgr)?;
        capture.fill(&bgr);
        waiting_bytes += live.time(|| writer.write(&scratch, instant, capture.bgra()))?;
    }
    live.time(|| drop(writer));
    let t_live = wall.elapsed().as_nanos();
    let reading = ram.read();
    out.text("input", "bgra");
    out.num("waiting_bytes", waiting_bytes);
    out.num("t_live_ns", t_live);
    out.num("a_rt_ns", live.cpu_from_cycles(cycle_rate));
    out.num("a_rt_gpt_ns", live.cpu_ns);
    out.num("live_cycles", live.cycles);
    out.raw(reading.json("live_"));
    out.num("m_live", reading.ram);
    // The encoder's activation check runs in this process after its RAM reading: it touches
    // neither the live part's figures nor the worker process before its M_0 (H3).
    check_activation(&request, &folder, out)?;
    out.text("status", "ok");
    Ok(())
}

/// D1: the named encoder is found by name and vendor in the Media Foundation
/// enumeration and activated from its enumeration object, then shut down.
/// DW4 has no Sink Writer path: whether it takes system-memory NV12 is confirmed.
fn check_activation(request: &Request, folder: &Folder, out: &mut Json) -> Result<()> {
    let activate = find_encoder(&request.name, &request.vendor, request.hardware)?;
    let activation = unsafe { activate.ActivateObject::<IMFTransform>() };
    let transform = match activation {
        Ok(transform) => {
            out.num("activated", 1);
            out.text("hresult", "0x00000000");
            transform
        }
        Err(error) => {
            out.num("activated", 0);
            out.text("hresult", &format!("{:#010x}", error.code().0));
            out.text("status", "not_activated");
            return Err(format!("activation failed: {error}"));
        }
    };
    if request.vendor == "VEN_1414" {
        let (w, h) = recorded_size(folder);
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
    Ok(())
}

/// The settings a run asks for (factors D1 to D5), echoed as `<name>_req` (M5).
struct Request {
    way: String,
    name: String,
    vendor: String,
    hardware: bool,
    rate: String,
    q: u32,
    tau: u32,
    hint: bool,
    beta: f64,
}

impl Request {
    fn from_args(args: &Args) -> Result<Self> {
        let vendor = args.text_or("vendor", "").to_owned();
        let beta: f64 = args.number("beta", clip_encoder::FALLBACK_BITS_PER_PIXEL_FRAME)?;
        // M5: the bitrate fallback is never 0 (quality mode keeps the product's 0.10).
        if beta.is_nan() || beta <= 0.0 {
            return Err(format!("beta={beta}: the bitrate fallback must be above 0 (M5)"));
        }
        Ok(Self {
            way: args.text("encoder")?.to_owned(),
            name: args.text_or("name", "").to_owned(),
            hardware: args.flag("hw", !vendor.is_empty()),
            vendor,
            // `rate=` is the cycle-counter rate (`exp-rate`), so the rate mode has its own key.
            rate: args.text_or("ratemode", "quality").to_owned(),
            q: args.number("q", 70)?,
            tau: args.number("gop", 20)?,
            hint: args.flag("hint", true),
            beta,
        })
    }

    /// q as a key: 0 in bitrate mode, where no quality level is set.
    fn q(&self) -> u32 {
        if self.rate == "quality" { self.q } else { 0 }
    }

    /// β · w · h · f in bits per second, as the copied `fallback_bitrate` computes it.
    fn bitrate(&self, width: u32, height: u32) -> u32 {
        let bits = self.beta * f64::from(width) * f64::from(height) * f64::from(capture_clip::RECORD_FPS);
        bits.round().clamp(1.0, f64::from(u32::MAX)) as u32
    }

    fn settings(&self, hardware: bool) -> EncodeSettings {
        EncodeSettings {
            codec: Codec::H264,
            hardware,
            adapter_luid: None,
            quality: (self.rate == "quality").then_some(self.q),
            bits_per_pixel_frame: self.beta,
            gop_frames: self.tau,
            screen_hint: self.hint,
            low_latency: false,
            zstd_level: -3,
            raw_waiting: false,
        }
    }

    /// The requested settings, and the key columns until the encoder reads them back.
    fn put(&self, out: &mut Json) {
        out.text("encoder", &self.way);
        out.text("rate_req", &self.rate);
        out.num("q_req", self.q());
        out.num("tau_f_req", self.tau);
        out.num("hint_req", u32::from(self.hint));
        out.float("beta_req", self.beta);
        out.text("rate", &self.rate);
        out.num("q", self.q());
        out.num("tau_f", self.tau);
        out.num("hint", u32::from(self.hint));
        out.float("beta", self.beta);
    }
}

/// The worker process (M2 to M4): activation, then the copied product
/// `encode_once` on the waiting frames (read, decode, σ scale, NV12, encode,
/// write), the clip's `frames.jsonl` and `clip.json`, the waiting folder deleted
/// at ready. CPU = cycle counter (a_w), RAM = OS peak above M_0 (M_worker).
/// Afterwards, outside the measured work: read back, settings, decode, quality.
fn run_worker(args: &Args, out: &mut Json) -> Result<()> {
    let folder = Folder::open(Path::new(args.text("folder")?))?;
    let request = Request::from_args(args)?;
    let scratch = PathBuf::from(args.text("scratch")?);
    let cycle_rate = args.rate()?;
    out.float("cycle_rate", cycle_rate);
    folder.put(out);
    out.num("worker_pid", std::process::id());
    request.put(out);
    crate::exp_settings::install(request.settings(request.hardware));

    // 2. The worker's measured work, from M_0 to ready.
    let samples = segment_samples(&folder, &scratch);
    let input = SegmentInput {
        canvas_width: folder.width,
        canvas_height: folder.height,
        scale: folder.sigma,
        fps: capture_clip::RECORD_FPS,
        samples: &samples,
        seed: None,
    };
    let clip_dir = scratch.join("clip");
    let mp4 = clip_dir.join("clip.mp4");
    let ram = RamMeter::start();
    let mut worker = Spent::default();
    // H3: every setup the worker needs is part of its measured work: Media Foundation and,
    // for a hardware encoder, the D3D11 device and its DXGI device manager.
    let _runtime = worker.time(MediaFoundationRuntime::start)?;
    let manager = worker.time(|| if request.hardware { device_manager(&request.vendor).map(Some) } else { Ok(None) })?;
    crate::exp_settings::set_d3d_manager(manager);
    let mut files = worker.time(|| ClipFiles::create(&clip_dir, folder.width as usize, folder.height as usize))?;
    let encoded = worker.time(|| clip_encoder::probe_encode_once(&mp4, &input, request.hardware))?;
    worker.time(|| -> Result<()> {
        files.count(&mp4)?;
        for &(instant, frame, new) in &folder.lines {
            files.line(instant, frame as usize, new)?;
        }
        // M4: the waiting files are deleted at ready, so they are not kept or counted.
        std::fs::remove_dir_all(scratch.join(clip_folder::WAITING_DIR)).map_err(|e| format!("waiting: {e}"))
    })?;
    let clip_json = format!(
        "{{\"way\":\"{}\",\"w\":{},\"h\":{},\"sigma\":{},\"out_w\":{},\"out_h\":{},\"frames\":{},\"segments\":[\"clip.mp4\"]}}",
        request.way,
        folder.width,
        folder.height,
        folder.sigma,
        encoded.width,
        encoded.height,
        folder.new_frames.len()
    );
    let clip_bytes = worker.time(|| files.finish(&clip_json))?;
    let reading = ram.read();
    out.num("t_worker_ns", worker.wall_ns);
    out.num("a_w_ns", worker.cpu_from_cycles(cycle_rate));
    out.num("a_w_gpt_ns", worker.cpu_ns);
    out.num("worker_cycles", worker.cycles);
    out.raw(reading.json("worker_"));
    out.num("m_worker", reading.ram);
    out.num("bytes", clip_bytes.payload);
    out.raw(clip_bytes.json());
    out.num("bytes_disk", clip_bytes.total());
    out.num("bytes_on_disk", ClipFiles::bytes_on_disk(&clip_dir)?);
    out.text("clip_hash", &format!("{:016x}", fnv1a(&std::fs::read(&mp4).unwrap_or_default())));
    out.num("out_w", encoded.width);
    out.num("out_h", encoded.height);
    out.text("encoder_used", &encoded.encoder);
    out.num("hardware_used", u32::from(encoded.hardware));
    out.num("keyframes_segment", encoded.keyframe_instants.len());
    // With hardware transforms off the Sink Writer can load only a software
    // encoder, and the software H.264 MFT reports no friendly name.
    let used_matches = encoded.encoder.contains(&request.name)
        || (!request.vendor.is_empty() && encoded.encoder.contains(&request.vendor))
        || (!request.hardware && !encoded.hardware);
    out.num("encoder_matches", u32::from(used_matches));

    // 3. Settings read back (M5): a differing run is recorded under its actual settings.
    let report = &encoded.quality;
    let quality_mode = report.mode == "quality";
    let q_read = if quality_mode { report.value } else { 0 };
    let hint_read = u32::from(report.screen_content_hint);
    let bitrate_req = request.bitrate(encoded.width, encoded.height);
    let bitrate_read = if quality_mode { 0 } else { report.value };
    let beta_read = if quality_mode || bitrate_read == bitrate_req {
        request.beta
    } else {
        f64::from(bitrate_read) / (f64::from(encoded.width) * f64::from(encoded.height) * f64::from(capture_clip::RECORD_FPS))
    };
    out.text("rate_read", &report.mode);
    out.num("rate_value", report.value);
    out.num("q_read", q_read);
    out.num("tau_f_read", report.gop_frames);
    out.num("hint_read", hint_read);
    out.num("bitrate_req", if request.rate == "bitrate" { bitrate_req } else { 0 });
    out.num("bitrate_read", bitrate_read);
    out.float("beta_read", beta_read);
    out.num("b_frames_read", report.b_frames);
    let refused = [
        ("rate", request.rate != report.mode),
        ("q", quality_mode && request.q() != q_read),
        ("tau_f", request.tau != report.gop_frames),
        ("hint", u32::from(request.hint) != hint_read),
        ("beta", !quality_mode && request.rate == "bitrate" && bitrate_read != bitrate_req),
    ]
    .into_iter()
    .find_map(|(name, differs)| differs.then_some(name));
    out.text("rate", &report.mode);
    out.num("q", q_read);
    out.num("tau_f", report.gop_frames);
    out.num("hint", hint_read);
    out.float("beta", beta_read);

    // 4. Read back from the file.
    let back = read_back(&mp4)?;
    out.num("frames", back.frames);
    out.num("keyframes", back.keyframes);
    out.num("b_frames", back.b_slices);
    out.num("max_gop", back.max_gop);
    out.text("key_orders", &list(back.key_orders.iter()));
    out.text("mp4_time_offsets", &time_offsets(&back.times_ms, &samples));

    // 5 and 6. Decode, frame mapping by time (M6), quality, R and flashes.
    let sampler = MemorySampler::start();
    let regions = TextRegion::parse(args.text_or("regions", ""))?;
    let quality = measure_quality(&folder, &input, &mp4, &sampler, cycle_rate, &regions, args.flag("ocr_fail", false), args.flag("e1_nv12", false),
        args.text_or("e1_noise", "").parse::<u8>().ok())?;
    quality.put(out);
    let status = if !used_matches {
        "wrong_encoder".to_owned()
    } else if let Some(name) = refused {
        format!("setting_refused:{name}")
    } else if quality.mapping_failed(folder.new_frames.len()) {
        "frame_map".to_owned()
    } else if quality.ocr_failed {
        // H4: R could not be read; n/a is kept for OCR that read no source text.
        "ocr_error".to_owned()
    } else {
        "ok".to_owned()
    };
    out.text("status", &status);
    Ok(())
}

/// The first values of a list, comma-free (the runner's CSV has no quoting).
fn list<T: std::fmt::Display>(values: impl Iterator<Item = T>) -> String {
    values.take(DIAG_ITEMS).map(|v| v.to_string()).collect::<Vec<_>>().join(" ")
}

/// Samples whose time differs from the time written for them, as `order:sample_ms>file_ms`.
fn time_offsets(times_ms: &[u64], samples: &[SegmentSample]) -> String {
    let off: Vec<String> = times_ms
        .iter()
        .zip(samples)
        .enumerate()
        .filter(|(_, (t, s))| t.abs_diff(s.local_ms) > INSTANT_MS / 2)
        .map(|(order, (t, s))| format!("{order}:{}>{t}", s.local_ms))
        .collect();
    format!("{} off; {}", off.len(), list(off.iter()))
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
    /// Decode order of every keyframe.
    key_orders: Vec<u64>,
    /// The time stored for every compressed sample, in milliseconds.
    times_ms: Vec<u64>,
}

fn read_back(clip: &Path) -> Result<ReadBack> {
    let url = HSTRING::from(clip.as_os_str());
    let reader = unsafe { MFCreateSourceReaderFromURL(&url, None) }.map_err(|e| format!("source reader: {e}"))?;
    let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let mut back = ReadBack { frames: 0, keyframes: 0, b_slices: 0, max_gop: 0, key_orders: Vec::new(), times_ms: Vec::new() };
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
            back.times_ms.push(u64::try_from((time + 5_000) / 10_000).unwrap_or(0));
            back.frames += 1;
            if unsafe { sample.GetUINT32(&MFSampleExtension_CleanPoint) }.unwrap_or(0) != 0 {
                back.key_orders.push(back.frames - 1);
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
    /// Σ (Y_decoded − Y_corpus)² and the pixels it covers, over the whole clip (M7).
    squared_error: u64,
    pixels: u64,
    r: Option<f64>,
    r_note: String,
    /// H4: OCR raised an error, so R is unknown (not n/a).
    ocr_failed: bool,
    r_regions: usize,
    r_source_chars: usize,
    flashes: Option<u64>,
    /// M6: decoded frames matched to a new frame by time, and the failures.
    n_new: usize,
    mapped: usize,
    extra: u64,
    mistimed: u64,
    /// Decoded frames off their decode-order sample's time, as `order:sample_ms>decoded_ms`.
    offsets: Vec<String>,
}

impl Quality {
    fn mapping_failed(&self, n_new: usize) -> bool {
        self.mapped != n_new || self.extra > 0 || self.mistimed > 0
    }

    fn put(&self, out: &mut Json) {
        out.num("decoded_frames", self.decoded);
        out.num("frames_mapped", self.mapped);
        out.num("frames_missing", self.n_new.saturating_sub(self.mapped));
        out.num("frames_extra", self.extra);
        out.num("frames_mistimed", self.mistimed);
        out.text("time_offsets", &format!("{} off; {}", self.offsets.len(), list(self.offsets.iter())));
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
        // M7: PSNR-Y from the clip's total MSE, inf when it is 0.
        let mse = self.squared_error as f64 / self.pixels.max(1) as f64;
        out.float("mse_y", mse);
        out.float("psnr_global", if mse > 0.0 { 10.0 * (255.0 * 255.0 / mse).log10() } else { f64::INFINITY });
        match self.r {
            Some(r) => out.float("r", r),
            None if self.ocr_failed => out.text("r", "error"),
            None => out.text("r", "n/a"),
        }
        out.text("r_note", &self.r_note);
        out.num("r_regions", self.r_regions);
        out.num("r_source_chars", self.r_source_chars);
        match self.flashes {
            Some(n) => out.num("flashes", n),
            None => out.text("flashes", "n/a"),
        }
    }
}

/// Decodes the clip with the copied product decoder, maps every decoded frame to
/// the new frame at its presentation time (M6) and measures it against that frame.
fn measure_quality(
    folder: &Folder,
    input: &SegmentInput,
    clip: &Path,
    sampler: &MemorySampler,
    rate: f64,
    regions: &[TextRegion],
    inject_ocr_failure: bool,
    nv12_only: bool,
    source_noise: Option<u8>,
) -> Result<Quality> {
    let n = folder.new_frames.len();
    let mut quality = Quality { rate, n_new: n, ..Quality::default() };
    let text = TEXT_SCENARIOS.contains(&folder.scenario.as_str());
    let ocr_orders: Vec<usize> = if text { (0..OCR_FRAMES.min(n)).map(|i| i * n / OCR_FRAMES.min(n)).collect() } else { Vec::new() };
    let mut ocr_decoded: Vec<(usize, Vec<u8>, u32, u32)> = Vec::new();
    let flash = (folder.scenario == "flash").then(FlashBox::from_env);
    let (mut flash_on, mut flashes) = (false, 0_u64);
    let times: Vec<u64> = input.samples.iter().map(|s| s.local_ms).collect();
    let mut seen = vec![false; n];
    let decode_base = memory_now().1;
    sampler.take_peaks();
    let mut decoder = clip_decoder::ClipDecoder::open(clip)?;
    loop {
        let frame = quality.dec.time(|| decoder.next_frame())?;
        let Some(frame) = frame else { break };
        quality.decoded += 1;
        let wall = Instant::now();
        let t = frame.t_ms();
        // Diagnostic: the time of the sample written at this decode order.
        let in_order = (quality.decoded as usize - 1).min(n.saturating_sub(1));
        if n > 0 && t.abs_diff(times[in_order]) > INSTANT_MS / 2 {
            quality.offsets.push(format!("{in_order}:{}>{t}", times[in_order]));
        }
        // M6: the new frame whose time is within half an instant of the decoded frame's.
        let next = times.partition_point(|&s| s < t);
        let order = [next.checked_sub(1), (next < n).then_some(next)]
            .into_iter()
            .flatten()
            .min_by_key(|&k| times[k].abs_diff(t))
            .filter(|&k| times[k].abs_diff(t) <= INSTANT_MS / 2);
        let Some(order) = order else {
            quality.mistimed += 1;
            continue;
        };
        if seen[order] {
            quality.extra += 1;
            continue;
        }
        seen[order] = true;
        quality.mapped += 1;
        let (_, corpus_frame) = folder.new_frames[order];
        let bgr = folder.frame_bgr(corpus_frame)?;
        let (rw, rh, reference) = clip_encoder::probe_reference_luma(input, &bgr, folder.width, folder.height);
        let (rw, rh) = (rw as usize, rh as usize);
        if (frame.width as usize) < rw || (frame.height as usize) < rh {
            return Err(format!("decoded {}x{} is smaller than {rw}x{rh}", frame.width, frame.height));
        }
        let decoded = luma_of_bgra(&frame.bgra, frame.width as usize, rw, rh);
        let squared = squared_error(&reference, &decoded);
        quality.squared_error += squared;
        quality.pixels += reference.len() as u64;
        let mse = squared as f64 / reference.len().max(1) as f64;
        quality.psnr.push(if mse > 0.0 { 10.0 * (255.0 * 255.0 / mse).log10() } else { f64::INFINITY });
        quality.ssim.push(ssim(&reference, &decoded, rw, rh));
        if let Some(flash) = &flash {
            let on = flash.lit(&frame.bgra, frame.width as usize, folder.sigma as usize);
            if on && !flash_on {
                flashes += 1;
            }
            flash_on = on;
        }
        if ocr_orders.contains(&order) {
            // Experiment E1 (`e1_nv12=1`): OCR reads the source after the NV12 conversion alone,
            // with no encoding, so R measures what 4:2:0 and the BT.709 rounding lose.
            // Noise test (`e1_noise=N`): OCR reads the source itself with ±N levels of
            // deterministic noise per channel, to measure how stable the OCR is (N = 0: control).
            let picture = if let Some(amplitude) = source_noise {
                let clean = scaled_bgra(&bgr, folder.width as usize, folder.height as usize, folder.sigma as usize, rw, rh);
                with_noise(&clean, amplitude, order as u64)
            } else if nv12_only {
                let (w, h, nv12) = clip_encoder::probe_reference_nv12(input, &bgr, folder.width, folder.height);
                crop_bgra(&nv12_to_bgra(&nv12, w as usize, h as usize), w as usize, rw, rh)
            } else {
                crop_bgra(&frame.bgra, frame.width as usize, rw, rh)
            };
            ocr_decoded.push((order, picture, rw as u32, rh as u32));
        }
        quality.t_quality += wall.elapsed().as_nanos();
    }
    quality.m_decode = sampler.take_peaks().1.saturating_sub(decode_base);
    drop(decoder);
    if flash.is_some() {
        quality.flashes = Some(flashes);
    }
    quality.r_regions = if text { regions.len() } else { 0 };
    if text && !regions.is_empty() {
        let wall = Instant::now();
        // `ocr_fail=1`: the step2d check of the error path makes OCR fail on purpose.
        let read = if inject_ocr_failure { Err("injected OCR failure (ocr_fail=1)".to_owned()) } else { ocr_share(folder, &ocr_decoded, regions) };
        match read {
            Ok((r, chars, note)) => {
                quality.r = r;
                quality.r_source_chars = chars;
                quality.r_note = note;
            }
            Err(error) => {
                quality.ocr_failed = true;
                quality.r_note = format!("OCR failed: {error}");
            }
        }
        quality.t_ocr = wall.elapsed().as_nanos();
    } else if text {
        quality.ocr_failed = true;
        quality.r_note = "OCR failed: no text regions given (corpus.json text_regions)".to_owned();
    } else {
        quality.r_note = "not a text scenario".to_owned();
    }
    Ok(quality)
}

/// R on the scenario's fixed text regions (M7): the mean over OCR frames and
/// regions of 1 − edit distance / source length, counting only pairs whose
/// source text is not empty. No source text at all gives R = n/a (`None`).
fn ocr_share(folder: &Folder, decoded: &[(usize, Vec<u8>, u32, u32)], regions: &[TextRegion]) -> Result<(Option<f64>, usize, String)> {
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| format!("OcrEngine: {e}"))?;
    let mut shares = Vec::new();
    let mut source_chars = 0;
    let mut per_region = vec![0_usize; regions.len()];
    for (order, bgra, w, h) in decoded {
        let bgr = folder.frame_bgr(folder.new_frames[*order].1)?;
        let source = scaled_bgra(&bgr, folder.width as usize, folder.height as usize, folder.sigma as usize, *w as usize, *h as usize);
        for (index, region) in regions.iter().enumerate() {
            let Some((x, y, rw, rh)) = region.rect(folder.width, folder.height, folder.sigma, *w, *h) else { continue };
            let source_text = ocr_text(&engine, &crop_rect(&source, *w, x, y, rw, rh), rw, rh)?;
            let chars = source_text.chars().filter(|c| !c.is_whitespace()).count();
            if chars == 0 {
                continue;
            }
            let decoded_text = ocr_text(&engine, &crop_rect(bgra, *w, x, y, rw, rh), rw, rh)?;
            source_chars += chars;
            per_region[index] += chars;
            shares.push(share_read(&source_text, &decoded_text));
        }
    }
    let r = (!shares.is_empty()).then(|| shares.iter().sum::<f64>() / shares.len() as f64);
    let note = format!("{} frames, {} region reads, source chars per region {}", decoded.len(), shares.len(), list(per_region.iter()));
    Ok((r, source_chars, note))
}

/// Noise test: each B, G, R value moved by a uniform integer in [−amplitude, amplitude]
/// (clamped to 0..255) from a fixed-seed generator, alpha kept, so runs repeat exactly.
fn with_noise(bgra: &[u8], amplitude: u8, seed: u64) -> Vec<u8> {
    let span = 2 * u64::from(amplitude) + 1;
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    bgra.iter()
        .enumerate()
        .map(|(i, &value)| {
            if i % 4 == 3 || amplitude == 0 {
                return value;
            }
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            let offset = i32::try_from((state >> 33) % span).unwrap_or(0) - i32::from(amplitude);
            u8::try_from((i32::from(value) + offset).clamp(0, 255)).unwrap_or(u8::MAX)
        })
        .collect()
}

/// Experiment E1: NV12 (BT.709 limited range, the inverse of `Nv12Canvas::draw`) back to
/// B, G, R, A. Each 2 × 2 block takes its own Cb, Cr (nearest, no interpolation), so the
/// only losses are the 4:2:0 averaging and the integer rounding.
fn nv12_to_bgra(nv12: &[u8], width: usize, height: usize) -> Vec<u8> {
    const Y_SCALE: f64 = 255.0 / 219.0;
    const CR_TO_R: f64 = 1.5748 * 255.0 / 224.0;
    const CB_TO_G: f64 = 0.1873 * 255.0 / 224.0;
    const CR_TO_G: f64 = 0.4681 * 255.0 / 224.0;
    const CB_TO_B: f64 = 1.8556 * 255.0 / 224.0;
    let (luma, chroma) = nv12.split_at(width * height);
    let mut out = vec![255_u8; width * height * 4];
    let to_u8 = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    for row in 0..height {
        for col in 0..width {
            let at = (row / 2) * width + (col / 2) * 2;
            let y = (f64::from(luma[row * width + col]) - 16.0) * Y_SCALE;
            let cb = f64::from(chroma[at]) - 128.0;
            let cr = f64::from(chroma[at + 1]) - 128.0;
            let px = (row * width + col) * 4;
            out[px] = to_u8(y + CB_TO_B * cb);
            out[px + 1] = to_u8(y - CB_TO_G * cb - CR_TO_G * cr);
            out[px + 2] = to_u8(y + CR_TO_R * cr);
        }
    }
    out
}

/// A `w` × `h` rectangle at (x, y) of a BGRA image `stride_px` wide.
fn crop_rect(bgra: &[u8], stride_px: u32, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
    let (stride, x, y, w, h) = (stride_px as usize, x as usize, y as usize, w as usize, h as usize);
    let mut out = Vec::with_capacity(w * h * 4);
    for row in y..y + h {
        let start = (row * stride + x) * 4;
        out.extend_from_slice(&bgra[start..start + w * 4]);
    }
    out
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

fn squared_error(a: &[u8], b: &[u8]) -> u64 {
    a.iter().zip(b).map(|(&x, &y)| u64::from(x.abs_diff(y)).pow(2)).sum()
}

fn mse(a: &[u8], b: &[u8]) -> f64 {
    squared_error(a, b) as f64 / a.len().max(1) as f64
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

    /// Ready-made `"key":value` fields (`RamReading::json`, `ClipBytes::json`).
    fn raw(&mut self, fields: String) {
        self.0.push((String::new(), fields));
    }

    fn finish(&self) -> String {
        let body: Vec<String> =
            self.0.iter().map(|(k, v)| if k.is_empty() { v.clone() } else { format!("\"{k}\":{v}") }).collect();
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
