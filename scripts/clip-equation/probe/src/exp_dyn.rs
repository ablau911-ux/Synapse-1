//! The fixed frame corpus reader (`exp-corpus` output), used by `exp-av1`.
//! The DYN1 experiments that also lived here were removed with that design.

use std::fs::File;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;

const BGR_CHANNELS: usize = 3;

#[derive(Deserialize, Serialize)]
pub(crate) struct CorpusLine {
    pub(crate) instant: u64,
    #[serde(default)]
    t_ms: u64,
    new: bool,
    frame: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

pub(crate) struct Corpus {
    folder: PathBuf,
    pub(crate) lines: Vec<CorpusLine>,
    decoder: zstd::bulk::Decompressor<'static>,
    cached: Option<u32>,
    pub(crate) picture: Vec<u8>,
}

impl Corpus {
    pub(crate) fn open(folder: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(folder.join("frames.jsonl")).map_err(|e| e.to_string())?;
        let mut lines = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<std::result::Result<Vec<CorpusLine>, _>>()
            .map_err(|e| e.to_string())?;
        let meta: serde_json::Value = serde_json::from_reader(
            File::open(folder.join("meta.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let instant_ms = meta["instant_ms"]
            .as_u64()
            .ok_or("corpus meta lacks instant_ms")?;
        for line in &mut lines {
            line.t_ms = line
                .instant
                .checked_mul(instant_ms)
                .ok_or("corpus instant time overflow")?;
        }
        if lines.is_empty() {
            return Err("empty corpus".into());
        }
        Ok(Self {
            folder: folder.to_owned(),
            lines,
            decoder: zstd::bulk::Decompressor::new().map_err(|e| e.to_string())?,
            cached: None,
            picture: Vec::new(),
        })
    }

    pub(crate) fn load(&mut self, index: usize) -> Result<()> {
        let line = &self.lines[index];
        if self.cached != Some(line.frame) {
            let packed = std::fs::read(self.folder.join(format!("frame-{:04}.zst", line.frame)))
                .map_err(|e| e.to_string())?;
            self.picture
                .resize(line.width as usize * line.height as usize * BGR_CHANNELS, 0);
            let n = self
                .decoder
                .decompress_to_buffer(&packed, self.picture.as_mut_slice())
                .map_err(|e| e.to_string())?;
            if n != self.picture.len() {
                return Err("corpus frame length differs from dimensions".into());
            }
            self.cached = Some(line.frame);
        }
        Ok(())
    }
}
