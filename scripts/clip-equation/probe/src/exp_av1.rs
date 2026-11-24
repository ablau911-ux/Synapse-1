//! exp-av1: replay a corpus scene through the PRODUCT's recorder pipeline
//! (`crate::clip_stream::ClipStream`, included from crates/synapse-mcp by `#[path]`), so a
//! fix proven on this bench is the fix the app runs (docs/research/window-recording/probe-plan.md).
//!
//!   clip-probe exp-av1 out=<dir> folder=<corpus scene>       replay a corpus scene in real time
//!   a corpus line with `"lost": true` is handed over as a failed capture (`capture_failed`)
//!   accepted for the old command line, only at the product's values:
//!   quality=35, key=200, settle=10
//!
//! Live recording (`hwnd=`) is the product's own path now (screenshot operation=record).
//! Output folder, written by the product:
//!   video/part-NNNN.ivf          AV1; a new part at each key picture; timestamp = instant
//!   stills/still-NNNNNNNN.webp   lossless WebP; the number is the instant it belongs to
//!   frames.jsonl                 one line per instant, in order (may carry `still_of`)
//!   probe.json                   counts, sizes, processor time, peak memory, encoder settings

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::Result;
use crate::clip_encoder::{AV1_QUALITY, KEY_INSTANTS};
use crate::clip_folder::LOST_CAPTURE_FAILED;
use crate::clip_stream::{ClipStream, SETTLE_INSTANTS, StageTime, StreamInput, StreamSummary};
use crate::clock::{self, HNS_PER_MS};
use crate::exp_cpu::Spent;
use crate::exp_dyn::Corpus;
use crate::exp_measure::os_peak_private;
use crate::exp_meter::{Args, process_cpu_ns};
use crate::slots::INSTANT_HNS;

const BGR_BYTES: usize = 3;
const OPAQUE: u8 = u8::MAX;
const HNS_PER_MS_F64: f64 = HNS_PER_MS as f64;

pub fn run() -> Result<()> {
    let args = Args::from_env();
    let out = PathBuf::from(args.text("out")?);
    for (key, product) in [
        ("quality", u64::from(AV1_QUALITY)),
        ("key", KEY_INSTANTS),
        ("settle", SETTLE_INSTANTS),
    ] {
        let given: u64 = args.number(key, product)?;
        if given != product {
            return Err(format!(
                "{key}={given}: exp-av1 records through the product, which uses {key}={product}"
            ));
        }
    }
    if args.text("hwnd").is_ok() {
        return Err("live mode (hwnd=) is the product's path now: use screenshot operation=record".into());
    }
    let folder = args
        .text("folder")
        .map_err(|_| "exp-av1 needs out=<dir> and folder=<corpus scene>".to_owned())?;
    // The scene is read before the recording starts: reading it is not recording.
    let mut corpus = Corpus::open(Path::new(folder))?;
    std::fs::create_dir_all(&out).map_err(|e| format!("create {}: {e}", out.display()))?;
    let started_hns = clock::now_hns();
    let mut stream = ClipStream::create(&out)?;
    let replayed = replay(&mut corpus, &mut stream, started_hns)?;
    let summary = stream.finish().map_err(|failure| failure.error)?;
    report(&out, summary, replayed)
}

/// What the replay itself measured.
struct Replayed {
    /// Reading and decompressing the scene's pictures and widening them to B,G,R,A.
    source: Spent,
    /// From the start of the recording until the first picture was handed to the stream.
    first_encoded_ms: Option<f64>,
    size: (u32, u32),
}

/// Replays a corpus scene: the picture of instant k is handed over at start + k instants.
fn replay(corpus: &mut Corpus, stream: &mut ClipStream, started_hns: i64) -> Result<Replayed> {
    let mut source = Spent::default();
    let mut bgra = Vec::new();
    let mut first_encoded_ms = None;
    let mut size = (0, 0);
    for index in 0..corpus.lines.len() {
        let due = i64::try_from(index).map_err(|_| "corpus too long")? * INSTANT_HNS;
        clock::sleep_until(started_hns + due);
        if corpus.lines[index].lost {
            // A failed capture: no picture is read, the product names the loss.
            let instant = corpus.lines[index].instant;
            stream.instant(instant, StreamInput::Lost(LOST_CAPTURE_FAILED))?;
            continue;
        }
        source.time(|| -> Result<()> {
            corpus.load(index)?;
            bgra.clear();
            bgra.extend(
                corpus
                    .picture
                    .chunks_exact(BGR_BYTES)
                    .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], OPAQUE]),
            );
            Ok(())
        })?;
        let line = &corpus.lines[index];
        let input = StreamInput::Picture {
            bgra: &bgra,
            width: line.width,
            height: line.height,
        };
        stream.instant(line.instant, input)?;
        if first_encoded_ms.is_none() {
            first_encoded_ms = Some((clock::now_hns() - started_hns) as f64 / HNS_PER_MS_F64);
            size = (line.width, line.height);
        }
    }
    Ok(Replayed {
        source,
        first_encoded_ms,
        size,
    })
}

/// probe.json, also printed as the last stdout line. The fields are those of the bench's
/// former own pipeline. `source` is measured here in thread cycles (`QueryThreadCycleTime`,
/// turned into nanoseconds at the process's own ratio of processor time to cycles); the
/// other stages run inside the product and report wall time only (`thread_cpu_ns` null;
/// `encode` runs on the product's encoder thread). `start` has no product counterpart (null).
fn report(out: &Path, summary: StreamSummary, replayed: Replayed) -> Result<()> {
    let cpu_ns = process_cpu_ns();
    let ns_per_cycle = cpu_ns as f64 / clock::process_cycles().max(1) as f64;
    let source_cpu_ns = (replayed.source.thread_cycles as f64 * ns_per_cycle) as u64;
    let times = &summary.times;
    let product = |stage: &StageTime| {
        json!({ "thread_cpu_ns": null, "wall_ns": u64::try_from(stage.total.as_nanos()).unwrap_or(u64::MAX) })
    };
    let stages = json!({
        "source": { "thread_cpu_ns": source_cpu_ns, "wall_ns": replayed.source.wall_ns },
        "compare": product(&times.compare),
        "convert": product(&times.video.convert),
        "encode": product(&times.video.encode),
        "still_encode": product(&times.stills),
        "writes": product(&times.index)
    });
    let report = json!({
        "instants": summary.instants,
        "encoded": summary.encoded,
        "same": summary.same,
        "lost": summary.lost.len(),
        "stills": summary.stills.len(),
        "bytes_video": summary.parts.iter().map(|part| part.bytes).sum::<u64>(),
        "bytes_stills": summary.stills.iter().map(|still| still.bytes).sum::<u64>(),
        "cpu_ns": cpu_ns,
        "stages": stages,
        "other_cpu_ns": cpu_ns.saturating_sub(source_cpu_ns),
        "ns_per_cycle": ns_per_cycle,
        "start": {
            "encoder_open_ms": null,
            "first_convert_ms": null,
            "first_encode_ms": null
        },
        "encoder_threads": summary.encoder.settings["threads"],
        "ram_peak": os_peak_private(),
        "first_encoded_ms": replayed.first_encoded_ms,
        "width": replayed.size.0,
        "height": replayed.size.1,
        "encoder": {
            "name": summary.encoder.name,
            "settings": summary.encoder.settings
        }
    });
    let path = out.join("probe.json");
    std::fs::write(&path, report.to_string())
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("{report}");
    Ok(())
}
