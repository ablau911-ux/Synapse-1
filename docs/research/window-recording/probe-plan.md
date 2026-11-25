# Building plan: AV1 window-recording probe

Written by Claude on 2026-11-24. This file is the builder's spec. A builder reports any deviation from it before or while making it.

## Decision

**Owner, 2026-11-24:** the recommendation in `recommendation.md` is accepted with **software AV1 as the standard encoder**.

The method: record one whole-window video while the screen changes, plus one lossless still each time the window settles. This probe is the smallest program that does that. It exists to measure what ffmpeg could not show: the memory, processor time and start-up of a recorder written for the purpose.

**The probe is not the product.** It is a separate program. Nothing in `crates/` changes in this plan.

## Facts the plan rests on

| fact | source |
|---|---|
| Software AV1 (libaom, realtime, screen mode) wrote 0.5–0.7 MB per minute on interface work, 2.5 on scrolling, 6.8 on the real YouTube page | `measurements.md`, first and second parts |
| It used 0.28–0.48 of one core and ran 4.9–10 times faster than real time at 20 pictures per second | `measurements.md`, libaom section |
| It started in about 70 ms | `measurements.md`, last section |
| ffmpeg's settings for it: `-usage realtime -cpu-used 8 -crf 35 -aom-params tune-content=screen -row-mt 1`, 4:2:0 | `.runs/measure/measure.py` |
| AV1 is licensed "no-charge, royalty-free"; libaom may be redistributed "in source and binary forms" | `proof.md` P67 |
| A key picture every 5 s costs 47 % more than every 30 s and cuts the worst-case fetch from 460 to 171 ms (measured on HEVC) | `measurements.md`, third part |
| On the real page the window settled 4 times in 40 s; exact stills cost 4.3 MB per minute there | `measurements.md`, second part |
| Lossless WebP stills averaged 697–727 KB for the real page and 15 KB for interface work | `measurements.md`, second and fourth parts |
| Windows reports changed regions, and delivers no picture while nothing changes | `proof.md` P9, P35 |
| The repository already has a Rust probe that captures a window with Windows.Graphics.Capture | `scripts/clip-equation/probe/` |
| A lossless capture of the real YouTube page exists for comparison | `.runs/measure/real-youtube-even.src.mkv` (git-ignored, 1.1 GB) |

## What is not known yet, and is the first thing to find out

**How the probe links the AV1 encoder.** Nothing here has been verified. Step 1 checks the three candidates and picks one by evidence:

1. libaom as a C library, called from Rust through bindings.
2. rav1e, an AV1 encoder written in Rust.
3. SVT-AV1 as a C library (measured here as larger than libaom on scrolling: 8.2 against 2.5 MB per minute).

For each: can it be built on this PC without network access problems, does it offer a realtime mode and a screen-content mode, what does its licence say (quote it), and what does a 20 s encode of `scroll-1920x1080` and `mix-1920x1080` cost in size, processor time and memory. The candidate must come within 20 % of the ffmpeg libaom figures above to be chosen. If none does, STOP and report.

## Parts, in order

1. **Encoder choice** (above). Output: a short table in this file under "Builder notes", with the measured figures and the licence quote.
2. **Capture to encoder.** A new probe command records one window by handle:
   - Windows.Graphics.Capture at up to 20 pictures per second;
   - a picture is encoded only when it differs from the one before it;
   - every picture carries its own time, so nothing is invented for still moments;
   - AV1 in realtime screen mode, one key picture every 5 s;
   - the output is a file that survives a kill of the program up to the last complete key-picture group.
3. **Settled stills.** When the window has not changed for 0.5 s, the probe writes one lossless still of it (WebP lossless), named by its time. No second still is written until the window has changed again.
4. **Index.** One line per recorded instant in a text file: time, whether a picture was encoded, whether a still was written, and, if an instant was lost, that it was lost and why. A lost instant is never left unnamed.
5. **Measurement run.** One recording of the real YouTube page with the same actions as in `measurements.md` (scroll at 8 s, typing from 15 s, pause at 36 s; 40 s). The window opens, is recorded and closes in one script.

## Probe interface (the contract between the checks and the code)

Added 2026-11-24 so the check-writer and the builder work against the same text.

**Command:**

    clip-probe exp-av1 out=<dir> seconds=<n> hwnd=<window handle>      record a live window
    clip-probe exp-av1 out=<dir> folder=<corpus scene folder>          replay a captured scene in real time

- `folder=` replays a scene of `C:\Users\Adam\clip-corpus` (one picture per instant from its `frames.jsonl`, 20 instants per second, paced by the clock) through exactly the same pipeline as a live window. The checks use this mode, because the source pictures are known.
- Optional: `quality=<n>` (default 35), `key=<instants>` (default 200, a key picture every 10 s; see Builder notes, part 1), `settle=<instants>` (default 10).
- Exit code 0 on success. Any failure prints one line to stderr and exits non-zero.

