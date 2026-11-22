//! M11: the candidate formats for one `waiting/` frame file (plan section
//! 0b; research `docs/clip-recorder-waiting-compression-research.md`, part 4).
//!
//!   raw         the BGRA rows as read back (alpha dropped: BGR rows)
//!   lz4         `lz4_flex` block on the whole frame
//!   tiles+lz4   only the tiles that differ from the previous frame, one `lz4_flex` block
//!   tiles+zstd  the same changed tiles, one zstd frame at level -1 or 1
//!
//! Every file starts with a 16-byte header: width, height (u32), tile side
//! (u16), flags (u8: keyframe, alpha kept), coder (u8) and the payload length
//! (u32). Tile files add one bit per tile (changed or not) before the
//! payload. A file is a keyframe (every tile present) every
//! `WAITING_KEYFRAME_FRAMES` frames and after a size change, so the decoder
//! never needs anything older than the last keyframe.
//!
//! Alpha: `Nv12Canvas::draw` reads only B, G, R; with alpha dropped the
//! decoder puts 255 in the alpha byte.

use crate::Result;

/// Plan M11: a full keyframe every 16 frames.
pub const WAITING_KEYFRAME_FRAMES: u32 = 16;
const HEADER_BYTES: usize = 16;
const FLAG_KEYFRAME: u8 = 1;
const FLAG_ALPHA: u8 = 2;
const OPAQUE: u8 = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coder {
    Raw,
    Lz4Whole,
    TilesLz4,
    TilesZstd(i32),
}

impl Coder {
    const fn id(self) -> u8 {
        match self {
            Self::Raw => 0,
            Self::Lz4Whole => 1,
            Self::TilesLz4 => 2,
            Self::TilesZstd(_) => 3,
        }
    }

    pub const fn uses_tiles(self) -> bool {
        matches!(self, Self::TilesLz4 | Self::TilesZstd(_))
    }

    pub fn name(self) -> String {
        match self {
            Self::Raw => "(a) raw".to_owned(),
            Self::Lz4Whole => "(b) lz4 whole".to_owned(),
            Self::TilesLz4 => "(c) tiles+lz4".to_owned(),
            Self::TilesZstd(level) => format!("(d) tiles+zstd {level}"),
        }
    }
}

/// One candidate: coder, tile side (tile coders only) and whether alpha is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Method {
    pub coder: Coder,
    pub tile: u32,
    pub keep_alpha: bool,
}

impl Method {
    /// Every combination plan M11 asks for.
    pub fn all() -> Vec<Self> {
        let mut methods = Vec::new();
        for keep_alpha in [true, false] {
            for coder in [Coder::Raw, Coder::Lz4Whole] {
                methods.push(Self {
                    coder,
                    tile: 0,
                    keep_alpha,
                });
            }
            for coder in [Coder::TilesLz4, Coder::TilesZstd(-1), Coder::TilesZstd(1)] {
                for tile in [32, 64, 128] {
                    methods.push(Self {
                        coder,
                        tile,
                        keep_alpha,
                    });
                }
            }
        }
        methods
    }

    pub fn name(&self) -> String {
        let alpha = if self.keep_alpha {
            "alpha kept"
        } else {
            "alpha dropped"
        };
        if self.coder.uses_tiles() {
            format!("{} tile {} {alpha}", self.coder.name(), self.tile)
        } else {
            format!("{} {alpha}", self.coder.name())
        }
    }
}

/// One captured frame: tightly packed BGRA rows.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl Picture {
    pub fn pixels(&self) -> usize {
        self.width as usize * self.height as usize
    }
}

/// The tile grid of one frame size.
#[derive(Clone, Copy)]
struct Grid {
    width: usize,
    height: usize,
    tile: usize,
    columns: usize,
    rows: usize,
}

impl Grid {
    fn new(width: u32, height: u32, tile: u32) -> Self {
        let (width, height, tile) = (width as usize, height as usize, tile.max(1) as usize);
        Self {
            width,
            height,
            tile,
            columns: width.div_ceil(tile),
            rows: height.div_ceil(tile),
        }
    }

