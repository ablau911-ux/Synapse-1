//! Clip decoder (docs/clip-recorder-plan-5.0.md, sections 3 and 5): the
//! picture shown at one instant of a clip folder.
//!
//! - `PictureSource` owns the rule of which recording shows instant k: the
//!   still of the picture on screen when one exists (exact), else the encoded
//!   video picture it repeats, else nothing (lost).
//! - `ClipDecoder` turns that source into pixels: a lossless WebP still, or
//!   the AV1 pictures of one video part decoded by dav1d (the `rav1d` crate,
//!   its Rust port) from the part's first (key) picture up to the one asked
//!   for. Decoding never reads more than one part.
//!
//! Every picture comes back as top-down RGBA with opaque alpha.

use std::ffi::{c_int, c_void};
use std::mem::MaybeUninit;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use image::RgbaImage;
use rav1d::include::dav1d::data::Dav1dData;
use rav1d::include::dav1d::dav1d::{Dav1dContext, Dav1dSettings};
use rav1d::include::dav1d::headers::{DAV1D_MC_BT709, DAV1D_PIXEL_LAYOUT_I420};
use rav1d::include::dav1d::picture::Dav1dPicture;
use rav1d::src::lib::{
    dav1d_close, dav1d_data_create, dav1d_data_unref, dav1d_default_settings, dav1d_get_picture,
    dav1d_open, dav1d_picture_unref, dav1d_send_data,
};

use super::clip_folder::{IndexLine, IvfPicture, part_path, read_ivf, still_path};

/// Decoder threads: one per processor thread, as the encoder has, at most
/// this many. A read-back late in a part decodes up to 200 pictures, so
/// decode speed sets the read-back time.
const MAX_DECODER_THREADS: usize = 64;
/// The `code` of the debug lines that time one picture read-back (stages
/// `read_part`, `decode`, `convert`; the viewer adds `png`).
pub(super) const VIEW_TIMING_CODE: &str = "CLIP_VIEW_TIMING";
/// dav1d's `max_frame_delay`: 0 lets dav1d choose, which turns on frame
/// threading (several pictures decoded at once); pictures then come out later
/// than their units go in, and `Av1Decoder` matches them by count.
const AUTOMATIC_FRAME_DELAY: c_int = 0;
/// dav1d's results: 0, or a negated `errno`; `DAV1D_ERR(EAGAIN)` means "take a
/// picture out first" (sending) or "no picture ready" (getting).
const DAV1D_OK: c_int = 0;
const DAV1D_AGAIN: c_int = -libc::EAGAIN;
/// The `bpc` of an 8-bit picture.
const EIGHT_BIT: c_int = 8;

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
    /// The lossless still of the picture on screen at `instant`, stored in
    /// the still file of instant `file` (`instant` itself, or the earlier
    /// instant its `still_of` names).
    Still { instant: u64, file: u64 },
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
    /// lost, unless its line keeps a still whose file is on disk (recovery
    /// found the video picture missing; the still shows the exact picture).
    pub(super) fn resolve(
        folder: &Path,
        lines: &[IndexLine],
        instant: u64,
    ) -> Result<Self, String> {
        let mut current = instant;
        loop {
            let line = find_line(lines, current)
                .ok_or_else(|| format!("frames.jsonl has no line for instant {current}"))?;
            if let Some(file) = line.still_file()
                && still_path(folder, file).is_file()
            {
                return Ok(Self::Still {
                    instant: current,
                    file,
                });
            }
            if let Some(cause) = &line.lost {
                let cause = if current == instant {
                    cause.clone()
                } else {
                    format!("unchanged since instant {current}, which was lost ({cause})")
                };
                return Ok(Self::Lost { cause });
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
            Self::Still { instant, .. } | Self::Video { instant, .. } => Some(*instant),
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

/// What the last `ClipDecoder::picture_at` spent, by stage (the
/// `VIEW_TIMING_CODE` debug lines tell the same): reading the IVF part when it
/// was opened for this picture, decoding up to the picture (for a still: its
/// WebP decode), and 4:2:0 to RGBA.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PictureTimes {
    #[allow(
        dead_code,
        reason = "read by the clip-probe bench (exp-av1-fetch); the read_part debug line logs it at open"
    )]
    pub(super) read_part: Duration,
    pub(super) decode: Duration,
    pub(super) convert: Duration,
    /// AV1 pictures decoded for this one (0 for a still, or for the picture
    /// decoded last time).
    pub(super) pictures_decoded: u32,
}

/// Reads pictures of one clip folder. Keeps the video part last decoded, so
/// pictures asked for in time order decode each picture once.
pub(super) struct ClipDecoder {
    folder: PathBuf,
    lines: Vec<IndexLine>,
    video: Option<PartReader>,
    times: PictureTimes,
}

