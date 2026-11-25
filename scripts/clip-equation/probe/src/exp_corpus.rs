//! Experiments step 1b (docs/clip-recorder-experiments-plan.md, F6, F7 and
//! "Procedure" step 1): the fixed corpus every configuration encodes later.
//!
//! For each scenario page and each F6 size, the probe opens the page in its
//! own Chrome `--app` window behind the owner's windows, captures t = 20 s at
//! f = 20 instants with Windows Graphics Capture and closes the window again.
//! Instant j covers composition times in (s_{j-1}, s_j], s_j = (k0 + j) · 50 ms
//! on the performance counter; its frame is the last one composed at or
//! before s_j (the product's rule), and it is new when composed inside the
//! instant. The page gets `?start=` so that its step j begins PAGE_LEAD
//! before s_j: every capture of a page sees the same step at the same instant.
//!
//! Output per capture, in `C:\Users\Adam\clip-corpus\<scenario>-<w>x<h>[suffix]\`:
//!   frame-NNNN.zst  zstd level −3 of the frame's B,G,R bytes, rows top to
//!                   bottom, w · h · 3 bytes; one file per new frame
//!   frames.jsonl    one line per instant: instant, frame, new, lost, size,
//!                   zstd bytes, FNV-1a 64 hash of B,G,R, flash box lit
//!   row.md          this capture's row of results/corpus.md
//! The frames are stored at the window size; σ is recorded beside them and
//! the encode step scales (the probe has no σ upscaler for B,G,R).
//!
//! `clip-probe exp-corpus [--suffix S] [name ...]`, names like `typing-1280x720`;
//! without names every scenario at every size.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;

use crate::Result;
use crate::chrome_window::ChromeWindow;
use crate::clock::{self, HNS_PER_MS};
use crate::compress::DisplayAwake;
use crate::exp_encoders::results_folder;
use crate::gpu::{MapWait, ReadBack};
use crate::observer::{self, Interval, composition_time};
use crate::slots::INSTANT_HNS;

/// N_inst = t · f = 20 s · 20.
const INSTANTS: i64 = 400;
/// From the window opening to instant 0: Chrome start plus page load.
const SETTLE_MS: i64 = 6000;
/// The capture starts this long before instant 0, so instant 0 has a frame.
const CAPTURE_LEAD_MS: i64 = 1000;
/// Page step j begins this long before s_j; composed one to three refreshes
/// later (144 Hz here), it lands inside instant j.
const PAGE_LEAD_MS: i64 = 40;
/// The waiting coder of the product (package 0b): zstd level −3.
const ZSTD_LEVEL: i32 = -3;
/// Package 0 result: 11 ms is the requested update interval (M3).
const REQUESTED_INTERVAL_MS: f64 = 11.0;
const CORPUS_ROOT: &str = r"C:\Users\Adam\clip-corpus";
/// The flash page's flash colour #ff2050 as B, G, R, and the lit-pixel floor.
const FLASH_BGR: [u8; 3] = [0x50, 0x20, 0xff];
const FLASH_MIN_PIXELS: usize = 1000;

/// (name, page, window title) of the F7 scenarios.
const SCENARIOS: [(&str, &str, &str); 6] = [
    ("typing", "typing-page.html", "clip0b typing"),
    ("flash", "flash-page.html", "Clip proof page"),
    ("scroll", "scroll-page.html", "clip0b scroll"),
    ("video", "video-page.html", "clip0b video"),
    ("still", "still-page.html", "clip0b still"),
    ("ui", "ui-page.html", "clip ui demo"),
];

/// F6: window width, height and σ (recorded size = σ · window size).
const SIZES: [(u32, u32, u32); 4] = [
    (1920, 1080, 1),
    (1280, 720, 1),
    (640, 360, 2),
    (506, 333, 3),
];

/// A Chrome `--app` window has no invisible resize borders: Windows Graphics
/// Capture delivers its outer size (measured: 1280 x 720 outer gave 1280 x 720
/// after 1294 x 727 outer gave 1294 x 727), so the window is opened at the F6 size.