    const fn count(&self) -> usize {
        self.columns * self.rows
    }

    /// Byte range of each pixel row of tile `index` in a BGRA frame.
    fn rows_of(&self, index: usize) -> impl Iterator<Item = std::ops::Range<usize>> + use<> {
        let (column, row) = (index % self.columns, index / self.columns);
        let (x0, y0) = (column * self.tile, row * self.tile);
        let x1 = (x0 + self.tile).min(self.width);
        let y1 = (y0 + self.tile).min(self.height);
        let stride = self.width * 4;
        (y0..y1).map(move |y| y * stride + x0 * 4..y * stride + x1 * 4)
    }

    /// Pixels in tile `index`.
    fn tile_pixels(&self, index: usize) -> usize {
        let (column, row) = (index % self.columns, index / self.columns);
        let w = (self.tile).min(self.width - column * self.tile);
        let h = (self.tile).min(self.height - row * self.tile);
        w * h
    }
}

/// Appends BGRA pixels as BGR or BGRA.
fn push_pixels(target: &mut Vec<u8>, bgra: &[u8], keep_alpha: bool) {
    if keep_alpha {
        target.extend_from_slice(bgra);
        return;
    }
    // Through a small stack buffer, so the target is never zero-filled first.
    const CHUNK_PIXELS: usize = 1024;
    let mut buffer = [0_u8; CHUNK_PIXELS * 3];
    target.reserve(bgra.len() / 4 * 3);
    for chunk in bgra.chunks(CHUNK_PIXELS * 4) {
        let pixels = chunk.len() / 4;
        for (bgr, pixel) in buffer.chunks_exact_mut(3).zip(chunk.chunks_exact(4)) {
            bgr.copy_from_slice(&pixel[..3]);
        }
        target.extend_from_slice(&buffer[..pixels * 3]);
    }
}

/// Writes stored pixels (BGR or BGRA) back as BGRA.
fn put_pixels(target: &mut [u8], stored: &[u8], keep_alpha: bool) {
    if keep_alpha {
        target.copy_from_slice(stored);
        return;
    }
    for (pixel, bgr) in target.chunks_exact_mut(4).zip(stored.chunks_exact(3)) {
        pixel[..3].copy_from_slice(bgr);
        pixel[3] = OPAQUE;
    }
}

/// One `lz4_flex` block into `packed`, which only ever grows (no zero-fill
/// per frame); returns the compressed length.
fn lz4_into(input: &[u8], packed: &mut Vec<u8>) -> Result<usize> {
    let bound = lz4_flex::block::get_maximum_output_size(input.len());
    if packed.len() < bound {
        packed.resize(bound, 0);
    }
    lz4_flex::block::compress_into(input, packed).map_err(|error| format!("lz4 compress: {error}"))
}

/// The compressing side. The caller hands in the previous source frame (the
/// live part would keep two read-back buffers and swap them, so no copy).
pub struct Encoder {
    method: Method,
    since_keyframe: u32,
    staging: Vec<u8>,
    packed: Vec<u8>,
    zstd: Option<zstd::bulk::Compressor<'static>>,
}

impl Encoder {
    pub fn new(method: Method) -> Result<Self> {
        let zstd = match method.coder {
            Coder::TilesZstd(level) => Some(
                zstd::bulk::Compressor::new(level)
                    .map_err(|error| format!("zstd level {level}: {error}"))?,
            ),
            _ => None,
        };
        Ok(Self {
            method,
            since_keyframe: 0,
            staging: Vec::new(),
            packed: Vec::new(),
            zstd,
        })
    }

