//! DYN1 proof surface. The format and rules are included from product source.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Result;
use crate::clip_decoder::ClipDecoder;
use crate::clip_exact::{self, ExactEntry, ExactReader, ExactRef, ExactStream};
use crate::clip_motion::PixelRect;
use crate::clip_moving::{MOVING_QUEUE_FRAMES, MovingAvailability, MovingCoder};
use crate::exp_cpu::Spent;
use crate::exp_measure::RamMeter;

const PROBE_FPS: u32 = 20;
const MILLISECONDS_PER_SECOND: u64 = 1_000;
const BGR_CHANNELS: usize = 3;

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
            "EXACT_WHOLE_FRAME_ROW_PERCENT": clip_exact::EXACT_WHOLE_FRAME_ROW_PERCENT,
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

fn source_luma(corpus: &mut Corpus, index: usize, rect: PixelRect) -> Result<Vec<u8>> {
    corpus.load(index)?;
    let stride = corpus.lines[index].width as usize * BGR_CHANNELS;
    let mut luma = Vec::with_capacity(rect.area() as usize);
    for row in rect.y..rect.bottom() {
        let start = row as usize * stride + rect.x as usize * BGR_CHANNELS;
        let end = start + rect.w as usize * BGR_CHANNELS;
        let line = corpus
            .picture
            .get(start..end)
            .ok_or("rectangle outside source")?;
        luma.extend(
            line.chunks_exact(BGR_CHANNELS)
                .map(|p| crate::exp_d::luma(i32::from(p[2]), i32::from(p[1]), i32::from(p[0]))),
        );
    }
    Ok(luma)
}

fn moving(folder: &Path, out: &Path, args: &HashMap<String, String>) -> Result<()> {
    let mut corpus = Corpus::open(folder)?;
    let rect_values = args
        .get("rect")
        .ok_or("moving requires rect=")?
        .split(',')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let rect =
        PixelRect::from(<[u32; 4]>::try_from(rect_values).map_err(|_| "rect needs four values")?);
    let source_bounds = PixelRect {
        x: 0,
        y: 0,
        w: corpus.lines[0].width,
        h: corpus.lines[0].height,
    };
    if !source_bounds.contains(&rect) || rect.area() == 0 {
        return Err("rectangle outside corpus".into());
    }
    let pause = args
        .get("pause")
        .map(|value| -> Result<(usize, usize)> {
            let (a, b) = value.split_once('-').ok_or("pause requires from-to")?;
            Ok((
                a.parse::<usize>().map_err(|e| e.to_string())?,
                b.parse::<usize>().map_err(|e| e.to_string())?,
            ))
        })
        .transpose()?;
    if args.get("force").is_some_and(|v| v == "unavailable") {
        MovingAvailability::set_unavailable("probe forced unavailable".into());
    }
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let ram = RamMeter::start();
    let mut spent = Spent::default();
    let mut coder = MovingCoder::start(out, PROBE_FPS)?;
    let mut expected = Vec::with_capacity(corpus.lines.len());
    let mut previous = 0;
    let mut behind = 0;
    let start = std::time::Instant::now();
    for i in 0..corpus.lines.len() {
        let deadline = start
            + std::time::Duration::from_millis(
                i as u64 * MILLISECONDS_PER_SECOND / u64::from(PROBE_FPS),
            );
        if let Some(delay) = deadline.checked_duration_since(std::time::Instant::now()) {
            std::thread::sleep(delay);
        }
        let paused = pause.is_some_and(|(a, b)| i >= a && i <= b);
        if !paused {
            previous = i;
        }
        expected.push(previous);
        corpus.load(previous)?;
        let line = &corpus.lines[i];
        let step = spent.time(|| {
            coder.follow(
                line.instant,
                Some(rect),
                &corpus.picture,
                line.width,
                line.new && !paused,
            )
        });
        behind += usize::from(step.behind);
    }
    let high_water = coder.high_water();
    let creation_ms = coder.creation_ms();
    let cpu_ns = spent.cpu_ns + coder.cpu_hns() as u64 * 100;
    let summary = coder.finish();
    let ram_peak = ram.read().ram;
    let mut scores = Vec::new();
    for segment in &summary.committed {
        let mut decoder = ClipDecoder::open(&out.join(&segment.file))?;
        for n in 0..segment.frames {
            let decoded = decoder.frame_number(n, &segment.key_frames)?;
            let luma = crate::exp_d::luma_of_bgra(
                &decoded.bgra,
                decoded.width as usize,
                rect.w as usize,
                rect.h as usize,
            );
            let index = segment.first_instant as usize + n as usize;
            let mut compare = |at: usize| -> Result<f64> {
                let reference = source_luma(&mut corpus, expected[at], rect)?;
                Ok(crate::exp_d::ssim(
                    &reference,
                    &luma,
                    rect.w as usize,
                    rect.h as usize,
                ))
            };
            scores.push(json!({ "frame": n, "current": compare(index)?,
                "previous": compare(index.saturating_sub(1))?, "next": compare((index + 1).min(expected.len() - 1))? }));
        }
    }
    let files = std::fs::read_dir(out.join("moving"))
        .map_err(|e| e.to_string())?
        .count();
    let report = json!({ "constants": { "MOVING_QUEUE_FRAMES": MOVING_QUEUE_FRAMES,
        "GOP_FRAMES": crate::clip_encoder::GOP_FRAMES }, "instants": corpus.lines.len(),
        "grant": summary.grant, "segments": summary.committed, "short": summary.short,
        "unavailable": summary.unavailable, "scores": scores, "creation_ms": creation_ms,
        "waited": summary.waited,
        "high_water": high_water, "behind": behind, "cpu_ns": cpu_ns, "ram_peak": ram_peak, "files": files });
    serde_json::to_writer(
        File::create(out.join("probe.json")).map_err(|e| e.to_string())?,
        &report,
    )
    .map_err(|e| e.to_string())?;
    println!("{report}");
    Ok(())
}

