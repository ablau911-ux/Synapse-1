//! The recorder pipeline of plan 5.0 (§2, §3, W2, W3): one input per instant.
//!
//!   picture equal to the picture before  → `same`
//!   picture changed                      → encoded into the AV1 video (`enc`)
//!   on screen for `SETTLE_INSTANTS` in a row, the changed instant included
//!                                        → one lossless still for the changed instant
//!   no picture known                     → `lost`, with its cause
//!   changed, but the encoder queue full  → `lost` (`encoder_behind`)
//!
//! Every instant gets exactly one index line, so N = N_enc + N_same + N_lost.
//! A changed picture is queued for the encoder thread (`VideoQueue`) with its
//! place [part, index] already decided, so this thread never waits for an encode.
//! Moved from the W0 probe (`scripts/clip-equation/probe/src/exp_av1.rs`:
//! `Recorder`, `StillRule`, `Stills`, `Index`), whose checks proved it.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use image_webp::{ColorType, WebPEncoder};

use super::capture_clip::RECORD_FPS;
use super::clip_encoder::{BGRA_BYTES, EncoderSettings, VideoPlace, VideoQueue, encoder_name};
use super::clip_folder::{
    EncoderInfo, FRAMES_FILE, IndexLine, LOST_ENCODER_BEHIND, LOST_NO_PICTURE_YET,
    LOST_WINDOW_RESIZED, LostInstant, PartInfo, STILLS_DIR, StillInfo, still_path,
};

/// `SETTLE_INSTANTS`: 0.5 s on screen, the changed instant included (plan §6).
pub(super) const SETTLE_INSTANTS: u64 = 10;
const MS_PER_SECOND: u64 = 1_000;
const RGB_BYTES: usize = 3;
/// Channel positions in a B,G,R,A pixel.
const BLUE: usize = 0;
const GREEN: usize = 1;
const RED: usize = 2;

/// What the capture knows about one instant.
pub(super) enum StreamInput<'a> {
    /// The window's picture: B,G,R,A rows, top to bottom, tightly packed.
    Picture {
        bgra: &'a [u8],
        width: u32,
        height: u32,
    },
    /// No picture was composed since the last instant, so it shows the previous one.
    Unchanged,
    /// No picture is known for this instant, for the named cause.
    Lost(&'static str),
}

/// What one instant wrote.
pub(super) struct Written {
    /// Bytes written to the clip folder (video the encoder thread wrote since
    /// the instant before, still, index lines).
    pub(super) bytes: u64,
    /// Video parts the encoder thread closed since the instant before, read back.
    pub(super) closed_parts: Vec<PartInfo>,
}

/// What a finished stream hands to `clip.json`.
pub(super) struct StreamSummary {
    pub(super) instants: u64,
    pub(super) encoded: u64,
    pub(super) same: u64,
    pub(super) lost: Vec<LostInstant>,
    /// The parts closed while `finish` drained the encoder queue, the last part included.
    pub(super) parts: Vec<PartInfo>,
    pub(super) stills: Vec<StillInfo>,
    pub(super) encoder: EncoderInfo,
}

/// The decision for one instant.
#[derive(Clone, Copy)]
enum Decision {
    Enc(VideoPlace),
    Same,
    Lost(&'static str),
}

/// The last picture handed to the video.
struct Shown {
    bgra: Vec<u8>,
    width: u32,
    height: u32,
}

/// Owns the rule "changed → encode, equal → same": per instant it decides
/// `enc`, `same` or `lost` by comparing pixels with the picture before,
/// queues changed pictures for the encoder, writes due stills and passes the
/// line to the index.
pub(super) struct ClipStream {
    folder: PathBuf,
    settings: EncoderSettings,
    next_instant: u64,
    /// Starts with the first changed picture.
    video: Option<VideoQueue>,
    shown: Option<Shown>,
    previous_lost: Option<&'static str>,
    still_rule: StillRule,
    stills: Stills,
    index: Index,
    encoded: u64,
    same: u64,
    lost: Vec<LostInstant>,
}

impl ClipStream {
    /// Creates `stills/` and `frames.jsonl` in the clip folder; the video
    /// part files start with the first changed picture.
    pub(super) fn create(folder: &Path) -> Result<Self, String> {
        Ok(Self {
            folder: folder.to_owned(),
            settings: EncoderSettings::standard(),
            next_instant: 0,
            video: None,
            shown: None,
            previous_lost: None,
            still_rule: StillRule::new(SETTLE_INSTANTS),
            stills: Stills::create(folder)?,
            index: Index::create(&folder.join(FRAMES_FILE))?,
            encoded: 0,
            same: 0,
            lost: Vec::new(),
        })
    }

