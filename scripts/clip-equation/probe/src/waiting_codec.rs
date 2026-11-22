//! M11: the candidate formats for one `waiting/` frame file (plan section
//! 0b; research `docs/clip-recorder-waiting-compression-research.md`, part 4;
//! widened by the owner on 2026-11-22 to every candidate):
//!
//!   raw        the BGRA rows as read back (alpha dropped: BGR rows)
//!   whole      one byte coder (`block_coders.rs`) on the whole frame
//!   png        `png` crate, `Compression::Fast` (fdeflate), RGB or RGBA
//!   qoi        `qoi` crate, 3 or 4 channels
//!   xor        the frame XOR the previous source frame, then one byte coder
//!   tiles      only the tiles that differ from the previous frame, one byte coder
//!   adaptive   tiles (32 px) + zstd 1 when fewer than X % of the tiles
//!              changed, otherwise the whole frame raw without alpha
//!
//! Every file starts with a 16-byte header: width, height (u32), tile side
//! (u16), flags (u8: keyframe, alpha kept, tile path), coder (u8) and the
//! payload length (u32). Tile files add one bit per tile before the payload.
//! Files that depend on the previous frame (xor, tiles, adaptive's tile path)
//! are replaced by a self-contained keyframe every `WAITING_KEYFRAME_FRAMES`
//! frames and after a size change. png and qoi store the B, G, R bytes in the
//! R, G, B slots; that is lossless and needs no swap.
//!
//! Alpha: `Nv12Canvas::draw` reads only B, G, R; with alpha dropped the
//! decoder puts 255 in the alpha byte.

use crate::Result;
use crate::block_coders::{Block, BlockCoder};

/// Plan M11: a full keyframe every 16 frames.
pub const WAITING_KEYFRAME_FRAMES: u32 = 16;
/// Adaptive coder's tile side (the best tile size of the first run).
const ADAPTIVE_TILE: u32 = 32;
const HEADER_BYTES: usize = 16;
const FLAG_KEYFRAME: u8 = 1;
const FLAG_ALPHA: u8 = 2;
const FLAG_TILE_PATH: u8 = 4;
const OPAQUE: u8 = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coder {
    Raw,
    Whole(Block),
    Png,
    Qoi,
    Xor(Block),
    Tiles(Block),
    /// Percent of changed tiles below which the tile path is taken.
    Adaptive(u8),
}

impl Coder {
    const fn id(self) -> u8 {
        match self {
            Self::Raw => 0,
            Self::Whole(_) => 1,
            Self::Png => 2,
            Self::Qoi => 3,
            Self::Xor(_) => 4,
            Self::Tiles(_) => 5,
            Self::Adaptive(_) => 6,
        }
    }

    pub const fn uses_tiles(self) -> bool {
        matches!(self, Self::Tiles(_))
    }

    /// The byte coder this coder runs, if any.
    const fn block(self) -> Option<Block> {
        match self {
            Self::Whole(block) | Self::Xor(block) | Self::Tiles(block) => Some(block),
            Self::Adaptive(_) => Some(Block::Zstd(1)),
            Self::Raw | Self::Png | Self::Qoi => None,
        }
    }

