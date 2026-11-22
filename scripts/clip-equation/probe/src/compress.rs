//! Package 0b (plan section 0b): compression of the waiting frames.
//!
//!   M10  capture the new frames of Chrome app windows the probe opens itself
//!        (pages in scripts/clip-equation/ that drive themselves); the frame
//!        of each 50 ms instant is the last one composed at or before it, and
//!        it is new when its composition time is new (the product's rule)
//!   M11  per scenario, one thread: bytes per frame and processor ns per pixel
//!        to compress and to decompress, for every method in `Method::all`
//!   M12  write cost per frame with those bytes: create `.tmp`, write, rename
//!   M13  exactness: decoded B, G, R equal the source; NV12 from the copied
//!        `Nv12Canvas::draw` equals the source's at the scenario's σ values
//!   M14  live share at 1920 x 1080 and 20 new frames per second (M4 worst)
//!
//! `clip-probe compress [scenario ...]` runs all scenarios or the named ones.
//! Frames stay in memory only and are dropped after each scenario; written
//! files are deleted after each method.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11DeviceContext};
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;

use crate::Result;
use crate::chrome_window::ChromeWindow;
use crate::clock::{self, CycleRate, HNS_PER_SECOND};
use crate::gpu::{MapWait, ReadBack};
use crate::nv12_copy::Nv12Hasher;
use crate::observer::{self, Interval, composition_time};
use crate::scratch::ScratchFolder;
use crate::slots::INSTANT_HNS;
use crate::waiting_codec::{Coder, Decoder, Encoder, Method, Picture, same_picture};

/// M10: each scenario is captured for at least 20 s.
const CAPTURE_SECONDS: u64 = 21;
/// Time for the page to load and start moving before the capture starts.
const SETTLE: Duration = Duration::from_millis(2500);
/// Package 0 result: 11 ms is the requested update interval (M3).
const REQUESTED_INTERVAL_MS: f64 = 11.0;
/// Memory bound for one scenario's frames (21 s at 20 new frames per second is 420).
const MAX_PICTURES: usize = 600;
/// Disk: at most this much written per method before the files are deleted.
const WRITE_CAP_BYTES: u64 = 500_000_000;
/// The owner's floor: free space never below 10 GB.
const MIN_FREE_BYTES: u64 = 10_000_000_000;
/// Scenarios with fewer new frames do not enter the worst-case figures.
const MIN_FRAMES_FOR_WORST: usize = 20;

// M14 inputs from package 0 (results-package0.md) and equation section 6.
const SPEED_RATIO: f64 = 3546.0 / 964.0;
const C_OBS_MS: f64 = 0.373;
const C_READ_1080: f64 = 0.349;
const PX_1080: f64 = 1920.0 * 1080.0;
const NEW_PER_SECOND: f64 = 20.0;
const CPU_LIMIT_PERCENT: f64 = 10.0;
/// (name, logical processors, cost multiplier, delivered frames per second).
const MACHINES: [(&str, f64, f64, f64); 2] = [
    ("development PC", 16.0, 1.0, 72.0),
    ("weakest machine (estimate)", 4.0, SPEED_RATIO, 60.0),
];

struct Scenario {
    name: &'static str,
    page: &'static str,
    query: &'static str,
    title: &'static str,
    size: (i32, i32),
    sigmas: &'static [u32],
}

const SCENARIOS: [Scenario; 8] = [
    Scenario {
        name: "flash",
        page: "flash-page.html",
        query: "",
        title: "Clip proof page",
        size: (1280, 720),
        sigmas: &[1],
    },
    Scenario {
        name: "flash-anim",
        page: "flash-page.html",
        query: "?anim=1",
        title: "Clip proof page",
        size: (1280, 720),
        sigmas: &[1],
    },
    Scenario {
        name: "scroll",
        page: "scroll-page.html",
        query: "",
        title: "clip0b scroll",
        size: (1280, 720),
        sigmas: &[1],
    },
    Scenario {
        name: "typing",
        page: "typing-page.html",
        query: "",
        title: "clip0b typing",
        size: (1280, 720),
        sigmas: &[1],
    },
    Scenario {
        name: "video",
        page: "video-page.html",
        query: "",
        title: "clip0b video",
        size: (1280, 720),
        sigmas: &[1],
    },
    Scenario {
        name: "still",
        page: "still-page.html",
        query: "",
        title: "clip0b still",
        size: (1280, 720),
        sigmas: &[1],
    },
    Scenario {
        name: "typing-small",
        page: "typing-page.html",
        query: "",
        title: "clip0b typing",
        size: (520, 340),
        sigmas: &[1, 2, 3],
    },
    Scenario {
        name: "scroll-small",
        page: "scroll-page.html",
        query: "",
        title: "clip0b scroll",
        size: (520, 340),
        sigmas: &[1, 2, 3],
    },
];

