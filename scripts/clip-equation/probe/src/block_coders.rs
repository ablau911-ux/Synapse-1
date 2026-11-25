//! Package 0b: the general-purpose byte coders the waiting-frame methods are
//! built from. Each `BlockCoder` owns its reusable contexts, so per-frame
//! figures do not count a context setup where the library allows reuse:
//!
//!   lz4         `lz4_flex` block (pure Rust, default safe features)
//!   lz4 HC      the `lz4` C binding, `LZ4_compress_HC` (it allocates its own
//!               state on every call; the binding offers no reusable state)
//!   zstd        `zstd::bulk` Compressor / Decompressor, reused
//!   zstd MT     the same with `NbWorkers` (zstd's own worker threads)
//!   deflate     raw deflate through `miniz_oxide` (its one-call helpers set
//!               up a fresh state per frame) and through `flate2` with the
//!               `zlib-rs` backend (Compress / Decompress reused)
//!   snappy      `snap::raw` Encoder / Decoder, reused

use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};

use crate::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    Lz4,
    Lz4Hc(i32),
    Zstd(i32),
    ZstdThreads(i32, u32),
    DeflateMiniz(u8),
    DeflateZlibRs(u32),
    Snappy,
}

impl Block {
    pub fn name(self) -> String {
        match self {
            Self::Lz4 => "lz4".to_owned(),
            Self::Lz4Hc(level) => format!("lz4 HC {level}"),
            Self::Zstd(level) => format!("zstd {level}"),
            Self::ZstdThreads(level, workers) => format!("zstd {level} x{workers} threads"),
            Self::DeflateMiniz(level) => format!("deflate miniz {level}"),
            Self::DeflateZlibRs(level) => format!("deflate zlib-rs {level}"),
            Self::Snappy => "snappy".to_owned(),
        }
    }

    /// True when the coder runs threads of its own (processor time must then
    /// be read for the whole process, not the calling thread).
    pub const fn threaded(self) -> bool {
        matches!(self, Self::ZstdThreads(..))
    }
}

pub struct BlockCoder {
    block: Block,
    zstd_in: Option<zstd::bulk::Compressor<'static>>,
    zstd_out: zstd::bulk::Decompressor<'static>,
    zlib_in: Option<Compress>,
    zlib_out: Decompress,
    snappy_in: snap::raw::Encoder,
    snappy_out: snap::raw::Decoder,
    packed: Vec<u8>,
}

impl BlockCoder {
    pub fn new(block: Block) -> Result<Self> {
        let zstd_in = match block {
            Block::Zstd(level) => Some(compressor(level)?),
            Block::ZstdThreads(level, workers) => {
                let mut zstd = compressor(level)?;
                zstd.set_parameter(zstd::zstd_safe::CParameter::NbWorkers(workers))
                    .map_err(|error| format!("zstd workers {workers}: {error}"))?;
                Some(zstd)
            }
            _ => None,
        };
        let zlib_in = match block {
            Block::DeflateZlibRs(level) => Some(Compress::new(Compression::new(level), false)),
            _ => None,
        };
        Ok(Self {
            block,
            zstd_in,
            zstd_out: zstd::bulk::Decompressor::new().map_err(|error| format!("zstd: {error}"))?,
            zlib_in,
            zlib_out: Decompress::new(false),
            snappy_in: snap::raw::Encoder::new(),
            snappy_out: snap::raw::Decoder::new(),
            packed: Vec::new(),
        })
    }

