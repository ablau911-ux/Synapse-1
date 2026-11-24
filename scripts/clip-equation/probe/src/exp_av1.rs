//! exp-av1: record one window as AV1 video plus one lossless still each time it settles
//! (docs/research/window-recording/probe-plan.md, parts 2 to 4 and "Probe interface").
//!
//!   clip-probe exp-av1 out=<dir> folder=<corpus scene>       replay a corpus scene in real time
//!   clip-probe exp-av1 out=<dir> seconds=<n> hwnd=<handle>   record a live window
//!   optional: quality=<n> (35), key=<instants> (200), settle=<instants> (10)
//!
//! Both modes hand `Recorder` one input per instant (20 per second). `Recorder` decides
//! `enc`, `same` or `lost`, encodes changed pictures, asks `StillRule` whether a still is
//! due, and hands the line to `Index`. Output folder:
//!   video/part-NNNN.ivf          AV1; a new part at each key picture; timestamp = instant
//!   stills/still-NNNNNNNN.webp   lossless WebP; the number is the instant it belongs to
//!   frames.jsonl                 one line per instant, in order
//!   probe.json                   counts, sizes, processor time, peak memory, encoder settings

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use image_webp::{ColorType, WebPEncoder};
use serde_json::json;

use crate::Result;
use crate::clock::{self, HNS_PER_MS};
use crate::exp_av1_live;
use crate::exp_av1_video::{EncoderSettings, Video, VideoPlace, libaom_version};
use crate::exp_dyn::Corpus;
use crate::exp_measure::os_peak_private;
use crate::exp_meter::{Args, process_cpu_ns};
use crate::slots::INSTANT_HNS;

const DEFAULT_QUALITY: u32 = 35;
const MAX_QUALITY: u32 = 63;
/// A key picture every 10 s (plan, Builder notes, part 1: `key=200`).
const DEFAULT_KEY_INSTANTS: u64 = 200;
/// 0.5 s on screen, the changed instant included.
const DEFAULT_SETTLE_INSTANTS: u64 = 10;
const INSTANT_MS: u64 = (INSTANT_HNS / HNS_PER_MS) as u64;
const BGR_BYTES: usize = 3;
const HNS_PER_MS_F64: f64 = HNS_PER_MS as f64;

/// Loss causes named in the index.
const LOST_NO_PICTURE_YET: &str = "no_picture_yet";
const LOST_WINDOW_RESIZED: &str = "window_resized";

pub fn run() -> Result<()> {
    let args = Args::from_env();
    let out = PathBuf::from(args.text("out")?);
    let quality = args.number("quality", DEFAULT_QUALITY)?;
    let key_instants = args.number("key", DEFAULT_KEY_INSTANTS)?;
    let settle = args.number("settle", DEFAULT_SETTLE_INSTANTS)?;
    if quality > MAX_QUALITY || key_instants == 0 || settle == 0 {
        return Err(format!(
            "quality must be 0..={MAX_QUALITY}, key and settle at least 1"
        ));
    }
    let settings = EncoderSettings::new(quality, key_instants);
    match (args.text("folder"), args.text("hwnd")) {
        (Ok(folder), Err(_)) => {
            // The scene is read before the recording starts: reading it is not recording.
            let mut corpus = Corpus::open(Path::new(folder))?;
            let mut recorder = Recorder::create(&out, settings, settle)?;
            replay(&mut corpus, &mut recorder)?;
            report(&out, recorder.finish()?, settings)
        }
        (Err(_), Ok(hwnd)) => {
            let hwnd = parse_handle(hwnd)?;
            let seconds: u64 = args.number("seconds", 0)?;
            if seconds == 0 {
                return Err("live mode needs seconds=<n> of at least 1".into());
            }
            let mut recorder = Recorder::create(&out, settings, settle)?;
            exp_av1_live::record(hwnd, seconds, &mut recorder)?;
            report(&out, recorder.finish()?, settings)
        }
        _ => Err("exp-av1 needs out= and either folder=<corpus scene> or hwnd=<handle> seconds=<n>".into()),
    }
}

