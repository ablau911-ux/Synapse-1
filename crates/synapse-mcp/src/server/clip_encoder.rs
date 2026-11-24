//! The clip's video (plan 5.0 §2, §6, W1): libaom AV1 in realtime screen
//! mode, the B,G,R,A to 4:2:0 conversion it is fed with, and the IVF parts it
//! writes. A new part starts at every key picture, so each part decodes on
//! its own, and a part is complete on disk before the next one starts.
//!
//! Moved from the W0 probe (`scripts/clip-equation/probe/src/exp_av1_video.rs`),
//! whose checks proved it; the probe's stage timers are left out.
//!
//! The encoder runs on its own thread behind a bounded queue (plan §6,
//! `VideoQueue`): a key picture takes far longer than one instant to encode,
//! and the recorder thread must never wait for it.

use std::ffi::{CStr, c_int, c_uint};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::Instant;

use libaom_sys as aom;
use serde_json::json;

use super::clip_folder::{IvfWriter, PartInfo, RECORD_FPS, VIDEO_DIR, part_path};
use super::clip_stream::StageTime;

/// Constant quality level (`-crf`), plan §6.
pub(super) const AV1_QUALITY: u32 = 35;
/// `KEY_INSTANTS`: a key picture at the first encoded picture this many
/// instants after the last one (10 s at 20 instants per second), plan §6.
pub(super) const KEY_INSTANTS: u64 = 200;
/// `ENCODER_QUEUE_PICTURES`: converted pictures waiting for the encoder thread
/// (plan §6). The first live run measured 250-650 ms for one key picture, that
/// is up to 13 instants; 30 pictures = 1.5 s holds more than twice the slowest
/// key picture, so the pictures behind it wait instead of being lost. A
/// converted 4:2:0 picture of a 1920 x 1008 window is about 2.9 MB, so a full
/// queue holds about 87 MB, and only while the encoder is behind.
pub(super) const ENCODER_QUEUE_PICTURES: usize = 30;
/// The encoder thread's name, as it shows in a debugger or a crash dump.
const ENCODER_THREAD_NAME: &str = "synapse-clip-encoder";
/// The ffmpeg settings W0 measured (`-usage realtime -cpu-used 8 -row-mt 1`).
const CPU_USED: c_int = 8;
const ROW_MULTITHREADING_ON: c_uint = 1;
/// No look-ahead: every picture leaves the encoder in the same call, so the
/// end of a recording has nothing left to drain.
const NO_LAG: c_uint = 0;
/// AV1 main profile: 4:2:0, 8 bit.
const MAIN_PROFILE: c_uint = 0;
const BIT_DEPTH: c_uint = 8;
/// libaom's largest thread count for one encoder.
const MAX_THREADS: usize = 64;
/// Row alignment of the encoder's own picture buffer.
const IMAGE_ALIGN: c_uint = 32;
/// Every picture lasts one instant.
const PICTURE_DURATION: std::ffi::c_ulong = 1;
pub(super) const BGRA_BYTES: usize = 4;
/// The encoder's name in `clip.json`, before its version.
const ENCODER_NAME: &str = "libaom";

/// The encoder settings a clip uses; stored in `clip.json`.
#[derive(Clone, Copy, Debug)]
pub(super) struct EncoderSettings {
    pub(super) quality: u32,
    pub(super) key_instants: u64,
    pub(super) threads: u32,
}

impl EncoderSettings {
    /// The settings of plan §6, with one encoder thread per processor thread.
    pub(super) fn standard() -> Self {
        let threads = std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(MAX_THREADS);
        Self {
            quality: AV1_QUALITY,
            key_instants: KEY_INSTANTS,
            threads: u32::try_from(threads).unwrap_or(1),
        }
    }

    pub(super) fn describe(&self) -> serde_json::Value {
        json!({
            "usage": "realtime", "cpu_used": CPU_USED, "end_usage": "q",
            "cq_level": self.quality, "bitrate_cap": "none", "tune_content": "screen",
            "row_mt": ROW_MULTITHREADING_ON, "threads": self.threads, "lag_in_frames": NO_LAG,
            "format": "I420 8 bit BT.601 limited", "key_instants": self.key_instants,
            "time_base": format!("1/{RECORD_FPS}")
        })
    }
}

