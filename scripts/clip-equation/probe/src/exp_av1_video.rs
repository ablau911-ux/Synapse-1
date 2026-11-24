//! exp-av1, the video half (docs/research/window-recording/probe-plan.md, part 2):
//! libaom in realtime screen mode, the B,G,R to 4:2:0 conversion it is fed with,
//! and the IVF parts it writes. A new part starts at every key picture, so each
//! part decodes on its own and a killed run keeps every finished part.

use std::ffi::{CStr, c_int, c_uint};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use libaom_sys as aom;
use serde_json::json;

use crate::Result;

/// Instants per second; one instant is one tick of the IVF and libaom time base.
pub const INSTANTS_PER_SECOND: u32 = 20;
/// The ffmpeg settings part 1 measured (`-usage realtime -cpu-used 8 -row-mt 1`).
const CPU_USED: c_int = 8;
const ROW_MULTITHREADING_ON: c_uint = 1;
/// No look-ahead: every picture leaves the encoder in the same call.
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
const BGR_BYTES: usize = 3;

/// BT.601 limited range, 8-bit fixed point (what ffmpeg's bgr24 to yuv420p gives).
const LUMA: [i32; 3] = [66, 129, 25];
const CHROMA_U: [i32; 3] = [-38, -74, 112];
const CHROMA_V: [i32; 3] = [112, -94, -18];
const FIXED_SHIFT: i32 = 8;
const FIXED_ROUND: i32 = 1 << (FIXED_SHIFT - 1);
const LUMA_OFFSET: i32 = 16;
const CHROMA_OFFSET: i32 = 128;

const IVF_SIGNATURE: &[u8; 4] = b"DKIF";
const IVF_VERSION: u16 = 0;
const IVF_HEADER_BYTES: u16 = 32;
const IVF_AV1_FOURCC: &[u8; 4] = b"AV01";
const IVF_TIME_SCALE: u32 = 1;
const IVF_PICTURE_COUNT_OFFSET: u64 = 24;
const IVF_PICTURE_HEADER_BYTES: usize = 12;

/// The encoder settings a run uses; reported in probe.json.
#[derive(Clone, Copy)]
pub struct EncoderSettings {
    /// Constant quality level (`-crf`), 0..63.
    pub quality: u32,
    /// A key picture at the first encoded picture this many instants after the last one.
    pub key_instants: u64,
    pub threads: u32,
}

impl EncoderSettings {
    pub fn new(quality: u32, key_instants: u64) -> Self {
        let threads = std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(MAX_THREADS);
        Self {
            quality,
            key_instants,
            threads: u32::try_from(threads).unwrap_or(1),
        }
    }

    pub fn describe(&self) -> serde_json::Value {
        json!({
            "usage": "realtime", "cpu_used": CPU_USED, "end_usage": "q",
            "cq_level": self.quality, "bitrate_cap": "none", "tune_content": "screen",
            "row_mt": ROW_MULTITHREADING_ON, "threads": self.threads, "lag_in_frames": NO_LAG,
            "format": "I420 8 bit BT.601 limited", "key_instants": self.key_instants,
            "time_base": format!("1/{INSTANTS_PER_SECOND}")
        })
    }
}

/// The libaom version string, e.g. "3.15.3".
pub fn libaom_version() -> String {
    // SAFETY: libaom returns a static, NUL-terminated string.
    unsafe { CStr::from_ptr(aom::aom_codec_version_str()) }
        .to_string_lossy()
        .into_owned()
}

/// One compressed picture.
struct Packet {
    data: Vec<u8>,
    key: bool,
}

/// One libaom AV1 encoder and the 4:2:0 picture it reads from.
struct Av1Encoder {
    context: Box<aom::aom_codec_ctx_t>,
    image: *mut aom::aom_image_t,
    width: usize,
    height: usize,
}

