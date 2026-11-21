# Clip recorder: building plan (version 3)

Status 2026-11-21 20:55 IST. Branch `feature/clip-capture`. Owner: Adam. Builders: Claude agents; review: Codex, read-only.

This is step 3 of the owner's work order: the equation in `docs/clip-recorder-equation.md` turned into an order of operations, each with its desired real-world result. Step 4 is the code, in which every tunable is a named constant that stands for one symbol of the equation, or is computed from such constants.

Version 1 (commit ad0047d1) was built as commits 7709f70c, 7e4cf3eb and be2447f8. Version 2 (commit f354496d) was reviewed against the code on 2026-11-21 (issue #290, two comments). Version 3 folds in that review. What changed from version 2 is listed at the end.

## Fixed by the owner

- Flashes from 0.050 s on screen are in the clip. 60 Hz is the reference screen.
- Never above 20 fps and never above 5 % of the processor time, on any machine. A weaker machine takes longer; nothing else gives way.
- Every machine from a Core i3 2.2 GHz with two cores, 4 GB memory and Intel graphics. Such a machine runs Windows 10, so every Windows feature used must have a Windows 10 path.
- 60 s by default, longer on demand. At least 480 lines. The size follows the window. Quality before bytes.
- Viewers are the existing agents, which see still images. No automated tests (upstream policy #1699; the test surface was removed again in 3f972db5).
- Disk: the recorder stops at storage pressure Level 1 (free disk below 2 GB), the rule that already exists in `synapse-storage` (owner, 2026-11-21).

## What the code had to be corrected on (facts found in the review)

These facts decide the shape of package 1. Each was read in the code.

| # | Fact | Where | Consequence |
|---|---|---|---|
| F1 | The capture handler reads every delivered frame back to the processor and creates a new staging texture for each | `synapse-capture/src/platform/windows/capture.rs`, `copy_texture_to_owned_buffer` | Observing 60 times a second would cost 3 times the read-back the equation allows. The equation's "a frame not chosen is never read back" is false for this code |
| F2 | A frame's time is taken with `Instant::now()` after the read-back, not the time Windows composed it | same file, line 279 | Choosing a frame "by its capture time" is not possible with this time |
| F3 | windows-capture 2.0.0 creates the frame pool with one buffer | `windows-capture-2.0.0/src/graphics_capture_api.rs:179` | While the handler holds a frame, Windows drops new ones. The handler must return in far less than 16.7 ms |
| F4 | The session always asks for "no border", "secondary windows" and a minimum update interval. windows-capture returns `BorderConfigUnsupported`, `SecondaryWindowsUnsupported` or `MinimumUpdateIntervalUnsupported` where Windows lacks them (Windows 10; the interval needs Windows 11 24H2) | same crate, about lines 300 to 353; `capture.rs` `start_graphics_capture_with_item` | The recorder fails at start on the weakest machine. The record path has no fallback (`CaptureBackendPreference::GraphicsCaptureApi`, `capture_clip.rs:423`) |
| F5 | windows-capture exposes the composition time: `Frame::timestamp()` returns `SystemRelativeTime` | `windows-capture-2.0.0/src/frame.rs:123` | F2 can be fixed without another dependency |
| F6 | A self-started worker in a job object already exists | `synapse-mcp/src/desktop_worker.rs`: `DesktopWorkerOp` (line 13), `current_exe` (line 1618), `create_worker_kill_on_close_job` (lines 983 to 1020) | The clip worker is one more operation of this worker, not a new mechanism |
| F7 | Storage pressure Level 1 is free disk below 2 GB; the probe polls every 30 s | `synapse-storage/src/pressure.rs:17` and `:14` | Reuse the threshold. 30 s is too slow while raw frames are written (up to 5 GB in 30 s), so the recorder checks free space itself with the same thresholds |
| F8 | The encoder already turns on hardware encoders and reports which one it got | `clip_encoder.rs:186` (`MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS`), `:406` (`MFT_FRIENDLY_NAME`) | No separate probe for a hardware encoder |
| F9 | The encoder writes fragmented MP4 and falls back to plain MP4 | `clip_encoder.rs:75` and `:84` | With plain MP4, a crash loses the encoded part of the segment |

## The process

The work is split in two because of the 5 % rule: what cannot wait runs in the Synapse service while the screen is recorded, and what can wait runs in a worker that Windows itself limits.

### O. Observer, in `synapse-capture` (new, for clips only)

The existing screenshot paths stay as they are. The clip recorder gets its own frame handler.

| # | Operation | Desired result | Equation |
|---|---|---|---|
| O1 | Start a Graphics Capture session on the window or monitor. Ask for "no border", "secondary windows" and the minimum update interval only where windows-capture reports them supported; use the default otherwise | Recording starts on Windows 10 and 11. On Windows 10 the yellow border shows and frames arrive per composition | R10 |
| O2 | In the frame handler: copy the frame on the graphics chip into the next texture of a small ring, store its composition time (`Frame::timestamp()`) and its dirty region when given, and return. No read-back in the handler | The handler returns in well under 16.7 ms, so the one-buffer pool drops nothing | C (1), C (3) |
| O3 | Measure the longest time the handler took and the longest gap between composition times | The clip can state whether C held (see A8) | conditions |
| O4 | On request for instant s_k: give the newest ring texture whose composition time is at or before s_k, read back to the processor only that one | The only read-back is one per new recorded frame | C (2), a_rt |

Ring size: `OBSERVE_RING` = ⌈T_s / T_r⌉ + 1 = 4 textures (T_s = 50 ms, T_r = 16.7 ms), so a frame that may still be chosen for s_k is never overwritten before s_k is decided.

### A. Live part, in the Synapse service

| # | Operation | Desired result | Equation |
|---|---|---|---|
| A1 | Read the machine: thread count n and free disk | No constant of one PC in the code | R10 |
| A2 | Refuse to start when the privacy gate pauses, when storage pressure is Level 1 or worse, or when another recording is running | Nothing is recorded that must not be; the 5 % rule holds because there is only one recorder | existing rules, B |
| A3 | Read the window size, compute σ and the recorded size; above the encoder limit, use the largest size within it at the same aspect ratio | Recorded size = window size, whole-number duplication up to at least 480 lines; above the limit, G reported not held | G |
| A4 | Start the observer (O1) at the reference rate, cursor on | Every screen refresh up to 60 Hz is seen | C (1) |
| A5 | For each instant s_k (every 50 ms): ask the observer for the frame at or before s_k (O4) | A late recorder cannot miss a flash | C (2) |
| A6 | If the chosen frame is the one already recorded: write only its line in `frames.jsonl` | A repeated frame costs nothing | B |
| A7 | If it is new: put the read-back frame in the waiting folder as one raw frame file, then write its line | The only pixel work in the service is one copy | a_rt |
| A8 | Count, per clip: the longest observer handler time against T_r, the longest composition gap, and the processor time of the live threads including the disk writes | The clip can state whether C and B held. A gap alone cannot tell "nothing changed" from "dropped"; a handler that always returns within T_r with the one-buffer pool can drop nothing | conditions |
| A9 | On a window resize that has held for `RESIZE_SETTLE_MS`, on every 60 s, or on stop: close the segment and start the next one at the current size. While a resize settles, fit frames into the current size with the existing `letterbox` (`capture_clip.rs:881`) | The size follows the window without a new segment for every frame of a drag; finished segments can be viewed while recording goes on | G, T |
| A10 | Check free disk once per second with the storage thresholds (F7). Stop at the requested length, on a stop call, when the window closes, when privacy pauses, or at pressure Level 1 | Length is 60 s by default and can be extended while recording; the disk never fills | T |

### B. Worker: an operation of the existing desktop worker

Windows runs the worker inside the desktop worker's job object (F6) with a hard processor limit added (`JOB_OBJECT_CPU_RATE_CONTROL_ENABLE` with `HARD_CAP`), at below-normal priority and without a window. The limit holds on any machine without the worker measuring anything, and it also covers the encoder's own threads.

| # | Operation | Desired result | Equation |
|---|---|---|---|
| B1 | Take the waiting frames in order, delete each after use | The waiting folder stays small on a capable machine | t_ready |
| B2 | Duplicate pixels by σ, convert the colours | Recorded size as computed in A3 | G |
| B3 | Encode with H.264 through Media Foundation with hardware transforms on (F8). If the writer refuses the size, open it again with hardware transforms off. Record the encoder name | The cheapest encoder the machine has, without a separate probe | E |
| B4 | Give every encoded frame its instant s_k as time stamp; frames that repeat are not encoded, the picture simply stays | Bytes follow changes, not time | r |
| B5 | Force a keyframe (`CODECAPI_AVEncVideoForceKeyFrame`) on the first new frame after τ since the last keyframe. Where the encoder does not support it, set the GOP size to τ · f and report that keyframes may come later than τ | Fewest bytes within the seek limit | τ |
| B6 | Quality mode at level q; if refused, bitrate = β · width · height · f, read back | Same quality on every machine | Q |
| B7 | When a segment's frames are done: finish its video file; when all are done: write `clip.json` with the conditions and mark the clip ready | The clip says what it guarantees | conditions |
| B8 | When the service starts: find clips with waiting frames and no `clip.json`, and start the worker on them | A service restart does not leave a clip half done | t_ready |

Crash loss: the worker deletes a raw frame once it is handed to the encoder. With fragmented MP4 a crash loses only the last fragment; with the plain MP4 fallback (F9) it loses the encoded part of the current segment (at most 60 s). Either way the lost instants are named in `clip.json` (condition T not held for them).

### C. Calls an agent makes (operations of the `screenshot` tool)

| Operation | Does | Returns |
|---|---|---|
| `record` | Starts a recording (window or monitor, length 60 s unless given) and returns at once; refused while another runs | clip id, folder |
| `record_status` | Reads the state | recording, encoding or ready; seconds recorded; frames waiting; conditions so far |
| `record_stop` | Ends the recording now | as `record_status` |
| `record_extend` | Adds time to a running recording | new end time |
| `clip_overview`, `clip_frames`, `clip_frame`, `clip_diff` | The viewer, on a ready clip or on its finished segments | text timeline and labelled PNG images |

### D. Viewer changes

| # | Operation | Desired result | Equation |
|---|---|---|---|
| D1 | Read clips made of several segments with frames at uneven times | Long and resized clips can be viewed | T, G |
| D2 | Shrink overview images by averaging all source pixels, never below 75 % of the **window** size: σ · σ_v ≥ 0.75, so σ_v ≥ 0.75 / σ | Text stays readable for the fewest tokens; a duplicated small window is not sent at its duplicated size | σ_v |
| D3 | Enlarge close-ups by whole-number duplication ×2 | Small text becomes readable without blur | σ_v |
| D4 | Refuse with a clear error while the part asked for is not ready, saying how many frames wait | No half-finished clip is shown as complete | t_ready |

## Clip folder

    clip.json                 written when the clip is ready
    frames.jsonl              one line per instant: frame, time, new or repeated, keyframe, segment
    segment-0001.mp4 ...      one video file per segment
    waiting/                  raw frames not yet encoded (empty when ready)
    views/                    images the viewer exported

`clip.json` gains a `conditions` block:

| Condition | Reported | Held when |
|---|---|---|
| C | longest handler time, longest composition gap, frames lost to the disk | handler time < T_r and no frame lost |
| G | window size, σ, recorded size per segment | recorded size = σ · window size |
| Q | quality level q or the bitrate used, encoder name | (no pass or fail: q_min is measured once, not per clip) |
| B | processor share of the live part and of the worker against 5 % | sum ≤ 5 % |
| T | length, bytes, instants lost | nothing lost |

For a failed condition, the instants where it failed.

## Parameter map (step 4: every constant is one symbol)

| Symbol | Constant | File | Value | Source of the value |
|---|---|---|---|---|
| f | `RECORD_FPS` | `server/capture_clip.rs` | 20 | base rule |
| T_r | `REFERENCE_REFRESH_HZ` | `server/capture_clip.rs` | 60 | base rule; the observer interval is computed from it |
| d_min | (computed) | | 1000 / f = 50 ms | requirement; equals 1000 / f |
| ring | `OBSERVE_RING` (computed) | `synapse-capture` clip observer | ⌈(1000 / f) / (1000 / T_r)⌉ + 1 = 4 | C |
| t_def | `DEFAULT_DURATION_MS` | `server/capture_clip.rs` | 60,000 | requirement |
| segment length | `SEGMENT_MS` | `server/capture_clip.rs` | 60,000 | finished segments are viewable while recording; bounds the plain-MP4 crash loss |
| resize settle | `RESIZE_SETTLE_MS` | `server/capture_clip.rs` | 500 | assumed; checked in the proof |
| h_min | `MIN_LINES` | `server/capture_clip.rs` | 480 | requirement |
| encoder limit | `MAX_ENCODE_WIDTH`, `MAX_ENCODE_HEIGHT` | `server/clip_encoder.rs` | 4096 × 2304 | H.264 level 5.1 frame size limit |
| disk budget D | (none: storage pressure Level 1) | `synapse-storage/src/pressure.rs` | free disk below 2 GB | existing rule; the free-bytes-to-level function is made public there so the thresholds have one owner |
| U_max | `CPU_LIMIT_PERCENT` | `desktop_worker.rs` (clip operation) | 5 | base rule |
| worker share of U_max | `WORKER_CPU_PERCENT` | `desktop_worker.rs` (clip operation) | 4 | 5 minus 1 for the live part; the live share including disk writes is measured in the proof |
| q | `H264_QUALITY_LEVEL` | `server/clip_encoder.rs` | 70 until q_min is measured | built value |
| β | `FALLBACK_BITS_PER_PIXEL_FRAME` | `server/clip_encoder.rs` | 0.10 | equation section 6: 15.0 MB per minute at 1252 × 890, 20 fps, keyframe every 1 s gives 0.090; with 5 s the same run gives 0.054. Derived again once τ is fixed |
| τ | `KEYFRAME_INTERVAL_MS` | `server/clip_encoder.rs` | 5,000 | seek time only (crash loss no longer depends on τ, see B); raised after v_dec is measured if seek allows |
| σ_v lower bound | `OVERVIEW_MIN_SCALE` | `server/clip_viewer.rs` | 0.75 of the window size | h_leg / h_text = 9 / 12, assumed |
| close-up factor | `CLOSEUP_SCALE` | `server/clip_viewer.rs` | 2 | whole-number duplication |
| N | `MAX_IMAGES_PER_CALL` | `server/clip_viewer.rs` | 8 | built value |

`MAX_FPS`, `DEFAULT_FPS`, `MAX_DURATION_MS`, `FALLBACK_BITRATE_BPS` and `KEYFRAME_INTERVAL_SECONDS` are removed. `MIN_FLASH_MS` and `OBSERVE_INTERVAL_MS` from version 2 are not constants: they are computed from f and T_r.

## Known limits that stay

- On the weakest machine with the software encoder, a 1920 × 1080 window that changes on every recorded frame produces raw waiting frames at 166 MB per second. A disk that cannot write that fast loses frames. The recorder does not hide it: the lost frames are counted and condition C is reported as not held for those instants.
- Disk writes cost processor time that the equation's a_rt does not contain. The proof measures it. If the live part goes above 1 %, the next version stores only the changed rectangle of a new frame (dirty regions exist from Windows 11 24H2).
- On Windows 10 the yellow capture border shows, and frames arrive per composition (up to the display rate) instead of at a set interval.

## Not in this build

Other codecs than H.264; colour conversion on the graphics chip; a rolling buffer that keeps only the last minutes; triggers; clicks and key presses in the clip; clip retention; removing the yellow border; storing only changed rectangles; making `screenshot gif` (`server/capture_gif.rs`, its own capture loop, issues #285 and #286) an export of a clip.

## Work packages

Each package is built by one agent from this plan with a time and tool-call limit, passes `cargo fmt --all` and `cargo clippy -p synapse-mcp -p synapse-capture --all-targets -- -D warnings`, is committed and pushed as `claude`, and is then reviewed by Codex, read-only. Findings are fixed before the next package starts.

| # | Package | Contains |
|---|---|---|
| 1 | Observer | O1 to O4 in `synapse-capture`; existing screenshot paths unchanged |
| 2 | Live part | A1 to A10, the four `record*` operations, `frames.jsonl`, the waiting folder, the public free-bytes-to-level function in `synapse-storage`, conditions C, G, T |
| 3 | Worker | The clip operation of the desktop worker, the processor limit on its job, B1 to B8, `clip.json` with all conditions |
| 4 | Viewer | D1 to D4 |
| 5 | Proof and constants | The measurement session below; README and tool-surface tables |

## Proof

After package 4, one session on the development PC with the new build deployed. Test window: a local HTML page that draws a counter on every screen refresh (requestAnimationFrame) and shows a 50 ms flash at set times (`scripts/clip-equation/flash-page.html`). A still window is recorded as well. 30 s each. It must show, from `clip.json` and the files:

1. Every 50 ms flash is in the clip. The counter read from each recorded frame equals the refresh shown at s_k (no frame older than one refresh).
2. The longest observer handler time is below 16.7 ms.
3. The processor share is at most 5 %: the live part, including disk writes, at most 1 %; the worker at most 4 %.
4. A still window produces no encoder work after the first frame.
5. Recorded size equals the window size; a window below 480 lines is duplicated; a drag-resize gives one new segment, not one per frame.
6. The decoded text is readable at level 70, and at which lower level it stops being readable (q_min).
7. The bytes per minute against the equation's figures, which gives K and P for the built encoder; v_dec for τ.
8. The viewer opens the clip, including a resized one with two segments.
9. Stopping the service while the worker encodes, then starting it again, finishes the clip (B8).

Not provable on this PC, checked once a Windows 10 machine is available: O1 on Windows 10 and the encoder found there. The weakest machine's figures remain the computed ones until one run on a real Core i3.

## Owner decisions still open

1. Go for package 1.
2. Worker share 4 % of the 5 % (recommended).
3. The worker as an operation of the existing desktop worker, a separate process (recommended: a job object is the only hard limit that also covers the encoder's own threads).

## Changes from version 2

- New section O and package 1: the capture handler did not meet C (F1 to F5).
- A1: no hardware encoder probe (F8); B3 retries without hardware transforms when the size is refused.
- A2: one recording at a time; disk by storage pressure Level 1 instead of a new 2 GB constant (F7).
- A3: encoder size limit. A8: loss measured by handler time, not by gaps. A9: resize settle time. A10: disk checked every second.
- B: the worker is an operation of the existing desktop worker (F6). B5 fallback. B8 resume after restart. Crash loss stated.
- D2: 75 % of the window size, not of the recorded size.
- Conditions: Q is reported without pass or fail.
- Constants: d_min and the observer interval computed; τ bounded by seek only; β's source named; Windows 10 named as the floor.
- Proof: counter page, handler time, disk cost, resize, restart.