fn layers(folder: &Path, out: &Path, args: &HashMap<String, String>) -> Result<()> {
    use crate::clip_layers::ClipLayers;
    let mut corpus = Corpus::open(folder)?;
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let mut index = File::create(out.join("frames.jsonl")).map_err(|e| e.to_string())?;
    let ram = RamMeter::start();
    let mut spent = Spent::default();
    let mut layers = ClipLayers::start(
        out,
        corpus.lines[0].width,
        corpus.lines[0].height,
        PROBE_FPS,
    )?;
    let mut entries = Vec::new();
    let mut lines = Vec::new();
    let mut segments = Vec::new();
    let mut bgra = Vec::new();
    let start = std::time::Instant::now();
    for i in 0..corpus.lines.len() {
        let deadline = start
            + std::time::Duration::from_millis(
                i as u64 * MILLISECONDS_PER_SECOND / u64::from(PROBE_FPS),
            );
        if let Some(delay) = deadline.checked_duration_since(std::time::Instant::now()) {
            std::thread::sleep(delay);
        }
        corpus.load(i)?;
        let line = &corpus.lines[i];
        let written = if line.new {
            bgra.clear();
            bgra.extend(
                corpus
                    .picture
                    .chunks_exact(BGR_CHANNELS)
                    .flat_map(|p| [p[0], p[1], p[2], u8::MAX]),
            );
            spent.time(|| layers.picture(line.instant, &bgra, line.width, line.height))?
        } else {
            spent.time(|| layers.repeat(line.instant))?
        };
        if let Some(exact) = written.exact {
            entries.push(ExactEntry {
                instant: line.instant,
                exact,
                width: line.width,
                height: line.height,
            });
        }
        segments.extend(written.committed);
        let mut stored = json!({ "instant": line.instant, "t_ms": line.t_ms, "new": line.new,
            "width": line.width, "height": line.height });
        if let Some(exact) = written.exact {
            stored["x"] = json!(exact);
        }
        if let Some(moving) = written.moving {
            stored["mv"] = json!(moving);
        }
        if let Some(rect) = written.moving_rect {
            stored["mv_rect"] = json!(rect);
        }
        if written.moving_behind {
            stored["lost"] = json!("moving_behind");
        }
        serde_json::to_writer(&mut index, &stored).map_err(|e| e.to_string())?;
        index.write_all(b"\n").map_err(|e| e.to_string())?;
        lines.push(stored);
    }
    let cpu_ns = spent.cpu_ns + layers.cpu_hns() as u64 * 100;
    let mut summary = layers.finish()?;
    segments.append(&mut summary.moving.committed);
    segments.sort_by_key(|s| s.index);
    let ram_peak = ram.read().ram;
    let mut exact = ExactReader::open(out, entries)?;
    let mut decoder: Option<(u32, ClipDecoder)> = None;
    let mut outside_mismatches = 0u64;
    let mut scores = Vec::new();
    for i in 0..lines.len() {
        corpus.load(i)?;
        let line = &lines[i];
        let segment = line["mv"][0]
            .as_u64()
            .and_then(|id| segments.iter().find(|s| u64::from(s.index) == id));
        let rect = segment.map(|s| s.rect);
        let (picture, width, height) = exact.picture_at(corpus.lines[i].instant)?;
        for row in 0..height {
            let stride = width as usize * BGR_CHANNELS;
            let parts = match rect.filter(|r| row >= r.y && row < r.bottom()) {
                Some(r) => [
                    0..r.x as usize * BGR_CHANNELS,
                    r.right() as usize * BGR_CHANNELS..stride,
                ],
                None => [0..stride, stride..stride],
            };
            for part in parts {
                let first = row as usize * stride + part.start;
                let end = row as usize * stride + part.end;
                outside_mismatches += picture[first..end]
                    .iter()
                    .zip(&corpus.picture[first..end])
                    .filter(|(a, b)| a != b)
                    .count() as u64;
            }
        }
        if let Some(segment) = segment {
            if decoder.as_ref().is_none_or(|(id, _)| *id != segment.index) {
                decoder = Some((segment.index, ClipDecoder::open(&out.join(&segment.file))?));
            }
            let number = line["mv"][1].as_u64().ok_or("missing moving frame")? as u32;
            let decoded = decoder
                .as_mut()
                .ok_or("missing decoder")?
                .1
                .frame_number(number, &segment.key_frames)?;
            let rect = segment.rect;
            let luma = crate::exp_d::luma_of_bgra(
                &decoded.bgra,
                decoded.width as usize,
                rect.w as usize,
                rect.h as usize,
            );
            let source = source_luma(&mut corpus, i, rect)?;
            scores.push(crate::exp_d::ssim(
                &source,
                &luma,
                rect.w as usize,
                rect.h as usize,
            ));
        }
    }
    let mean_ssim = if scores.is_empty() {
        1.0
    } else {
        scores.iter().sum::<f64>() / scores.len() as f64
    };
    let report = json!({ "lines": lines, "segments": segments, "exact": summary.exact,
        "unavailable": summary.moving.unavailable, "grant": summary.moving.grant, "short": summary.moving.short,
        "waited": summary.moving.waited, "creation_ms": summary.moving.creation_ms,
        "outside_mismatches": outside_mismatches, "mean_ssim": mean_ssim, "cpu_ns": cpu_ns, "ram_peak": ram_peak,
        "constants": { "BLOCK": crate::clip_motion::BLOCK, "MOVING_COST_SHARE": crate::clip_motion::MOVING_COST_SHARE,
        "MOVING_HOLD_MS": crate::clip_motion::MOVING_HOLD_MS, "SHORT_LIVED_MS": crate::clip_motion::SHORT_LIVED_MS,
        "ANIMATING_CHANGE_SHARE": crate::clip_motion::ANIMATING_CHANGE_SHARE } });
    serde_json::to_writer(
        File::create(out.join("layers.json")).map_err(|e| e.to_string())?,
        &report,
    )
    .map_err(|e| e.to_string())?;
    if args.get("clipjson").is_some_and(|v| v == "1") {
        metadata(out, &report)?;
    }
    println!("{report}");
    Ok(())
}

