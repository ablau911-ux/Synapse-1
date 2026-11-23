//! DYN1 proof surface. The format and rules are included from product source.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Result;
use crate::clip_exact::{self, ExactEntry, ExactReader, ExactRef, ExactStream};
use crate::exp_cpu::Spent;
use crate::exp_measure::RamMeter;

#[derive(Deserialize, Serialize)]
struct CorpusLine {
    instant: u64,
    #[serde(default)]
    t_ms: u64,
    new: bool,
    frame: u32,
    width: u32,
    height: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    x: Option<ExactRef>,
}

struct Corpus {
    folder: PathBuf,
    lines: Vec<CorpusLine>,
    decoder: zstd::bulk::Decompressor<'static>,
    cached: Option<u32>,
    picture: Vec<u8>,
}

impl Corpus {
    fn open(folder: &Path) -> Result<Self> {
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

    fn load(&mut self, index: usize) -> Result<()> {
        let line = &self.lines[index];
        if self.cached != Some(line.frame) {
            let packed = std::fs::read(self.folder.join(format!("frame-{:04}.zst", line.frame)))
                .map_err(|e| e.to_string())?;
            self.picture.resize(
                line.width as usize * line.height as usize * clip_exact::BGR_BYTES_PER_PIXEL,
                0,
            );
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

fn exact(folder: &Path, out: &Path) -> Result<()> {
    let mut corpus = Corpus::open(folder)?;
    corpus.load(0)?;
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let mut index = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out.join("frames.jsonl"))
        .map_err(|e| e.to_string())?;
    let ram = RamMeter::start();
    let mut spent = Spent::default();
    let mut writer =
        spent.time(|| ExactStream::create(out, corpus.lines[0].width, corpus.lines[0].height))?;
    let mut entries = Vec::new();
    for i in 0..corpus.lines.len() {
        corpus.load(i)?;
        let line = &mut corpus.lines[i];
        let reference = spent.time(|| -> Result<Option<ExactRef>> {
            writer.resize(line.width, line.height)?;
            if line.new {
                writer.write(&corpus.picture, None)
            } else {
                Ok(None)
            }
        })?;
        line.x = reference;
        if let Some(exact) = reference {
            entries.push(ExactEntry {
                instant: line.instant,
                exact,
                width: line.width,
                height: line.height,
            });
        }
        spent.time(|| -> Result<()> {
            serde_json::to_writer(&mut index, line).map_err(|e| e.to_string())?;
            index.write_all(b"\n").map_err(|e| e.to_string())
        })?;
    }
    let summary = spent.time(|| writer.finish())?;
    let ram_peak = ram.read().ram;
    drop(writer);
    drop(index);
    let mut reader = ExactReader::open(out, entries)?;
    let mut reader_mismatches = 0u64;
    // Every instant sequentially, then backwards across group and repeat boundaries.
    for i in (0..corpus.lines.len()).chain(
        (0..corpus.lines.len())
            .rev()
            .step_by(clip_exact::EXACT_GROUP_PICTURES as usize),
    ) {
        corpus.load(i)?;
        let line = &corpus.lines[i];
        let (picture, width, height) = reader.picture_at(line.instant)?;
        if (width, height) != (line.width, line.height) {
            return Err("reader returned wrong dimensions".into());
        }
        reader_mismatches += picture
            .iter()
            .zip(&corpus.picture)
            .filter(|(a, b)| a != b)
            .count() as u64;
    }
    let report = json!({
        "constants": {
            "EXACT_GROUP_PICTURES": clip_exact::EXACT_GROUP_PICTURES,
            "EXACT_ZSTD_LEVEL": clip_exact::EXACT_ZSTD_LEVEL,
            "EXACT_WINDOW_LOG": clip_exact::EXACT_WINDOW_LOG,
            "BGR_BYTES_PER_PIXEL": clip_exact::BGR_BYTES_PER_PIXEL,
            "EXACT_COUNT_BYTES": clip_exact::EXACT_COUNT_BYTES,
            "EXACT_BAND_BYTES": clip_exact::EXACT_BAND_BYTES,
            "EXACT_KEY_HEADER_BYTES": clip_exact::EXACT_KEY_HEADER_BYTES
        },
        "records": summary.records, "groups": summary.groups, "bytes": summary.bytes,
        "cpu_ns": spent.cpu_ns, "cpu_cycles": spent.cycles, "ram_peak": ram_peak,
        "reader_mismatches": reader_mismatches
    });
    serde_json::to_writer(
        File::create(out.join("probe.json")).map_err(|e| e.to_string())?,
        &report,
    )
    .map_err(|e| e.to_string())?;
    println!("{report}");
    Ok(())
}

pub fn run() -> Result<()> {
    let args: HashMap<String, String> = std::env::args()
        .skip(2)
        .filter_map(|arg| {
            arg.split_once('=')
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
        })
        .collect();
    let folder = Path::new(args.get("folder").ok_or("exp-dyn requires folder=")?);
    let out = Path::new(args.get("out").ok_or("exp-dyn requires out=")?);
    match args.get("mode").map(String::as_str) {
        Some("exact") => exact(folder, out),
        _ => Err("exp-dyn requires mode=exact".into()),
    }
}
