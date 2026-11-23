//! DYN1 exact layer: changed row bands in independently seekable zstd groups.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zstd::zstd_safe::zstd_sys::ZSTD_EndDirective as EndDirective;
use zstd::zstd_safe::{
    self, CCtx, CParameter, DCtx, DParameter, InBuffer, OutBuffer, ResetDirective,
};

use super::clip_motion::PixelRect;

pub const EXACT_GROUP_PICTURES: u32 = 20;
pub const EXACT_ZSTD_LEVEL: i32 = 1;
pub const EXACT_WINDOW_LOG: u32 = 27;
pub const BGR_BYTES_PER_PIXEL: usize = 3;
pub const EXACT_COUNT_BYTES: usize = size_of::<u16>();
pub const EXACT_BAND_BYTES: usize = EXACT_COUNT_BYTES * 2;
pub const EXACT_KEY_HEADER_BYTES: usize = EXACT_COUNT_BYTES + EXACT_BAND_BYTES;
/// Broad changes keep whole-frame alignment in the streaming dictionary.
pub const EXACT_WHOLE_FRAME_ROW_PERCENT: u32 = 75;
const PERCENT_DENOMINATOR: u32 = 100;
const STREAM_SLACK: usize = 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(from = "(u32, u64, u32, u32)", into = "(u32, u64, u32, u32)")]
pub struct ExactRef {
    pub group: u32,
    pub offset: u64,
    pub bytes: u32,
    pub raw: u32,
}

impl ExactRef {
    pub fn is_key(&self) -> bool {
        self.offset == 0
    }
}

impl From<(u32, u64, u32, u32)> for ExactRef {
    fn from((group, offset, bytes, raw): (u32, u64, u32, u32)) -> Self {
        Self {
            group,
            offset,
            bytes,
            raw,
        }
    }
}