    /// Encodes `picture` into `out` (replacing its content); `previous` is the
    /// source frame encoded just before it, if any.
    pub fn encode(
        &mut self,
        picture: &Picture,
        previous: Option<&Picture>,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        let size = (picture.width, picture.height);
        let previous = previous.filter(|p| (p.width, p.height) == size);
        let resized = previous.is_none();
        let keyframe = resized || self.since_keyframe.is_multiple_of(WAITING_KEYFRAME_FRAMES);
        self.since_keyframe = if keyframe { 1 } else { self.since_keyframe + 1 };
        out.clear();
        out.resize(HEADER_BYTES, 0);
        let keep_alpha = self.method.keep_alpha;
        match self.method.coder {
            Coder::Raw => push_pixels(out, &picture.bgra, keep_alpha),
            Coder::Lz4Whole => {
                let input = if keep_alpha {
                    &picture.bgra
                } else {
                    self.staging.clear();
                    push_pixels(&mut self.staging, &picture.bgra, false);
                    &self.staging
                };
                let written = lz4_into(input, &mut self.packed)?;
                out.extend_from_slice(&self.packed[..written]);
            }
            Coder::TilesLz4 | Coder::TilesZstd(_) => {
                self.encode_tiles(picture, previous.filter(|_| !keyframe), out)?;
            }
        }
        let payload = (out.len() - HEADER_BYTES) as u32;
        let flags = (u8::from(keyframe) * FLAG_KEYFRAME) | (u8::from(keep_alpha) * FLAG_ALPHA);
        out[0..4].copy_from_slice(&picture.width.to_le_bytes());
        out[4..8].copy_from_slice(&picture.height.to_le_bytes());
        out[8..10].copy_from_slice(&(self.method.tile as u16).to_le_bytes());
        out[10] = flags;
        out[11] = self.method.coder.id();
        out[12..16].copy_from_slice(&payload.to_le_bytes());
        Ok(())
    }

    /// Keyframe when `previous` is None: every tile is stored.
    fn encode_tiles(
        &mut self,
        picture: &Picture,
        previous: Option<&Picture>,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        let grid = Grid::new(picture.width, picture.height, self.method.tile);
        let bitmap_at = out.len();
        out.resize(bitmap_at + grid.count().div_ceil(8), 0);
        self.staging.clear();
        for index in 0..grid.count() {
            let changed = previous.is_none_or(|previous| {
                grid.rows_of(index)
                    .any(|range| previous.bgra[range.clone()] != picture.bgra[range])
            });
            if changed {
                out[bitmap_at + index / 8] |= 1 << (index % 8);
                for range in grid.rows_of(index) {
                    push_pixels(
                        &mut self.staging,
                        &picture.bgra[range],
                        self.method.keep_alpha,
                    );
                }
            }
        }
        if self.staging.is_empty() {
            return Ok(());
        }
        match self.method.coder {
            Coder::TilesZstd(_) => {
                let Some(zstd) = self.zstd.as_mut() else {
                    return Err("zstd compressor missing".to_owned());
                };
                self.packed.clear();
                self.packed
                    .reserve(zstd::zstd_safe::compress_bound(self.staging.len()));
                zstd.compress_to_buffer(&self.staging, &mut self.packed)
                    .map_err(|error| format!("zstd compress: {error}"))?;
                out.extend_from_slice(&self.packed);
            }
            _ => {
                let written = lz4_into(&self.staging, &mut self.packed)?;
                out.extend_from_slice(&self.packed[..written]);
            }
        }
        Ok(())
    }
}

/// The decompressing side: owns the BGRA canvas that tile files update.
pub struct Decoder {
    canvas: Vec<u8>,
    size: (u32, u32),
    staging: Vec<u8>,
    zstd: zstd::bulk::Decompressor<'static>,
}

impl Decoder {
    pub fn new() -> Result<Self> {
        Ok(Self {
            canvas: Vec::new(),
            size: (0, 0),
            staging: Vec::new(),
            zstd: zstd::bulk::Decompressor::new().map_err(|error| format!("zstd: {error}"))?,
        })
    }

    pub fn canvas(&self) -> &[u8] {
        &self.canvas
    }

