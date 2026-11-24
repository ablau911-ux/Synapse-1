//! Clip decoder (docs/clip-recorder-plan-5.0.md, sections 3 and 5): the
//! picture shown at one instant of a clip folder.
//!
//! - `PictureSource` owns the rule of which recording shows instant k: the
//!   still of the picture on screen when one exists (exact), else the encoded
//!   video picture it repeats, else nothing (lost).
//! - `ClipDecoder` turns that source into pixels: a lossless WebP still, or
//!   the AV1 pictures of one video part decoded by libaom from the part's
//!   first (key) picture up to the one asked for. Decoding never reads more
//!   than one part.
//!
//! Every picture comes back as top-down RGBA with opaque alpha.

use std::ffi::CStr;
use std::os::raw::{c_int, c_uint};
use std::path::{Path, PathBuf};
use std::time::Instant;

use image::RgbaImage;
use libaom_sys as aom;

use super::clip_folder::{IndexLine, IvfPicture, part_path, read_ivf, still_path};

/// Decoder threads: one per processor thread, as the encoder has, at most
/// libaom's limit for one context. A read-back late in a part decodes up to
/// 200 pictures, so decode speed sets the read-back time.
const MAX_DECODER_THREADS: usize = 64;
/// The `code` of the debug lines that time one picture read-back (stages
/// `read_part`, `decode`, `convert`; the viewer adds `png`).
pub(super) const VIEW_TIMING_CODE: &str = "CLIP_VIEW_TIMING";
/// libaom may return 8-bit pictures for 8-bit streams (`allow_lowbitdepth`).
const ALLOW_LOW_BIT_DEPTH: c_uint = 1;
/// No `AOM_CODEC_USE_*` flags.
const NO_DECODER_FLAGS: aom::aom_codec_flags_t = 0;

const OPAQUE: u8 = u8::MAX;
const RGB_BYTES: usize = 3;

/// 8-bit fixed point for the YUV to RGB conversion.
const FIXED_SHIFT: i32 = 8;
const FIXED_ROUND: i32 = 1 << (FIXED_SHIFT - 1);
const CHROMA_OFFSET: i32 = 128;
/// The chroma sample of a grey pixel.
const NEUTRAL_CHROMA: u8 = 128;

/// What a returned picture is (plan section 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PictureKind {
    /// The window's exact pixels: a lossless still of a settle point.
    ExactStill,
    /// A decoded AV1 picture: video quality.
    Video,
    /// Nothing was recorded for the instant.
    Lost,
}

impl PictureKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::ExactStill => "exact still",
            Self::Video => "video",
            Self::Lost => "lost",
        }
    }
}

/// Where the picture shown at one instant comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PictureSource {
    /// The lossless still that belongs to `instant`.
    Still { instant: u64 },
    /// Picture `index` of video part `part`, encoded at `instant`.
    Video { instant: u64, part: u32, index: u32 },
    /// No picture: `cause` names why.
    Lost { cause: String },
}

impl PictureSource {
    /// The rule of plan section 5 on the index lines of a clip. From instant
    /// k back over unchanged (`same`) instants to the instant whose picture
    /// is on screen: a still there (or on any unchanged instant between)
    /// gives the exact picture, otherwise the encoded video picture is used.
    /// A lost instant, or an unchanged one that repeats a lost picture, is
    /// lost.
    pub(super) fn resolve(folder: &Path, lines: &[IndexLine], instant: u64) -> Result<Self, String> {
        let mut current = instant;
        loop {
            let line = find_line(lines, current)
                .ok_or_else(|| format!("frames.jsonl has no line for instant {current}"))?;
            if let Some(cause) = &line.lost {
                let cause = if current == instant {
                    cause.clone()
                } else {
                    format!("unchanged since instant {current}, which was lost ({cause})")
                };
                return Ok(Self::Lost { cause });
            }
            if line.still == Some(true) && still_path(folder, current).is_file() {
                return Ok(Self::Still { instant: current });
            }
            if let Some((part, index)) = line.enc {
                return Ok(Self::Video {
                    instant: current,
                    part,
                    index,
                });
            }
            if line.same != Some(true) {
                return Err(format!(
                    "frames.jsonl line of instant {current} has none of enc, same or lost"
                ));
            }
            current = current.checked_sub(1).ok_or_else(|| {
                "frames.jsonl marks instant 0 as same, but nothing comes before it".to_owned()
            })?;
        }
    }

