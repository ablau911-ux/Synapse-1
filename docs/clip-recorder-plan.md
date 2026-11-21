# Clip recorder: building plan (version 2)

Status 2026-11-21 19:45 IST. Branch `feature/clip-capture`. Owner: Adam. Builders: Claude agents; review: Codex, read-only.

This is step 3 of the owner's work order: the equation in `docs/clip-recorder-equation.md` turned into an order of operations, each with its desired real-world result. Step 4 is the code, in which every tunable is a named constant that stands for one symbol of the equation.

Version 1 of this plan (commit ad0047d1) was built as commits 7709f70c, 7e4cf3eb and be2447f8. Version 2 changes that code where the equation requires it (equation document, section 5).

## Fixed by the owner

- Flashes from 0.050 s on screen are in the clip. 60 Hz is the reference screen.
- Never above 20 fps and never above 5 % of the processor time, on any machine. A weaker machine takes longer; nothing else gives way.
- Every machine from a Core i3 2.2 GHz with two cores, 4 GB memory and Intel graphics.
- 60 s by default, longer on demand. At least 480 lines. The size follows the window. Quality before bytes.
- Viewers are the existing agents, which see still images. No automated tests (upstream policy #1699).

## The process

The work is split in two because of the 5 % rule: what cannot wait runs in the Synapse service while the screen is recorded, and what can wait runs in a worker that Windows itself limits.

### A. Live part, in the Synapse service

| # | Operation | Desired result | Equation |
|---|---|---|---|
| A1 | Read the machine: thread count, hardware H.264 encoder present, free disk | No constant of one PC in the code | R10 |
| A2 | Check the privacy pause and the disk; refuse to start when paused or the disk is critical | Nothing is recorded that must not be | existing rule |
| A3 | Read the window size, compute σ and the recorded size | Recorded size = window size, whole-number duplication up to at least 480 lines | G |
| A4 | Observe the window through its own capture stream at 60 frames per second, cursor on | Every screen refresh up to 60 Hz is seen | C (1) |
| A5 | For each instant s_k (every 50 ms) choose the last observed frame captured at or before s_k | A late recorder cannot miss a flash | C (2) |
| A6 | If the chosen frame is the one already recorded: write only its line in `frames.jsonl` | A repeated frame costs nothing | B |
| A7 | If it is new: read it back and put it in the waiting folder as one raw frame file, then write its line | The only pixel work in the service is one copy | a_rt |
| A8 | Count observed frames lost and the largest observation gap; measure the processor time of the live threads | The clip can state whether C and B held | conditions |
| A9 | On a window resize, on every 60 s, or on stop: close the segment and start the next one at the current size | The size keeps following the window; a long clip can be opened in parts | G, T |
| A10 | Stop at the requested length, on a stop call, when the window closes, when privacy pauses, or when the disk budget is reached | Length is 60 s by default and can be extended while recording | T |

### B. Worker, the same program started in worker mode

Windows runs the worker inside a job object with a hard processor limit, at below-normal priority and without a window. The limit holds on any machine without the worker measuring anything.

| # | Operation | Desired result | Equation |
|---|---|---|---|
| B1 | Take the waiting frames in order, delete each after use | The waiting folder stays small on a capable machine | t_ready |
| B2 | Duplicate pixels by σ, convert the colours | Recorded size as computed in A3 | G |
| B3 | Encode with H.264 through Media Foundation: the hardware encoder when A1 found one that accepts the size, the Windows software encoder otherwise | The cheapest encoder the machine has | E |
| B4 | Give every encoded frame its instant s_k as time stamp; frames that repeat are not encoded, the picture simply stays | Bytes follow changes, not time | r |
| B5 | Force a keyframe on the first new frame after τ since the last keyframe | Fewest bytes within the seek and crash limits | τ |
| B6 | Quality mode at level q; if refused, bitrate = β · width · height · f, read back | Same quality on every machine | Q |
| B7 | When a segment's frames are done: finish its video file; when all are done: write `clip.json` with the conditions and mark the clip ready | The clip says what it guarantees | conditions |

### C. Calls an agent makes (operations of the `screenshot` tool)

| Operation | Does | Returns |
|---|---|---|
| `record` | Starts a recording (window or monitor, length 60 s unless given) and returns at once | clip id, folder |
| `record_status` | Reads the state | recording, encoding or ready; seconds recorded; frames waiting; conditions so far |
| `record_stop` | Ends the recording now | as `record_status` |
| `record_extend` | Adds time to a running recording | new end time |
| `clip_overview`, `clip_frames`, `clip_frame`, `clip_diff` | The viewer, on a ready clip or on its finished segments | text timeline and labelled PNG images |

### D. Viewer changes

| # | Operation | Desired result | Equation |
|---|---|---|---|
| D1 | Read clips made of several segments with frames at uneven times | Long and resized clips can be viewed | T, G |
| D2 | Shrink overview images by averaging all source pixels, never below 75 % of the recorded size | Text stays readable for the fewest tokens | σ_v |
| D3 | Enlarge close-ups by whole-number duplication ×2 | Small text becomes readable without blur | σ_v |
| D4 | Refuse with a clear error while the part asked for is not ready, saying how many frames wait | No half-finished clip is shown as complete | t_ready |

## Clip folder

    clip.json                 written when the clip is ready
    frames.jsonl              one line per instant: frame, time, new or repeated, keyframe, segment
    segment-0001.mp4 ...      one video file per segment
    waiting/                  raw frames not yet encoded (empty when ready)
    views/                    images the viewer exported

`clip.json` gains a `conditions` block. For each of C, G, Q, B and T: the measured value, the limit, and whether it held; for a failed condition, the instants where it failed.

## Parameter map (step 4: every constant is one symbol)

| Symbol | Constant | File | Value | Source of the value |
|---|---|---|---|---|
| f | `RECORD_FPS` | `server/capture_clip.rs` | 20 | base rule |
| observation rate | `OBSERVE_INTERVAL_MS` | `server/capture_clip.rs` | 16 | 60 Hz reference |
| d_min | `MIN_FLASH_MS` | `server/capture_clip.rs` | 50 | requirement; equals 1000 / f |
| t_def | `DEFAULT_DURATION_MS` | `server/capture_clip.rs` | 60,000 | requirement |
| segment length | `SEGMENT_MS` | `server/capture_clip.rs` | 60,000 | one default clip |
| h_min | `MIN_LINES` | `server/capture_clip.rs` | 480 | requirement |
| U_max | `CPU_LIMIT_PERCENT` | `server/clip_worker.rs` | 5 | base rule |
| worker share of U_max | `WORKER_CPU_PERCENT` | `server/clip_worker.rs` | 4 | 5 minus 1 for the live part (computed live need: at most 0.5 %) |
| disk budget D | `CLIP_DISK_RESERVE_BYTES` | `server/capture_clip.rs` | stop when free disk falls below 2 GB | keeps the machine usable; to confirm with the owner |
| q | `H264_QUALITY_LEVEL` | `server/clip_encoder.rs` | 70 until q_min is measured | built value |
| β | `FALLBACK_BITS_PER_PIXEL_FRAME` | `server/clip_encoder.rs` | 0.10 | measured need 0.09 at level 70 |
| τ | `KEYFRAME_INTERVAL_MS` | `server/clip_encoder.rs` | 5,000 | measured saving 40 to 79 %; crash loss at most 5 s |
| σ_v lower bound | `OVERVIEW_MIN_SCALE` | `server/clip_viewer.rs` | 0.75 | h_leg / h_text = 9 / 12, assumed |
| close-up factor | `CLOSEUP_SCALE` | `server/clip_viewer.rs` | 2 | whole-number duplication |
| N | `MAX_IMAGES_PER_CALL` | `server/clip_viewer.rs` | 8 | built value |

`MAX_FPS`, `MAX_DURATION_MS`, `FALLBACK_BITRATE_BPS` and `KEYFRAME_INTERVAL_SECONDS` are removed.

## Known limit that stays

On the weakest machine with the software encoder, a 1920 × 1080 window that changes on every recorded frame produces raw waiting frames at 166 MB per second. A disk that cannot write that fast loses frames. The recorder does not hide it: the lost frames are counted and condition C is reported as not held for those instants.

## Not in this build

Other codecs than H.264; colour conversion on the graphics chip; a rolling buffer that keeps only the last minutes; triggers; clicks and key presses in the clip; clip retention; removing the yellow border.

## Work packages

Each package is built by one agent from this plan with a time and tool-call limit, passes `cargo fmt --all` and `cargo clippy -p synapse-mcp --all-targets -- -D warnings`, is committed and pushed as `claude`, and is then reviewed by Codex read-only. Findings are fixed before the next package starts.

| # | Package | Contains |
|---|---|---|
| 1 | Live part | A1 to A10, the four `record*` operations, `frames.jsonl`, the waiting folder, conditions C, G, T |
| 2 | Worker | Worker mode of the program, the job object limit, B1 to B7, `clip.json` with all conditions |
| 3 | Viewer | D1 to D4 |
| 4 | Proof and constants | The measurement session below; README and tool-surface tables |

## Proof

After package 3, one session on the development PC with the new build deployed: a still window and a window with changing text and flashes of 50 ms, recorded for 30 s each. It must show, from `clip.json` and the files:

1. Every 50 ms flash is in the clip, and the largest observation gap is at most 16.7 ms.
2. The measured processor share is at most 5 %, and the worker's share at most 4 %.
3. A still window produces no encoder work after the first frame.
4. Recorded size equals the window size; a window below 480 lines is duplicated.
5. The decoded text is readable at level 70, and at which lower level it stops being readable (q_min).
6. The bytes per minute against the equation's figures, which gives K and P for the built encoder.
7. The viewer opens the clip, including a resized one with two segments.

The weakest machine itself cannot be proven here. Its figures remain the computed ones until one run on a real Core i3.