/// The encoder's name with libaom's version, e.g. "libaom 3.15.3".
pub(super) fn encoder_name() -> String {
    // SAFETY: libaom returns a static, NUL-terminated string.
    let version = unsafe { CStr::from_ptr(aom::aom_codec_version_str()) };
    format!("{ENCODER_NAME} {}", version.to_string_lossy())
}

/// Where an encoded picture went: its part and its index in that part.
pub(super) type VideoPlace = (u32, u32);

/// One compressed picture.
struct Packet {
    data: Vec<u8>,
    key: bool,
}

/// One picture in 4:2:0: the luma plane, then U, then V, each plane's rows
/// tightly packed. Made on the recorder thread, encoded on the encoder thread.
struct I420Picture {
    width: usize,
    height: usize,
    data: Vec<u8>,
}

impl I420Picture {
    const fn chroma_size(width: usize, height: usize) -> (usize, usize) {
        (width.div_ceil(2), height.div_ceil(2))
    }

    /// Each plane with its row length and row count: Y, U, V.
    fn planes(&self) -> [(&[u8], usize, usize); 3] {
        let (chroma_width, chroma_height) = Self::chroma_size(self.width, self.height);
        let luma = self.width * self.height;
        let chroma = chroma_width * chroma_height;
        [
            (&self.data[..luma], self.width, self.height),
            (&self.data[luma..luma + chroma], chroma_width, chroma_height),
            (&self.data[luma + chroma..], chroma_width, chroma_height),
        ]
    }
}

/// B,G,R,A rows (top to bottom) into 4:2:0 planes: BT.601 limited range,
/// 8-bit fixed point (what ffmpeg's bgr24 to yuv420p gives). The largest
/// stage of the recorder (W0: 0.20 of a core at 20 pictures per second),
/// kept in this one type so it can be replaced on its own.
struct I420Converter {
    width: usize,
    height: usize,
}

impl I420Converter {
    const LUMA: [i32; 3] = [66, 129, 25];
    const CHROMA_U: [i32; 3] = [-38, -74, 112];
    const CHROMA_V: [i32; 3] = [112, -94, -18];
    const FIXED_SHIFT: i32 = 8;
    const FIXED_ROUND: i32 = 1 << (Self::FIXED_SHIFT - 1);
    const LUMA_OFFSET: i32 = 16;
    const CHROMA_OFFSET: i32 = 128;
    const BLUE: usize = 0;
    const GREEN: usize = 1;
    const RED: usize = 2;

    const fn new(width: u32, height: u32) -> Self {
        Self {
            width: width as usize,
            height: height as usize,
        }
    }

    /// The 4:2:0 picture of `bgra`.
    fn convert(&self, bgra: &[u8]) -> Result<I420Picture, String> {
        let (width, height) = (self.width, self.height);
        if bgra.len() != width * height * BGRA_BYTES {
            return Err(format!(
                "picture has {} bytes, {width}x{height} needs {}",
                bgra.len(),
                width * height * BGRA_BYTES
            ));
        }
        let (chroma_width, chroma_height) = I420Picture::chroma_size(width, height);
        let luma_bytes = width * height;
        let chroma_bytes = chroma_width * chroma_height;
        let mut data = vec![0_u8; luma_bytes + 2 * chroma_bytes];
        let (luma, chroma) = data.split_at_mut(luma_bytes);
        let (u_plane, v_plane) = chroma.split_at_mut(chroma_bytes);
        let pixel = |x: usize, y: usize| {
            let at = (y * width + x) * BGRA_BYTES;
            [
                i32::from(bgra[at + Self::RED]),
                i32::from(bgra[at + Self::GREEN]),
                i32::from(bgra[at + Self::BLUE]),
            ]
        };
        let weigh = |rgb: [i32; 3], weights: [i32; 3]| {
            (weights[0] * rgb[0] + weights[1] * rgb[1] + weights[2] * rgb[2] + Self::FIXED_ROUND)
                >> Self::FIXED_SHIFT
        };
        for (y, row) in luma.chunks_exact_mut(width).enumerate() {
            for (x, out) in row.iter_mut().enumerate() {
                *out = (weigh(pixel(x, y), Self::LUMA) + Self::LUMA_OFFSET).clamp(0, 255) as u8;
            }
        }
        for cy in 0..chroma_height {
            for cx in 0..chroma_width {
                let (mut sum, mut count) = ([0_i32; 3], 0_i32);
                for y in (cy * 2)..(cy * 2 + 2).min(height) {
                    for x in (cx * 2)..(cx * 2 + 2).min(width) {
                        for (total, value) in sum.iter_mut().zip(pixel(x, y)) {
                            *total += value;
                        }
                        count += 1;
                    }
                }
                let mean = sum.map(|total| (total + count / 2) / count);
                u_plane[cy * chroma_width + cx] =
                    (weigh(mean, Self::CHROMA_U) + Self::CHROMA_OFFSET).clamp(0, 255) as u8;
                v_plane[cy * chroma_width + cx] =
                    (weigh(mean, Self::CHROMA_V) + Self::CHROMA_OFFSET).clamp(0, 255) as u8;
            }
        }
        Ok(I420Picture {
            width,
            height,
            data,
        })
    }
}