    pub(super) const fn kind(&self) -> PictureKind {
        match self {
            Self::Still { .. } => PictureKind::ExactStill,
            Self::Video { .. } => PictureKind::Video,
            Self::Lost { .. } => PictureKind::Lost,
        }
    }

    /// The instant whose recording is shown; `None` when lost.
    pub(super) const fn shown_instant(&self) -> Option<u64> {
        match self {
            Self::Still { instant } | Self::Video { instant, .. } => Some(*instant),
            Self::Lost { .. } => None,
        }
    }
}

/// The index line of `instant` (lines are in instant order).
pub(super) fn find_line(lines: &[IndexLine], instant: u64) -> Option<&IndexLine> {
    lines
        .binary_search_by_key(&instant, |line| line.instant)
        .ok()
        .and_then(|position| lines.get(position))
}

/// The picture at one instant.
pub(super) struct ClipPicture {
    pub(super) instant: u64,
    pub(super) t_ms: u64,
    pub(super) source: PictureSource,
    /// `None` exactly when the source is lost.
    pub(super) image: Option<RgbaImage>,
}

impl ClipPicture {
    pub(super) const fn kind(&self) -> PictureKind {
        self.source.kind()
    }
}

/// Reads pictures of one clip folder. Keeps the video part last decoded, so
/// pictures asked for in time order decode each picture once.
pub(super) struct ClipDecoder {
    folder: PathBuf,
    lines: Vec<IndexLine>,
    video: Option<PartReader>,
}

impl ClipDecoder {
    pub(super) fn new(folder: &Path, lines: Vec<IndexLine>) -> Self {
        Self {
            folder: folder.to_path_buf(),
            lines,
            video: None,
        }
    }

    /// Where the picture of `instant` comes from, without decoding.
    pub(super) fn source(&self, instant: u64) -> Result<PictureSource, String> {
        PictureSource::resolve(&self.folder, &self.lines, instant)
    }

    /// The picture shown at `instant`.
    pub(super) fn picture_at(&mut self, instant: u64) -> Result<ClipPicture, String> {
        let t_ms = find_line(&self.lines, instant)
            .map(|line| line.t_ms)
            .ok_or_else(|| format!("instant {instant} is outside the clip"))?;
        let source = self.source(instant)?;
        let image = match &source {
            PictureSource::Still { instant } => Some(decode_still(&still_path(&self.folder, *instant))?),
            PictureSource::Video { part, index, .. } => Some(self.video_picture(*part, *index)?),
            PictureSource::Lost { .. } => None,
        };
        Ok(ClipPicture {
            instant,
            t_ms,
            source,
            image,
        })
    }

    /// Picture `index` of part `part`: read on from the part in use when it
    /// is not past `index`, else decode the part from its key picture.
    fn video_picture(&mut self, part: u32, index: u32) -> Result<RgbaImage, String> {
        let reusable = self
            .video
            .as_ref()
            .is_some_and(|reader| reader.part == part && reader.can_reach(index));
        if !reusable {
            self.video = Some(PartReader::open(&self.folder, part)?);
        }
        let reader = self
            .video
            .as_mut()
            .ok_or_else(|| format!("video part {part} could not be opened"))?;
        reader.picture(index)
    }
}

/// One video part being decoded in order.
struct PartReader {
    part: u32,
    path: PathBuf,
    /// The part's complete pictures (a torn last one is left out).
    pictures: Vec<IvfPicture>,
    decoder: Av1Decoder,
    /// The next picture the decoder has not seen yet.
    next: u32,
    /// The last picture converted, with its index.
    last: Option<(u32, RgbaImage)>,
}

