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
| A still is exact | each still equals the captured picture of its instant, pixel for pixel |
| A still appears when the window settles | on the typing scene, one still within 0.6 s after typing stops |
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

(empty)