/// One libaom AV1 encoder and the 4:2:0 picture it reads from.
struct Av1Encoder {
    context: Box<aom::aom_codec_ctx_t>,
    image: *mut aom::aom_image_t,
    width: usize,
    height: usize,
}

impl Av1Encoder {
    fn create(width: u32, height: u32, settings: EncoderSettings) -> Result<Self, String> {
        // SAFETY: returns libaom's static interface table.
        let iface = unsafe { aom::aom_codec_av1_cx() };
        // SAFETY: a plain C struct; libaom fills every field below.
        let mut config: aom::aom_codec_enc_cfg_t = unsafe { std::mem::zeroed() };
        // SAFETY: a valid interface and a writable configuration.
        let code = unsafe {
            aom::aom_codec_enc_config_default(iface, &raw mut config, aom::AOM_USAGE_REALTIME)
        };
        if code != aom::AOM_CODEC_OK {
            return Err(format!(
                "aom_codec_enc_config_default failed: libaom error {code}"
            ));
        }
        config.g_w = width;
        config.g_h = height;
        config.g_profile = MAIN_PROFILE;
        config.g_bit_depth = aom::AOM_BITS_8;
        config.g_input_bit_depth = BIT_DEPTH;
        config.g_timebase = aom::aom_rational {
            num: 1,
            den: RECORD_FPS as c_int,
        };
        config.g_threads = settings.threads;
        config.g_lag_in_frames = NO_LAG;
        config.rc_end_usage = aom::AOM_Q;
        // Key pictures are placed by `VideoQueue`, at instant distances, not by libaom.
        config.kf_mode = aom::AOM_KF_DISABLED;
        // SAFETY: a plain C struct that `aom_codec_enc_init_ver` initialises.
        let mut context: Box<aom::aom_codec_ctx_t> = Box::new(unsafe { std::mem::zeroed() });
        // SAFETY: valid context, interface and configuration; the ABI version of these bindings.
        let code = unsafe {
            aom::aom_codec_enc_init_ver(
                &raw mut *context,
                iface,
                &raw const config,
                0,
                aom::AOM_ENCODER_ABI_VERSION as c_int,
            )
        };
        if code != aom::AOM_CODEC_OK {
            return Err(format!("aom_codec_enc_init_ver failed: libaom error {code}"));
        }
        // SAFETY: libaom allocates and owns the picture; checked for null below.
        let image = unsafe {
            aom::aom_img_alloc(
                std::ptr::null_mut(),
                aom::AOM_IMG_FMT_I420,
                width,
                height,
                IMAGE_ALIGN,
            )
        };
        // Built before the null check so `Drop` releases the context either way.
        let mut encoder = Self {
            context,
            image,
            width: width as usize,
            height: height as usize,
        };
        if image.is_null() {
            return Err("aom_img_alloc failed".to_owned());
        }
        encoder.control_int("AOME_SET_CPUUSED", aom::AOME_SET_CPUUSED, CPU_USED)?;
        encoder.control_uint("AOME_SET_CQ_LEVEL", aom::AOME_SET_CQ_LEVEL, settings.quality)?;
        encoder.control_int(
            "AV1E_SET_TUNE_CONTENT",
            aom::AV1E_SET_TUNE_CONTENT,
            aom::AOM_CONTENT_SCREEN as c_int,
        )?;
        encoder.control_uint(
            "AV1E_SET_ROW_MT",
            aom::AV1E_SET_ROW_MT,
            ROW_MULTITHREADING_ON,
        )?;
        Ok(encoder)
    }