impl PartReader {
    fn open(folder: &Path, part: u32) -> Result<Self, String> {
        let path = part_path(folder, part);
        let started = Instant::now();
        let pictures = read_ivf(&path)?;
        tracing::debug!(
            code = VIEW_TIMING_CODE,
            stage = "read_part",
            part,
            pictures = pictures.len(),
            ms = elapsed_ms(started),
            "clip picture read-back: IVF part read"
        );
        Ok(Self {
            part,
            pictures,
            path,
            decoder: Av1Decoder::new()?,
            next: 0,
            last: None,
        })
    }

    fn can_reach(&self, index: u32) -> bool {
        index >= self.next || self.last.as_ref().is_some_and(|(last, _)| *last == index)
    }

    fn picture(&mut self, index: u32) -> Result<RgbaImage, String> {
        if let Some((last, image)) = &self.last
            && *last == index
        {
            return Ok(image.clone());
        }
        let started = Instant::now();
        let first = self.next;
        while self.next <= index {
            let current = self.next;
            let data = usize::try_from(current)
                .ok()
                .and_then(|position| self.pictures.get(position))
                .map(|picture| picture.data.as_slice())
                .ok_or_else(|| {
                    format!(
                        "{} holds {} complete pictures; picture {current} is missing",
                        self.path.display(),
                        self.pictures.len()
                    )
                })?;
            self.decoder.decode(data)?;
            self.next += 1;
        }
        if self.next == first {
            return Err(format!("{} gave no picture {index}", self.path.display()));
        }
        tracing::debug!(
            code = VIEW_TIMING_CODE,
            stage = "decode",
            part = self.part,
            index,
            pictures = self.next - first,
            ms = elapsed_ms(started),
            "clip picture read-back: AV1 pictures decoded up to the one asked for"
        );
        let started = Instant::now();
        let image = self.decoder.rgba()?;
        tracing::debug!(
            code = VIEW_TIMING_CODE,
            stage = "convert",
            part = self.part,
            index,
            ms = elapsed_ms(started),
            "clip picture read-back: 4:2:0 to RGBA"
        );
        self.last = Some((index, image.clone()));
        Ok(image)
    }
}

/// Decodes a lossless WebP still to RGBA.
fn decode_still(path: &Path) -> Result<RgbaImage, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let mut decoder = image_webp::WebPDecoder::new(std::io::BufReader::new(file))
        .map_err(|error| format!("{} is not a readable WebP: {error}", path.display()))?;
    let (width, height) = decoder.dimensions();
    let size = decoder
        .output_buffer_size()
        .ok_or_else(|| format!("{} is too large to decode", path.display()))?;
    let mut buffer = vec![0_u8; size];
    decoder
        .read_image(&mut buffer)
        .map_err(|error| format!("could not decode {}: {error}", path.display()))?;
    let rgba = if decoder.has_alpha() {
        for pixel in buffer.chunks_exact_mut(RGB_BYTES + 1) {
            pixel[RGB_BYTES] = OPAQUE;
        }
        buffer
    } else {
        buffer
            .chunks_exact(RGB_BYTES)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], OPAQUE])
            .collect()
    };
    RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| format!("{} decoded to a buffer of the wrong size", path.display()))
}

/// Milliseconds since `started`, for the `VIEW_TIMING_CODE` lines.
pub(super) fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// One libaom AV1 decoder context.
struct Av1Decoder {
    context: Box<aom::aom_codec_ctx_t>,
    /// The picture the last `decode` produced; libaom keeps it valid until
    /// the next `decode`.
    frame: *const aom::aom_image_t,
}

impl Av1Decoder {
    fn new() -> Result<Self, String> {
        let threads = std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(MAX_DECODER_THREADS);
        let config = aom::aom_codec_dec_cfg_t {
            threads: c_uint::try_from(threads).unwrap_or(1),
            w: 0,
            h: 0,
            allow_lowbitdepth: ALLOW_LOW_BIT_DEPTH,
        };
        // SAFETY: a plain C struct of pointers and integers; all-zero is its
        // unset state, and `aom_codec_dec_init_ver` initialises it.
        let mut context: Box<aom::aom_codec_ctx_t> = Box::new(unsafe { std::mem::zeroed() });
        // SAFETY: the context and configuration live through the call; the
        // ABI version is the one these bindings were generated for.
        let code = unsafe {
            aom::aom_codec_dec_init_ver(
                &raw mut *context,
                aom::aom_codec_av1_dx(),
                &raw const config,
                NO_DECODER_FLAGS,
                aom::AOM_DECODER_ABI_VERSION as c_int,
            )
        };
        if code != aom::AOM_CODEC_OK {
            return Err(format!(
                "aom_codec_dec_init_ver failed: libaom error {code}"
            ));
        }
        Ok(Self {
            context,
            frame: std::ptr::null(),
        })
    }