**Output folder:**

| path | content |
|---|---|
| `video/part-NNNN.ivf` | AV1 in the IVF container. A new part starts at each key picture. Each picture's timestamp is its instant number. |
| `stills/still-NNNNNNNN.webp` | Lossless WebP of the whole window; the number is the instant the still belongs to, zero-padded to 8 digits. |
| `frames.jsonl` | One JSON line per instant, in order: `{"instant":k,"t_ms":k*50}` plus exactly one of `"enc":[part,index_in_part]`, `"same":true`, `"lost":"<cause>"`; plus `"still":true` on the instant a still belongs to. |
| `probe.json` | `{"instants","encoded","same","lost","stills","bytes_video","bytes_stills","cpu_ns","ram_peak","first_encoded_ms","width","height","encoder":{"name","settings"}}` |

- `cpu_ns` is the processor time of the whole probe process; `ram_peak` its peak private bytes; `first_encoded_ms` the time from the start of recording to the first encoded picture.
- Instant 0 is always encoded (it is a key picture). An instant whose picture equals the one before it is `same` and nothing is encoded for it.
- Parts are numbered from `part-0000`.
- Still rule, exactly: instant k gets a still if and only if its picture differs from instant k − 1's (instant 0 counts as a change) and the picture is then shown for `settle` instants in a row, k included: instants k + 1 … k + settle − 1 all show the same picture as k. With the default 10 that is 0.5 s on screen. A recording that ends before instant k + settle − 1 writes no still for k.
  - Corrected 2026-11-24: an earlier wording required `settle` further instants after k (0.55 s). That was off by one against the accepted rule "unchanged for half a second" and against the measurement behind it (run length of at least 10 pictures: 4 settle points in the mixed scene, 2 in typing).
- "The video decodes" means every picture is really decoded (ffprobe `-count_frames`), not only counted.

## Files

- `scripts/clip-equation/probe/src/exp_av1.rs` (new): the command of parts 2–4.
- `scripts/clip-equation/probe/src/main.rs`: one line to register the command.
- `scripts/clip-equation/probe/Cargo.toml`: the encoder dependency chosen in part 1.
- `scripts/clip-equation/spec/av1-probe.mjs` and `av1-probe.feature` (new): the checks below.
- `docs/research/window-recording/probe-plan.md` (this file): Builder notes and the result table.

## Proof

Each check is written first and must fail before the code exists. One round per check.

| check | passes when |
|---|---|
| Every instant is accounted for | the index has one line per instant of the run; pictures + unchanged instants + named losses = instants |
| Nothing is lost silently | lost instants in the index = 0, or each has a cause |
| The video decodes | every encoded picture decodes; picture count = encoded lines in the index |
| A still is exact | each still equals the captured picture of its instant, pixel for pixel; at least one still exists over the scenes checked |
| A still appears when the window settles | on the mixed scene (`mix-1920x1080`), the instants marked `still` are exactly the settle points of the still rule, and there is at least one. Corrected 2026-11-24: the first wording named the typing scene, but in that capture the picture never stays unchanged for 0.5 s (longest run 10 instants), so it has no settle point. |
| Read-back | a picture at an arbitrary time is fetched in under 0.5 s |

Reported, not gated (these are the reason the probe exists):

| measure | compare with |
|---|---|
| File size per minute on the real page, video and stills apart | 6.8 MB per minute of video, 4.3 of stills |
| Processor time of the probe per second of recording | 0.28–0.48 of one core |
| Peak memory of the probe | ffmpeg's 382–412 MB |
| Time from the start call to the first encoded picture | about 70 ms |
| Closeness of the video to a lossless capture taken in the same run, if both can be taken together | 0.9796 |

## Rules

- One cargo build at a time, `-j 4`, priority Idle.
- One round per check. A failed run is read once in full and its cause fixed; no rerun until green. No threshold or assertion is loosened.
- Claude runs the measurement run itself and reports it.
- No network during builds unless part 1 shows a dependency must be fetched; if so, report first.
- A test window is open only while it is recorded.
- Typed test text is neutral (`hello test 123`); no project or personal names go to outside sites.
- No change under `crates/`. No commit of `.runs/`.

## Out of scope

The product integration; the UI Automation text and event track; hardware HEVC; the viewer tools; the current recorder's faults in #290.

## Builder notes

### 2026-11-24: part 1 done by Claude. Encoder: libaom, linked through `libaom-sys`

Measured through ffmpeg (same libraries), one run each, key picture every 5 s, scripts `scripts/clip-equation/spec/w0-encoder-choice.py` and `w0-key-distance.py`. Each size cell: MB per minute (closeness).