impl ClipDecoder {
    pub(super) fn new(folder: &Path, lines: Vec<IndexLine>) -> Self {
        Self {
            folder: folder.to_path_buf(),
            lines,
            video: None,
            times: PictureTimes::default(),
        }
    }

    /// What the last `picture_at` spent, by stage.
    #[allow(
        dead_code,
        reason = "read by the clip-probe bench (exp-av1-fetch), which includes this module"
    )]
    pub(super) const fn times(&self) -> PictureTimes {
        self.times
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
        self.times = PictureTimes::default();
        let image = match &source {
            PictureSource::Still { file, .. } => {
                let started = Instant::now();
                let image = decode_still(&still_path(&self.folder, *file))?;
                self.times.decode = started.elapsed();
                Some(image)
            }
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
        let mut read_part = Duration::ZERO;
        if !reusable {
            let reader = PartReader::open(&self.folder, part)?;
            read_part = reader.read_time;
            self.video = Some(reader);
        }
        let reader = self
            .video
            .as_mut()
            .ok_or_else(|| format!("video part {part} could not be opened"))?;
        let image = reader.picture(index)?;
        self.times = PictureTimes {
            read_part,
            ..reader.times
        };
        Ok(image)
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
    /// Time `open` spent reading the part.
    read_time: Duration,
    /// What the last `picture` call spent.
    times: PictureTimes,
}

impl PartReader {
    fn open(folder: &Path, part: u32) -> Result<Self, String> {
        let path = part_path(folder, part);
        let started = Instant::now();
        let pictures = read_ivf(&path)?;
        let read_time = started.elapsed();
        tracing::debug!(
            code = VIEW_TIMING_CODE,
            stage = "read_part",
            part,
            pictures = pictures.len(),
            ms = millis(read_time),
            "clip picture read-back: IVF part read"
        );
        Ok(Self {
            part,
            pictures,
            path,
            decoder: Av1Decoder::new()?,
            next: 0,
            last: None,
            read_time,
            times: PictureTimes::default(),
        })
    }

    fn can_reach(&self, index: u32) -> bool {
        index >= self.next || self.last.as_ref().is_some_and(|(last, _)| *last == index)
    }

    fn picture(&mut self, index: u32) -> Result<RgbaImage, String> {
        if let Some((last, image)) = &self.last
            && *last == index
        {
            self.times = PictureTimes::default();
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
        self.decoder.finish_last()?;
        let decode = started.elapsed();
        let started = Instant::now();
        let image = self.decoder.rgba()?;
        self.times = PictureTimes {
            read_part: Duration::ZERO,
            decode,
            convert: started.elapsed(),
            pictures_decoded: self.next - first,
        };
        tracing::debug!(
            code = VIEW_TIMING_CODE,
            stage = "decode",
            part = self.part,
            index,
            pictures = self.times.pictures_decoded,
            ms = millis(self.times.decode),
            "clip picture read-back: AV1 pictures decoded up to the one asked for"
        );
        tracing::debug!(
            code = VIEW_TIMING_CODE,
            stage = "convert",
            part = self.part,
            index,
            ms = millis(self.times.convert),
            "clip picture read-back: 4:2:0 to RGBA"
        );
        self.last = Some((index, image.clone()));
        Ok(image)
    }
}

/// Decodes a lossless WebP still to RGBA.
pub(super) fn decode_still(path: &Path) -> Result<RgbaImage, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    decode_still_bytes(&bytes, path)
}

/// Decodes the bytes of a lossless WebP still (`path` names it in errors) to
/// RGBA.
pub(super) fn decode_still_bytes(bytes: &[u8], path: &Path) -> Result<RgbaImage, String> {
    let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes))
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
    millis(started.elapsed())
}

fn millis(time: Duration) -> u64 {
    u64::try_from(time.as_millis()).unwrap_or(u64::MAX)
}

/// One dav1d decoder (rav1d) with frame threading. Pictures come out later
/// than their units go in, so they are matched by count: the recorder's
/// stream shows every picture, in order, so the n-th picture dav1d returns
/// is picture n − 1 of the part.
struct Av1Decoder {
    context: Option<Dav1dContext>,
    /// Units sent so far.
    sent: u32,
    /// Pictures taken out so far.
    taken: u32,
    /// The last picture taken, with its number in the part; one reference
    /// held until a later picture replaces it or the decoder drops.
    held: Option<(u32, Dav1dPicture)>,
}