pub fn run() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(2).collect();
    let mut suffix = String::new();
    if let Some(at) = args.iter().position(|a| a == "--suffix") {
        suffix = args.get(at + 1).cloned().ok_or("--suffix needs a value")?;
        args.drain(at..=at + 1);
    }
    let _awake = DisplayAwake::request();
    let pages = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("no pages folder")?
        .to_path_buf();
    for (scenario, page, title) in SCENARIOS {
        for (width, height, sigma) in SIZES {
            let name = format!("{scenario}-{width}x{height}");
            if !args.is_empty() && !args.contains(&name) {
                continue;
            }
            let folder = Path::new(CORPUS_ROOT).join(format!("{name}{suffix}"));
            let capture = Capture {
                scenario,
                page,
                title,
                width,
                height,
                sigma,
            };
            match capture.run(&pages, &folder) {
                Ok(row) => println!("exp-corpus {row}"),
                Err(error) => println!("exp-corpus {name}{suffix} error={error}"),
            }
        }
    }
    write_summary()
}

struct Capture {
    scenario: &'static str,
    page: &'static str,
    title: &'static str,
    width: u32,
    height: u32,
    sigma: u32,
}

/// One new frame handed from the capture thread to the writer.
pub struct Kept {
    instant: i64,
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

/// What the writer stored for one new frame.
struct Stored {
    instant: i64,
    width: u32,
    height: u32,
    bytes: u64,
    hash: u64,
    flash: bool,
}

impl Capture {
    fn run(&self, pages: &Path, folder: &Path) -> Result<String> {
        prepare_folder(folder)?;
        // Instant grid and the page's start on the Unix clock.
        let unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis() as i64;
        let now = clock::now_hns();
        let k0 = (now + SETTLE_MS * HNS_PER_MS).div_euclid(INSTANT_HNS) + 1;
        let s0 = k0 * INSTANT_HNS;
        let page_start_unix = unix_ms + (s0 - PAGE_LEAD_MS * HNS_PER_MS - now) / HNS_PER_MS;
        let outer = (self.width as i32, self.height as i32);
        let position = if self.width >= 1920 { (0, 0) } else { (40, 40) };
        let window = ChromeWindow::open(
            &pages.join(self.page),
            &format!("?start={page_start_unix}"),
            self.title,
            position,
            outer,
        )?;
        let capture_from = s0 - CAPTURE_LEAD_MS * HNS_PER_MS;
        if clock::now_hns() > capture_from {
            return Err("the window opened too late for instant 0".to_owned());
        }
        clock::sleep_until(capture_from);
        let (sender, receiver) = channel();
        let folder_for_writer = folder.to_path_buf();
        let flash_test = self.scenario == "flash";
        let writer =
            std::thread::spawn(move || write_frames(&receiver, &folder_for_writer, flash_test));
        let control = observer::start_with_cursor::<CorpusFrames>(
            window.raw_hwnd(),
            Interval::Millis(REQUESTED_INTERVAL_MS),
            false,
            (k0, sender),
        )?;
        clock::sleep_until(s0 + INSTANTS * INSTANT_HNS + 300 * HNS_PER_MS);
        let handler = observer::stop(control)?;
        drop(window);
        let delivered = {
            let mut handler = handler.lock();
            handler.finish();
            handler.delivered
        };
        let stored = writer
            .join()
            .map_err(|_| "writer thread panicked".to_owned())??;
        self.write_index(folder, &stored, delivered)
    }