    fn control_int(
        &mut self,
        name: &str,
        id: aom::aome_enc_control_id,
        value: c_int,
    ) -> Result<(), String> {
        // SAFETY: an initialised encoder; this control takes one int.
        let code = unsafe { aom::aom_codec_control(&raw mut *self.context, id as c_int, value) };
        self.check(code, name)
    }

    fn control_uint(
        &mut self,
        name: &str,
        id: aom::aome_enc_control_id,
        value: c_uint,
    ) -> Result<(), String> {
        // SAFETY: an initialised encoder; this control takes one unsigned int.
        let code = unsafe { aom::aom_codec_control(&raw mut *self.context, id as c_int, value) };
        self.check(code, name)
    }

    fn check(&self, code: aom::aom_codec_err_t, step: &str) -> Result<(), String> {
        if code == aom::AOM_CODEC_OK {
            return Ok(());
        }
        // SAFETY: an initialised encoder; libaom returns a static or context-owned string.
        let text = unsafe { CStr::from_ptr(aom::aom_codec_error(&raw const *self.context)) };
        Err(format!("{step} failed: {}", text.to_string_lossy()))
    }

    /// Copies `picture` into the encoder's own picture buffer, row by row.
    fn fill(&mut self, picture: &I420Picture) -> Result<(), String> {
        if (picture.width, picture.height) != (self.width, self.height) {
            return Err(format!(
                "picture is {}x{}, the encoder {}x{}",
                picture.width, picture.height, self.width, self.height
            ));
        }
        // SAFETY: allocated in `create`, non-null, owned by this encoder alone.
        let image = unsafe { &mut *self.image };
        for (plane, (source, row_bytes, rows)) in picture.planes().into_iter().enumerate() {
            let stride = usize::try_from(image.stride[plane]).unwrap_or(0);
            if stride < row_bytes {
                return Err("the encoder's picture rows are shorter than the window".to_owned());
            }
            // SAFETY: allocated by `aom_img_alloc` for these dimensions and not
            // shared; each plane holds at least `stride * rows` bytes (rows
            // rounded up to even for U and V).
            let target = unsafe { std::slice::from_raw_parts_mut(image.planes[plane], stride * rows) };
            for (row, line) in source.chunks_exact(row_bytes).enumerate() {
                target[row * stride..row * stride + row_bytes].copy_from_slice(line);
            }
        }
        Ok(())
    }

    /// Encodes one 4:2:0 picture; `pts` is its instant number.
    fn encode(&mut self, picture: &I420Picture, pts: i64, force_key: bool) -> Result<Packet, String> {
        self.fill(picture)?;
        let flags = if force_key {
            aom::AOM_EFLAG_FORCE_KF as aom::aom_enc_frame_flags_t
        } else {
            0
        };
        // SAFETY: an initialised encoder and its own filled picture.
        let code = unsafe {
            aom::aom_codec_encode(
                &raw mut *self.context,
                self.image,
                pts,
                PICTURE_DURATION,
                flags,
            )
        };
        self.check(code, "aom_codec_encode")?;
        let mut packets = Vec::new();
        let mut iterator: aom::aom_codec_iter_t = std::ptr::null();
        loop {
            // SAFETY: an initialised encoder; the packet stays valid until the next call.
            let packet =
                unsafe { aom::aom_codec_get_cx_data(&raw mut *self.context, &raw mut iterator) };
            if packet.is_null() {
                break;
            }
            // SAFETY: non-null packets point at libaom's packet struct.
            let packet = unsafe { &*packet };
            if packet.kind != aom::AOM_CODEC_CX_FRAME_PKT {
                continue;
            }
            // SAFETY: frame packets carry the `frame` member of the union.
            let frame = unsafe { packet.data.frame };
            // SAFETY: libaom's buffer of `sz` bytes, valid until the next call.
            let data = unsafe { std::slice::from_raw_parts(frame.buf.cast::<u8>(), frame.sz) };
            packets.push(Packet {
                data: data.to_vec(),
                key: frame.flags & aom::AOM_FRAME_IS_KEY != 0,
            });
        }
        match packets.len() {
            1 => packets.pop().ok_or_else(|| "no packet".to_owned()),
            n => Err(format!(
                "libaom returned {n} pictures for instant {pts}, expected 1"
            )),
        }
    }
}