impl Av1Decoder {
    fn new() -> Result<Self, String> {
        let threads = std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(MAX_DECODER_THREADS);
        let mut settings = MaybeUninit::<Dav1dSettings>::uninit();
        // SAFETY: `dav1d_default_settings` writes every field of the settings.
        let mut settings = unsafe {
            dav1d_default_settings(NonNull::from(&mut settings).cast());
            settings.assume_init()
        };
        settings.n_threads = c_int::try_from(threads).unwrap_or(1);
        settings.max_frame_delay = AUTOMATIC_FRAME_DELAY;
        let mut context: Option<Dav1dContext> = None;
        // SAFETY: both pointers are valid through the call; dav1d reads the
        // settings and writes the context.
        let opened = unsafe {
            dav1d_open(
                Some(NonNull::from(&mut context)),
                Some(NonNull::from(&mut settings)),
            )
        };
        check(opened.0, "dav1d_open")?;
        Ok(Self {
            context,
            sent: 0,
            taken: 0,
            held: None,
        })
    }

    /// Sends one compressed picture (one IVF picture is one temporal unit)
    /// and takes out a picture if one is ready, without waiting for frames
    /// still being decoded; `rgba` waits for the picture of the last unit.
    fn decode(&mut self, data: &[u8]) -> Result<(), String> {
        let mut buffer = Dav1dData::default();
        // SAFETY: `buffer` is writable; dav1d allocates `data.len()` bytes it owns.
        let target = unsafe { dav1d_data_create(Some(NonNull::from(&mut buffer)), data.len()) };
        if target.is_null() {
            return Err("dav1d_data_create failed".to_owned());
        }
        // SAFETY: dav1d allocated `data.len()` writable bytes at `target`.
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), target, data.len()) };
        let sent = self.send(&mut buffer);
        // SAFETY: `buffer` came from `dav1d_data_create`; an emptied buffer unrefs to nothing.
        unsafe { dav1d_data_unref(Some(NonNull::from(&mut buffer))) };
        sent?;
        self.sent += 1;
        // The first get after a send never waits (dav1d clears its drain flag on send).
        self.take()?;
        Ok(())
    }

    /// Hands the whole unit to dav1d. "Again" means its input is full: a
    /// picture is taken out (the first get only takes a ready one, the second
    /// waits for one), then the unit is sent again.
    fn send(&mut self, buffer: &mut Dav1dData) -> Result<(), String> {
        while buffer.sz > 0 {
            // SAFETY: an open context and a buffer from `dav1d_data_create`.
            let sent = unsafe { dav1d_send_data(self.context, Some(NonNull::from(&mut *buffer))) };
            match sent.0 {
                DAV1D_OK => {}
                DAV1D_AGAIN => {
                    if !self.take()? && !self.take()? {
                        return Err(
                            "dav1d refused data and gave no picture to make room".to_owned()
                        );
                    }
                }
                code => return check(code, "dav1d_send_data"),
            }
        }
        Ok(())
    }

    /// Takes the next picture out and holds it, numbered by count; `false`
    /// when dav1d has none (a get after the first since a send drains, so
    /// "again" there means no picture is left).
    fn take(&mut self) -> Result<bool, String> {
        let mut picture = Dav1dPicture::default();
        // SAFETY: an open context and a writable picture.
        let got = unsafe { dav1d_get_picture(self.context, Some(NonNull::from(&mut picture))) };
        match got.0 {
            DAV1D_OK => {
                self.release();
                self.held = Some((self.taken, picture));
                self.taken += 1;
                Ok(true)
            }
            DAV1D_AGAIN => Ok(false),
            code => check(code, "dav1d_get_picture").map(|()| false),
        }
    }

    fn release(&mut self) {
        if let Some((_, picture)) = self.held.take() {
            unref(picture);
        }
    }

    /// Drains dav1d until the picture of the last unit sent is out (part of
    /// decoding: it waits for the frames still being decoded).
    fn finish_last(&mut self) -> Result<(), String> {
        let wanted = self
            .sent
            .checked_sub(1)
            .ok_or_else(|| "the AV1 decoder was sent no picture".to_owned())?;
        while self
            .held
            .as_ref()
            .is_none_or(|(number, _)| *number < wanted)
        {
            if !self.take()? {
                return Err(format!(
                    "dav1d gave {} pictures for {} units; picture {wanted} never came out",
                    self.taken, self.sent
                ));
            }
        }
        Ok(())
    }

    /// The picture of the last unit sent, as RGBA.
    fn rgba(&mut self) -> Result<RgbaImage, String> {
        self.finish_last()?;
        let wanted = self.sent - 1;
        let Some((number, picture)) = self.held.as_ref() else {
            return Err("the AV1 decoder returned no picture".to_owned());
        };
        if *number != wanted {
            return Err(format!(
                "dav1d's picture {number} came out where picture {wanted} was due"
            ));
        }
        if picture.p.layout != DAV1D_PIXEL_LAYOUT_I420 || picture.p.bpc != EIGHT_BIT {
            return Err(format!(
                "decoded picture layout {} at {} bits is not 8-bit 4:2:0",
                picture.p.layout, picture.p.bpc
            ));
        }
        let size = |value: c_int| {
            usize::try_from(value).map_err(|_| "decoded picture has a negative size".to_owned())
        };
        let (width, height) = (size(picture.p.w)?, size(picture.p.h)?);
        let (chroma_width, chroma_rows) = (width.div_ceil(2), height.div_ceil(2));
        // SAFETY: dav1d sets the sequence header of every picture it outputs;
        // it stays valid while the picture is held.
        let header = picture.seq_hdr.map(|header| unsafe { header.as_ref() });
        let matrix = YuvMatrix::of(
            header.is_some_and(|header| header.mtrx == DAV1D_MC_BT709),
            header.is_some_and(|header| header.color_range != 0),
        );
        // The planes stay valid while `self.held` holds its reference, which
        // outlives this call.
        let luma = Plane::of(picture.data[0], picture.stride[0], width, height)?;
        let blue = Plane::of(
            picture.data[1],
            picture.stride[1],
            chroma_width,
            chroma_rows,
        )?;
        let red = Plane::of(
            picture.data[2],
            picture.stride[1],
            chroma_width,
            chroma_rows,
        )?;
        i420_to_rgba(width, height, [&luma, &blue, &red], matrix)
    }
}

