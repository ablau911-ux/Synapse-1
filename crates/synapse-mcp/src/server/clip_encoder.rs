//! The clip's video (plan 5.0 §2, §6, W1): libaom AV1 in realtime screen
//! mode, the B,G,R,A to 4:2:0 conversion it is fed with, and the IVF parts it
//! writes. A new part starts at every key picture, so each part decodes on
//! its own, and a part is complete on disk before the next one starts.
//!
//! Moved from the W0 probe (`scripts/clip-equation/probe/src/exp_av1_video.rs`),
//! whose checks proved it; the probe's stage timers are left out.

use std::ffi::{CStr, c_int, c_uint};
use std::path::{Path, PathBuf};

use libaom_sys as aom;
use serde_json::json;

use super::capture_clip::RECORD_FPS;
use super::clip_folder::{IvfWriter, PartInfo, VIDEO_DIR, part_path};

/// Constant quality level (`-crf`), plan §6.
pub(super) const AV1_QUALITY: u32 = 35;
/// `KEY_INSTANTS`: a key picture at the first encoded picture this many
/// instants after the last one (10 s at 20 instants per second), plan §6.
pub(super) const KEY_INSTANTS: u64 = 200;
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

    /// Fills the three planes of `image` from `bgra`.
    fn convert(&self, bgra: &[u8], image: &mut aom::aom_image_t) -> Result<(), String> {
        let (width, height) = (self.width, self.height);
        if bgra.len() != width * height * BGRA_BYTES {
            return Err(format!(
                "picture has {} bytes, {width}x{height} needs {}",
                bgra.len(),
                width * height * BGRA_BYTES
            ));
        }
        let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
        let stride = |plane: usize| usize::try_from(image.stride[plane]).unwrap_or(0);
        let (luma_stride, u_stride, v_stride) = (stride(0), stride(1), stride(2));
        if luma_stride < width || u_stride < chroma_width || v_stride < chroma_width {
            return Err("the encoder's picture rows are shorter than the window".to_owned());
        }
        // SAFETY: allocated by `aom_img_alloc` for these dimensions and not
        // shared; each plane holds at least `stride * rows` bytes (rows
        // rounded up to even).
        let (luma, u_plane, v_plane) = unsafe {
            (
                std::slice::from_raw_parts_mut(image.planes[0], luma_stride * height),
                std::slice::from_raw_parts_mut(image.planes[1], u_stride * chroma_height),
                std::slice::from_raw_parts_mut(image.planes[2], v_stride * chroma_height),
            )
        };
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
        for y in 0..height {
            let row = &mut luma[y * luma_stride..y * luma_stride + width];
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
                u_plane[cy * u_stride + cx] =
                    (weigh(mean, Self::CHROMA_U) + Self::CHROMA_OFFSET).clamp(0, 255) as u8;
                v_plane[cy * v_stride + cx] =
                    (weigh(mean, Self::CHROMA_V) + Self::CHROMA_OFFSET).clamp(0, 255) as u8;
            }
        }
        Ok(())
    }
}

/// One libaom AV1 encoder and the 4:2:0 picture it reads from.
struct Av1Encoder {
    context: Box<aom::aom_codec_ctx_t>,
    image: *mut aom::aom_image_t,
    converter: I420Converter,
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
        // Key pictures are placed by `ClipEncoder`, at instant distances, not by libaom.
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
            converter: I420Converter::new(width, height),
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

    /// Converts and encodes one B,G,R,A picture; `pts` is its instant number.
    fn encode(&mut self, bgra: &[u8], pts: i64, force_key: bool) -> Result<Packet, String> {
        // SAFETY: allocated in `create`, non-null, owned by this encoder alone.
        let image = unsafe { &mut *self.image };
        self.converter.convert(bgra, image)?;
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

/// What one encoded picture produced.
pub(super) struct Encoded {
    pub(super) place: VideoPlace,
    /// Bytes written to the video for this picture.
    pub(super) bytes: u64,
    /// The part this picture's key picture closed, read back.
    pub(super) closed_part: Option<PartInfo>,
}

/// The clip's video: one AV1 encoder, a new IVF part at each key picture.
pub(super) struct ClipEncoder {
    folder: PathBuf,
    encoder: Av1Encoder,
    width: u32,
    height: u32,
    key_instants: u64,
    last_key: Option<u64>,
    part: Option<OpenPart>,
}

impl ClipEncoder {
    /// Opens the encoder for pictures of `width` x `height`; `folder` is the
    /// clip folder (parts go to its `video/`).
    pub(super) fn create(
        folder: &Path,
        width: u32,
        height: u32,
        settings: EncoderSettings,
    ) -> Result<Self, String> {
        let video = folder.join(VIDEO_DIR);
        std::fs::create_dir_all(&video)
            .map_err(|error| format!("could not create {}: {error}", video.display()))?;
        Ok(Self {
            folder: folder.to_owned(),
            encoder: Av1Encoder::create(width, height, settings)?,
            width,
            height,
            key_instants: settings.key_instants,
            last_key: None,
            part: None,
        })
    }

    pub(super) const fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Encodes the picture of `instant` (B,G,R,A rows) and appends it to the
    /// current part; a key picture first closes that part and starts the next.
    pub(super) fn encode(&mut self, instant: u64, bgra: &[u8]) -> Result<Encoded, String> {
        let force_key = self
            .last_key
            .is_none_or(|key| instant >= key + self.key_instants);
        let pts = i64::try_from(instant).map_err(|_| "instant out of range".to_owned())?;
        let packet = self.encoder.encode(bgra, pts, force_key)?;
        let mut closed_part = None;
        if packet.key {
            self.last_key = Some(instant);
            closed_part = self.start_part(instant)?;
        }
        let Some(part) = self.part.as_mut() else {
            return Err("the first encoded picture is not a key picture".to_owned());
        };
        let index = part.writer.pictures();
        let bytes = part.writer.append(instant, &packet.data)?;
        part.last_instant = instant;
        Ok(Encoded {
            place: (part.number, index),
            bytes,
            closed_part,
        })
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

    /// Closes the last part; returns it read back. Realtime libaom without
    /// look-ahead holds no pictures, so nothing is left to drain.
    pub(super) fn finish(mut self) -> Result<Option<PartInfo>, String> {
        match self.part.take() {
            Some(open) => self.close(open).map(Some),
            None => Ok(None),
        }
    }
}
