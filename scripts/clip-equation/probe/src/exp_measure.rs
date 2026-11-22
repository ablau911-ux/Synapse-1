//! Measurement rules shared by ways Z and D (docs/clip-recorder-experiments-plan.md,
//! "Measurement rules, the same for both ways"). One owner per rule:
//!
//! - M1 input: [`CaptureFrame`] holds one new frame as B,G,R,A, the format Windows
//!   capture delivers, expanded from the corpus before the timer starts
//!   ([`CaptureFrame::fill`], never timed). A way that needs B,G,R calls
//!   [`bgra_to_bgr`] inside its own timed work.
//! - M2 CPU: [`crate::exp_cpu::Spent`], the process cycle counter; a way adds every
//!   part of its work (conversion, setup, compression, writing, training) with
//!   [`crate::exp_cpu::Spent::plus`]. Corpus decoding and verification stay outside.
//! - M3 RAM: [`RamMeter`] takes M_0 after the input is prepared; RAM = the operating
//!   system's peak of private bytes (`PeakPagefileUsage`) after the work, minus M_0.
//! - M4 bytes: [`ClipFiles`] writes and counts every file the product keeps for a
//!   clip: the frame files (or encoded segments), `frames.jsonl` (one line per
//!   instant, the same format for both ways) and `clip.json`.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::Result;
use crate::exp_meter::memory_now;

/// One instant of the clip, in milliseconds (f = 20 fps).
const INSTANT_MS: u64 = 50;
const BGRA_BYTES_PER_PIXEL: usize = 4;

/// M1: one new frame as the capture delivers it, B,G,R,A with A = 255.
pub struct CaptureFrame {
    bgra: Vec<u8>,
}

impl CaptureFrame {
    pub fn new(px: usize) -> Self {
        Self { bgra: vec![255; px * BGRA_BYTES_PER_PIXEL] }
    }

    /// The corpus frame (B,G,R rows) expanded to B,G,R,A. Input preparation: never timed.
    pub fn fill(&mut self, bgr: &[u8]) {
        for (pixel, source) in self.bgra.chunks_exact_mut(BGRA_BYTES_PER_PIXEL).zip(bgr.chunks_exact(3)) {
            pixel[..3].copy_from_slice(source);
        }
    }

    pub fn bgra(&self) -> &[u8] {
        &self.bgra
    }
}

/// M1: the product's conversion (`WaitingFrameWriter::write`): the alpha byte is
/// dropped, B, G and R stay. Part of the way's timed work.
pub fn bgra_to_bgr(bgra: &[u8], bgr: &mut Vec<u8>) {
    bgr.clear();
    bgr.extend(bgra.chunks_exact(BGRA_BYTES_PER_PIXEL).flat_map(|pixel| [pixel[0], pixel[1], pixel[2]]));
}

/// The operating system's peak of private bytes (`PeakPagefileUsage`) of this process.
pub fn os_peak_private() -> u64 {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    let size = u32::try_from(std::mem::size_of_val(&counters)).unwrap_or(u32::MAX);
    // SAFETY: the pseudo handle of this process and a sized out-parameter.
    let _ = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut counters).cast(), size) };
    counters.PeakPagefileUsage as u64
}

/// M3: M_0 and the OS peak at that moment; read after the work.
pub struct RamMeter {
    m0: u64,
    peak_at_m0: u64,
}

/// M3: one reading. `ram` = `peak` − M_0. The reading is the work's own only when the
/// work set a new peak (`peak` > `peak_at_m0`); the checks verify that.
pub struct RamReading {
    pub m0: u64,
    pub peak_at_m0: u64,
    pub peak: u64,
    pub ram: u64,
}

impl RamMeter {
    /// Takes M_0 = private bytes now. Call it after the input is prepared.
    pub fn start() -> Self {
        Self { m0: memory_now().1, peak_at_m0: os_peak_private() }
    }

    pub fn read(&self) -> RamReading {
        let peak = os_peak_private();
        RamReading { m0: self.m0, peak_at_m0: self.peak_at_m0, peak, ram: peak.saturating_sub(self.m0) }
    }
}

impl RamReading {
    /// `"<prefix>m0_private"`, `_os_peak_at_m0`, `_os_peak_private` and `_ram_os` as JSON fields.
    pub fn json(&self, prefix: &str) -> String {
        format!(
            "\"{prefix}m0_private\":{},\"{prefix}os_peak_at_m0\":{},\"{prefix}os_peak_private\":{},\"{prefix}ram_os\":{}",
            self.m0, self.peak_at_m0, self.peak, self.ram
        )
    }
}