/// One method's figures in one scenario.
struct Row {
    method: Method,
    frames: usize,
    bytes_per_frame: f64,
    bytes_per_px: f64,
    compress_ns_px: f64,
    write_us_frame: f64,
}

pub fn run() -> Result<()> {
    let wanted: Vec<String> = std::env::args().skip(2).collect();
    let rate = CycleRate::calibrate();
    let pages = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("no pages folder")?
        .to_path_buf();
    let mut rows = Vec::new();
    for scenario in SCENARIOS
        .iter()
        .filter(|s| wanted.is_empty() || wanted.iter().any(|w| w == s.name))
    {
        let (pictures, seconds, delivered) = capture(scenario, &pages)?;
        rows.extend(measure(scenario, &pictures, seconds, delivered, rate)?);
    }
    report_live_share(&rows);
    Ok(())
}

/// M10: opens the scenario's page, captures it and closes it again.
fn capture(scenario: &Scenario, pages: &std::path::Path) -> Result<(Vec<Picture>, f64, u32)> {
    let window = ChromeWindow::open(
        &pages.join(scenario.page),
        scenario.query,
        scenario.title,
        (40, 40),
        scenario.size,
    )?;
    std::thread::sleep(SETTLE);
    let control = observer::start_with_cursor::<CollectFrames>(
        window.raw_hwnd(),
        Interval::Millis(REQUESTED_INTERVAL_MS),
        false,
        (),
    )?;
    let from = clock::now_hns();
    std::thread::sleep(Duration::from_secs(CAPTURE_SECONDS));
    let handler = observer::stop(control)?;
    let seconds = (clock::now_hns() - from) as f64 / HNS_PER_SECOND as f64;
    drop(window);
    let mut handler = handler.lock();
    handler.finish();
    if handler.overflow {
        println!(
            "M10 scenario={} note=frame cap {MAX_PICTURES} reached",
            scenario.name
        );
    }
    Ok((
        std::mem::take(&mut handler.pictures),
        seconds,
        handler.delivered,
    ))
}