    /// The encoder's name and settings for `clip.json`.
    pub(super) fn encoder_info(&self) -> EncoderInfo {
        EncoderInfo {
            name: encoder_name(),
            settings: self.settings.describe(),
        }
    }

    /// Takes the input of `instant`; instants arrive in order from 0.
    pub(super) fn instant(&mut self, instant: u64, input: StreamInput) -> Result<Written, String> {
        if instant != self.next_instant {
            return Err(format!(
                "instant {instant} arrived, {} expected",
                self.next_instant
            ));
        }
        self.next_instant += 1;
        let mut written = Written {
            bytes: 0,
            closed_parts: Vec::new(),
        };
        let decision = self.decide(instant, input)?;
        if let Some(video) = &self.video {
            // An encoder error reaches the recording here and stops it.
            let video_written = video.written()?;
            written.bytes += video_written.bytes;
            written.closed_parts = video_written.closed_parts;
        }
        match decision {
            Decision::Enc(_) => self.encoded += 1,
            Decision::Same => self.same += 1,
            Decision::Lost(cause) => self.lost.push(LostInstant {
                instant,
                cause: cause.to_owned(),
            }),
        }
        self.previous_lost = match decision {
            Decision::Lost(cause) => Some(cause),
            _ => None,
        };
        self.index.add(line_of(instant, decision));
        if let Some(settled) = self.still_rule.observe(instant, decision) {
            // Every instant since `settled` was `same`, so the shown picture is its picture.
            let shown = self
                .shown
                .as_ref()
                .ok_or_else(|| "a still is due before any picture".to_owned())?;
            written.bytes += self.stills.write(settled, shown)?;
            self.index.mark_still(settled)?;
        }
        written.bytes += self.index.write_decided(self.still_rule.open())?;
        Ok(written)
    }

    fn decide(&mut self, instant: u64, input: StreamInput) -> Result<Decision, String> {
        match input {
            StreamInput::Lost(cause) => Ok(Decision::Lost(cause)),
            StreamInput::Unchanged => Ok(match (self.previous_lost, &self.shown) {
                // The picture before was not known, so this one is not either.
                (Some(cause), _) => Decision::Lost(cause),
                (None, Some(_)) => Decision::Same,
                (None, None) => Decision::Lost(LOST_NO_PICTURE_YET),
            }),
            StreamInput::Picture {
                bgra,
                width,
                height,
            } => {
                if bgra.len() != width as usize * height as usize * BGRA_BYTES {
                    return Err(format!(
                        "instant {instant}: picture has {} bytes for {width}x{height}",
                        bgra.len()
                    ));
                }
                let same = self.shown.as_ref().is_some_and(|shown| {
                    (shown.width, shown.height) == (width, height) && shown.bgra == bgra
                });
                if same {
                    return Ok(Decision::Same);
                }
                if self
                    .video
                    .as_ref()
                    .is_some_and(|video| video.size() != (width, height))
                {
                    return Ok(Decision::Lost(LOST_WINDOW_RESIZED));
                }
                Ok(match self.queue(instant, bgra, width, height)? {
                    Some(place) => Decision::Enc(place),
                    None => Decision::Lost(LOST_ENCODER_BEHIND),
                })
            }
        }
    }