fn metadata(folder: &Path, report: &serde_json::Value) -> Result<()> {
    use crate::clip_folder::{
        CLIP_SCHEMA_VERSION, ClipClock, ClipGeometry, ClipJson, ClipState, ClipTargetInfo,
        CommittedSegment, ConditionStatus, Conditions, LossCause, LostInstant, MovingInfo,
        ObserverGrant, StopReason,
    };
    let lines = report["lines"]
        .as_array()
        .ok_or("layers report has no lines")?;
    let first = lines.first().ok_or("layers report is empty")?;
    let width = first["width"].as_u64().ok_or("missing width")? as u32;
    let height = first["height"].as_u64().ok_or("missing height")? as u32;
    let segments = report["segments"]
        .as_array()
        .ok_or("missing segments")?
        .iter()
        .map(|s| {
            CommittedSegment::read_back(
                folder,
                serde_json::from_value(s.clone()).map_err(|e| e.to_string())?,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let recorded_ms = lines.len() as u64 * MILLISECONDS_PER_SECOND / u64::from(PROBE_FPS);
    let mut clip = ClipJson {
        schema_version: CLIP_SCHEMA_VERSION,
        way: "DYN1".into(),
        clip_id: "dynamic-proof".into(),
        synapse_version: env!("CARGO_PKG_VERSION").into(),
        state: ClipState::Ready,
        failure: None,
        stop_reason: Some(StopReason::Completed),
        fps: PROBE_FPS,
        end_ms: recorded_ms,
        recorded_ms,
        target: ClipTargetInfo::default(),
        clock: ClipClock::default(),
        observer: ObserverGrant::default(),
        geometry: ClipGeometry {
            window_width: width,
            window_height: height,
            scale: 1,
            recorded_width: width,
            recorded_height: height,
        },
        lost: lines
            .iter()
            .filter(|l| l["lost"] == "moving_behind")
            .filter_map(|l| l["instant"].as_u64())
            .map(|instant| LostInstant {
                instant,
                cause: LossCause::MovingBehind,
            })
            .collect(),
        exact: Some(
            serde_json::from_value::<crate::clip_exact::ExactSummary>(report["exact"].clone())
                .map_err(|e| e.to_string())?
                .into(),
        ),
        moving: MovingInfo {
            status: if segments.is_empty() {
                "not_used"
            } else {
                "available"
            }
            .into(),
            reason: report["unavailable"].as_str().map(str::to_owned),
            encoder: serde_json::from_value(report["grant"].clone()).map_err(|e| e.to_string())?,
            segments,
        },
        conditions: Conditions {
            t: Some(ConditionStatus {
                status: "held".into(),
                detail: Some(json!({ "bytes": 0, "length_ms": recorded_ms })),
                ..ConditionStatus::default()
            }),
            ..Conditions::default()
        },
    };
    clip.set_total_bytes(folder)?;
    clip.create(folder)
}

fn seek(folder: &Path) -> Result<()> {
    let clip = crate::clip_folder::ClipJson::read(folder)?;
    let segment = clip
        .moving
        .segments
        .first()
        .ok_or("clip has no moving segment")?;
    let path = folder.join(&segment.segment.file);
    let mut decoder = ClipDecoder::open(&path)?;
    let mut hashes = Vec::new();
    use sha2::{Digest, Sha256};
    for number in 0..segment.segment.frames {
        hashes.push(Sha256::digest(
            decoder
                .frame_number(number, &segment.segment.key_frames)?
                .bgra,
        ));
    }
    let mut decoder = ClipDecoder::open(&path)?;
    for number in (0..segment.segment.frames).rev() {
        let frame = decoder.frame_number(number, &segment.segment.key_frames)?;
        if Sha256::digest(&frame.bgra) != hashes[number as usize] {
            let matched = hashes
                .iter()
                .position(|hash| *hash == Sha256::digest(&frame.bgra));
            return Err(format!(
                "random seek frame {number} at {} hns differs from sequential decode; matches {matched:?}",
                frame.t_hns
            ));
        }
    }
    println!(
        "random seek | sequential pixels = reverse seek pixels | 0 mismatches | {} frames | PASS",
        hashes.len()
    );
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
        Some("moving") => moving(folder, out, &args),
        Some("layers") => layers(folder, out, &args),
        Some("metadata") => {
            let report = serde_json::from_reader(
                File::open(folder.join("layers.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            metadata(folder, &report)
        }
        Some("recover") => crate::clip_recovery::ClipRecovery::run(folder),
        Some("seek") => seek(folder),
        _ => Err("exp-dyn requires mode=exact, mode=moving or mode=layers".into()),
    }
}