/// M4: the files of one clip and their bytes, written as the product keeps them.
pub struct ClipFiles {
    dir: PathBuf,
    index: std::fs::File,
    width: usize,
    height: usize,
    payload: u64,
    index_bytes: u64,
    lines: usize,
    files: usize,
}

/// M4: the bytes of every retained file of one clip.
pub struct ClipBytes {
    /// Frame files, encoded segments and the dictionary: what the viewer decodes.
    pub payload: u64,
    pub index: u64,
    pub clip_json: u64,
    pub lines: usize,
    pub files: usize,
}

impl ClipBytes {
    pub fn total(&self) -> u64 {
        self.payload + self.index + self.clip_json
    }

    /// `frame_bytes`, `index_bytes`, `clip_json_bytes`, `index_lines` and `files` as JSON fields.
    pub fn json(&self) -> String {
        format!(
            "\"frame_bytes\":{},\"index_bytes\":{},\"clip_json_bytes\":{},\"index_lines\":{},\"files\":{}",
            self.payload, self.index, self.clip_json, self.lines, self.files
        )
    }
}

impl ClipFiles {
    /// Creates the clip folder and its `frames.jsonl`. Frames are `width` × `height`.
    pub fn create(dir: &Path, width: usize, height: usize) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        let index = std::fs::File::create(dir.join("frames.jsonl")).map_err(|e| format!("frames.jsonl: {e}"))?;
        Ok(Self { dir: dir.to_owned(), index, width, height, payload: 0, index_bytes: 0, lines: 0, files: 2 })
    }

    /// A file the viewer decodes (frame, segment or dictionary); returns its bytes.
    pub fn frame(&mut self, name: &str, data: &[u8]) -> Result<u64> {
        let path = self.dir.join(name);
        std::fs::write(&path, data).map_err(|e| format!("write {}: {e}", path.display()))?;
        self.payload += data.len() as u64;
        self.files += 1;
        Ok(data.len() as u64)
    }

    /// A file another writer made in the clip folder (way D: an MP4 segment), counted from its size on disk.
    pub fn count(&mut self, path: &Path) -> Result<u64> {
        let bytes = std::fs::metadata(path).map_err(|e| format!("size of {}: {e}", path.display()))?.len();
        self.payload += bytes;
        self.files += 1;
        Ok(bytes)
    }

    /// One `frames.jsonl` line, appended as the product appends it at each instant:
    /// instant, time, new or repeated, the stored frame it shows, frame size, cursor
    /// position (the headless corpus has no cursor: 0, 0).
    pub fn line(&mut self, instant: u64, frame: usize, new: bool) -> Result<()> {
        let line = format!(
            "{{\"instant\":{instant},\"t_ms\":{},\"new\":{new},\"frame\":{frame},\"width\":{},\"height\":{},\"cursor\":[0,0]}}\n",
            instant * INSTANT_MS,
            self.width,
            self.height
        );
        self.index.write_all(line.as_bytes()).map_err(|e| format!("frames.jsonl: {e}"))?;
        self.index_bytes += line.len() as u64;
        self.lines += 1;
        Ok(())
    }

    /// Closes `frames.jsonl` and writes `clip.json`; returns the bytes of every file.
    pub fn finish(self, clip_json: &str) -> Result<ClipBytes> {
        drop(self.index);
        std::fs::write(self.dir.join("clip.json"), clip_json).map_err(|e| format!("clip.json: {e}"))?;
        Ok(ClipBytes {
            payload: self.payload,
            index: self.index_bytes,
            clip_json: clip_json.len() as u64,
            lines: self.lines,
            files: self.files,
        })
    }

    /// Σ sizes on disk of every file in a clip folder (the check of the counted bytes).
    pub fn bytes_on_disk(dir: &Path) -> Result<u64> {
        let mut total = 0;
        for entry in std::fs::read_dir(dir).map_err(|e| format!("list {}: {e}", dir.display()))? {
            let entry = entry.map_err(|e| format!("list {}: {e}", dir.display()))?;
            total += entry.metadata().map_err(|e| format!("size: {e}"))?.len();
        }
        Ok(total)
    }
}
