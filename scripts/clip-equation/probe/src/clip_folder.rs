//! PROBE COPY for the clip recorder experiments: the waiting-frame symbols of
//! `crates/synapse-mcp/src/server/clip_folder.rs` at commit e95e1717, copied
//! verbatim: `WAITING_DIR`, `TEMP_SUFFIX`, `WAITING_FRAME_EXTENSION`,
//! `WAITING_ZSTD_LEVEL`, `WAITING_BYTES_PER_PIXEL`, `BGRA_BYTES_PER_PIXEL`,
//! `raw_frame_path`, `WaitingFrameWriter`, `WaitingReadError`,
//! `WaitingFrameReader`, `LossCause` and `QualityReport`. Changed lines carry
//! `// probe:`: the zstd level is a factor (architecture Z), and the serde
//! derives are dropped (nothing is serialised here).

use std::path::{Path, PathBuf};

pub(super) const WAITING_DIR: &str = "waiting";
const TEMP_SUFFIX: &str = ".tmp";
/// Extension of a waiting frame: B, G, R rows compressed with zstd (A8).
const WAITING_FRAME_EXTENSION: &str = "zst";
/// zstd level of the waiting frames (package 0b full comparison, `ca46079d`).
const WAITING_ZSTD_LEVEL: i32 = -3;
/// A waiting frame keeps B, G and R; the alpha byte is dropped (A8).
pub(super) const WAITING_BYTES_PER_PIXEL: u64 = 3;
/// The observer reads frames back as B, G, R, A.
const BGRA_BYTES_PER_PIXEL: usize = 4;

/// The waiting frame of instant k: tightly packed B, G, R rows at the captured
/// size (the size is in its `frames.jsonl` line), compressed whole with zstd
/// (A8). Each file stands alone.
pub(super) fn raw_frame_path(folder: &Path, instant: u64) -> PathBuf {
    folder
        .join(WAITING_DIR)
        .join(format!("frame-{instant:08}.{WAITING_FRAME_EXTENSION}"))
}

/// A8: writes the waiting frames of one recording through one reused zstd
/// context.
pub(super) struct WaitingFrameWriter {
    compressor: zstd::bulk::Compressor<'static>,
    bgr: Vec<u8>,
    compressed: Vec<u8>,
}

impl WaitingFrameWriter {
    pub(super) fn new() -> Result<Self, String> {
        let _ = WAITING_ZSTD_LEVEL; // probe: the level is a factor (Z)
        let compressor = zstd::bulk::Compressor::new(crate::exp_settings::settings().zstd_level) // probe
            .map_err(|error| format!("could not open the zstd compressor: {error}"))?;
        Ok(Self {
            compressor,
            bgr: Vec::new(),
            compressed: Vec::new(),
        })
    }

    /// Drops the alpha byte of a BGRA frame (B, G, R stay), compresses the
    /// whole frame with zstd at `WAITING_ZSTD_LEVEL`, and writes it as the
    /// waiting frame of instant k (`.tmp`, then rename). Returns the bytes
    /// written to disk.
    pub(super) fn write(
        &mut self,
        folder: &Path,
        instant: u64,
        bgra: &[u8],
    ) -> Result<u64, String> {
        self.bgr.clear();
        self.bgr.extend(
            bgra.chunks_exact(BGRA_BYTES_PER_PIXEL)
                .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]]),
        );
        self.compressed.clear();
        if crate::exp_settings::settings().raw_waiting {
            // probe: architecture D0, raw waiting frames (no zstd)
            std::mem::swap(&mut self.compressed, &mut self.bgr); // probe
        } else {
        self.compressed
            .reserve(zstd::zstd_safe::compress_bound(self.bgr.len()));
        self.compressor
            .compress_to_buffer(&self.bgr, &mut self.compressed)
            .map_err(|error| format!("could not compress waiting frame {instant}: {error}"))?;
        } // probe
        let path = raw_frame_path(folder, instant);
        let temp = path.with_extension(format!("{WAITING_FRAME_EXTENSION}{TEMP_SUFFIX}"));
        std::fs::write(&temp, &self.compressed)
            .map_err(|error| format!("could not write {}: {error}", temp.display()))?;
        std::fs::rename(&temp, &path)
            .map_err(|error| format!("could not rename {}: {error}", temp.display()))?;
        Ok(self.compressed.len() as u64)
    }
}