    /// Decodes one compressed picture; `rgba` converts the picture it produced.
    fn decode(&mut self, data: &[u8]) -> Result<(), String> {
        self.frame = std::ptr::null();
        // SAFETY: `data` is readable for `data.len()` bytes through the call.
        let code = unsafe {
            aom::aom_codec_decode(
                &raw mut *self.context,
                data.as_ptr(),
                data.len(),
                std::ptr::null_mut(),
            )
        };
        self.check(code, "aom_codec_decode")?;
        let mut iterator: aom::aom_codec_iter_t = std::ptr::null();
        loop {
            // SAFETY: the iterator starts null and is only advanced by libaom.
            let image = unsafe { aom::aom_codec_get_frame(&raw mut *self.context, &raw mut iterator) };
            if image.is_null() {
                break;
            }
            self.frame = image.cast_const();
        }
        Ok(())
    }

    /// The picture the last `decode` produced, as RGBA.
    fn rgba(&self) -> Result<RgbaImage, String> {
        if self.frame.is_null() {
            return Err("the AV1 decoder returned no picture".to_owned());
        }
        // SAFETY: set by the last `decode`; libaom keeps the image valid until
        // the next decode call, and none happened since.
        i420_to_rgba(unsafe { &*self.frame })
    }

    fn check(&self, code: aom::aom_codec_err_t, step: &str) -> Result<(), String> {
        if code == aom::AOM_CODEC_OK {
            return Ok(());
        }
        // SAFETY: libaom returns a static or context-owned NUL-terminated string.
        let text = unsafe { aom::aom_codec_error(&raw const *self.context) };
        let text = if text.is_null() {
            String::new()
        } else {
            // SAFETY: checked non-null above.
            unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
        };
        Err(format!("{step} failed: libaom error {code} {text}"))
    }
}

impl Drop for Av1Decoder {
    fn drop(&mut self) {
        // SAFETY: the context was initialised by `aom_codec_dec_init_ver`.
        unsafe {
            aom::aom_codec_destroy(&raw mut *self.context);
        }
    }
}

/// YUV to RGB coefficients in `FIXED_SHIFT` fixed point.
struct YuvMatrix {
    luma: i32,
    luma_offset: i32,
    red_v: i32,
    green_u: i32,
    green_v: i32,
    blue_u: i32,
}

/// BT.601 limited range: what the recorder's RGB to 4:2:0 conversion writes
/// when the stream names no matrix (libaom's default is "unspecified").
const BT601_LIMITED: YuvMatrix = YuvMatrix {
    luma: 298,
    luma_offset: 16,
    red_v: 409,
    green_u: -100,
    green_v: -208,
    blue_u: 516,
};
const BT709_LIMITED: YuvMatrix = YuvMatrix {
    luma: 298,
    luma_offset: 16,
    red_v: 459,
    green_u: -55,
    green_v: -136,
    blue_u: 541,
};
const BT601_FULL: YuvMatrix = YuvMatrix {
    luma: 256,
    luma_offset: 0,
    red_v: 359,
    green_u: -88,
    green_v: -183,
    blue_u: 454,
};
const BT709_FULL: YuvMatrix = YuvMatrix {
    luma: 256,
    luma_offset: 0,
    red_v: 403,
    green_u: -48,
    green_v: -120,
    blue_u: 475,
};