    /// frames.jsonl, row.md; returns the row.
    fn write_index(&self, folder: &Path, stored: &[Stored], delivered: u32) -> Result<String> {
        let mut lines = String::new();
        let (mut new, mut repeat, mut lost) = (0_usize, 0_usize, 0_usize);
        let mut next = 0;
        let mut last: Option<usize> = None;
        for instant in 0..INSTANTS {
            if stored.get(next).is_some_and(|s| s.instant == instant) {
                last = Some(next);
                next += 1;
                new += 1;
            } else if last.is_some() {
                repeat += 1;
            } else {
                lost += 1;
            }
            match last {
                Some(index) => {
                    let s = &stored[index];
                    let _ = writeln!(
                        lines,
                        "{{\"instant\": {instant}, \"frame\": {index}, \"new\": {}, \"lost\": false, \"width\": {}, \"height\": {}, \"zstd_bytes\": {}, \"hash\": \"{:016x}\", \"flash\": {}}}",
                        s.instant == instant,
                        s.width,
                        s.height,
                        s.bytes,
                        s.hash,
                        s.flash
                    );
                }
                None => {
                    let _ = writeln!(
                        lines,
                        "{{\"instant\": {instant}, \"frame\": null, \"new\": false, \"lost\": true}}"
                    );
                }
            }
        }
        std::fs::write(folder.join("frames.jsonl"), lines)
            .map_err(|e| format!("write frames.jsonl: {e}"))?;
        let recorded = stored.first().map_or((0, 0), |s| (s.width, s.height));
        let total: u64 = stored.iter().map(|s| s.bytes).sum();
        let per_frame = if stored.is_empty() {
            0.0
        } else {
            total as f64 / stored.len() as f64
        };
        let seconds = INSTANTS as f64 / 20.0;
        let flashes = if self.scenario == "flash" {
            // One flash per run of consecutive new frames with the box lit.
            let mut count = 0;
            let mut previous: Option<&Stored> = None;
            for s in stored {
                if s.flash && !previous.is_some_and(|p| p.flash && p.instant + 1 == s.instant) {
                    count += 1;
                }
                previous = Some(s);
            }
            count.to_string()
        } else {
            "-".to_owned()
        };
        let row = format!(
            "| {} | {}x{} | {}x{} | {}x{} | {} | {} | {} | {} | {:.2} | {:.0} | {} | {} | {} | {} |",
            self.scenario,
            self.width,
            self.height,
            recorded.0,
            recorded.1,
            recorded.0 * self.sigma,
            recorded.1 * self.sigma,
            self.sigma,
            INSTANTS,
            new,
            repeat,
            new as f64 / seconds,
            per_frame,
            total,
            lost,
            flashes,
            delivered
        );
        let name = folder
            .file_name()
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        std::fs::write(folder.join("row.md"), format!("{name}\t{row}\n"))
            .map_err(|e| format!("write row.md: {e}"))?;
        Ok(format!("{name} {row}"))
    }
}

/// Empties a corpus folder of this probe's own files (frames, index, row).
fn prepare_folder(folder: &Path) -> Result<()> {
    std::fs::create_dir_all(folder).map_err(|e| format!("create {}: {e}", folder.display()))?;
    for entry in std::fs::read_dir(folder)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if (name.starts_with("frame-") && name.ends_with(".zst"))
            || name == "frames.jsonl"
            || name == "row.md"
        {
            std::fs::remove_file(entry.path()).map_err(|e| format!("remove {name}: {e}"))?;
        }
    }
    Ok(())
}

/// The writer thread: B,G,R from B,G,R,A, zstd −3, one file per new frame.
fn write_frames(receiver: &Receiver<Kept>, folder: &Path, flash_test: bool) -> Result<Vec<Stored>> {
    let mut stored = Vec::new();
    let mut bgr = Vec::new();
    for kept in receiver {
        bgr.clear();
        bgr.reserve(kept.bgra.len() / 4 * 3);
        let mut lit = 0_usize;
        for pixel in kept.bgra.chunks_exact(4) {
            bgr.extend_from_slice(&pixel[..3]);
            lit += usize::from(flash_test && pixel[..3] == FLASH_BGR);
        }
        let packed = zstd::bulk::compress(&bgr, ZSTD_LEVEL).map_err(|e| format!("zstd: {e}"))?;
        let path = folder.join(format!("frame-{:04}.zst", stored.len()));
        std::fs::write(&path, &packed).map_err(|e| format!("write {}: {e}", path.display()))?;
        stored.push(Stored {
            instant: kept.instant,
            width: kept.width,
            height: kept.height,
            bytes: packed.len() as u64,
            hash: fnv1a(&bgr),
            flash: lit >= FLASH_MIN_PIXELS,
        });
    }
    Ok(stored)
}