impl Drop for Av1Encoder {
    fn drop(&mut self) {
        // SAFETY: both were created in `create` and are released once, here.
        unsafe {
            aom::aom_codec_destroy(&raw mut *self.context);
            if !self.image.is_null() {
                aom::aom_img_free(self.image);
            }
        }
    }
}

/// The part being written and the instants it holds.
struct OpenPart {
    number: u32,
    writer: IvfWriter,
    first_instant: u64,
    last_instant: u64,
}

/// One picture for the encoder thread, with the place the recorder gave it.
struct Job {
    instant: u64,
    picture: I420Picture,
    force_key: bool,
    place: VideoPlace,
}

/// The video's stage totals over one recording: conversion and queueing on
/// the recorder thread, encoding on the encoder thread.
#[derive(Clone, Copy, Default)]
pub(super) struct VideoTimes {
    pub(super) convert: StageTime,
    pub(super) submit: StageTime,
    /// The most pictures seen waiting in the queue right after a submit.
    pub(super) max_queue_depth: usize,
    /// Every libaom encode call (key pictures included).
    pub(super) encode: StageTime,
    pub(super) key_encode: StageTime,
}

/// What the encoder thread wrote: video bytes and the parts it closed, read back.
#[derive(Default)]
pub(super) struct VideoWritten {
    pub(super) bytes: u64,
    pub(super) closed_parts: Vec<PartInfo>,
}

impl VideoWritten {
    fn add(&mut self, more: Self) {
        self.bytes += more.bytes;
        self.closed_parts.extend(more.closed_parts);
    }
}

/// The encoder thread's side: one AV1 encoder, a new IVF part at each key
/// picture. libaom encodes one size per encoder, so a picture of a new size
/// (always a planned key picture) gets a new encoder and opens a new part.
struct ClipEncoder {
    folder: PathBuf,
    encoder: Av1Encoder,
    settings: EncoderSettings,
    /// The size of the current encoder and part.
    width: u32,
    height: u32,
    part: Option<OpenPart>,
    /// Encode times (only `encode` and `key_encode` are this side's).
    times: VideoTimes,
}

impl ClipEncoder {
    fn create(
        folder: &Path,
        width: u32,
        height: u32,
        settings: EncoderSettings,
    ) -> Result<Self, String> {
        Ok(Self {
            folder: folder.to_owned(),
            encoder: Av1Encoder::create(width, height, settings)?,
            settings,
            width,
            height,
            part: None,
            times: VideoTimes::default(),
        })
    }

    /// Encodes `job` and appends it to the current part; a key picture first
    /// closes that part and starts the next. The picture must land where the
    /// recorder said, because the index already names that place.
    fn encode(&mut self, job: Job) -> Result<VideoWritten, String> {
        let pts = i64::try_from(job.instant).map_err(|_| "instant out of range".to_owned())?;
        let size = (
            u32::try_from(job.picture.width).map_err(|_| "picture too wide".to_owned())?,
            u32::try_from(job.picture.height).map_err(|_| "picture too tall".to_owned())?,
        );
        if size != (self.width, self.height) {
            if !job.force_key {
                return Err(format!(
                    "instant {}: the picture size changed without a planned key picture",
                    job.instant
                ));
            }
            // The key picture below closes the part of the old size and opens one of this size.
            self.encoder = Av1Encoder::create(size.0, size.1, self.settings)?;
            (self.width, self.height) = size;
        }
        let started = Instant::now();
        let packet = self.encoder.encode(&job.picture, pts, job.force_key)?;
        let took = started.elapsed();
        self.times.encode.add(took);
        if packet.key {
            self.times.key_encode.add(took);
        }
        if packet.key != job.force_key {
            return Err(format!(
                "instant {}: libaom gave key={}, key={} was planned",
                job.instant, packet.key, job.force_key
            ));
        }
        let mut written = VideoWritten::default();
        if packet.key {
            written.closed_parts.extend(self.start_part(job.instant)?);
        }
        let Some(part) = self.part.as_mut() else {
            return Err("the first encoded picture is not a key picture".to_owned());
        };
        let place = (part.number, part.writer.pictures());
        if place != job.place {
            return Err(format!(
                "instant {}: picture went to {place:?}, the index names {:?}",
                job.instant, job.place
            ));
        }
        written.bytes = part.writer.append(job.instant, &packet.data)?;
        part.last_instant = job.instant;
        Ok(written)
    }