impl Av1Encoder {
    fn create(width: u32, height: u32, settings: EncoderSettings) -> Result<Self> {
        // SAFETY: returns libaom's static interface table.
        let iface = unsafe { aom::aom_codec_av1_cx() };
        // SAFETY: a plain C struct; libaom fills every field below.
        let mut config: aom::aom_codec_enc_cfg_t = unsafe { std::mem::zeroed() };
        // SAFETY: a valid interface and a writable configuration.
        let code = unsafe {
            aom::aom_codec_enc_config_default(iface, &raw mut config, aom::AOM_USAGE_REALTIME)
        };
        if code != aom::AOM_CODEC_OK {
            return Err(format!("aom_codec_enc_config_default failed: libaom error {code}"));
        }
        config.g_w = width;
        config.g_h = height;
        config.g_profile = MAIN_PROFILE;
        config.g_bit_depth = aom::AOM_BITS_8;
        config.g_input_bit_depth = BIT_DEPTH;
        config.g_timebase = aom::aom_rational {
            num: 1,
            den: INSTANTS_PER_SECOND as c_int,
        };
        config.g_threads = settings.threads;
        config.g_lag_in_frames = NO_LAG;
        config.rc_end_usage = aom::AOM_Q;
        // Key pictures are placed by `Video`, at instant distances, not by libaom.
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
        let mut encoder = Self {
            context,
            image,
            width: width as usize,
            height: height as usize,
        };
        if image.is_null() {
            return Err("aom_img_alloc failed".into());
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

    fn control_int(&mut self, name: &str, id: aom::aome_enc_control_id, value: c_int) -> Result<()> {
        // SAFETY: an initialised encoder; this control takes one int.
        let code = unsafe { aom::aom_codec_control(&raw mut *self.context, id as c_int, value) };
        self.check(code, name)
    }

    fn control_uint(&mut self, name: &str, id: aom::aome_enc_control_id, value: c_uint) -> Result<()> {
        // SAFETY: an initialised encoder; this control takes one unsigned int.
        let code = unsafe { aom::aom_codec_control(&raw mut *self.context, id as c_int, value) };
        self.check(code, name)
    }

    fn check(&self, code: aom::aom_codec_err_t, step: &str) -> Result<()> {
        if code == aom::AOM_CODEC_OK {
            return Ok(());
        }
        // SAFETY: an initialised encoder; libaom returns a static or context-owned string.
        let text = unsafe { CStr::from_ptr(aom::aom_codec_error(&raw const *self.context)) };
        Err(format!("{step} failed: {}", text.to_string_lossy()))
    }

    /// Encodes one picture; `pts` is its instant number.
    fn encode(&mut self, bgr: &[u8], pts: i64, force_key: bool) -> Result<Packet> {
        self.fill(bgr)?;
        let flags = if force_key {
            aom::AOM_EFLAG_FORCE_KF as aom::aom_enc_frame_flags_t
        } else {
            0
        };
        // SAFETY: an initialised encoder and its own filled picture.
        let code = unsafe {
            aom::aom_codec_encode(&raw mut *self.context, self.image, pts, PICTURE_DURATION, flags)
        };
        self.check(code, "aom_codec_encode")?;
        let mut packets = Vec::new();
        let mut iterator: aom::aom_codec_iter_t = std::ptr::null();
        loop {
            // SAFETY: an initialised encoder; the packet stays valid until the next call.
            let packet = unsafe { aom::aom_codec_get_cx_data(&raw mut *self.context, &raw mut iterator) };
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
            n => Err(format!("libaom returned {n} pictures for instant {pts}, expected 1")),
        }
    }

    /// B,G,R rows (top to bottom) into the encoder's 4:2:0 planes.
    fn fill(&mut self, bgr: &[u8]) -> Result<()> {
        let (width, height) = (self.width, self.height);
        if bgr.len() != width * height * BGR_BYTES {
            return Err(format!(
                "picture has {} bytes, {width}x{height} needs {}",
                bgr.len(),
                width * height * BGR_BYTES
            ));
        }
        let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
        // SAFETY: allocated by `aom_img_alloc` for these dimensions and not shared.
        let image = unsafe { &*self.image };
        let stride = |plane: usize| usize::try_from(image.stride[plane]).unwrap_or(0);
        // SAFETY: each plane holds at least `stride * rows` bytes (rows rounded up to even).
        let (luma, u_plane, v_plane) = unsafe {
            (
                std::slice::from_raw_parts_mut(image.planes[0], stride(0) * height),
                std::slice::from_raw_parts_mut(image.planes[1], stride(1) * chroma_height),
                std::slice::from_raw_parts_mut(image.planes[2], stride(2) * chroma_height),
            )
        };
        let pixel = |x: usize, y: usize| {
            let at = (y * width + x) * BGR_BYTES;
            [i32::from(bgr[at + 2]), i32::from(bgr[at + 1]), i32::from(bgr[at])]
        };
        let weigh = |rgb: [i32; 3], weights: [i32; 3]| {
            (weights[0] * rgb[0] + weights[1] * rgb[1] + weights[2] * rgb[2] + FIXED_ROUND)
                >> FIXED_SHIFT
        };
        for y in 0..height {
            let row = &mut luma[y * stride(0)..y * stride(0) + width];
            for (x, out) in row.iter_mut().enumerate() {
                *out = (weigh(pixel(x, y), LUMA) + LUMA_OFFSET).clamp(0, 255) as u8;
            }
        }
        for cy in 0..chroma_height {
            for cx in 0..chroma_width {
                let (mut sum, mut count) = ([0_i32; 3], 0_i32);
                for y in (cy * 2)..(cy * 2 + 2).min(height) {
                    for x in (cx * 2)..(cx * 2 + 2).min(width) {
                        let rgb = pixel(x, y);
                        for channel in 0..3 {
                            sum[channel] += rgb[channel];
                        }
                        count += 1;
                    }
                }
                let mean = sum.map(|total| (total + count / 2) / count);
                u_plane[cy * stride(1) + cx] =
                    (weigh(mean, CHROMA_U) + CHROMA_OFFSET).clamp(0, 255) as u8;
                v_plane[cy * stride(2) + cx] =
                    (weigh(mean, CHROMA_V) + CHROMA_OFFSET).clamp(0, 255) as u8;
            }
        }
        Ok(())
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

/// One IVF file: the 32-byte header, then per picture its size, its timestamp and its bytes.
/// Pictures go straight to the file, so a kill keeps every picture written before it.
struct IvfFile {
    file: File,
    pictures: u32,
    bytes: u64,
}

impl IvfFile {
    fn create(path: &Path, width: u32, height: u32) -> Result<Self> {
        let narrow = |value: u32| {
            u16::try_from(value).map_err(|_| format!("IVF cannot hold a size of {value}"))
        };
        let mut header = Vec::with_capacity(usize::from(IVF_HEADER_BYTES));
        header.extend_from_slice(IVF_SIGNATURE);
        header.extend_from_slice(&IVF_VERSION.to_le_bytes());
        header.extend_from_slice(&IVF_HEADER_BYTES.to_le_bytes());
        header.extend_from_slice(IVF_AV1_FOURCC);
        header.extend_from_slice(&narrow(width)?.to_le_bytes());
        header.extend_from_slice(&narrow(height)?.to_le_bytes());
        header.extend_from_slice(&INSTANTS_PER_SECOND.to_le_bytes());
        header.extend_from_slice(&IVF_TIME_SCALE.to_le_bytes());
        header.extend_from_slice(&0_u32.to_le_bytes()); // picture count, set at close
        header.extend_from_slice(&0_u32.to_le_bytes()); // unused
        let mut file =
            File::create(path).map_err(|e| format!("create {}: {e}", path.display()))?;
        file.write_all(&header)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        Ok(Self {
            file,
            pictures: 0,
            bytes: header.len() as u64,
        })
    }

    fn append(&mut self, timestamp: u64, data: &[u8]) -> Result<()> {
        let size = u32::try_from(data.len()).map_err(|_| "picture too large for IVF")?;
        let mut record = Vec::with_capacity(IVF_PICTURE_HEADER_BYTES + data.len());
        record.extend_from_slice(&size.to_le_bytes());
        record.extend_from_slice(&timestamp.to_le_bytes());
        record.extend_from_slice(data);
        self.file
            .write_all(&record)
            .map_err(|e| format!("write IVF picture: {e}"))?;
        self.pictures += 1;
        self.bytes += record.len() as u64;
        Ok(())
    }

    /// Writes the picture count into the header; returns the file's size.
    fn close(mut self) -> Result<u64> {
        self.file
            .seek(SeekFrom::Start(IVF_PICTURE_COUNT_OFFSET))
            .and_then(|_| self.file.write_all(&self.pictures.to_le_bytes()))
            .map_err(|e| format!("write IVF picture count: {e}"))?;
        Ok(self.bytes)
    }
}

/// Where an encoded picture went: its part and its index in that part.
pub type VideoPlace = (u32, u32);

/// The recorded video: one encoder, a new IVF part at each key picture.
pub struct Video {
    folder: PathBuf,
    encoder: Av1Encoder,
    width: u32,
    height: u32,
    key_instants: u64,
    last_key: Option<u64>,
    part: Option<(u32, IvfFile)>,
    closed_bytes: u64,
}

impl Video {
    pub fn create(folder: &Path, width: u32, height: u32, settings: EncoderSettings) -> Result<Self> {
        std::fs::create_dir_all(folder).map_err(|e| format!("create {}: {e}", folder.display()))?;
        Ok(Self {
            folder: folder.to_owned(),
            encoder: Av1Encoder::create(width, height, settings)?,
            width,
            height,
            key_instants: settings.key_instants,
            last_key: None,
            part: None,
            closed_bytes: 0,
        })
    }

    pub const fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Encodes the picture of `instant` (B,G,R rows) and appends it to the current part.
    pub fn encode(&mut self, instant: u64, bgr: &[u8]) -> Result<VideoPlace> {
        let force_key = self
            .last_key
            .is_none_or(|key| instant >= key + self.key_instants);
        let pts = i64::try_from(instant).map_err(|_| "instant out of range")?;
        let packet = self.encoder.encode(bgr, pts, force_key)?;
        if packet.key {
            self.last_key = Some(instant);
            self.start_part()?;
        }
        let Some((number, file)) = self.part.as_mut() else {
            return Err("the first encoded picture is not a key picture".into());
        };
        let index = file.pictures;
        file.append(instant, &packet.data)?;
        Ok((*number, index))
    }

    fn start_part(&mut self) -> Result<()> {
        let number = match self.part.take() {
            Some((number, file)) => {
                self.closed_bytes += file.close()?;
                number + 1
            }
            None => 0,
        };
        let path = self.folder.join(format!("part-{number:04}.ivf"));
        self.part = Some((number, IvfFile::create(&path, self.width, self.height)?));
        Ok(())
    }

    /// Closes the last part; returns the bytes of all parts.
    pub fn finish(mut self) -> Result<u64> {
        if let Some((_, file)) = self.part.take() {
            self.closed_bytes += file.close()?;
        }
        Ok(self.closed_bytes)
    }
}