/// FNV-1a, 64 bit: the same bytes give the same hash on every run.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// results/corpus.md from every capture folder's row.md.
fn write_summary() -> Result<()> {
    let mut rows: Vec<String> = std::fs::read_dir(CORPUS_ROOT)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("row.md")).ok())
        .collect();
    rows.sort();
    let mut text = String::from(
        "# Clip experiments, step 1: corpus\n\n\
         Written by `clip-probe exp-corpus` (scripts/clip-equation/probe/src/exp_corpus.rs) from the \
         capture folders in `C:\\Users\\Adam\\clip-corpus\\` (not in the repository). \
         Plan: docs/clip-recorder-experiments-plan.md, F6, F7, Procedure step 1.\n\n\
         Each capture: t = 20 s at f = 20 instants (N_inst = 400); frames stored at the window size as \
         zstd level −3 of B,G,R, one file per new frame; `frames.jsonl` has one line per instant. \
         Recorded size = σ · stored size; the encode step scales by σ. \
         n_new = new frames per second; bytes/frame = mean zstd −3 bytes of the stored frames; \
         D_wait = their sum; lost = instants without any frame; delivered = frames Windows Graphics Capture delivered.\n\n\
         | folder | scenario | window | stored | recorded | σ | instants | N_new | N_rep | n_new (1/s) | zstd −3 bytes/frame | D_wait bytes | lost | flashes | delivered |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for row in rows {
        let (name, cells) = row
            .trim_end()
            .split_once('\t')
            .unwrap_or(("", row.trim_end()));
        let _ = writeln!(text, "| {name} {cells}");
    }
    let path = results_folder()?.join("corpus.md");
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("exp-corpus summary={}", path.display());
    Ok(())
}

/// The capture handler: per instant, the last frame composed at or before
/// its end; a kept frame is new. Kept frames go to the writer thread.
pub struct CorpusFrames {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    readback: Option<ReadBack>,
    k0: i64,
    held: Option<i64>,
    sender: Option<Sender<Kept>>,
    delivered: u32,
}

impl CorpusFrames {
    fn keep_held(&mut self) {
        let (Some(instant), Some(readback), Some(sender)) = (
            self.held.take(),
            self.readback.as_ref(),
            self.sender.as_ref(),
        ) else {
            return;
        };
        let (width, height) = readback.size();
        let _ = sender.send(Kept {
            instant,
            width,
            height,
            bgra: readback.pixels().to_vec(),
        });
    }

    /// Keeps the last held frame and ends the writer's input.
    fn finish(&mut self) {
        self.keep_held();
        self.sender = None;
    }
}

impl GraphicsCaptureApiHandler for CorpusFrames {
    type Flags = (i64, Sender<Kept>);
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self> {
        let (k0, sender) = ctx.flags;
        Ok(Self {
            device: ctx.device,
            context: ctx.device_context,
            readback: None,
            k0,
            held: None,
            sender: Some(sender),
            delivered: 0,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _control: InternalCaptureControl,
    ) -> Result<()> {
        self.delivered += 1;
        // Instant of a composition time: the first s_j at or after it.
        let composed = composition_time(frame);
        let absolute =
            composed.div_euclid(INSTANT_HNS) + i64::from(composed.rem_euclid(INSTANT_HNS) != 0);
        let instant = (absolute - self.k0).max(0);
        if self.held.is_some_and(|held| instant > held) {
            self.keep_held();
        }
        if instant >= INSTANTS {
            return Ok(());
        }
        let size = (frame.width(), frame.height());
        if self.readback.as_ref().is_none_or(|r| r.size() != size) {
            self.keep_held();
            self.readback = Some(ReadBack::new(&self.device, size.0, size.1)?);
        }
        let Some(readback) = self.readback.as_mut() else {
            return Err("no read-back buffer".to_owned());
        };
        readback.read(&self.context, frame.as_raw_texture(), MapWait::Block)?;
        self.held = Some(instant);
        Ok(())
    }
}