    /// Closes the open part (if any) and opens the next one at `instant`.
    fn start_part(&mut self, instant: u64) -> Result<Option<PartInfo>, String> {
        let (number, closed) = match self.part.take() {
            Some(open) => {
                let next = open.number + 1;
                (next, Some(self.close(open)?))
            }
            None => (0, None),
        };
        let writer = IvfWriter::create(
            &part_path(&self.folder, number),
            self.width,
            self.height,
            RECORD_FPS,
        )?;
        self.part = Some(OpenPart {
            number,
            writer,
            first_instant: instant,
            last_instant: instant,
        });
        Ok(closed)
    }

    fn close(&self, open: OpenPart) -> Result<PartInfo, String> {
        let pictures = open.writer.pictures();
        open.writer.close()?;
        PartInfo::read_back(
            &self.folder,
            open.number,
            open.first_instant,
            open.last_instant,
            pictures,
        )
    }

    /// Closes the last part; returns it read back, with the encode times.
    /// Realtime libaom without look-ahead holds no pictures, so nothing is
    /// left to drain.
    fn finish(mut self) -> Result<(Option<PartInfo>, VideoTimes), String> {
        let last = match self.part.take() {
            Some(open) => Some(self.close(open)?),
            None => None,
        };
        Ok((last, self.times))
    }
}

/// The encoder thread: encodes the queued pictures in order until the
/// recorder drops its end of the queue, then closes the last part. An error
/// is also sent to the recorder at once, so the recording stops on it.
fn encode_queued(
    folder: &Path,
    width: u32,
    height: u32,
    settings: EncoderSettings,
    queued: &Receiver<Job>,
    waiting: &AtomicUsize,
    report: &Sender<Result<VideoWritten, String>>,
) -> Result<(Option<PartInfo>, VideoTimes), String> {
    let result = ClipEncoder::create(folder, width, height, settings).and_then(|mut video| {
        for job in queued {
            waiting.fetch_sub(1, Ordering::Relaxed);
            let written = video.encode(job)?;
            // A send fails only when the recorder dropped the queue without
            // `finish`; there is no one left to tell then.
            let _ = report.send(Ok(written));
        }
        video.finish()
    });
    if let Err(error) = &result {
        let _ = report.send(Err(error.clone()));
    }
    result
}

/// The clip's video behind a bounded queue (plan §6). On the recorder thread
/// it converts each changed picture to 4:2:0, gives it its place [part,
/// index] (a new part at every key picture: `key_instants` apart, and at the
/// first picture of a new window size) and queues it; the encoder thread
/// encodes the pictures in order and checks each place. A full queue refuses
/// the picture at once: the recorder never waits for an encode.
pub(super) struct VideoQueue {
    /// Converts pictures of the size it was made for; replaced when the size changes.
    converter: I420Converter,
    /// The size of the last queued picture.
    queued_size: Option<(u32, u32)>,
    key_instants: u64,
    last_key: Option<u64>,
    /// The place of the last queued picture.
    last_place: Option<VideoPlace>,
    jobs: SyncSender<Job>,
    /// Pictures queued and not yet taken by the encoder thread.
    waiting: Arc<AtomicUsize>,
    done: Receiver<Result<VideoWritten, String>>,
    thread: JoinHandle<Result<(Option<PartInfo>, VideoTimes), String>>,
    /// Conversion and queueing times; the thread adds its encode times at `finish`.
    times: VideoTimes,
}

