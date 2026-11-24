# Clip recorder plan 5.0: one video stream plus settled stills

Created 2026-11-24 06:20 IST by Claude. This plan replaces every earlier clip recorder plan. The earlier documents are archived in `docs/archive/clip-recorder-before-5.0/` and are not in force (owner, 2026-11-24). It is the builder's spec; a builder reports any deviation before or while making it.

Every number in this plan comes from `docs/research/window-recording/measurements.md` (measured on this PC) or `docs/research/window-recording/proof.md` (quoted from a source, P-numbers). Nothing is taken from memory.

## 1. Decision

- **2026-11-23, owner:** the earlier design decisions for the recorder no longer bind; research starts from the task alone. Task: record a clip of an application window on Windows so that an AI model can read back what happened. Goals: maximum quality, small files, low CPU, low RAM.
- **2026-11-24, owner:** the method of `docs/research/window-recording/recommendation.md` is accepted, with **software AV1 as the standard encoder**.
- **What that replaces:** plan 4.2's dynamic recorder (a lossless zstd layer for everything, plus H.264 for one moving rectangle). Measured on a real YouTube page it wrote 187 MB per minute, lost 27–31 of 802 instants when the rectangle changed, and returned lost instants as normal pictures (#290, comments 1374 and 1375).

**The method:**

    while the window changes  → one AV1 video stream of the whole window
    when the window settles   → one lossless still of the whole window
    while nothing changes     → nothing is written

Basis:
- Citrix: "Build to lossless compresses the screen using H.264 (or H.265) during screen activity and sharpens to pixel perfect (lossless) when activity stops." (P34)
- Measured, real YouTube page: 3.4–6.8 MB per minute of AV1 video plus 4.3 of stills, against 187.
- AV1 is "no-charge, royalty-free" and its encoder library may be redistributed (P67).

## 2. Architecture

    capture (Windows.Graphics.Capture) → recorder → changed?  ──no──→ index line only
                                                       │yes
                                                       ├→ AV1 encoder (realtime, screen mode) → video file
                                                       └→ settled for 0.5 s? → lossless still
    recording ends → clip folder → viewer

- **One rule decides, one type owns it:** a picture is encoded when it differs from the picture before it. There is no moving rectangle, no per-area choice, and no lossless storage of changing pixels.
- **Kept from the existing recorder,** unless a section below changes it: the capture observer; the instant grid at 20 instants per second; the cursor sampled with the frame and not drawn; `recorder.lock`; the clip registry; atomic `clip.json`; the operations `record`, `record_status`, `record_stop`, `record_extend` and the viewer operations.
- **Removed:** the exact zstd stream, the moving-area rule and its block windows, the moving coder with its queue and segments, the Media Foundation sink-writer encoder, and the composition of two layers in the viewer.

## 3. Clip folder