/// M11 to M13 for one scenario; prints one markdown table.
fn measure(
    scenario: &Scenario,
    pictures: &[Picture],
    seconds: f64,
    delivered: u32,
    rate: CycleRate,
) -> Result<Vec<Row>> {
    let first = pictures
        .first()
        .ok_or_else(|| format!("{}: no frame captured", scenario.name))?;
    let pixels: usize = pictures.iter().map(Picture::pixels).sum();
    let raw_bytes = pixels as f64 * 4.0 / pictures.len() as f64;
    println!(
        "\nM10 scenario={} size={}x{} seconds={seconds:.1} delivered_per_s={:.1} new_frames={} new_per_s={:.2} raw_bytes_per_frame={raw_bytes:.0} sigmas={:?}",
        scenario.name,
        first.width,
        first.height,
        f64::from(delivered) / seconds,
        pictures.len(),
        pictures.len() as f64 / seconds,
        scenario.sigmas
    );
    let source_nv12: Vec<Vec<u64>> = {
        let mut hashers: Vec<Nv12Hasher> = scenario
            .sigmas
            .iter()
            .map(|&s| Nv12Hasher::new(s))
            .collect();
        pictures
            .iter()
            .map(|p| {
                hashers
                    .iter_mut()
                    .map(|h| h.hash(&p.bgra, p.width, p.height))
                    .collect()
            })
            .collect()
    };
    println!(
        "| method | bytes/frame | ratio | compress ns/px | decompress ns/px | write µs/frame (files) | B,G,R exact | NV12 exact (σ) |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    let mut rows = Vec::new();
    for method in Method::all() {
        let mut encoder = Encoder::new(method)?;
        let mut files = Vec::with_capacity(pictures.len());
        // One reused output buffer, as the live part would have; the copy kept
        // for M12 and M13 is made outside the timed part.
        let (mut cycles, mut out) = (0_u64, Vec::new());
        for (index, picture) in pictures.iter().enumerate() {
            let previous = index.checked_sub(1).map(|at| &pictures[at]);
            let start = clock::thread_cycles();
            encoder.encode(picture, previous, &mut out)?;
            cycles += clock::thread_cycles() - start;
            files.push(out.clone());
        }
        let compress_ns_px = rate.seconds(cycles) * 1e9 / pixels as f64;

        let mut decoder = Decoder::new()?;
        let mut hashers: Vec<Nv12Hasher> = scenario
            .sigmas
            .iter()
            .map(|&s| Nv12Hasher::new(s))
            .collect();
        let (mut cycles, mut exact) = (0_u64, 0_usize);
        let mut nv12_exact = vec![0_usize; scenario.sigmas.len()];
        for (index, (file, picture)) in files.iter().zip(pictures).enumerate() {
            let start = clock::thread_cycles();
            let size = decoder.decode(file)?;
            cycles += clock::thread_cycles() - start;
            if size == (picture.width, picture.height)
                && same_picture(decoder.canvas(), &picture.bgra, method.keep_alpha)
            {
                exact += 1;
            }
            // NV12 needs checking only where the decoded bytes differ from the
            // source at all: alpha dropped (decoded alpha is 255).
            if !method.keep_alpha {
                for (slot, hasher) in hashers.iter_mut().enumerate() {
                    if hasher.hash(decoder.canvas(), size.0, size.1) == source_nv12[index][slot] {
                        nv12_exact[slot] += 1;
                    }
                }
            }
        }
        let decompress_ns_px = rate.seconds(cycles) * 1e9 / pixels as f64;
        let bytes: usize = files.iter().map(Vec::len).sum();
        let bytes_per_frame = bytes as f64 / files.len() as f64;
        let (write_us_frame, write_files) = write_cost(&files, rate)?;
        let nv12 = if method.keep_alpha {
            "all 4 bytes equal".to_owned()
        } else {
            scenario
                .sigmas
                .iter()
                .zip(&nv12_exact)
                .map(|(sigma, n)| format!("σ{sigma} {n}/{}", files.len()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "| {} | {bytes_per_frame:.0} | {:.1} | {compress_ns_px:.3} | {decompress_ns_px:.3} | {write_us_frame:.0} ({write_files}) | {exact}/{} | {nv12} |",
            method.name(),
            raw_bytes / bytes_per_frame,
            files.len()
        );
        rows.push(Row {
            method,
            frames: files.len(),
            bytes_per_frame,
            bytes_per_px: bytes as f64 / pixels as f64,
            compress_ns_px,
            write_us_frame,
        });
    }
    Ok(rows)
}

/// M12: thread processor µs per file to create `<n>.tmp`, write it through
/// the file cache and rename it; files are deleted afterwards.
fn write_cost(files: &[Vec<u8>], rate: CycleRate) -> Result<(f64, usize)> {
    let mut total = 0_u64;
    let count = files
        .iter()
        .take_while(|file| {
            total += file.len() as u64;
            total <= WRITE_CAP_BYTES
        })
        .count()
        .max(1);
    let total: u64 = files[..count].iter().map(|f| f.len() as u64).sum();
    let folder = ScratchFolder::create("0b")?;
    if folder.free_bytes()? < total + MIN_FREE_BYTES {
        return Err(format!(
            "refused: writing {total} bytes would leave less than {MIN_FREE_BYTES} free"
        ));
    }
    folder.reserve(total)?;
    let mut cycles = 0_u64;
    for (index, file) in files[..count].iter().enumerate() {
        let temporary = folder.path().join(format!("{index:06}.tmp"));
        let done = folder.path().join(format!("{index:06}.wf"));
        let start = clock::thread_cycles();
        let mut handle = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("open {}: {error}", temporary.display()))?;
        handle
            .write_all(file)
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        drop(handle);
        fs::rename(&temporary, &done).map_err(|error| format!("rename: {error}"))?;
        cycles += clock::thread_cycles() - start;
    }
    Ok((rate.micros(cycles) / count as f64, count))
}

/// M14: the live share at 1920 x 1080 and 20 new frames per second, per
/// method at its best tile size, with the worst scenario's figures.
fn report_live_share(rows: &[Row]) {
    // Write cost per file = a + b · bytes, least squares over every M12 point.
    let points: Vec<(f64, f64)> = rows
        .iter()
        .map(|r| (r.bytes_per_frame, r.write_us_frame))
        .collect();
    let n = points.len() as f64;
    let (sx, sy) = points
        .iter()
        .fold((0.0, 0.0), |(sx, sy), (x, y)| (sx + x, sy + y));
    let (mx, my) = (sx / n, sy / n);
    let sxx: f64 = points.iter().map(|(x, _)| (x - mx) * (x - mx)).sum();
    let sxy: f64 = points.iter().map(|(x, y)| (x - mx) * (y - my)).sum();
    let per_byte = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let per_file = my - per_byte * mx;
    println!(
        "\nM12 write model: {per_file:.1} µs per file + {:.4} µs per KB ({:.3} ns per raw-1080p pixel for a raw BGRA frame)",
        per_byte * 1000.0,
        (per_file + per_byte * PX_1080 * 4.0) * 1e3 / PX_1080
    );

    let mut families: Vec<(Coder, bool)> = Vec::new();
    for row in rows {
        if !families.contains(&(row.method.coder, row.method.keep_alpha)) {
            families.push((row.method.coder, row.method.keep_alpha));
        }
    }
    println!(
        "\nM14 at 1920 x 1080, {NEW_PER_SECOND} new frames per second, worst scenario per figure (frames ≥ {MIN_FRAMES_FOR_WORST})"
    );
    println!(
        "| method | best tile | bytes/px worst / mean | compress ns/px worst | write µs/frame at 1080p | live share dev PC | live share weakest (estimate) | within {CPU_LIMIT_PERCENT} % |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for (coder, keep_alpha) in families {
        let tiles: Vec<u32> = if coder.uses_tiles() {
            vec![32, 64, 128]
        } else {
            vec![0]
        };
        let of_tile = |tile: u32| -> Vec<&Row> {
            rows.iter()
                .filter(|r| {
                    r.method.coder == coder
                        && r.method.keep_alpha == keep_alpha
                        && r.method.tile == tile
                        && r.frames >= MIN_FRAMES_FOR_WORST
                })
                .collect()
        };
        let mean_bytes = |rows: &[&Row]| {
            rows.iter().map(|r| r.bytes_per_px).sum::<f64>() / rows.len().max(1) as f64
        };
        let Some(tile) = tiles
            .iter()
            .copied()
            .min_by(|a, b| mean_bytes(&of_tile(*a)).total_cmp(&mean_bytes(&of_tile(*b))))
        else {
            continue;
        };
        let chosen = of_tile(tile);
        if chosen.is_empty() {
            continue;
        }
        let worst_bytes = chosen.iter().map(|r| r.bytes_per_px).fold(0.0, f64::max);
        let worst_compress = chosen.iter().map(|r| r.compress_ns_px).fold(0.0, f64::max);
        let write_us = per_file + per_byte * worst_bytes * PX_1080;
        let shares: Vec<f64> = MACHINES
            .iter()
            .map(|(_, threads, ratio, delivered)| {
                let per_frame_s = PX_1080 * (C_READ_1080 + worst_compress) * 1e-9 + write_us * 1e-6;
                ratio * (delivered * C_OBS_MS * 1e-3 + NEW_PER_SECOND * per_frame_s) / threads
                    * 100.0
            })
            .collect();
        let name = Method {
            coder,
            tile,
            keep_alpha,
        }
        .name();
        println!(
            "| {name} | {} | {worst_bytes:.4} / {:.4} | {worst_compress:.3} | {write_us:.0} | {:.2} % | {:.2} % | {} |",
            if coder.uses_tiles() {
                tile.to_string()
            } else {
                "-".to_owned()
            },
            mean_bytes(&chosen),
            shares[0],
            shares[1],
            if shares[1] <= CPU_LIMIT_PERCENT {
                "yes"
            } else {
                "no"
            }
        );
    }
}

/// The M10 handler: reads back every delivered frame and keeps, per 50 ms
/// instant, the last frame composed at or before it; a kept frame is a new
/// frame (its composition time is new).
pub struct CollectFrames {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    readback: Option<ReadBack>,
    held_instant: Option<i64>,
    pictures: Vec<Picture>,
    delivered: u32,
    overflow: bool,
}

impl CollectFrames {
    fn keep_held(&mut self) {
        if self.held_instant.take().is_none() {
            return;
        }
        let Some(readback) = self.readback.as_ref() else {
            return;
        };
        if self.pictures.len() >= MAX_PICTURES {
            self.overflow = true;
            return;
        }
        let (width, height) = readback.size();
        self.pictures.push(Picture {
            width,
            height,
            bgra: readback.pixels().to_vec(),
        });
    }

    /// Keeps the last frame when the capture has stopped.
    fn finish(&mut self) {
        self.keep_held();
    }
}

impl GraphicsCaptureApiHandler for CollectFrames {
    type Flags = ();
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> Result<Self> {
        Ok(Self {
            device: ctx.device,
            context: ctx.device_context,
            readback: None,
            held_instant: None,
            pictures: Vec::with_capacity(MAX_PICTURES),
            delivered: 0,
            overflow: false,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _control: InternalCaptureControl,
    ) -> Result<()> {
        self.delivered += 1;
        // Instant s_k is the first instant at or after the composition time.
        let composed = composition_time(frame);
        let instant =
            composed.div_euclid(INSTANT_HNS) + i64::from(composed.rem_euclid(INSTANT_HNS) != 0);
        if self.held_instant.is_some_and(|held| instant > held) {
            self.keep_held();
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
        self.held_instant = Some(instant);
        Ok(())
    }
}