| candidate | scrolling | mixed | real YouTube page | CPU, cores | RAM | slowest speed |
|---|---|---|---|---|---|---|
| libaom, realtime, screen, setting 35 | 3.75 (0.9837) | 0.72 (0.9974) | 7.40 (0.9749) | 0.48–0.81 | 409–411 MB | 2.9 × real time |
| rav1e, speed 10, quantizer 80 | 10.70 (0.9702) | 1.70 (0.9952) | 8.07 (0.9651) | 3.7–7.8 | about 800 MB | 0.3 × |
| rav1e, speed 10, low latency | 9.72 (0.9620) | 1.50 (0.9934) | 6.27 (0.9473) | 2.0–3.5 | about 560 MB | 0.5 × |
| SVT-AV1, preset 10, screen, setting 35 | 11.04 (0.9836) | 1.67 (0.9972) | 5.70 (0.9766) | 0.40–0.57 | 1,331–1,413 MB | 12.2 × |

- **rav1e fails:** slower than real time on every scene, larger and less close. Its own README claims only "near real-time" at the fastest setting.
- **SVT-AV1 fails:** 2.9 times libaom's size on scrolling and 2.3 times on the mixed scene (the rule allows 20 %), and about 1.4 GB of memory.
- **libaom is chosen.** Rust package: `libaom-sys` 0.18.1, which carries libaom 3.15.3 (crates.io, updated 2026-11-16, "Bindings for libaom. Part of libavif-sys"). Licence: P67 in `proof.md`. Building it downloads packages from crates.io; this was reported to the owner on 2026-11-24.
- **Not yet verified:** that `libaom-sys` builds on this PC and exposes the realtime usage, the speed setting, the screen-content tuning and row multithreading. That is the builder's first action; if it does not, STOP and report.

**Key picture distance** (libaom, setting 35):

| key picture every | real page | worst fetch | scrolling | mixed |
|---|---|---|---|---|
| 5 s | 7.40 | 181 ms | 3.75 | 0.72 |
| 10 s | 4.04 | 340 ms | 2.69 | 0.53 |
| 15 s | 3.81 | 399 ms | 2.68 | 0.53 |
| 30 s | 3.42 | 676 ms | 2.46 | 0.51 |

Chosen: 10 s (`key=200`). Plan 5.0 §6 and §7 were corrected to this.

### 2026-11-24: scout's map of the existing probe (read-only agent; paths under `scripts/clip-equation/probe/src/`)

Reuse these; do not rewrite them.

| need | what exists | where |
|---|---|---|
| Register a command | `match command.as_str()`; each command is `fn run() -> Result<()>`; `mod` list | `main.rs:191-211`, `main.rs:21-78` |
| Read `key=value` arguments | `exp_meter::Args::from_env()`, `text`, `text_or`, `number`, `flag` | `exp_meter.rs:27-62` |
| Result type | `pub type Result<T> = std::result::Result<T, String>;` errors by `.map_err(\|e\| e.to_string())?` | `main.rs:86` |
| Capture a window by handle | `observer::start_with_cursor::<H>(hwnd, Interval, with_cursor, flags)`, `observer::stop`, `observer::composition_time(frame)`; frames are BGRA 8 bit | `observer.rs:58-126` |
| A capture handler to copy the pattern from | `CorpusFrames` (`GraphicsCaptureApiHandler`, `on_frame_arrived`, `keep_held`, `finish`) | `exp_corpus.rs:377-446` |
| Read a frame back from the graphics chip | `gpu::ReadBack::{new, read, pixels, size}`: tightly packed BGRA | `gpu.rs:264-292` |
| The instant grid | `slots::INSTANT_HNS` (50 ms), `clock::{now_hns, sleep_until, HNS_PER_MS}`; which instant a frame belongs to | `slots.rs:33`, `clock.rs:22-48`, `exp_corpus.rs:156-157, 436-442` |
| Replay a corpus scene | `Corpus::open`, `load`, `lines`, `picture` (B,G,R, 3 bytes per pixel, top to bottom); the real-time pacing loop. `Corpus` and `CorpusLine` are private to `exp_dyn.rs` and need `meta.json` | `exp_dyn.rs:19-85, 247-255` |
| Processor time of the whole process | `exp_meter::process_cpu_ns()` | `exp_meter.rs:117` |
| Peak private bytes | `exp_measure::os_peak_private()` | `exp_measure.rs:66` |
| BGRA to BGR | `exp_measure::bgra_to_bgr` | `exp_measure.rs:57` |
| Write the JSON report | `json!({...})` to `out/probe.json`, then `println!("{report}")` as the last stdout line | `exp_dyn.rs:313-318` |
| Cargo setup | package `clip-probe`, edition 2024, its own empty `[workspace]`; no lint denies; `png` and `zstd` are dependencies | `Cargo.toml` |

**Not present, to be added:** an AV1 encoder binding, a lossless WebP encoder, an IVF writer.

**Encoder settings to reproduce** (what ffmpeg passed to libaom): realtime usage; speed (`cpu-used`) 8; constant quality 35 with no bitrate cap; `tune-content=screen`; row multithreading on; 4:2:0, 8 bit; key picture every 200 pictures.