    /// Decodes one file into the canvas; returns the frame size.
    pub fn decode(&mut self, file: &[u8]) -> Result<(u32, u32)> {
        let header = file
            .get(..HEADER_BYTES)
            .ok_or("file shorter than its header")?;
        let word = |at: usize| {
            u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
        };
        let size = (word(0), word(4));
        let tile = u32::from(u16::from_le_bytes([header[8], header[9]]));
        let (keyframe, keep_alpha) = (
            header[10] & FLAG_KEYFRAME != 0,
            header[10] & FLAG_ALPHA != 0,
        );
        let coder = header[11];
        let payload = file
            .get(HEADER_BYTES..HEADER_BYTES + word(12) as usize)
            .ok_or("payload shorter than its length")?;
        if size != self.size {
            if !keyframe {
                return Err("size changed without a keyframe".to_owned());
            }
            self.size = size;
            self.canvas = vec![0; size.0 as usize * size.1 as usize * 4];
        }
        let pixel_bytes = if keep_alpha { 4 } else { 3 };
        let pixels = size.0 as usize * size.1 as usize;
        match coder {
            0 => put_pixels(&mut self.canvas, payload, keep_alpha),
            1 => {
                if keep_alpha {
                    lz4_flex::block::decompress_into(payload, &mut self.canvas)
                        .map_err(|error| format!("lz4 decompress: {error}"))?;
                } else {
                    self.staging.resize(pixels * pixel_bytes, 0);
                    lz4_flex::block::decompress_into(payload, &mut self.staging)
                        .map_err(|error| format!("lz4 decompress: {error}"))?;
                    put_pixels(&mut self.canvas, &self.staging, false);
                }
            }
            2 | 3 => self.decode_tiles(payload, size, tile, keep_alpha, coder == 3)?,
            other => return Err(format!("unknown coder {other}")),
        }
        Ok(size)
    }

    fn decode_tiles(
        &mut self,
        payload: &[u8],
        size: (u32, u32),
        tile: u32,
        keep_alpha: bool,
        zstd: bool,
    ) -> Result<()> {
        let grid = Grid::new(size.0, size.1, tile);
        let (bitmap, packed) = payload
            .split_at_checked(grid.count().div_ceil(8))
            .ok_or("tile bitmap missing")?;
        let changed = |index: usize| bitmap[index / 8] & (1 << (index % 8)) != 0;
        let pixel_bytes = if keep_alpha { 4 } else { 3 };
        let stored: usize = (0..grid.count())
            .filter(|&index| changed(index))
            .map(|index| grid.tile_pixels(index) * pixel_bytes)
            .sum();
        if stored == 0 {
            return Ok(());
        }
        if zstd {
            self.staging.clear();
            self.staging.reserve(stored);
            self.zstd
                .decompress_to_buffer(packed, &mut self.staging)
                .map_err(|error| format!("zstd decompress: {error}"))?;
        } else {
            self.staging.resize(stored, 0);
            lz4_flex::block::decompress_into(packed, &mut self.staging)
                .map_err(|error| format!("lz4 decompress: {error}"))?;
        }
        if self.staging.len() != stored {
            return Err(format!(
                "{} bytes decoded, {stored} expected",
                self.staging.len()
            ));
        }
        let mut at = 0;
        for index in (0..grid.count()).filter(|&index| changed(index)) {
            for range in grid.rows_of(index) {
                let length = range.len() / 4 * pixel_bytes;
                put_pixels(
                    &mut self.canvas[range],
                    &self.staging[at..at + length],
                    keep_alpha,
                );
                at += length;
            }
        }
        Ok(())
    }
}

/// True when B, G and R of every pixel match (alpha kept: all four bytes).
pub fn same_picture(decoded: &[u8], source: &[u8], keep_alpha: bool) -> bool {
    if keep_alpha {
        return decoded == source;
    }
    decoded.len() == source.len()
        && decoded
            .chunks_exact(4)
            .zip(source.chunks_exact(4))
            .all(|(a, b)| a[..3] == b[..3])
}
