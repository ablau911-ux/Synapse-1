# Clip recorder plan 5.0: one video stream plus settled stills

Created 2026-11-24 06:20 IST by Claude. This plan replaces every earlier clip recorder plan. The earlier documents are archived in `docs/archive/clip-recorder-before-5.0/` and are not in force (owner, 2026-11-24). It is the builder's spec; a builder reports any deviation before or while making it.

Every number in this plan comes from `docs/research/window-recording/measurements.md` (measured on this PC) or `docs/research/window-recording/proof.md` (quoted from a source, P-numbers). Nothing is taken from memory.

## State, 2026-11-24

Built on branch `feature/clip-capture`, head 0b5a8a36. Every figure below was measured or built on 2026-11-24; §7 has the measurements.

| item | state |
|---|---|
| recording works on a real page | yes: real YouTube page 1904x1012, 40 s, 0 of 802 instants lost, 5.5–7.4 MB per minute in total against 187 for the old recorder |
| viewer works | yes: every answer says `exact still`, `video` or `lost`; a late video picture comes back in 414 ms in the app; Claude read the text back correctly |
| bench 10 of 10 | all 10 checks of `run-spec.mjs av1-probe` pass (§8) |
| old design removed | yes: the exact, moving and layers design and its files are deleted (W8) |
| known limits | processor time 0.45–0.97 of one core on the real page, inside the owner's target of at most 1 core per recording (#314); a killed recording is padded with lost instants (`service_restart`) up to the requested length; `clip_diff` compares two exact stills only |
| not done | UI Automation text and event track (later); hardware HEVC upgrade (later); change analysis on video (O2) |
| merged | into `main` 2026-11-25 (`4bf20bae`, follow-ups `9451157e`, `697dcc0a`) and installed into the running service |

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
                                                       ├→ convert to 4:2:0 → bounded queue (30) → encoder thread: AV1 (realtime, screen mode) → video file
                                                       └→ settled for 0.5 s? → lossless still
    recording ends → clip folder → viewer