| file | rule |
|---|---|
| `video/part-NNNN.*` | The AV1 stream. A new part starts at a key picture. A part is complete on disk before the next one starts, so a killed recorder loses at most the part being written. |
| `stills/still-<instant>.webp` | One lossless WebP of the whole window for each settle point. |
| `frames.jsonl` | One line per instant: time; `enc` (the picture's place in the video) or `same` (unchanged, shows the picture before it); `still` if a still was written; `lost` with its cause if the instant was lost. |
| `clip.json` | Method id, width, height, instants, encoder name and settings, the parts, the stills, the lost instants (each exactly once) and the conditions. |

**Bytes:**

    B = Σ video parts + Σ stills + frames.jsonl + clip.json

**Instants (nothing unaccounted):**

    N = N_enc + N_same + N_lost

**Still rule:**

    still at instant k  ⇔  picture(k) ≠ picture(k − 1)  and  picture(j) = picture(k) for every j in (k, k + S]      S = SETTLE_INSTANTS

The still is written at instant k + S and belongs to instant k. No second still is written until the picture has changed again.

**End of recording:** the encoder is drained, the last part is closed, `clip.json` is written complete. A `record` call that fails before the first picture removes the folder it created and never touches a folder that existed before the call (#290 comment 1375).

**Recovery:** a clip whose recorder died keeps every complete part and every still; instants after the last complete part are named lost, once.

## 4. Quality

- **Exact where the window is still:** at every settle point the clip holds the window's exact pixels.

      Σ |still(k) − captured(k)| = 0

- **Video where it changes.** Measured closeness to the source (SSIM, RGB) for libaom AV1 in realtime screen mode:

| scene | quality setting 35 | quality setting 25 |
|---|---|---|
| typing | 0.9994 at 0.1 MB per minute | not measured |
| interface | 0.9960 at 0.7 | not measured |
| scrolling | 0.9823 at 2.5 | not measured |
| mixed, 60 s | 0.9969 at 0.5 | see `measurements.md`, second part |
| real YouTube page | 0.9703 at 3.4 | 0.9796 at 6.8 |

- **Readable by a model.** Claude read the same text, character for character, from the source and from the x264, Intel HEVC and SVT-AV1 outputs: typing, the middle of a scroll, and 12–13 pixel grey text beside a playing video. **The libaom output itself has not had this reading test.** It is the first proof of W1.
- **Known limit, measured:** text beside a playing video stays video quality. On the real page the side column was still only 7 % of the time and the header 5 %, because the glow around the player follows the video. Settling by region would not change that.

## 5. Viewer

The reader of a clip is an AI model. Every answer must be true to what was recorded.

- **One picture at time t:**
  1. If a still exists for the instant shown at t, return the still (exact).
  2. Otherwise decode the video from the last key picture at or before t.

         N_dec ≤ KEY_INSTANTS      (100 at a key picture every 5 s)

     Measured stand-in: 171 ms worst case with a key picture every 5 s (HEVC, software decoder, starting the program included).
- **Each returned picture says what it is:** `exact still`, `video`, or `lost`. A lost instant is never returned as a recorded picture; the answer names the loss, its cause, and the nearest recorded instants before and after it (#290 comment 1375, finding 2).
- **Full resolution and crops:** a model shrinks a large image before reading it (P7), so the viewer keeps full resolution and serves regions at native size. This exists today and stays.
- **Findings of the live test by an AI agent to fix here** (#290 comment 1374):
  - the recorder is hidden inside the `screenshot` tool's operation list; it gets its own tool name;
  - `clip_overview` spent all 8 pictures on the first 1.1 s and cut its timeline at 18 s; it must spread over the whole clip;
  - `record_status` uses one-letter condition keys; they get words;
  - `max_long_edge` was ignored; it is honoured or removed from the description;
  - the picture label covers the top-left corner; it moves outside the picture.
- **Change analysis (the old "D5" rules):** its thresholds were derived for a lossless clip. With a video stream they must be derived again from measured AV1 noise on unchanged content. Until that is measured, change analysis compares stills with stills only. Open item O2.

## 6. Parameter map

| constant | start value | source |
|---|---|---|
| instants per second | 20 | existing recorder; all measurements were at 20 |
| encoder | libaom AV1, realtime usage, speed 8, screen content tuning, row multithreading, 4:2:0 | measurements, libaom section |
| AV1 quality setting | 35, to be confirmed by the reading test in W1; 25 if 35 fails it | §4 |
| `KEY_INSTANTS` (key picture distance) | 100 (5 s) | measured on HEVC: 47 % more size than 30 s, worst fetch 171 ms against 460 ms |
| `SETTLE_INSTANTS` | 10 (0.5 s) | measurements, second part: 4 settle points in 40 s on the real page |
| still format | WebP lossless | P16; measured 697–727 KB per still on the real page, 15 KB on interface work |
| encoder queue | pictures wait for the encoder in a bounded queue; a full queue names the instant lost | start-up about 70 ms measured, so under 2 instants |

Removed: zstd level and window, `MOVING_*` constants, the H.264 quality level, the GOP of 20, `MOVING_QUEUE_FRAMES`, the block grid of the motion rule.

## 7. Targets (gates of the build)

Measured with ffmpeg standing in for the recorder. The product must reach them on the same scenes; a miss is reported with its numbers, never hidden.

| measure | scene | target |
|---|---|---|
| Size | real YouTube page | video ≤ 4.1 MB per minute at setting 35 (3.4 measured + 20 %); stills reported apart |
| Size | mixed, 60 s | ≤ 0.6 MB per minute (0.5 + 20 %) |
| Size | scrolling | ≤ 3.0 MB per minute (2.5 + 20 %) |
| Lost instants | every scene | 0, or each named with its cause |
| Processor time | every scene | ≤ 0.5 of one core (measured 0.28–0.48) |
| Peak memory of the recorder | every scene | reported; ffmpeg's whole program used 382–412 MB |
| Start | first encoded picture after the `record` call | ≤ 2 instants (100 ms); measured stand-in about 70 ms |
| Read-back | one picture at any time | ≤ 0.5 s |

## 8. Work packages

| id | what | files | proof |
|---|---|---|---|
| W0 | The probe of `docs/research/window-recording/probe-plan.md`: choose how to link the AV1 encoder by measurement; measure a purpose-built recorder | `scripts/clip-equation/probe/` | that plan's checks; the result table in its Builder notes |
| W1 | `ClipEncoder` becomes the AV1 encoder chosen in W0: realtime screen mode, key picture every `KEY_INSTANTS`, parts complete on disk | `server/clip_encoder.rs` | reading test on libaom output (typing, mid-scroll, small text); size targets of §7 on the corpus scenes |
| W2 | Recorder pipeline: changed? → encode; bounded queue; every instant gets its index line | `server/capture_clip.rs`, new `server/clip_stream.rs` | check: N = N_enc + N_same + N_lost on every scene; lost = 0 |
| W3 | Settled stills by the rule of §3 | `server/clip_stream.rs` | check: each still equals the captured picture; one still within 0.6 s after typing stops |
| W4 | Clip folder and `clip.json` of §3; a failed `record` leaves nothing behind | `server/clip_folder.rs`, `server/capture_clip.rs` | check: folder holds exactly the §3 files; B = Σ files; failed `record` then retry with the same folder succeeds |
| W5 | Recovery of a killed recorder | `server/clip_recovery.rs` | check: kill mid-recording → complete parts and stills kept, later instants named lost once |
| W6 | Viewer: one picture by §5; `exact still`, `video` or `lost` on every answer | `server/clip_decoder.rs`, `server/clip_viewer.rs` | check: red on the existing clip `.runs/claude-live/clip2` at 11,100 ms (returned "recorded pixels"); green: named lost. Read-back ≤ 0.5 s |
| W7 | Viewer for an AI: own tool name, overview spread over the clip, worded status keys, label outside the picture, `max_long_edge` | `server/m1_tools/mod.rs`, `server/clip_viewer.rs` | an agent finds and uses the recorder from the tool list alone (live test) |
| W8 | Remove the old design: exact stream, motion rule, moving coder, layer composition, Media Foundation encoder | `server/clip_exact.rs`, `clip_motion.rs`, `clip_moving.rs`, `clip_layers.rs`, old parts of `clip_encoder.rs` and `clip_viewer.rs`, their probe modes and specs | cargo build and clippy clean; no reference left |

Order: W0 → W1 → W2 → W3 → W4 → W5 → W6 → W7 → W8. Each package is built by test first: its check is written and seen to fail before the code.

Code rules: one type owns one rule; no pass-through layers; named constants only; readable top to bottom by one person.

Not in this plan, each needs its own plan: the UI Automation text and event track; the hardware HEVC upgrade; change analysis on video (O2).

## 9. Verification round

1. cargo fmt, clippy, then ONE release build (`-j 4`, priority Idle).
2. Every check of §8, one round each. A failed run is read once in full and its cause fixed; no rerun until green; no threshold is loosened.
3. Codex review, read-only.
4. **Live test by Claude through the real server, as an AI uses it:** `record` on a real window (the YouTube page: play, scroll, type `hello test 123`, pause), `record_stop`, then read the clip back through the viewer only. Compare size, processor time, memory and lost instants with §7. The window is open only while it is recorded.
5. Report in plain comparisons against today's recorder (187 MB per minute = 100 %).

## 10. Issues

- #298: the research, the proof ledger and the measurements behind this plan.
- #290: the faults of the 4.2 recorder. Lost instants on rectangle changes, the oversized rectangle and the 187 MB per minute end with W8. "Lost instant returned as recorded pixels" is W6. "Failed `record` leaves its folder" is W4. The viewer points are W7.
- Still to investigate from #290: three fully black pictures at 12.75–13.05 s of the agent's clip (page or capture, unknown); 12 `CLIP_RECOVERY_FAILED` warnings at every server start for clips of older formats.

## 11. Open items for the owner

- **O1: old limits.** Plan 4.2 carried a 10 % processor rule and a 700 MB memory limit from earlier decisions. §7 replaces them with measured targets. Confirm, or name the limits you want.
- **O2: change analysis on video** (§5). Derive new thresholds by measurement, or keep it to stills only.
- **O3: clips of older formats** in `%LOCALAPPDATA%\synapse\clips`. The viewer refuses them today and logs a warning for each at every start. Delete them, or keep a reader for them?