/// A window handle, decimal or 0x-prefixed hexadecimal.
fn parse_handle(text: &str) -> Result<isize> {
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => isize::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.map_err(|_| format!("hwnd={text} is not a window handle"))
}

/// Replays a corpus scene: the picture of instant k is handed over at start + k instants.
fn replay(corpus: &mut Corpus, recorder: &mut Recorder) -> Result<()> {
    let start = clock::now_hns();
    for index in 0..corpus.lines.len() {
        let due = i64::try_from(index).map_err(|_| "corpus too long")? * INSTANT_HNS;
        clock::sleep_until(start + due);
        corpus.load(index)?;
        let line = &corpus.lines[index];
        let input = Input::Picture {
            bgr: &corpus.picture,
            width: line.width,
            height: line.height,
        };
        recorder.instant(line.instant, input)?;
    }
    Ok(())
}

/// What a source knows about one instant.
pub enum Input<'a> {
    /// The window's picture: B,G,R rows, top to bottom.
    Picture { bgr: &'a [u8], width: u32, height: u32 },
    /// No picture was composed since the last instant, so the picture is the previous one.
    Unchanged,
    /// No picture is known for this instant, for the named cause.
    Lost(&'static str),
}

/// What the index says about one instant.
#[derive(Clone, Copy)]
enum Kind {
    Enc(VideoPlace),
    Same,
    Lost(&'static str),
}

/// The last picture handed to the video.
struct Shown {
    bgr: Vec<u8>,
    width: u32,
    height: u32,
}

/// The pipeline both modes feed: per instant it decides `enc`, `same` or `lost` by comparing
/// pixels with the picture before, encodes, writes due stills and passes the line to the index.
pub struct Recorder {
    folder: PathBuf,
    settings: EncoderSettings,
    started_hns: i64,
    next_instant: u64,
    video: Option<Video>,
    shown: Option<Shown>,
    previous_lost: Option<&'static str>,
    still_rule: StillRule,
    stills: Stills,
    index: Index,
    totals: Totals,
}

/// Counts and sizes for probe.json.
#[derive(Default)]
struct Totals {
    encoded: u64,
    same: u64,
    lost: u64,
    first_encoded_ms: Option<f64>,
}

/// What a finished recording reports.
struct Summary {
    instants: u64,
    totals: Totals,
    stills: u64,
    bytes_stills: u64,
    bytes_video: u64,
    size: (u32, u32),
}

impl Recorder {
    /// Opens the output folder; the recording starts now (`first_encoded_ms` counts from here).
    fn create(folder: &Path, settings: EncoderSettings, settle: u64) -> Result<Self> {
        let started_hns = clock::now_hns();
        std::fs::create_dir_all(folder)
            .map_err(|e| format!("create {}: {e}", folder.display()))?;
        Ok(Self {
            folder: folder.to_owned(),
            settings,
            started_hns,
            next_instant: 0,
            video: None,
            shown: None,
            previous_lost: None,
            still_rule: StillRule::new(settle),
            stills: Stills::create(&folder.join("stills"))?,
            index: Index::create(&folder.join("frames.jsonl"))?,
            totals: Totals::default(),
        })
    }

    /// Takes the input of `instant`; instants arrive in order from 0.
    pub fn instant(&mut self, instant: u64, input: Input) -> Result<()> {
        if instant != self.next_instant {
            return Err(format!(
                "instant {instant} arrived, {} expected",
                self.next_instant
            ));
        }
        self.next_instant += 1;
        let kind = self.decide(instant, input)?;
        match kind {
            Kind::Enc(_) => self.totals.encoded += 1,
            Kind::Same => self.totals.same += 1,
            Kind::Lost(_) => self.totals.lost += 1,
        }
        self.previous_lost = match kind {
            Kind::Lost(cause) => Some(cause),
            _ => None,
        };
        self.index.add(Line {
            instant,
            kind,
            still: false,
        });
        if let Some(settled) = self.still_rule.observe(instant, kind) {
            // Every instant since `settled` was `same`, so the shown picture is its picture.
            let shown = self.shown.as_ref().ok_or("a still is due before any picture")?;
            self.stills.write(settled, shown)?;
            self.index.mark_still(settled)?;
        }
        self.index.write_decided(self.still_rule.open())
    }

    fn decide(&mut self, instant: u64, input: Input) -> Result<Kind> {
        match input {
            Input::Lost(cause) => Ok(Kind::Lost(cause)),
            Input::Unchanged => Ok(match (self.previous_lost, &self.shown) {
                // The picture before was not known, so this one is not either.
                (Some(cause), _) => Kind::Lost(cause),
                (None, Some(_)) => Kind::Same,
                (None, None) => Kind::Lost(LOST_NO_PICTURE_YET),
            }),
            Input::Picture { bgr, width, height } => {
                if bgr.len() != width as usize * height as usize * BGR_BYTES {
                    return Err(format!(
                        "instant {instant}: picture has {} bytes for {width}x{height}",
                        bgr.len()
                    ));
                }
                if let Some(shown) = &self.shown
                    && (shown.width, shown.height) == (width, height)
                    && shown.bgr == bgr
                {
                    return Ok(Kind::Same);
                }
                if self
                    .video
                    .as_ref()
                    .is_some_and(|video| video.size() != (width, height))
                {
                    return Ok(Kind::Lost(LOST_WINDOW_RESIZED));
                }
                self.encode(instant, bgr, width, height).map(Kind::Enc)
            }
        }
    }

    fn encode(&mut self, instant: u64, bgr: &[u8], width: u32, height: u32) -> Result<VideoPlace> {
        let video = match &mut self.video {
            Some(video) => video,
            None => self.video.insert(Video::create(
                &self.folder.join("video"),
                width,
                height,
                self.settings,
            )?),
        };
        let place = video.encode(instant, bgr)?;
        if self.totals.first_encoded_ms.is_none() {
            self.totals.first_encoded_ms =
                Some((clock::now_hns() - self.started_hns) as f64 / HNS_PER_MS_F64);
        }
        let shown = self.shown.get_or_insert_with(|| Shown {
            bgr: Vec::new(),
            width,
            height,
        });
        shown.bgr.clear();
        shown.bgr.extend_from_slice(bgr);
        (shown.width, shown.height) = (width, height);
        Ok(place)
    }

    /// Writes the waiting index lines (an open still is not written) and closes the video.
    fn finish(mut self) -> Result<Summary> {
        self.index.write_decided(None)?;
        let size = self.video.as_ref().map_or((0, 0), Video::size);
        let bytes_video = match self.video.take() {
            Some(video) => video.finish()?,
            None => 0,
        };
        Ok(Summary {
            instants: self.next_instant,
            totals: self.totals,
            stills: self.stills.count,
            bytes_stills: self.stills.bytes,
            bytes_video,
            size,
        })
    }
}

/// The still rule: instant k gets a still when its picture changed from instant k − 1
/// (instant 0 counts as a change) and stays on screen for `settle` instants in a row,
/// k included. The still is written at instant k + settle − 1 and belongs to instant k.
/// A lost instant ends the run; a recording that ends first writes no still for k.
struct StillRule {
    settle: u64,
    /// The changed instant waiting for its still, and the instants it has been on screen.
    candidate: Option<(u64, u64)>,
}

impl StillRule {
    const fn new(settle: u64) -> Self {
        Self {
            settle,
            candidate: None,
        }
    }

    /// Takes the decision of `instant`; returns the instant whose still is due now.
    fn observe(&mut self, instant: u64, kind: Kind) -> Option<u64> {
        match kind {
            Kind::Enc(_) => self.candidate = Some((instant, 1)),
            Kind::Same => {
                if let Some((_, shown)) = self.candidate.as_mut() {
                    *shown += 1;
                }
            }
            Kind::Lost(_) => self.candidate = None,
        }
        match self.candidate {
            Some((changed, shown)) if shown >= self.settle => {
                self.candidate = None;
                Some(changed)
            }
            _ => None,
        }
    }

    /// The changed instant whose still is not decided yet.
    fn open(&self) -> Option<u64> {
        self.candidate.map(|(changed, _)| changed)
    }
}

/// Lossless WebP stills of the whole window.
struct Stills {
    folder: PathBuf,
    rgb: Vec<u8>,
    count: u64,
    bytes: u64,
}

impl Stills {
    fn create(folder: &Path) -> Result<Self> {
        std::fs::create_dir_all(folder)
            .map_err(|e| format!("create {}: {e}", folder.display()))?;
        Ok(Self {
            folder: folder.to_owned(),
            rgb: Vec::new(),
            count: 0,
            bytes: 0,
        })
    }

    fn write(&mut self, instant: u64, picture: &Shown) -> Result<()> {
        self.rgb.clear();
        self.rgb.extend(
            picture
                .bgr
                .chunks_exact(BGR_BYTES)
                .flat_map(|pixel| [pixel[2], pixel[1], pixel[0]]),
        );
        let mut encoded = Vec::new();
        WebPEncoder::new(&mut encoded)
            .encode(&self.rgb, picture.width, picture.height, ColorType::Rgb8)
            .map_err(|e| format!("WebP still of instant {instant}: {e}"))?;
        let path = self.folder.join(format!("still-{instant:08}.webp"));
        std::fs::write(&path, &encoded).map_err(|e| format!("write {}: {e}", path.display()))?;
        self.count += 1;
        self.bytes += encoded.len() as u64;
        Ok(())
    }
}

/// One line of frames.jsonl.
struct Line {
    instant: u64,
    kind: Kind,
    still: bool,
}

impl Line {
    fn text(&self) -> String {
        let mut line = json!({ "instant": self.instant, "t_ms": self.instant * INSTANT_MS });
        match self.kind {
            Kind::Enc((part, index)) => line["enc"] = json!([part, index]),
            Kind::Same => line["same"] = json!(true),
            Kind::Lost(cause) => line["lost"] = json!(cause),
        }
        if self.still {
            line["still"] = json!(true);
        }
        format!("{line}\n")
    }
}

/// frames.jsonl: one line per instant, in order. A line waits while its own or an earlier
/// instant's still is undecided (at most `settle` lines); every decided line goes straight
/// to the file, so a killed run keeps it.
struct Index {
    file: File,
    waiting: VecDeque<Line>,
}

impl Index {
    fn create(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| format!("create {}: {e}", path.display()))?;
        Ok(Self {
            file,
            waiting: VecDeque::new(),
        })
    }

    fn add(&mut self, line: Line) {
        self.waiting.push_back(line);
    }

    fn mark_still(&mut self, instant: u64) -> Result<()> {
        let line = self
            .waiting
            .iter_mut()
            .find(|line| line.instant == instant)
            .ok_or_else(|| format!("index line {instant} was written before its still"))?;
        line.still = true;
        Ok(())
    }

    /// Writes every waiting line before `open` (all of them when nothing is open).
    fn write_decided(&mut self, open: Option<u64>) -> Result<()> {
        while let Some(line) = self.waiting.front() {
            if open.is_some_and(|open| line.instant >= open) {
                break;
            }
            self.file
                .write_all(line.text().as_bytes())
                .map_err(|e| format!("write frames.jsonl: {e}"))?;
            self.waiting.pop_front();
        }
        Ok(())
    }
}

/// probe.json, also printed as the last stdout line.
fn report(out: &Path, summary: Summary, settings: EncoderSettings) -> Result<()> {
    let report = json!({
        "instants": summary.instants,
        "encoded": summary.totals.encoded,
        "same": summary.totals.same,
        "lost": summary.totals.lost,
        "stills": summary.stills,
        "bytes_video": summary.bytes_video,
        "bytes_stills": summary.bytes_stills,
        "cpu_ns": process_cpu_ns(),
        "ram_peak": os_peak_private(),
        "first_encoded_ms": summary.totals.first_encoded_ms,
        "width": summary.size.0,
        "height": summary.size.1,
        "encoder": {
            "name": format!("libaom {}", libaom_version()),
            "settings": settings.describe()
        }
    });
    let path = out.join("probe.json");
    std::fs::write(&path, report.to_string())
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("{report}");
    Ok(())
}