- **One rule decides, one type owns it:** a picture is encoded when it differs from the picture before it. There is no moving rectangle, no per-area choice, and no lossless storage of changing pixels.
- **Encoder thread (found by test):** the encoder runs on its own thread behind a bounded queue of 30 converted pictures (1.5 s). The recorder thread decides same or changed, converts the picture to 4:2:0 and never waits for an encode. A full queue names the instant lost with cause `encoder_behind`. Reason: the encode of a key picture took 0.25–0.65 s and, with the encoder on the recorder thread, cost 36 of 809 instants; with its own thread, 0 of 802.
- **Window size change (found by test):** the first picture at a new size starts a new part with a key picture at that size. Each part records its width and height. Reason: a resize lost every later instant (558 of 802); after the change, 0.
- **Kept from the existing recorder,** unless a section below changes it: the capture observer; the instant grid at 20 instants per second; the cursor sampled with the frame and not drawn; `recorder.lock`; the clip registry; atomic `clip.json`; the operations `record`, `record_status`, `record_stop`, `record_extend` and the viewer operations.
- **Longest clip: 10 minutes** (`MAX_DURATION_MS` = 600000, owner's decision of 2026-11-24). `record` refuses a longer `duration_ms`, `record_extend` refuses an addition that would pass it, and recovery caps a `clip.json` that plans more instants than 10 minutes at 20 per second at that length (it recovers what is on disk and never leaves the clip `recording`), so no number in a call or in a clip folder makes recovery write lost lines without limit.
- **Removed:** the exact zstd stream, the moving-area rule and its block windows, the moving coder with its queue and segments, the Media Foundation sink-writer encoder, and the composition of two layers in the viewer. Done in W8: the old design and its files are deleted.

## 3. Clip folder

| file | rule |
|---|---|
| `video/part-NNNN.ivf` | The AV1 stream in IVF parts, numbered from 0000. A new part starts at each key picture, and at the first picture of a new window size. Each part records its width and height. A part is complete on disk before the next one starts, so a killed recorder loses at most the part being written. |
| `stills/still-NNNNNNNN.webp` | One lossless WebP of the whole window for each settle point; NNNNNNNN is the instant, eight digits. Written with the crate image-webp. |
| `frames.jsonl` | One line per instant: time; `enc` ([part, index], the picture's place in the video) or `same` (unchanged, shows the picture before it); `still` if a still was written; `still_of: <instant>` if the settled picture equals a still already stored; `lost` with its cause if the instant was lost. |
| `clip.json` | `schema_version` 4, method `av1-video-and-stills`, width, height, instants, encoder name and settings, the parts, the stills, the lost instants (each exactly once) and the conditions. |

**Bytes:**

    B = Σ video parts + Σ stills + frames.jsonl + clip.json

**Instants (nothing unaccounted):**

    N = N_enc + N_same + N_lost

**Still rule:**

    still at instant k  ⇔  picture(k) ≠ picture(k − 1)  and  picture(j) = picture(k) for every j in (k, k + S − 1]      S = SETTLE_INSTANTS

So the picture is on screen for S instants in a row, k included (0.5 s at S = 10). The still is written at instant k + S − 1 and belongs to instant k. Instant 0 counts as a change. No second still is written until the picture has changed again.

**A still is never stored twice (found by test):** a settled picture whose pixels (SHA-256) equal a stored still gets the index mark `still_of: <instant of that still>` and no file. **Still budget:** the stills written in the last 60 s may not exceed 20 MiB; beyond that the settled picture stays as video. Reason: a blinking text cursor wrote 29 stills in 16 s (70 MB per minute); after the change, 2 files and 27 reuses.

**End of recording:** the encoder is drained, the last part is closed, `clip.json` is written complete. A `record` call that fails before the first picture removes the folder it created and never touches a folder that existed before the call (#290 comment 1375).

**Recovery:** a clip whose recorder died keeps every complete part and every still; instants after the last complete part are named lost, once.

Measured 2026-11-24, server killed mid-recording: the clip opens, the recorded pictures are readable, and the rest are named lost with cause `service_restart`, padded to the requested length. Known limit: the clip then holds lost instants up to the length the `record` call asked for, not only up to the kill.

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

  The sizes in this table were measured with a key picture every 30 s. §7 has the sizes at the chosen 10 s.

- **Readable by a model.** Claude read the same text, character for character, from the source and from the x264, Intel HEVC and SVT-AV1 outputs: typing, the middle of a scroll, and 12–13 pixel grey text beside a playing video. The libaom output had this reading test on 2026-11-24, in the live test on the real YouTube page (play, scroll, type, pause): Claude read the text back correctly.
- **Known limit, measured:** text beside a playing video stays video quality. On the real page the side column was still only 7 % of the time and the header 5 %, because the glow around the player follows the video. Settling by region would not change that.

## 5. Viewer

The reader of a clip is an AI model. Every answer must be true to what was recorded.

- **One picture at time t:**
  1. If a still exists for the instant shown at t, return the still (exact).
  2. Otherwise decode the video from the last key picture at or before t.

         N_dec ≤ KEY_INSTANTS      (200 at a key picture every 10 s)

     Measured stand-in: 340 ms worst case with a key picture every 10 s (libaom AV1 file, ffmpeg's decoder, starting the program included).

     **Decoder (found by test):** the crate rav1d 1.1.0 (Rust port of dav1d, BSD-2-Clause) with frame threading. Pictures are matched to their index line by output count. The first, the deepest and the last picture of a real clip are bit-identical to libaom's decode. Reason: with the first decoder a picture late in a part took 690 ms on the bench and 741 ms in the app (limit 500 ms); with rav1d, 343 ms and 414 ms.
  3. Each picture is returned at the width and height recorded for its part, so a clip with a window size change reads back at each size.
- **Each returned picture says what it is:** `exact still`, `video`, or `lost`. A lost instant is never returned as a recorded picture; the answer names the loss, its cause, and the nearest recorded instants before and after it (#290 comment 1375, finding 2). Built so; measured in the app: an exact still is answered in about 70 ms.
- **Overview:** each stored still is shown once; repeats (`still_of`) are said in words.
- **Clips of an older format:** the viewer answers with an error that names the format (settles O3).
- **Full resolution and crops:** a model shrinks a large image before reading it (P7), so the viewer keeps full resolution and serves regions at native size. This exists today and stays.
- **Findings of the live test by an AI agent to fix here** (#290 comment 1374):
  - the recorder is hidden inside the `screenshot` tool's operation list; it gets its own tool name;
  - `clip_overview` spent all 8 pictures on the first 1.1 s and cut its timeline at 18 s; it must spread over the whole clip;
  - `record_status` uses one-letter condition keys; they get words;
  - `max_long_edge` was ignored; it is honoured or removed from the description;
  - the picture label covers the top-left corner; it moves outside the picture.
- **Change analysis (the old "D5" rules):** its thresholds were derived for a lossless clip. With a video stream they must be derived again from measured AV1 noise on unchanged content. Until that is measured, change analysis compares stills with stills only. Built so: `clip_diff` compares two exact stills only. Open item O2 stays open.

## 6. Parameter map

| constant | start value | source |
|---|---|---|
| instants per second | 20 | existing recorder; all measurements were at 20 |
| encoder | libaom AV1 (crate libaom-sys 0.18.1, libaom 3.15.3), realtime usage, speed (cpu-used) 8, screen content tuning, row multithreading, 4:2:0 8 bit | measurements, libaom section |
| AV1 quality setting | 35, constant quality; the reading test passed at 35 on 2026-11-24 | §4 |
| `KEY_INSTANTS` (key picture distance) | 200 (10 s) | measured on libaom AV1, W0 (2026-11-24): real page 4.04 MB per minute and worst fetch 340 ms at 10 s; 7.40 and 181 ms at 5 s; 3.42 and 676 ms at 30 s |
| `SETTLE_INSTANTS` | 10 (0.5 s) | measurements, second part: 4 settle points in 40 s on the real page |
| still format | WebP lossless (crate image-webp) | P16; measured 697–727 KB per still on the real page, 15 KB on interface work |
| still budget | 20 MiB of stills in the last 60 s; beyond that the settled picture stays as video | a blinking cursor wrote 70 MB per minute of stills without it (§3) |
| encoder queue | 30 converted pictures (1.5 s) in front of the encoder thread; a full queue names the instant lost (`encoder_behind`) | a key picture's encode took 0.25–0.65 s (§2) |
| decoder | rav1d 1.1.0 with frame threading | late picture 343 ms on the bench, 414 ms in the app (§5) |

Removed: zstd level and window, `MOVING_*` constants, the H.264 quality level, the GOP of 20, `MOVING_QUEUE_FRAMES`, the block grid of the motion rule.

## 7. Targets (gates of the build)

Measured with ffmpeg standing in for the recorder, libaom AV1 at quality setting 35 and a key picture every 10 s. The product must reach them on the same scenes; a miss is reported with its numbers, never hidden.

**Correction, 2026-11-24.** The first version of this table took its sizes from runs with a key picture every 30 s while §6 set 5 s. That was Claude's error. W0 measured the trade on libaom itself:

| key picture every | real page, MB per minute | worst fetch | scrolling | mixed |
|---|---|---|---|---|
| 5 s | 7.40 | 181 ms | 3.75 | 0.72 |
| 10 s | 4.04 | 340 ms | 2.69 | 0.53 |
| 15 s | 3.81 | 399 ms | 2.68 | 0.53 |
| 30 s | 3.42 | 676 ms | 2.46 | 0.51 |

10 s is chosen: it is the shortest distance that keeps the real page near 4 MB per minute, and its worst fetch is under 0.5 s.

| measure | scene | target | measured in the app, 2026-11-24 |
|---|---|---|---|
| Size | real YouTube page | video ≤ 4.85 MB per minute (4.04 + 20 %); stills reported apart | video 4.1–5.4 MB per minute, so the runs above 4.85 miss the target; stills up to 2.9; total 5.5–7.4, against 187 for the old recorder |
| Size | scrolling | ≤ 3.23 MB per minute (2.69 + 20 %) | — |
| Size | mixed, 60 s | ≤ 0.64 MB per minute (0.53 + 20 %) | — |
| Size | text-only page, 20 s | none set | 0.55 MB per minute in total |
| Lost instants | every scene | 0, or each named with its cause | real page 0 of 802; text-only page 0 of 402; window minimized 3 s: 0; server killed mid-recording: the rest named lost (`service_restart`) |
| Processor time | every scene | at most 1 core per recording (owner, 2026-11-24: "every pc can spend 1 core", #314); also not above ffmpeg's on the same scene, measured in the same session. Measured so far: mixed 0.43–0.48 of one core, scrolling 0.42–0.67, real page 0.46–0.78 (the higher figures were taken while other work ran on the PC). **Target not required (owner, 2026-11-24: working correctly, not perfect).** | real page 0.45–0.97 of one core over the day's runs; text-only page 0.21 |
| Peak memory of the recorder | every scene | not above ffmpeg's whole program: about 410 MB | peak memory of the whole server 281–296 MB |
| Start | first encoded picture after the `record` call | ≤ 2 instants (100 ms); measured stand-in about 70 ms. **Target not required (owner, 2026-11-24: working correctly, not perfect).** | `record` call answered in 230 ms |
| Stop | `record_stop` to clip ready | none set | 0.6 s |
| Read-back | one picture at any time | ≤ 0.5 s (measured stand-in 340 ms worst case) | late video picture 414 ms (343 ms on the bench); exact still about 70 ms |

Measured by Claude through the real server, one round each: real YouTube page 1904x1012, 40 s (play, scroll, type, pause). Odd and small window sizes on the bench (506x333, 640x360, 1280x720): all checks pass.

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

**State, 2026-11-24:** W0–W8 are done in the form this plan now describes (W0 probe, W1–W5 recording, W6–W7 viewer, W8 old design removed). The differences from the first text of this plan were each found by a test: the encoder thread and the window size change (§2), stills never stored twice and the still budget (§3), the rav1d decoder and the viewer answers (§5), and the build of libaom below.

**Build of libaom (found by test):** libaom must be built with the Ninja generator; with a Visual Studio generator the cmake crate drops /O2 and the encoder takes three times the processor time. It must also be built with the dynamic C runtime through `cmake/msvc-dynamic-runtime.cmake`, because libaom-sys forces the static runtime and the workspace is dynamic.

**Test bench:** `scripts/clip-equation/probe` includes the product's `clip_folder.rs`, `clip_encoder.rs`, `clip_stream.rs` and `clip_decoder.rs` by `#[path]`, so a fix proven on the bench is the fix in the app. Commands: `exp-av1 out=<dir> folder=<corpus scene>` and `exp-av1-fetch folder=<clip> instant=<k> [png=<file>]`. Checks: `node scripts/clip-equation/spec/run-spec.mjs av1-probe out=<dir>`, 10 scenarios: every instant accounted for; nothing lost silently; the video decodes; a still is exact; a still appears when the window settles; unchanged instants cost nothing; read-back; a resized window keeps recording; a late picture comes back in time; the fetched picture is the recorded one. All 10 pass.

Code rules: one type owns one rule; no pass-through layers; named constants only; readable top to bottom by one person.

Not in this plan, each needs its own plan: the UI Automation text and event track (later); the hardware HEVC upgrade (later); change analysis on video (O2).

## 9. Verification round

1. cargo fmt, clippy, then ONE release build (`-j 4`, priority Idle).
2. Every check of §8, one round each. A failed run is read once in full and its cause fixed; no rerun until green; no threshold is loosened. State, 2026-11-24: the 10 bench checks pass.
3. Codex review, read-only. State, 2026-11-24: running, result not in yet.
4. **Live test by Claude through the real server, as an AI uses it:** `record` on a real window (the YouTube page: play, scroll, type `hello test 123`, pause), `record_stop`, then read the clip back through the viewer only. Compare size, processor time, memory and lost instants with §7. The window is open only while it is recorded. State, 2026-11-24: done; the results are in §7.
5. Report in plain comparisons against today's recorder (187 MB per minute = 100 %).

## 10. Issues

- #298: the research, the proof ledger and the measurements behind this plan.
- #290: the faults of the 4.2 recorder. Lost instants on rectangle changes, the oversized rectangle and the 187 MB per minute end with W8. "Lost instant returned as recorded pixels" is W6. "Failed `record` leaves its folder" is W4. The viewer points are W7.
- Still to investigate from #290: three fully black pictures at 12.75–13.05 s of the agent's clip (page or capture, unknown); 12 `CLIP_RECOVERY_FAILED` warnings at every server start for clips of older formats.

## 11. Open items for the owner

- **O1: old limits.** Plan 4.2 carried a 10 % processor rule and a 700 MB memory limit from earlier decisions. §7 replaces them with measured targets. Confirm, or name the limits you want. Owner, 2026-11-24: the processor-time target of 0.5 core and the start target of 100 ms are not pass/fail limits (working correctly, not perfect).
- **O2: change analysis on video** (§5). Derive new thresholds by measurement, or keep it to stills only. Still open; today `clip_diff` compares two exact stills only.
- **O3: clips of older formats** in `%LOCALAPPDATA%\synapse\clips`. The viewer refuses them today and logs a warning for each at every start. Delete them, or keep a reader for them? Settled, 2026-11-24: a clip of an older format gets an error that names the format.
- **O4: merge and install.** Merge `feature/clip-capture` to main and install it into the running service. Owner decides.