impl YuvMatrix {
    /// The matrix the stream signals: BT.709 when named, else BT.601.
    fn of(image: &aom::aom_image_t) -> &'static Self {
        let full = image.range == aom::AOM_CR_FULL_RANGE;
        match (image.mc == aom::AOM_CICP_MC_BT_709, full) {
            (true, true) => &BT709_FULL,
            (true, false) => &BT709_LIMITED,
            (false, true) => &BT601_FULL,
            (false, false) => &BT601_LIMITED,
        }
    }

    fn rgba(&self, y: u8, u: u8, v: u8) -> [u8; 4] {
        let luma = self.luma * (i32::from(y) - self.luma_offset) + FIXED_ROUND;
        let (u, v) = (i32::from(u) - CHROMA_OFFSET, i32::from(v) - CHROMA_OFFSET);
        let channel = |value: i32| u8::try_from((value >> FIXED_SHIFT).clamp(0, 255)).unwrap_or(OPAQUE);
        [
            channel(luma + self.red_v * v),
            channel(luma + self.green_u * u + self.green_v * v),
            channel(luma + self.blue_u * u),
            OPAQUE,
        ]
    }
}

/// One plane of a decoded picture: its rows and stride.
struct Plane<'a> {
    bytes: &'a [u8],
    stride: usize,
}

impl<'a> Plane<'a> {
    /// Plane `plane` of `image`, `width` x `rows` samples.
    fn of(image: &'a aom::aom_image_t, plane: u32, width: usize, rows: usize) -> Result<Self, String> {
        let index = plane as usize;
        let stride = image
            .stride
            .get(index)
            .and_then(|stride| usize::try_from(*stride).ok())
            .filter(|stride| *stride >= width)
            .ok_or_else(|| format!("decoded plane {plane} has an invalid stride"))?;
        let pointer = image
            .planes
            .get(index)
            .copied()
            .filter(|pointer| !pointer.is_null())
            .ok_or_else(|| format!("decoded plane {plane} has no data"))?;
        let length = stride * rows.saturating_sub(1) + width;
        // SAFETY: libaom allocates `stride` bytes for each of the plane's
        // rows; the last row is read only up to `width`.
        let bytes = unsafe { std::slice::from_raw_parts(pointer.cast_const(), length) };
        Ok(Self { bytes, stride })
    }

    fn row(&self, row: usize, width: usize) -> &'a [u8] {
        let start = row * self.stride;
        self.bytes.get(start..start + width).unwrap_or_default()
    }
}

/// Converts an 8-bit 4:2:0 picture to RGBA (chroma by nearest sample).
fn i420_to_rgba(image: &aom::aom_image_t) -> Result<RgbaImage, String> {
    if image.fmt != aom::AOM_IMG_FMT_I420 {
        return Err(format!(
            "decoded picture format {} is not 8-bit 4:2:0",
            image.fmt
        ));
    }
    let (width, height) = (image.d_w as usize, image.d_h as usize);
    let (x_shift, y_shift) = (image.x_chroma_shift, image.y_chroma_shift);
    let chroma_width = (width + (1 << x_shift) - 1) >> x_shift;
    let chroma_rows = (height + (1 << y_shift) - 1) >> y_shift;
    let luma = Plane::of(image, aom::AOM_PLANE_Y, width, height)?;
    let blue = Plane::of(image, aom::AOM_PLANE_U, chroma_width, chroma_rows)?;
    let red = Plane::of(image, aom::AOM_PLANE_V, chroma_width, chroma_rows)?;
    let matrix = YuvMatrix::of(image);
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let luma_row = luma.row(y, width);
        let (u_row, v_row) = (
            blue.row(y >> y_shift, chroma_width),
            red.row(y >> y_shift, chroma_width),
        );
        for (x, &sample) in luma_row.iter().enumerate() {
            let chroma = x >> x_shift;
            let u = u_row.get(chroma).copied().unwrap_or(NEUTRAL_CHROMA);
            let v = v_row.get(chroma).copied().unwrap_or(NEUTRAL_CHROMA);
            rgba.extend_from_slice(&matrix.rgba(sample, u, v));
        }
    }
    RgbaImage::from_raw(image.d_w, image.d_h, rgba)
        .ok_or_else(|| "a decoded picture converted to a buffer of the wrong size".to_owned())
}