impl From<ExactRef> for (u32, u64, u32, u32) {
    fn from(value: ExactRef) -> Self {
        (value.group, value.offset, value.bytes, value.raw)
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct ExactSummary {
    pub groups: u32,
    pub records: u64,
    pub bytes: u64,
}

pub struct ExactStream {
    dir: PathBuf,
    width: u32,
    height: u32,
    picture: Vec<u8>,
    cctx: CCtx<'static>,
    file: Option<File>,
    group: u32,
    records_in_group: u32,
    offset: u64,
    payload: Vec<u8>,
    out: Vec<u8>,
    summary: ExactSummary,
    finished: bool,
}

fn codec<T>(result: Result<T, usize>) -> Result<T, String> {
    result.map_err(|code| format!("exact zstd: {}", zstd_safe::get_error_name(code)))
}

fn picture_bytes(width: u32, height: u32) -> Result<usize, String> {
    if width == 0 || height == 0 || height > u32::from(u16::MAX) {
        return Err("exact picture has invalid dimensions".into());
    }
    let bytes = u64::from(width) * u64::from(height) * BGR_BYTES_PER_PIXEL as u64;
    let max_header = EXACT_COUNT_BYTES as u64 + u64::from(height) * EXACT_BAND_BYTES as u64;
    if bytes + max_header > u64::from(u32::MAX) {
        return Err("exact record exceeds the format's u32 length".into());
    }
    usize::try_from(bytes).map_err(|_| "exact picture exceeds address space".into())
}

fn group_path(dir: &Path, group: u32) -> PathBuf {
    dir.join(format!("group-{group:06}.zst"))
}

impl ExactStream {
    pub fn create(folder: &Path, width: u32, height: u32) -> Result<Self, String> {
        let bytes = picture_bytes(width, height)?;
        let dir = folder.join("frames");
        std::fs::create_dir_all(&dir).map_err(|e| format!("create exact directory: {e}"))?;
        let mut cctx = CCtx::try_create().ok_or("allocate exact compressor")?;
        for param in [
            CParameter::CompressionLevel(EXACT_ZSTD_LEVEL),
            CParameter::WindowLog(EXACT_WINDOW_LOG),
            CParameter::EnableLongDistanceMatching(false),
            CParameter::ChecksumFlag(true),
        ] {
            codec(cctx.set_parameter(param))?;
        }
        Ok(Self {
            dir,
            width,
            height,
            picture: vec![0; bytes],
            cctx,
            file: None,
            group: 0,
            records_in_group: 0,
            offset: 0,
            payload: Vec::new(),
            out: Vec::new(),
            summary: ExactSummary::default(),
            finished: false,
        })
    }

    pub fn write(
        &mut self,
        source: &[u8],
        moving: Option<PixelRect>,
    ) -> Result<Option<ExactRef>, String> {
        if self.finished {
            return Err("exact writer is finished".into());
        }
        if source.len() != self.picture.len() {
            return Err("exact source length does not match picture dimensions".into());
        }
        let bounds = PixelRect {
            x: 0,
            y: 0,
            w: self.width,
            h: self.height,
        };
        let moving = moving.and_then(|rect| rect.intersect(&bounds));
        let stride = self.width as usize * BGR_BYTES_PER_PIXEL;
        let mut bands: Vec<(u16, u16)> = Vec::new();
        for y in 0..self.height {
            let row = y as usize * stride;
            let parts = match moving.filter(|rect| y >= rect.y && y < rect.bottom()) {
                Some(rect) => [
                    0..rect.x as usize * BGR_BYTES_PER_PIXEL,
                    rect.right() as usize * BGR_BYTES_PER_PIXEL..stride,
                ],
                None => [0..stride, stride..stride],
            };
            let mut changed = false;
            for part in parts {
                let range = row + part.start..row + part.end;
                if self.picture[range.clone()] != source[range.clone()] {
                    self.picture[range.clone()].copy_from_slice(&source[range]);
                    changed = true;
                }
            }
            if changed {
                if let Some((_, count)) = bands
                    .last_mut()
                    .filter(|(first, count)| u32::from(*first) + u32::from(*count) == y)
                {
                    *count += 1;
                } else {
                    bands.push((y as u16, 1));
                }
            }
        }
        if bands.is_empty() {
            return Ok(None);
        }
        let changed_rows: u32 = bands.iter().map(|(_, count)| u32::from(*count)).sum();
        if changed_rows * PERCENT_DENOMINATOR > self.height * EXACT_WHOLE_FRAME_ROW_PERCENT {
            bands.clear();
            bands.push((0, self.height as u16));
        }
        if self.file.is_none() {
            self.group = self.summary.groups;
            self.file = Some(
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(group_path(&self.dir, self.group))
                    .map_err(|e| format!("create exact group: {e}"))?,
            );
            self.summary.groups += 1;
            self.records_in_group = 0;
            self.offset = 0;
            bands.clear();
            bands.push((0, self.height as u16));
        }
        self.payload.clear();
        self.payload
            .extend_from_slice(&(bands.len() as u16).to_le_bytes());
        for (first, count) in &bands {
            self.payload.extend_from_slice(&first.to_le_bytes());
            self.payload.extend_from_slice(&count.to_le_bytes());
        }
        for (first, count) in bands {
            self.payload.extend_from_slice(
                &self.picture
                    [first as usize * stride..(usize::from(first) + usize::from(count)) * stride],
            );
        }
        let raw = self.payload.len() as u32;
        let offset = self.offset;
        let bytes = self.compress(EndDirective::ZSTD_e_flush)?;
        let reference = ExactRef {
            group: self.group,
            offset,
            bytes,
            raw,
        };
        self.records_in_group += 1;
        self.summary.records += 1;
        if self.records_in_group == EXACT_GROUP_PICTURES {
            self.close_group()?;
        }
        Ok(Some(reference))
    }

    fn compress(&mut self, directive: EndDirective) -> Result<u32, String> {
        self.out.resize(
            zstd_safe::compress_bound(self.payload.len()) + STREAM_SLACK,
            0,
        );
        let mut input = InBuffer::around(&self.payload);
        let mut total = 0u32;
        loop {
            let mut output = OutBuffer::around(self.out.as_mut_slice());
            let left = codec(
                self.cctx
                    .compress_stream2(&mut output, &mut input, directive),
            )?;
            let bytes = output.pos();
            let file = self.file.as_mut().ok_or("exact group is not open")?;
            file.write_all(&self.out[..bytes])
                .map_err(|e| format!("write exact group: {e}"))?;
            total = total
                .checked_add(bytes as u32)
                .ok_or("exact compressed record too large")?;
            if left == 0 && input.pos == input.src.len() {
                break;
            }
        }
        // File is unbuffered: the record reaches the OS before frames.jsonl can name it.
        self.offset += u64::from(total);
        self.summary.bytes += u64::from(total);
        Ok(total)
    }

    fn close_group(&mut self) -> Result<(), String> {
        if self.file.is_some() {
            self.payload.clear();
            self.compress(EndDirective::ZSTD_e_end)?;
            self.file.take();
        }
        Ok(())
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        if self.finished {
            return Err("exact writer is finished".into());
        }
        let bytes = picture_bytes(width, height)?;
        if (width, height) != (self.width, self.height) {
            self.close_group()?;
            self.width = width;
            self.height = height;
            self.picture.clear();
            self.picture.resize(bytes, 0);
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<ExactSummary, String> {
        self.close_group()?;
        self.finished = true;
        Ok(self.summary)
    }
}

#[derive(Clone, Copy)]
pub struct ExactEntry {
    pub instant: u64,
    pub exact: ExactRef,
    pub width: u32,
    pub height: u32,
}

pub struct ExactReader {
    dir: PathBuf,
    entries: Vec<ExactEntry>,
    dctx: DCtx<'static>,
    picture: Vec<u8>,
    size: (u32, u32),
    position: Option<usize>,
    compressed: Vec<u8>,
    payload: Vec<u8>,
}

impl ExactReader {
    pub fn open(folder: &Path, entries: Vec<ExactEntry>) -> Result<Self, String> {
        let mut group_records = 0;
        for (index, entry) in entries.iter().enumerate() {
            let bytes = picture_bytes(entry.width, entry.height)?;
            let max_raw = bytes + EXACT_COUNT_BYTES + entry.height as usize * EXACT_BAND_BYTES;
            if entry.exact.raw as usize > max_raw
                || (entry.exact.raw as usize) < EXACT_KEY_HEADER_BYTES
                || entry.exact.bytes == 0
                || entry.exact.bytes as usize > zstd_safe::compress_bound(max_raw) + STREAM_SLACK
            {
                return Err("invalid exact record length".into());
            }
            let previous = index.checked_sub(1).map(|i| &entries[i]);
            if previous.is_some_and(|p| p.instant >= entry.instant) {
                return Err("exact instants must increase".into());
            }
            if entry.exact.is_key() {
                if entry.exact.raw as usize != EXACT_KEY_HEADER_BYTES + bytes
                    || previous.is_some_and(|p| p.exact.group >= entry.exact.group)
                {
                    return Err("invalid exact key record".into());
                }
                group_records = 0;
            } else if !previous.is_some_and(|p| {
                p.exact.group == entry.exact.group
                    && p.exact.offset.checked_add(u64::from(p.exact.bytes))
                        == Some(entry.exact.offset)
                    && (p.width, p.height) == (entry.width, entry.height)
            }) {
                return Err("exact group lacks contiguous records or a key".into());
            }
            group_records += 1;
            if group_records > EXACT_GROUP_PICTURES {
                return Err("exact group exceeds record limit".into());
            }
        }
        let mut dctx = DCtx::try_create().ok_or("allocate exact decompressor")?;
        codec(dctx.set_parameter(DParameter::WindowLogMax(EXACT_WINDOW_LOG)))?;
        Ok(Self {
            dir: folder.join("frames"),
            entries,
            dctx,
            picture: Vec::new(),
            size: (0, 0),
            position: None,
            compressed: Vec::new(),
            payload: Vec::new(),
        })
    }

    pub fn picture_at(&mut self, instant: u64) -> Result<(&[u8], u32, u32), String> {
        let end = self
            .entries
            .partition_point(|entry| entry.instant <= instant);
        let target = end
            .checked_sub(1)
            .ok_or("no exact picture at this instant")?;
        let group = self.entries[target].exact.group;
        let start = match self.position {
            Some(position) if position <= target && self.entries[position].exact.group == group => {
                position + 1
            }
            _ => {
                self.position = None;
                self.entries[..=target].partition_point(|entry| entry.exact.group < group)
            }
        };
        for index in start..=target {
            if let Err(error) = self.apply(index) {
                self.position = None;
                return Err(error);
            }
            self.position = Some(index);
        }
        Ok((&self.picture, self.size.0, self.size.1))
    }

    fn apply(&mut self, index: usize) -> Result<(), String> {
        let entry = self.entries[index];
        if entry.exact.is_key() {
            codec(self.dctx.reset(ResetDirective::SessionOnly))?;
            self.size = (entry.width, entry.height);
            self.picture
                .resize(picture_bytes(entry.width, entry.height)?, 0);
        }
        let mut file = File::open(group_path(&self.dir, entry.exact.group))
            .map_err(|e| format!("open exact group: {e}"))?;
        file.seek(SeekFrom::Start(entry.exact.offset))
            .map_err(|e| format!("seek exact record: {e}"))?;
        self.compressed.resize(entry.exact.bytes as usize, 0);
        file.read_exact(&mut self.compressed)
            .map_err(|e| format!("read exact record: {e}"))?;
        self.payload.resize(entry.exact.raw as usize, 0);
        let mut input = InBuffer::around(&self.compressed);
        let mut output = OutBuffer::around(self.payload.as_mut_slice());
        while input.pos < input.src.len() {
            let before = (input.pos, output.pos());
            codec(self.dctx.decompress_stream(&mut output, &mut input))?;
            if before == (input.pos, output.pos()) {
                return Err("exact decompressor made no progress".into());
            }
        }
        if output.pos() != entry.exact.raw as usize {
            return Err("exact record decompressed length differs from index".into());
        }
        self.apply_bands(entry.exact.is_key())
    }

    fn apply_bands(&mut self, key: bool) -> Result<(), String> {
        let number = |at| -> Result<usize, String> {
            let bytes = self
                .payload
                .get(at..at + EXACT_COUNT_BYTES)
                .ok_or("truncated exact band header")?;
            Ok(u16::from_le_bytes([bytes[0], bytes[1]]) as usize)
        };
        let count = number(0)?;
        let stride = self.size.0 as usize * BGR_BYTES_PER_PIXEL;
        let mut at = EXACT_COUNT_BYTES + count * EXACT_BAND_BYTES;
        let mut previous_end = 0;
        if count == 0
            || (key
                && (count != 1
                    || number(EXACT_COUNT_BYTES)? != 0
                    || number(EXACT_COUNT_BYTES * 2)? != self.size.1 as usize))
        {
            return Err("invalid exact key bands".into());
        }
        for band in 0..count {
            let header = EXACT_COUNT_BYTES + band * EXACT_BAND_BYTES;
            let first = number(header)?;
            let rows = number(header + EXACT_COUNT_BYTES)?;
            if rows == 0 || first < previous_end || first + rows > self.size.1 as usize {
                return Err("exact bands overlap or exceed picture".into());
            }
            let end = at + rows * stride;
            let bytes = self
                .payload
                .get(at..end)
                .ok_or("truncated exact band pixels")?;
            self.picture[first * stride..(first + rows) * stride].copy_from_slice(bytes);
            previous_end = first + rows;
            at = end;
        }
        if at != self.payload.len() {
            return Err("trailing exact record bytes".into());
        }
        Ok(())
    }
}