    pub fn name(self) -> String {
        match self {
            Self::Raw => "raw".to_owned(),
            Self::Whole(block) => format!("whole {}", block.name()),
            Self::Png => "png fast".to_owned(),
            Self::Qoi => "qoi".to_owned(),
            Self::Xor(block) => format!("xor+{}", block.name()),
            Self::Tiles(block) => format!("tiles+{}", block.name()),
            Self::Adaptive(percent) => format!("adaptive {percent}%"),
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
    /// Every candidate of the widened run.
    pub fn all() -> Vec<Self> {
        let mut coders = vec![
            Coder::Raw,
            Coder::Whole(Block::Lz4),
            Coder::Whole(Block::Lz4Hc(3)),
            Coder::Whole(Block::Lz4Hc(9)),
        ];
        coders.extend([-7, -5, -3, -1, 1, 3, 6].map(|level| Coder::Whole(Block::Zstd(level))));
        coders.extend([
            Coder::Whole(Block::ZstdThreads(1, 2)),
            Coder::Whole(Block::DeflateMiniz(1)),
            Coder::Whole(Block::DeflateZlibRs(1)),
            Coder::Png,
            Coder::Qoi,
            Coder::Whole(Block::Snappy),
            Coder::Xor(Block::Lz4),
            Coder::Xor(Block::Zstd(-1)),
            Coder::Xor(Block::Zstd(1)),
        ]);
        let mut methods = Vec::new();
        for keep_alpha in [true, false] {
            for &coder in &coders {
                methods.push(Self { coder, tile: 0, keep_alpha });
            }
            for block in [Block::Lz4, Block::Zstd(-1), Block::Zstd(1)] {
                for tile in [32, 64, 128] {
                    methods.push(Self { coder: Coder::Tiles(block), tile, keep_alpha });
                }
            }
        }
        for percent in [25, 50, 75] {
            methods.push(Self {
                coder: Coder::Adaptive(percent),
                tile: ADAPTIVE_TILE,
                keep_alpha: false,
            });
        }
        methods
    }

    pub fn name(&self) -> String {
        let alpha = if self.keep_alpha { "alpha kept" } else { "alpha dropped" };
        if self.coder.uses_tiles() {
            format!("{} tile {} {alpha}", self.coder.name(), self.tile)
        } else {
            format!("{} {alpha}", self.coder.name())
        }
    }

    /// True when the coder runs its own threads.
    pub fn threaded(&self) -> bool {
        self.coder.block().is_some_and(Block::threaded)
    }

    const fn pixel_bytes(&self) -> usize {
        if self.keep_alpha { 4 } else { 3 }
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

    fn tile_pixels(&self, index: usize) -> usize {
        let (column, row) = (index % self.columns, index / self.columns);
        let w = self.tile.min(self.width - column * self.tile);
        let h = self.tile.min(self.height - row * self.tile);
        w * h
    }

    fn changed(&self, index: usize, picture: &[u8], previous: &[u8]) -> bool {
        self.rows_of(index).any(|range| previous[range.clone()] != picture[range])
    }
}

/// Fills `rows` with the stored rows of every changed tile (every tile when
/// `previous` is None) and returns the bitmap of changed tiles.
fn gather_changed_tiles(
    grid: &Grid,
    picture: &[u8],
    previous: Option<&[u8]>,
    keep_alpha: bool,
    rows: &mut Vec<u8>,
) -> Vec<u8> {
    let mut bitmap = vec![0_u8; grid.count().div_ceil(8)];
    rows.clear();
    for index in 0..grid.count() {
        if previous.is_none_or(|previous| grid.changed(index, picture, previous)) {
            bitmap[index / 8] |= 1 << (index % 8);
            for range in grid.rows_of(index) {
                push_pixels(rows, &picture[range], keep_alpha);
            }
        }
    }
    bitmap
}

/// Appends BGRA pixels as BGR or BGRA, never zero-filling the target first.
fn push_pixels(target: &mut Vec<u8>, bgra: &[u8], keep_alpha: bool) {
    if keep_alpha {
        target.extend_from_slice(bgra);
        return;
    }
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

/// Appends `current XOR previous` in the stored layout (BGR or BGRA).
fn push_xor(target: &mut Vec<u8>, current: &[u8], previous: &[u8], keep_alpha: bool) {
    if keep_alpha {
        target.extend(current.iter().zip(previous).map(|(a, b)| a ^ b));
        return;
    }
    const CHUNK_PIXELS: usize = 1024;
    let mut buffer = [0_u8; CHUNK_PIXELS * 3];
    target.reserve(current.len() / 4 * 3);
    for (chunk, before) in current.chunks(CHUNK_PIXELS * 4).zip(previous.chunks(CHUNK_PIXELS * 4)) {
        let pixels = chunk.len() / 4;
        for ((bgr, pixel), old) in buffer.chunks_exact_mut(3).zip(chunk.chunks_exact(4)).zip(before.chunks_exact(4)) {
            bgr[0] = pixel[0] ^ old[0];
            bgr[1] = pixel[1] ^ old[1];
            bgr[2] = pixel[2] ^ old[2];
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

/// XORs stored differences (BGR or BGRA) into the BGRA canvas.
fn xor_pixels(target: &mut [u8], stored: &[u8], keep_alpha: bool) {
    if keep_alpha {
        for (a, b) in target.iter_mut().zip(stored) {
            *a ^= b;
        }
        return;
    }
    for (pixel, bgr) in target.chunks_exact_mut(4).zip(stored.chunks_exact(3)) {
        pixel[0] ^= bgr[0];
        pixel[1] ^= bgr[1];
        pixel[2] ^= bgr[2];
    }
}

/// The compressing side. The caller hands in the previous source frame (the
/// live part would keep two read-back buffers and swap them, so no copy).
pub struct Encoder {
    method: Method,
    block: Option<BlockCoder>,
    since_keyframe: u32,
    staging: Vec<u8>,
    rows: Vec<u8>,
    packed: Vec<u8>,
    tile_path_frames: usize,
    full_path_frames: usize,
}

impl Encoder {
    pub fn new(method: Method) -> Result<Self> {
        Ok(Self {
            method,
            block: method.coder.block().map(BlockCoder::new).transpose()?,
            since_keyframe: 0,
            staging: Vec::new(),
            rows: Vec::new(),
            packed: Vec::new(),
            tile_path_frames: 0,
            full_path_frames: 0,
        })
    }

    /// Adaptive coder: frames stored on the tile path and on the raw path.
    pub const fn paths(&self) -> (usize, usize) {
        (self.tile_path_frames, self.full_path_frames)
    }

    /// Encodes `picture` into `out` (replacing its content); `previous` is the
    /// source frame encoded just before it, if any.
    pub fn encode(&mut self, picture: &Picture, previous: Option<&Picture>, out: &mut Vec<u8>) -> Result<()> {
        let size = (picture.width, picture.height);
        let previous = previous.filter(|p| (p.width, p.height) == size).map(|p| p.bgra.as_slice());
        let due = previous.is_none() || self.since_keyframe.is_multiple_of(WAITING_KEYFRAME_FRAMES);
        let keep_alpha = self.method.keep_alpha;
        out.clear();
        out.resize(HEADER_BYTES, 0);
        let mut tile_path = false;
        let keyframe = match self.method.coder {
            Coder::Raw => {
                push_pixels(out, &picture.bgra, keep_alpha);
                true
            }
            Coder::Whole(_) => {
                let repacked = self.repack(picture);
                let input = repacked.as_deref().unwrap_or(&picture.bgra);
                let result = self.block_coder()?.compress(input, out);
                self.restore(repacked);
                result?;
                true
            }
            Coder::Png => {
                let repacked = self.repack(picture);
                let input = repacked.as_deref().unwrap_or(&picture.bgra);
                let result = encode_png(input, size, keep_alpha, out);
                self.restore(repacked);
                result?;
                true
            }
            Coder::Qoi => {
                let repacked = self.repack(picture);
                let input = repacked.as_deref().unwrap_or(&picture.bgra);
                let bound = qoi::encode_max_len(size.0, size.1, self.method.pixel_bytes() as u8);
                if self.packed.len() < bound {
                    self.packed.resize(bound, 0);
                }
                let result = qoi::encode_to_buf(&mut self.packed, input, size.0, size.1)
                    .map_err(|error| format!("qoi encode: {error}"));
                self.restore(repacked);
                out.extend_from_slice(&self.packed[..result?]);
                true
            }
            Coder::Xor(_) => {
                let mut input = std::mem::take(&mut self.staging);
                input.clear();
                match previous.filter(|_| !due) {
                    Some(previous) => push_xor(&mut input, &picture.bgra, previous, keep_alpha),
                    None => push_pixels(&mut input, &picture.bgra, keep_alpha),
                }
                let result = self.block_coder()?.compress(&input, out);
                self.staging = input;
                result?;
                due
            }
            Coder::Tiles(_) => {
                let grid = Grid::new(size.0, size.1, self.method.tile);
                let mut rows = std::mem::take(&mut self.rows);
                let bitmap = gather_changed_tiles(&grid, &picture.bgra, previous.filter(|_| !due), keep_alpha, &mut rows);
                out.extend_from_slice(&bitmap);
                let result = if rows.is_empty() { Ok(()) } else { self.block_coder()?.compress(&rows, out) };
                self.rows = rows;
                result?;
                due
            }
            Coder::Adaptive(percent) => {
                let grid = Grid::new(size.0, size.1, self.method.tile);
                let tiles = previous.filter(|_| !due).filter(|previous| {
                    let changed = (0..grid.count()).filter(|&i| grid.changed(i, &picture.bgra, previous)).count();
                    changed * 100 < usize::from(percent) * grid.count()
                });
                if let Some(previous) = tiles {
                    let mut rows = std::mem::take(&mut self.rows);
                    let bitmap = gather_changed_tiles(&grid, &picture.bgra, Some(previous), false, &mut rows);
                    out.extend_from_slice(&bitmap);
                    let result = if rows.is_empty() { Ok(()) } else { self.block_coder()?.compress(&rows, out) };
                    self.rows = rows;
                    result?;
                    self.tile_path_frames += 1;
                    tile_path = true;
                    false
                } else {
                    push_pixels(out, &picture.bgra, false);
                    self.full_path_frames += 1;
                    true
                }
            }
        };
        self.since_keyframe = if keyframe { 1 } else { self.since_keyframe + 1 };
        let payload = (out.len() - HEADER_BYTES) as u32;
        let flags = (u8::from(keyframe) * FLAG_KEYFRAME)
            | (u8::from(keep_alpha) * FLAG_ALPHA)
            | (u8::from(tile_path) * FLAG_TILE_PATH);
        out[0..4].copy_from_slice(&picture.width.to_le_bytes());
        out[4..8].copy_from_slice(&picture.height.to_le_bytes());
        out[8..10].copy_from_slice(&(self.method.tile as u16).to_le_bytes());
        out[10] = flags;
        out[11] = self.method.coder.id();
        out[12..16].copy_from_slice(&payload.to_le_bytes());
        Ok(())
    }

    /// Alpha dropped: the frame repacked to BGR in the reused staging buffer
    /// (give it back with `restore`). Alpha kept: None, the BGRA rows are
    /// coded as read back, with no copy.
    fn repack(&mut self, picture: &Picture) -> Option<Vec<u8>> {
        if self.method.keep_alpha {
            return None;
        }
        let mut input = std::mem::take(&mut self.staging);
        input.clear();
        push_pixels(&mut input, &picture.bgra, false);
        Some(input)
    }

    fn restore(&mut self, repacked: Option<Vec<u8>>) {
        if let Some(buffer) = repacked {
            self.staging = buffer;
        }
    }

    fn block_coder(&mut self) -> Result<&mut BlockCoder> {
        self.block.as_mut().ok_or_else(|| "byte coder missing".to_owned())
    }
}

fn encode_png(input: &[u8], size: (u32, u32), keep_alpha: bool, out: &mut Vec<u8>) -> Result<()> {
    let mut encoder = png::Encoder::new(&mut *out, size.0, size.1);
    encoder.set_color(if keep_alpha { png::ColorType::Rgba } else { png::ColorType::Rgb });
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(|error| format!("png header: {error}"))?;
    writer.write_image_data(input).map_err(|error| format!("png write: {error}"))?;
    writer.finish().map_err(|error| format!("png finish: {error}"))
}

/// The decompressing side: owns the BGRA canvas that dependent files update.
pub struct Decoder {
    method: Method,
    block: Option<BlockCoder>,
    canvas: Vec<u8>,
    size: (u32, u32),
    staging: Vec<u8>,
}

impl Decoder {
    pub fn new(method: Method) -> Result<Self> {
        Ok(Self {
            method,
            block: method.coder.block().map(BlockCoder::new).transpose()?,
            canvas: Vec::new(),
            size: (0, 0),
            staging: Vec::new(),
        })
    }

    pub fn canvas(&self) -> &[u8] {
        &self.canvas
    }

    /// Decodes one file into the canvas; returns the frame size.
    pub fn decode(&mut self, file: &[u8]) -> Result<(u32, u32)> {
        let header = file.get(..HEADER_BYTES).ok_or("file shorter than its header")?;
        let word = |at: usize| u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]]);
        let size = (word(0), word(4));
        let tile = u32::from(u16::from_le_bytes([header[8], header[9]]));
        let flags = header[10];
        let (keyframe, keep_alpha, tile_path) =
            (flags & FLAG_KEYFRAME != 0, flags & FLAG_ALPHA != 0, flags & FLAG_TILE_PATH != 0);
        if header[11] != self.method.coder.id() {
            return Err(format!("coder {} in a {} file", header[11], self.method.name()));
        }
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
        let stored = size.0 as usize * size.1 as usize * if keep_alpha { 4 } else { 3 };
        match self.method.coder {
            Coder::Raw => put_pixels(&mut self.canvas, payload, keep_alpha),
            Coder::Whole(_) => {
                self.decompress(payload, stored)?;
                put_pixels(&mut self.canvas, &self.staging, keep_alpha);
            }
            Coder::Png => {
                let decoder = png::Decoder::new(std::io::Cursor::new(payload));
                let mut reader = decoder.read_info().map_err(|error| format!("png header: {error}"))?;
                self.staging.resize(stored, 0);
                reader.next_frame(&mut self.staging).map_err(|error| format!("png decode: {error}"))?;
                put_pixels(&mut self.canvas, &self.staging, keep_alpha);
            }
            Coder::Qoi => {
                self.staging.resize(stored, 0);
                qoi::decode_to_buf(&mut self.staging, payload).map_err(|error| format!("qoi decode: {error}"))?;
                put_pixels(&mut self.canvas, &self.staging, keep_alpha);
            }
            Coder::Xor(_) => {
                self.decompress(payload, stored)?;
                if keyframe {
                    put_pixels(&mut self.canvas, &self.staging, keep_alpha);
                } else {
                    xor_pixels(&mut self.canvas, &self.staging, keep_alpha);
                }
            }
            Coder::Tiles(_) => self.decode_tiles(payload, size, tile, keep_alpha)?,
            Coder::Adaptive(_) if tile_path => self.decode_tiles(payload, size, tile, false)?,
            Coder::Adaptive(_) => put_pixels(&mut self.canvas, payload, false),
        }
        Ok(size)
    }

    fn decompress(&mut self, payload: &[u8], expected: usize) -> Result<()> {
        let block = self.block.as_mut().ok_or("byte coder missing")?;
        block.decompress(payload, expected, &mut self.staging)
    }

    fn decode_tiles(&mut self, payload: &[u8], size: (u32, u32), tile: u32, keep_alpha: bool) -> Result<()> {
        let grid = Grid::new(size.0, size.1, tile);
        let (bitmap, packed) = payload.split_at_checked(grid.count().div_ceil(8)).ok_or("tile bitmap missing")?;
        let changed = |index: usize| bitmap[index / 8] & (1 << (index % 8)) != 0;
        let pixel_bytes = if keep_alpha { 4 } else { 3 };
        let stored: usize = (0..grid.count())
            .filter(|&index| changed(index))
            .map(|index| grid.tile_pixels(index) * pixel_bytes)
            .sum();
        if stored == 0 {
            return Ok(());
        }
        self.decompress(packed, stored)?;
        let mut at = 0;
        for index in (0..grid.count()).filter(|&index| changed(index)) {
            for range in grid.rows_of(index) {
                let length = range.len() / 4 * pixel_bytes;
                put_pixels(&mut self.canvas[range], &self.staging[at..at + length], keep_alpha);
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
        && decoded.chunks_exact(4).zip(source.chunks_exact(4)).all(|(a, b)| a[..3] == b[..3])
}