impl Drop for Av1Decoder {
    fn drop(&mut self) {
        self.release();
        // SAFETY: the context came from `dav1d_open` and is closed once, here.
        unsafe { dav1d_close(Some(NonNull::from(&mut self.context))) };
    }
}

/// Releases dav1d's reference to a picture.
fn unref(mut picture: Dav1dPicture) {
    // SAFETY: a picture from `dav1d_get_picture`, released once.
    unsafe { dav1d_picture_unref(Some(NonNull::from(&mut picture))) };
}

/// A dav1d result as `Result`: 0 is success, anything else names the step.
fn check(code: c_int, step: &str) -> Result<(), String> {
    if code == DAV1D_OK {
        Ok(())
    } else {
        Err(format!("{step} failed: dav1d error {code}"))
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
    /// The matrix the stream signals: BT.709 when named, else BT.601; full
    /// or limited range.
    const fn of(bt709: bool, full: bool) -> &'static Self {
        match (bt709, full) {
            (true, true) => &BT709_FULL,
            (true, false) => &BT709_LIMITED,
            (false, true) => &BT601_FULL,
            (false, false) => &BT601_LIMITED,
        }
    }

    fn rgba(&self, y: u8, u: u8, v: u8) -> [u8; 4] {
        let luma = self.luma * (i32::from(y) - self.luma_offset) + FIXED_ROUND;
        let (u, v) = (i32::from(u) - CHROMA_OFFSET, i32::from(v) - CHROMA_OFFSET);
        let channel =
            |value: i32| u8::try_from((value >> FIXED_SHIFT).clamp(0, 255)).unwrap_or(OPAQUE);
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
    /// A plane of `width` x `rows` samples at `pointer`, rows `stride` bytes
    /// apart. The caller keeps the picture that owns it alive for `'a`.
    fn of(
        pointer: Option<NonNull<c_void>>,
        stride: isize,
        width: usize,
        rows: usize,
    ) -> Result<Self, String> {
        let stride = usize::try_from(stride)
            .ok()
            .filter(|stride| *stride >= width)
            .ok_or_else(|| "decoded plane has an invalid stride".to_owned())?;
        let pointer = pointer.ok_or_else(|| "decoded plane has no data".to_owned())?;
        let length = stride * rows.saturating_sub(1) + width;
        // SAFETY: dav1d allocates `stride` bytes for each of the plane's rows;
        // the last row is read only up to `width`.
        let bytes = unsafe {
            std::slice::from_raw_parts(pointer.as_ptr().cast::<u8>().cast_const(), length)
        };
        Ok(Self { bytes, stride })
    }

    fn row(&self, row: usize, width: usize) -> &'a [u8] {
        let start = row * self.stride;
        self.bytes.get(start..start + width).unwrap_or_default()
    }
}

/// Converts an 8-bit 4:2:0 picture (Y, U, V planes) to RGBA (chroma by nearest sample).
fn i420_to_rgba(
    width: usize,
    height: usize,
    [luma, blue, red]: [&Plane<'_>; 3],
    matrix: &YuvMatrix,
) -> Result<RgbaImage, String> {
    let (x_shift, y_shift) = (1, 1);
    let chroma_width = width.div_ceil(2);
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
    let size =
        |value: usize| u32::try_from(value).map_err(|_| "decoded picture is too large".to_owned());
    RgbaImage::from_raw(size(width)?, size(height)?, rgba)
        .ok_or_else(|| "a decoded picture converted to a buffer of the wrong size".to_owned())
}