    /// Appends the compressed form of `input` to `out`.
    pub fn compress(&mut self, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        match self.block {
            Block::Lz4 => {
                grow(
                    &mut self.packed,
                    lz4_flex::block::get_maximum_output_size(input.len()),
                );
                let n = lz4_flex::block::compress_into(input, &mut self.packed)
                    .map_err(|error| format!("lz4 compress: {error}"))?;
                out.extend_from_slice(&self.packed[..n]);
            }
            Block::Lz4Hc(level) => {
                let bound = lz4::block::compress_bound(input.len())
                    .map_err(|error| format!("lz4 bound: {error}"))?;
                grow(&mut self.packed, bound);
                let n = lz4::block::compress_to_buffer(
                    input,
                    Some(lz4::block::CompressionMode::HIGHCOMPRESSION(level)),
                    false,
                    &mut self.packed,
                )
                .map_err(|error| format!("lz4 HC compress: {error}"))?;
                out.extend_from_slice(&self.packed[..n]);
            }
            Block::Zstd(_) | Block::ZstdThreads(..) => {
                let Some(zstd) = self.zstd_in.as_mut() else {
                    return Err("zstd compressor missing".to_owned());
                };
                self.packed.clear();
                self.packed
                    .reserve(zstd::zstd_safe::compress_bound(input.len()));
                zstd.compress_to_buffer(input, &mut self.packed)
                    .map_err(|error| format!("zstd compress: {error}"))?;
                out.extend_from_slice(&self.packed);
            }
            Block::DeflateMiniz(level) => {
                out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(input, level));
            }
            Block::DeflateZlibRs(_) => {
                let Some(zlib) = self.zlib_in.as_mut() else {
                    return Err("deflate compressor missing".to_owned());
                };
                zlib.reset();
                self.packed.clear();
                self.packed.reserve(input.len() + input.len() / 8 + 1024);
                loop {
                    let status = zlib
                        .compress_vec(
                            &input[zlib.total_in() as usize..],
                            &mut self.packed,
                            FlushCompress::Finish,
                        )
                        .map_err(|error| format!("deflate compress: {error}"))?;
                    if status == Status::StreamEnd {
                        break;
                    }
                    self.packed.reserve(self.packed.capacity());
                }
                out.extend_from_slice(&self.packed);
            }
            Block::Snappy => {
                grow(&mut self.packed, snap::raw::max_compress_len(input.len()));
                let n = self
                    .snappy_in
                    .compress(input, &mut self.packed)
                    .map_err(|error| format!("snappy compress: {error}"))?;
                out.extend_from_slice(&self.packed[..n]);
            }
        }
        Ok(())
    }

    /// Replaces `out` with the `expected` bytes that `input` decodes to.
    pub fn decompress(&mut self, input: &[u8], expected: usize, out: &mut Vec<u8>) -> Result<()> {
        match self.block {
            Block::Lz4 => {
                out.resize(expected, 0);
                let n = lz4_flex::block::decompress_into(input, out)
                    .map_err(|error| format!("lz4 decompress: {error}"))?;
                out.truncate(n);
            }
            Block::Lz4Hc(_) => {
                out.resize(expected, 0);
                let n = lz4::block::decompress_to_buffer(input, Some(expected as i32), out)
                    .map_err(|error| format!("lz4 decompress: {error}"))?;
                out.truncate(n);
            }
            Block::Zstd(_) | Block::ZstdThreads(..) => {
                out.clear();
                out.reserve(expected);
                self.zstd_out
                    .decompress_to_buffer(input, out)
                    .map_err(|error| format!("zstd decompress: {error}"))?;
            }
            Block::DeflateMiniz(_) => {
                *out = miniz_oxide::inflate::decompress_to_vec(input)
                    .map_err(|error| format!("inflate: {error:?}"))?;
            }
            Block::DeflateZlibRs(_) => {
                self.zlib_out.reset(false);
                out.clear();
                out.reserve(expected);
                self.zlib_out
                    .decompress_vec(input, out, FlushDecompress::Finish)
                    .map_err(|error| format!("inflate: {error}"))?;
            }
            Block::Snappy => {
                out.resize(expected, 0);
                let n = self
                    .snappy_out
                    .decompress(input, out)
                    .map_err(|error| format!("snappy decompress: {error}"))?;
                out.truncate(n);
            }
        }
        if out.len() != expected {
            return Err(format!(
                "{} decoded {} bytes, {expected} expected",
                self.block.name(),
                out.len()
            ));
        }
        Ok(())
    }
}

fn compressor(level: i32) -> Result<zstd::bulk::Compressor<'static>> {
    zstd::bulk::Compressor::new(level).map_err(|error| format!("zstd level {level}: {error}"))
}

/// Grows a scratch buffer without zero-filling it again on every frame.
fn grow(buffer: &mut Vec<u8>, length: usize) {
    if buffer.len() < length {
        buffer.resize(length, 0);
    }
}