/// Why a waiting frame could not be read back: the loss cause its instants
/// get, and the detail for the log.
pub(super) struct WaitingReadError {
    pub(super) cause: LossCause,
    pub(super) message: String,
}

/// Reads waiting frames back through one reused zstd context.
pub(super) struct WaitingFrameReader {
    decompressor: zstd::bulk::Decompressor<'static>,
}

impl WaitingFrameReader {
    pub(super) fn new() -> Result<Self, String> {
        let decompressor = zstd::bulk::Decompressor::new()
            .map_err(|error| format!("could not open the zstd decompressor: {error}"))?;
        Ok(Self { decompressor })
    }

    /// A waiting frame as B, G, R rows of `width` × `height`. A missing file
    /// is `waiting_missing`; a short or damaged file, or one of another size,
    /// is `waiting_unreadable`.
    pub(super) fn read(
        &mut self,
        path: &Path,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, WaitingReadError> {
        let bytes = std::fs::read(path).map_err(|error| WaitingReadError {
            cause: if error.kind() == std::io::ErrorKind::NotFound {
                LossCause::WaitingMissing
            } else {
                LossCause::WaitingUnreadable
            },
            message: format!("could not read {}: {error}", path.display()),
        })?;
        self.decode(path, &bytes, width, height)
            .map_err(|message| WaitingReadError {
                cause: LossCause::WaitingUnreadable,
                message,
            })
    }

    fn decode(
        &mut self,
        path: &Path,
        bytes: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        let expected =
            usize::try_from(u64::from(width) * u64::from(height) * WAITING_BYTES_PER_PIXEL)
                .map_err(|_| format!("{} is too large for this machine", path.display()))?;
        if crate::exp_settings::settings().raw_waiting {
            // probe: architecture D0, the file is the B, G, R rows
            return (bytes.len() == expected).then(|| bytes.to_vec()).ok_or_else(|| format!("{} is not {expected} bytes", path.display())); // probe
        }
        // The content size in the zstd frame header is checked first, so a
        // damaged header never sizes an allocation.
        let stored = zstd::zstd_safe::get_frame_content_size(bytes)
            .ok()
            .flatten()
            .and_then(|size| usize::try_from(size).ok());
        if stored != Some(expected) {
            return Err(format!(
                "{} holds {stored:?} bytes, not the {expected} of a {width}x{height} frame",
                path.display()
            ));
        }
        let bgr = self
            .decompressor
            .decompress(bytes, expected)
            .map_err(|error| format!("could not decompress {}: {error}", path.display()))?;
        if bgr.len() != expected {
            return Err(format!(
                "{} decompressed to {} bytes, not {expected}",
                path.display(),
                bgr.len()
            ));
        }
        Ok(bgr)
    }
}

/// Why a frame for an instant is missing from the clip (E1, condition C).
#[derive(Clone, Copy, Debug, PartialEq, Eq)] // probe: no serde
pub(super) enum LossCause {
    SlotOverflow,
    Disk,
    LoadShedding,
    ServiceRestart,
    /// The clip worker could not read the waiting frame back (damaged or
    /// short file); the picture before it was kept in the segment.
    WaitingUnreadable,
    /// The waiting frame of a new picture was not on disk when the clip
    /// worker encoded its segment; the picture before it was kept.
    WaitingMissing,
}

/// The rate control the encoder accepted and read back (B7, condition Q).
#[derive(Clone, Debug, Default)] // probe: no serde
pub(super) struct QualityReport {
    /// `quality` or `bitrate`.
    pub(super) mode: String,
    /// Quality level q, or bits per second.
    pub(super) value: u32,
    pub(super) gop_frames: u32,
    pub(super) b_frames: u32,
    pub(super) screen_content_hint: bool,
    /// `held`, `unverified` or `not_held` (C6).
    pub(super) status: String,
}