impl VideoQueue {
    /// Starts the encoder thread for pictures of `width` x `height`; `folder`
    /// is the clip folder (parts go to its `video/`).
    pub(super) fn start(
        folder: &Path,
        width: u32,
        height: u32,
        settings: EncoderSettings,
    ) -> Result<Self, String> {
        let video = folder.join(VIDEO_DIR);
        std::fs::create_dir_all(&video)
            .map_err(|error| format!("could not create {}: {error}", video.display()))?;
        let (jobs, queued) = std::sync::mpsc::sync_channel(ENCODER_QUEUE_PICTURES);
        let (report, done) = std::sync::mpsc::channel();
        let waiting = Arc::new(AtomicUsize::new(0));
        let folder = folder.to_owned();
        let thread_waiting = Arc::clone(&waiting);
        let thread = std::thread::Builder::new()
            .name(ENCODER_THREAD_NAME.to_owned())
            .spawn(move || {
                encode_queued(
                    &folder,
                    width,
                    height,
                    settings,
                    &queued,
                    &thread_waiting,
                    &report,
                )
            })
            .map_err(|error| format!("could not start the encoder thread: {error}"))?;
        Ok(Self {
            converter: I420Converter::new(width, height),
            queued_size: None,
            key_instants: settings.key_instants,
            last_key: None,
            last_place: None,
            jobs,
            waiting,
            done,
            thread,
            times: VideoTimes::default(),
        })
    }

    /// Converts and queues the picture of `instant` (B,G,R,A rows of `width`
    /// x `height`); returns its place, or `None` when the queue is full (the
    /// encoder is behind). The first picture of a new size is a key picture
    /// that starts a new part.
    pub(super) fn submit(
        &mut self,
        instant: u64,
        bgra: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Option<VideoPlace>, String> {
        let resized = self.queued_size.is_some_and(|size| size != (width, height));
        let force_key = resized
            || self
                .last_key
                .is_none_or(|key| instant >= key + self.key_instants);
        if (self.converter.width, self.converter.height) != (width as usize, height as usize) {
            self.converter = I420Converter::new(width, height);
        }
        let place = match self.last_place {
            Some((part, index)) if !force_key => (part, index + 1),
            Some((part, _)) => (part + 1, 0),
            None => (0, 0),
        };
        let converter = &self.converter;
        let picture = self.times.convert.time(|| converter.convert(bgra))?;
        let job = Job {
            instant,
            picture,
            force_key,
            place,
        };
        // Counted before the send, so the thread's decrement never runs first.
        let waiting = self.waiting.fetch_add(1, Ordering::Relaxed) + 1;
        let jobs = &self.jobs;
        let sent = self.times.submit.time(|| jobs.try_send(job));
        if sent.is_err() {
            self.waiting.fetch_sub(1, Ordering::Relaxed);
        } else {
            self.times.max_queue_depth = self.times.max_queue_depth.max(waiting);
        }
        match sent {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => return Ok(None),
            Err(TrySendError::Disconnected(_)) => {
                return Err(self
                    .written()
                    .err()
                    .unwrap_or_else(|| "the encoder thread ended".to_owned()));
            }
        }
        if force_key {
            self.last_key = Some(instant);
        }
        self.last_place = Some(place);
        self.queued_size = Some((width, height));
        Ok(Some(place))
    }

    /// What the encoder thread wrote since the last call; its error, if it failed.
    pub(super) fn written(&self) -> Result<VideoWritten, String> {
        let mut written = VideoWritten::default();
        for message in self.done.try_iter() {
            written.add(message?);
        }
        Ok(written)
    }

    /// Lets the encoder thread encode every queued picture and close the last
    /// part; returns what it wrote since the last `written`, the last part
    /// included, and the video's stage times.
    pub(super) fn finish(self) -> Result<(VideoWritten, VideoTimes), String> {
        let Self {
            jobs,
            done,
            thread,
            mut times,
            ..
        } = self;
        drop(jobs);
        let (last_part, thread_times) = thread
            .join()
            .map_err(|_| "the encoder thread panicked".to_owned())??;
        let mut written = VideoWritten::default();
        for message in done.try_iter() {
            written.add(message?);
        }
        written.closed_parts.extend(last_part);
        times.encode = thread_times.encode;
        times.key_encode = thread_times.key_encode;
        Ok((written, times))
    }
}