    /// Queues the changed picture for the encoder; returns its place, or
    /// `None` when the queue is full (the shown picture stays the one before).
    fn queue(
        &mut self,
        instant: u64,
        bgra: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Option<VideoPlace>, String> {
        let video = match &mut self.video {
            Some(video) => video,
            None => self.video.insert(VideoQueue::start(
                &self.folder,
                width,
                height,
                self.settings,
            )?),
        };
        let Some(place) = video.submit(instant, bgra)? else {
            return Ok(None);
        };
        let shown = self.shown.get_or_insert_with(|| Shown {
            bgra: Vec::new(),
            width,
            height,
        });
        shown.bgra.clear();
        shown.bgra.extend_from_slice(bgra);
        (shown.width, shown.height) = (width, height);
        Ok(Some(place))
    }

    /// Writes the waiting index lines (an open still is not written: the
    /// recording ended before its picture settled), lets the encoder thread
    /// encode every queued picture and closes the last part. An encoder error
    /// is returned, so the clip fails instead of turning ready.
    pub(super) fn finish(mut self) -> Result<StreamSummary, String> {
        self.index.write_decided(None)?;
        let encoder = self.encoder_info();
        let parts = match self.video.take() {
            Some(video) => video.finish()?.closed_parts,
            None => Vec::new(),
        };
        Ok(StreamSummary {
            instants: self.next_instant,
            encoded: self.encoded,
            same: self.same,
            lost: self.lost,
            parts,
            stills: self.stills.written,
            encoder,
        })
    }
}

/// The index line of a decided instant (its still is marked later).
fn line_of(instant: u64, decision: Decision) -> IndexLine {
    let mut line = IndexLine {
        instant,
        t_ms: instant * MS_PER_SECOND / u64::from(RECORD_FPS),
        enc: None,
        same: None,
        lost: None,
        still: None,
    };
    match decision {
        Decision::Enc(place) => line.enc = Some(place),
        Decision::Same => line.same = Some(true),
        Decision::Lost(cause) => line.lost = Some(cause.to_owned()),
    }
    line
}

/// The still rule (plan §3): instant k gets a still when its picture changed
/// from instant k − 1 (instant 0 counts as a change) and stays on screen for
/// `settle` instants in a row, k included. The still is written at instant
/// k + settle − 1 and belongs to instant k. A lost instant ends the run; a
/// recording that ends first writes no still for k.
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
    fn observe(&mut self, instant: u64, decision: Decision) -> Option<u64> {
        match decision {
            Decision::Enc(_) => self.candidate = Some((instant, 1)),
            Decision::Same => {
                if let Some((_, shown)) = self.candidate.as_mut() {
                    *shown += 1;
                }
            }
            Decision::Lost(_) => self.candidate = None,
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
    written: Vec<StillInfo>,
}

impl Stills {
    fn create(folder: &Path) -> Result<Self, String> {
        let stills = folder.join(STILLS_DIR);
        std::fs::create_dir_all(&stills)
            .map_err(|error| format!("could not create {}: {error}", stills.display()))?;
        Ok(Self {
            folder: folder.to_owned(),
            rgb: Vec::new(),
            written: Vec::new(),
        })
    }

    /// Writes the still of `instant`; returns its bytes.
    fn write(&mut self, instant: u64, picture: &Shown) -> Result<u64, String> {
        self.rgb.clear();
        self.rgb.extend(
            picture
                .bgra
                .chunks_exact(BGRA_BYTES)
                .flat_map(|pixel| [pixel[RED], pixel[GREEN], pixel[BLUE]]),
        );
        let mut encoded =
            Vec::with_capacity(picture.width as usize * picture.height as usize * RGB_BYTES);
        WebPEncoder::new(&mut encoded)
            .encode(&self.rgb, picture.width, picture.height, ColorType::Rgb8)
            .map_err(|error| format!("WebP still of instant {instant}: {error}"))?;
        let path = still_path(&self.folder, instant);
        std::fs::write(&path, &encoded)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        let bytes = encoded.len() as u64;
        self.written
            .push(StillInfo::new(&self.folder, instant, bytes));
        Ok(bytes)
    }
}

/// `frames.jsonl`: one line per instant, in order. A line waits while its
/// own or an earlier instant's still is undecided (at most `SETTLE_INSTANTS`
/// lines); every decided line goes straight to the file (unbuffered), so a
/// killed recorder keeps it. An `enc` line can reach the file before the
/// encoder thread wrote its picture; recovery names such a line lost.
struct Index {
    file: File,
    waiting: VecDeque<IndexLine>,
}

impl Index {
    fn create(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| format!("could not create {}: {error}", path.display()))?;
        Ok(Self {
            file,
            waiting: VecDeque::new(),
        })
    }

    fn add(&mut self, line: IndexLine) {
        self.waiting.push_back(line);
    }

    fn mark_still(&mut self, instant: u64) -> Result<(), String> {
        let line = self
            .waiting
            .iter_mut()
            .find(|line| line.instant == instant)
            .ok_or_else(|| format!("index line {instant} was written before its still"))?;
        line.still = Some(true);
        Ok(())
    }

    /// Writes every waiting line before `open` (all of them when nothing is
    /// open); returns the bytes written.
    fn write_decided(&mut self, open: Option<u64>) -> Result<u64, String> {
        let mut bytes = 0;
        while let Some(line) = self.waiting.front() {
            if open.is_some_and(|open| line.instant >= open) {
                break;
            }
            let text = line.text()?;
            self.file
                .write_all(text.as_bytes())
                .map_err(|error| format!("could not write {FRAMES_FILE}: {error}"))?;
            bytes += text.len() as u64;
            self.waiting.pop_front();
        }
        Ok(bytes)
    }
}
