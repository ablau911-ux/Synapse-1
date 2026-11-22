# Clip recorder: building plan (version 4)

Status 2026-11-22 02:45 IST. Branch `feature/clip-capture`. Owner: Adam. Builders: Claude agents; review: Codex, read-only.

This is step 3 of the owner's work order: the equation in `docs/clip-recorder-equation.md` turned into an order of operations, each with its desired real-world result. Step 4 is the code, in which every tunable is a named constant that stands for one symbol of the equation, or is computed from such constants.

History. Version 1 (commit ad0047d1) was built as commits 7709f70c, 7e4cf3eb and be2447f8. Version 2 (f354496d) was reviewed against the code (issue #290). Version 3 (30392a46) folded in that review and was itself reviewed by Codex and Opus, consolidated, and decided by Fable. The full record is in `docs/clip-recorder-review-v3/` (`1-codex-gpt-6-astra.md`, `2-opus-4-8.md`, `3-consolidation-and-jev.md`, `4-fable-decisions.md`; the last one is binding). Decision numbers below (S1 to S9, C1 to C18, O1 to O5, E1 to E8) refer to `4-fable-decisions.md`.

What changed since version 3, in short:

- The owner answered E1 to E8 (2026-11-22). Nothing is open for the owner before packages 0 and 1.
- A new package 0 measures the real costs before any product code is built.
- Section O: settings gated one by one, Windows 10 1903 as the floor, frame slots assigned by instant instead of a 4-texture ring, a threading rule, identity before read-back, no claim that a short handler proves no loss.
- Section A: no scaling in the live part, a privacy check before every raw write, a disk rule on the clip volume, durable clip state from the start, the worker cap computed each second.
- Section B: a new long-lived worker mode (package 3 split into 3a and 3b), one worker and one queue, raw frames kept until their segment is committed, keyframe interval counted in frames, no forced keyframe.
- Section D: an image ceiling and a token budget beside the 75 % floor, region-level transients, caret and animation rules, cursor mask.
- New section "Kept from version 1". Proof rewritten as manual cases.
- File:line references were checked again with Serena on 2026-11-22; several v3 references were one line off and are corrected.

The full list is at the end.

## Fixed by the owner

- Flashes from 0.050 s on screen are in the clip. 60 Hz is the reference screen.
- Never above 20 fps and never above 5 % of the processor time, on any machine. A weaker machine takes longer; nothing else gives way except as E1 below states.
- Every machine from a Core i3 2.2 GHz with two cores, 4 GB memory and Intel graphics. Such a machine runs Windows 10, so every Windows feature used must have a Windows 10 path.
- 60 s by default, longer on demand. At least 480 lines. The size follows the window. Quality before bytes.
- Viewers are the existing agents, which see still images. No automated tests (upstream policy #1699; the test surface was removed again in 3f972db5).
- Disk: the recorder stops at storage pressure Level 1 (free disk below 2 GB), the rule that already exists in `synapse-storage` (owner, 2026-11-21).

Answered on 2026-11-22 (the owner said "continue" after Fable's recommendations, so each question takes Fable's recommendation):

| # | Question | Answer (2026-11-22) |
|---|---|---|
| E1 | What gives way when a machine cannot meet every condition | Time first (the clip is ready later). Then length: the recording stops at storage pressure Level 1, the existing rule. Only after both, frames for named instants are lost, counted and reported per instant. Never the 5 % processor cap, never the size, never the quality. |
| E2 | The worker as a separate process | Yes. This overrides the version 1 owner decision "no child processes". A job object is the only hard limit that also covers the encoder's own threads. |
| E3 | Worker share of the processor | 5 % minus the live part's measured share, recomputed every second. Not a fixed 4 %. |
| E4 | Frames captured while a window is dragged to a new size | One transition segment whose canvas is the largest size seen during the drag; frames are placed on it unscaled. G is reported as "padded" for those instants. |
| E5 | Minimum Windows version | Windows 10 version 1903 (build 18362). Windows capture does not exist before 1803 and windows-capture 2.0.0 needs 1903. On Windows 10 the yellow border always shows. |
| E6 | Monitor recording while an excluded app is visible but not in front | Recording refuses to start, or stops, when an excluded app is visible anywhere on the recorded monitor, even if it is not in front. |
| E7 | Go | Packages 0 and 1 may start once version 4 is committed. |
| E8 | Raw write volume through the disk | Accepted for this build: about 31 MB per second, about 113 GB per hour, at 1920 × 1080 with typical activity (3.8 new frames per second). Decided again after package 0 has measured the real figure. |

## What the code had to be corrected on (facts found in the reviews)

These facts decide the shape of packages 0 to 4. Each was read in the code; line numbers were checked with Serena on 2026-11-22 unless marked "crate".

| # | Fact | Where | Consequence |
|---|---|---|---|
| F1 | The capture handler reads every delivered frame back to the processor and creates a new staging texture for each | `synapse-capture/src/platform/windows/capture.rs:285` (`copy_texture_to_owned_buffer`) | Observing 60 times a second would cost 3 times the read-back the equation allows. The clip gets its own handler (section O) |
| F2 | A frame's time is taken with `Instant::now()` after the read-back, not the time Windows composed it | same file, line 279 | Choosing a frame by its capture time is not possible with this time |
| F3 | windows-capture 2.0.0 creates the frame pool with one buffer | crate: `windows-capture-2.0.0/src/graphics_capture_api.rs:179` | While the handler holds a frame, Windows drops new ones. The handler must return quickly. A short handler alone does not prove that nothing was lost (C1) |
| F4 | The shared session settings are never "Default": cursor, border, secondary windows, interval and dirty regions are always set explicitly. windows-capture rejects each one where Windows lacks it (cursor included) | `capture.rs:129-152`; `capture_clip.rs:418-424`; crate: `graphics_capture_api.rs` about lines 134 to 158 and 300 to 353 | The recorder fails at start on Windows 10. Section O gates every setting (S2). The record path pins the Graphics Capture backend (`capture_clip.rs:424`) |
| F5 | windows-capture exposes the composition time: `Frame::timestamp()` returns `SystemRelativeTime` | crate: `windows-capture-2.0.0/src/frame.rs:123` | F2 can be fixed without another dependency |
| F6 | A self-started process in a job object already exists: `DesktopWorkerOp`, `current_exe`, `create_worker_kill_on_close_job` | `synapse-mcp/src/desktop_worker.rs:13`, `:1619`, `:983` | The clip worker reuses these two primitives only (see F10) |
| F7 | Storage pressure Level 1 is free disk below 2 GB; the probe polls every 30 s | `synapse-storage/src/pressure.rs:18` (`LEVEL_1_FREE_BYTES`) and `:15` (`POLL_INTERVAL`) | Reuse the threshold. 30 s is too slow while raw frames are written, so the recorder applies the threshold itself every second on the clip volume (C13) |
| F8 | The encoder already turns on hardware encoders and reports which one it got | `clip_encoder.rs:187` (`MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS`), `:407` (`MFT_FRIENDLY_NAME`) | No separate probe for a hardware encoder |
| F9 | The encoder writes fragmented MP4 and falls back to plain MP4 | `clip_encoder.rs:75` and `:84` | Durability no longer rests on the container: raw frames stay until a segment is committed (S6) |
| F10 | The existing desktop worker launcher is a bounded one-shot: it starts the child suspended on a hidden desktop, waits at most 10 s, reads one envelope, then ends it; it needs a window handle | `desktop_worker.rs:1598` (`WORKER_TIMEOUT_MS` = 10,000), `:1663` (`lpDesktop`), `:1683` (`CREATE_SUSPENDED`), `:1740` (`wait_for_worker_process`) | An encoder that runs for minutes and survives a restart cannot be "one more operation" of this launcher. It is a new long-lived worker mode (S3) |
| F11 | Each spawn creates its own anonymous job | `desktop_worker.rs:993` (`CreateJobObjectW(None, ...)`) | A processor cap on the clip worker's job does not throttle other worker operations |
| F12 | The `record` contract requires a read-back of `clip.mp4` path, size and SHA-256 | `tool_profiles.rs:391-396`; computed by `capture_clip.rs:388` (`file_sha256`) | With an asynchronous recorder and segment files the contract must be rewritten (O1) |
| F13 | The decoder assumes a keyframe every second and opens one `clip.mp4` | `clip_decoder.rs:28-29` (`FORWARD_READ_LIMIT_MS` = 1,000), `:65` (`open`) | The forward-read limit is computed from the keyframe interval (O2); segment reading is D1 |
| F14 | The viewer has an absolute ceiling (long edge 1280) and 8 images per call, no cursor mask, and counts a change as short-lived only when the whole picture returns | `clip_viewer.rs:49` (`DEFAULT_LONG_EDGE`), `:46` (`MAX_IMAGES_PER_CALL`), `:19` and `:637` (no cursor mask), `:548-552` (whole-picture revert test) | Keep a ceiling beside the floor (S4); region-level transients and noise rules (C15) |
| F15 | The encoder time-stamps frames from a frame counter at a constant rate | `clip_encoder.rs:116-119` (`write_nv12`), `:452` (`frame_time_hns`) | `write_nv12` must take the instant and the duration (O3, C8) |
| F16 | The equation's read-back cost was measured as a memory copy, and the cost of observing a frame is absent | `scripts/clip-equation/bench.rs:74` (`copy_from_slice`); `run_equation.py:14` (`C_READ`) | The live budget is unproven until package 0 measures staging copy, Map and c_obs (S1) |
| F17 | `CapturedFrame` carries only `captured_at: Instant` | `synapse-capture/src/frame.rs:30` | Clip frames are not routed through `CapturedFrame`; the observer keeps the composition time itself |

## The process

The work is split by the 5 % rule. What cannot wait runs in the Synapse service while the screen is recorded (sections O and A). What can wait runs in a worker that Windows itself limits (section B). Package 0 measures the costs of both before either is built.

### 0. Measurements first (package 0, in `scripts/clip-equation/`)

Package 0 changes no product code. It writes measurement programs and one results file next to `bench.rs`, `measure_encoder.ps1` and `run_equation.py`, and updates `run_equation.py` with the measured terms. Its figures fill the parameter map; no new plan version is needed for them.

| # | Measurement | Replaces or feeds | Equation |
|---|---|---|---|
| M1 | Real read-back cost: GPU copy into a staging texture, `Map`, tight row copy, at 854 × 480, 1280 × 720 and 1920 × 1080 | c_read (today a memory copy, F16) | a_rt |
| M2 | Observer cost per delivered frame (GPU copy into a slot plus the time stamp, no read-back) at the display rate | c_obs (absent today) | a_rt |
| M3 | Delivery: with the counter page, does Windows deliver every refresh with the interval at Default (the Windows 10 path), at 16.7 ms and at 11 ms; the observed rate on this 144 Hz display with Default; the longest gap | f_o, `REQUESTED_UPDATE_INTERVAL_MS`; delivery completeness | C (1) |
| M4 | New-frame rate n_new while the mouse moves over the window, while a text caret blinks, and while text is typed | n_new (today 3.8 per second from one window) | a_rt, write volume |
| M5 | Raw write volume and its processor cost: MB per second and processor time to write raw frames to the clip volume; the sustained rate this PC's disk takes before frames would be lost | c_disk (a term missing from a_rt), E8 figure | a_rt, T |
| M6 | Threading rule: multithread protection on the capture device; `Map` with "do not wait" and retry against a blocking `Map`; the longest hold time of the handler while the reader maps | C3 mechanism | C (3) |
| M7 | Selection lateness: how many instants the reader runs behind the open instant under load | `MAX_SELECTION_LATENESS_INSTANTS`, slot count | C (3) |
| M8 | Encoder settings on the Windows software encoder and, where present, the hardware encoder: GOP size of 20 frames accepted and read back; B-picture count 0 accepted and read back; screen-content hint accepted or not; keyframe positions found in the output | `GOP_FRAMES`, O4 | τ, r |
| M9 | The live share at the measured values, which gives the worker's remainder | worker cap formula | B |

Package 0 reports, for each figure, the development PC value and the weakest-machine estimate (scaled as in equation section 6). The weakest-machine figures stay estimates until a run on a real Core i3.

### O. Observer, in `synapse-capture` (new, for clips only)

The existing screenshot paths stay as they are. The clip recorder gets its own frame handler and its own capture handle.

| # | Operation | Desired result | Equation |
|---|---|---|---|
| O1 | Refuse to start below Windows 10 1903 (`MIN_WINDOWS_BUILD`). Build the session settings for the clip: ask for cursor, "no border", secondary windows, the minimum update interval and dirty regions each only where windows-capture reports that setting supported; use Default otherwise. Where the interval is supported, request `REQUESTED_UPDATE_INTERVAL_MS` | Recording starts on Windows 10 1903 and later and on Windows 11. On Windows 10 the yellow border shows and frames arrive per composition, up to the display rate | R10 |
| O2 | In the frame handler: read the composition time (`Frame::timestamp()`), compute the instant k it belongs to, and copy the frame on the graphics chip into the candidate slot of the open instant. When a frame for a later instant arrives, pin the candidate slot: it is the frame for s_k. Store the composition time and the dirty region when given. Return. The handler only issues GPU copies and never maps | The handler holds the one-buffer pool for as short a time as possible; the chosen frame per instant is fixed by composition time, not by when the reader gets to it | C (2), C (3) |
| O3 | Measure the longest hold time (composition time to frame release) and count pinned-slot overflows. Each overflow is a lost instant, recorded with its cause | The clip reports proven losses and the hold time against T_r | conditions |
| O4 | On request for instant s_k: return the identity (composition time) of the pinned frame first. Read back to the processor only when the caller asks for it after its repeat check, on the reader thread | A repeated frame costs no read-back; the only read-back is one per new recorded frame | C (2), a_rt |

Slots. `OBSERVE_SLOTS` = `MAX_SELECTION_LATENESS_INSTANTS` + 2: one candidate slot for the open instant, one pinned slot per instant the reader may still be behind, and one spare. A pinned slot is never overwritten. When no slot is free, the instant is recorded as lost. The lateness bound comes from package 0 (M7).

Threading rule (C3). Multithread protection is on for the capture device. The handler only issues GPU copies and never maps. The reader thread never holds the device context across a wait. The mechanism (package 0 proposes `Map` with "do not wait" and a retry) is confirmed by M6.

Requested interval (C1). `REQUESTED_UPDATE_INTERVAL_MS` starts at 11 ms, because the research measured about 45 fps at a 16.7 ms request and about 60 fps at 11.1 ms on a 60 Hz display (windows-capture issue #190). Package 0 confirms or replaces the value. Where the interval is unsupported (Windows 10 and Windows 11 before 24H2), Windows delivers per composition and the slots absorb rates above 60 per second.

What a clip can and cannot claim (C1). A handler that returns quickly does not prove that nothing was lost: Windows' delivery, scheduling and pool recreation happen outside it. Each clip therefore reports three separate things: proven losses (overflow, privacy discard, disk, load shedding), the longest hold time against T_r, and delivery completeness as "checked only in the proof". Clip frames are not routed through `CapturedFrame` (F17).

### A. Live part, in the Synapse service

The live part does no scaling and no encoding. It stores every new frame at its captured size (C5).

| # | Operation | Desired result | Equation |
|---|---|---|---|
| A1 | Read the machine: thread count n, Windows build, and the free space of the volume that holds the clip folder | No constant of one PC in the code | R10 |
| A2 | Refuse to start when: the privacy gate pauses; the target app is excluded; for a monitor, an excluded app is visible anywhere on it (E6); storage pressure on the clip volume is Level 1 or worse; another recording is running; the folder exists; Windows is older than 1903 | Nothing is recorded that must not be; the 5 % rule holds because there is only one recorder | existing rules, B |
| A3 | Read the window size, compute σ and the recorded size for the report and for the worker. The live part itself does not scale | The worker knows the size; the live part stays one copy per new frame | G |
| A4 | Write `clip.json` at once with state `recording` (written to a temporary name, then renamed), list the clip in the active-clip registry when its folder is not the default one, and read the file back for the `record` response | A crash at any point leaves a state to recover from; `record` returns something it has verified (O1) | t_ready, T |
| A5 | Start the observer (O1) | Every screen refresh that Windows delivers is seen | C (1) |
| A6 | For each instant s_k (every 1000 / f ms): ask the observer for the identity of the frame for s_k (O4) | A late recorder cannot miss a flash | C (2) |
| A7 | If the identity equals the last recorded one: write only its line in `frames.jsonl` | A repeated frame costs nothing | B |
| A8 | If it is new: check the privacy gate before the write, frame zero included (C12). When the gate pauses, frames composed after the pause instant are discarded, also those already in a slot. Otherwise read the frame back (O4), write it into `waiting/` as one raw frame file at its captured size, then write its line with the frame size and the cursor position (C15) | Nothing private reaches the disk; the only pixel work in the service is one copy | a_rt |
| A9 | Measure the live part's processor share every second, disk writes included. Set the worker's cap to `CPU_LIMIT_PERCENT` minus that share (E3). If the live part alone reaches its share of 5 %, it skips the read-back of new frames for the rest of that second; those instants are lost and named (E1) | The sum never exceeds 5 %; time gives way first, frames only last | B |
| A10 | Count, per clip: proven losses with their cause, the longest hold time against T_r, the live share, and the worker cap given | The clip can state which conditions held | conditions |
| A11 | Every `DISK_CHECK_MS`, on the volume that holds the clip folder: free space as measured, minus the bytes written since the measurement, minus a reserve for the segment being finalized (C13). Stop when that falls below the storage Level 1 threshold (F7) | The disk keeps the existing reserve | T |
| A12 | Stop at the requested length, on a stop call, when the window closes, when privacy pauses, under the disk rule, or on a write failure. `record_extend` moves the end. A write failure is a recoverable stop that keeps committed segments. State becomes `encoding` with a stop reason (C14) | Length is 60 s by default and can be extended; no stop loses committed work | T |

Ownership (C14). The recording belongs to the service, not to the MCP call that started it, and runs to its requested end. `record_stop` is idempotent by clip id. Committed segments always stay.

Segments are not formed live. The worker forms them from `frames.jsonl` (B2), so a window drag costs the live part nothing.

### B. Worker: a new long-lived worker mode of the same executable

The worker is a separate process (E2), started from `current_exe` (`desktop_worker.rs:1619`) inside a job made by `create_worker_kill_on_close_job` (`desktop_worker.rs:983`). It reuses those two primitives only. It does not use the desktop worker's launcher: no hidden desktop, no window handle, no 10 s wait, no one-shot envelope (F10).

The job gets a hard processor limit (`JOB_OBJECT_CPU_RATE_CONTROL_ENABLE` with `HARD_CAP`). The service sets that limit every second to 5 % minus the live share it measured (A9). When the remainder is zero, the worker waits; the clip is ready later. The worker runs at below-normal priority and without a window. The limit also covers the encoder's own threads. Each spawn has its own job (F11), so the cap touches no other worker.

One clip worker runs at a time for the whole service, with a first-in first-out queue that recovered clips also enter (C11). The job is kill-on-close, so the worker ends with the service; B10 resumes the work.

Package 3a builds the process, the cap, the queue and recovery (B1, B2, B8 to B10). Package 3b builds the encoding (B3 to B7).

| # | Operation | Desired result | Equation |
|---|---|---|---|
| B1 | Take the next clip from the queue; read its `clip.json` state and committed segments | Work resumes where it stopped | t_ready |
| B2 | Form segments from `frames.jsonl`: close a segment every `SEGMENT_MS`, when its retained raw bytes reach `SEGMENT_RAW_BUDGET_BYTES`, when the window size has been stable for `RESIZE_SETTLE_MS` at a new size, and at stop. Frames captured during a drag form one transition segment whose canvas is the largest size seen during the drag; frames are placed on it unscaled (E4) | The size follows the window without one segment per drag frame; pixels stay exact | G, T |
| B3 | Duplicate pixels by σ and convert the colours. Above the encoder bound, use the largest size within the bound at the same aspect ratio | Recorded size as computed in A3; above the bound, G reported not held | G |
| B4 | Encode with H.264 through Media Foundation with hardware transforms on (F8). The encoder is the one Media Foundation grants, named in the clip. On any encoder failure, restart the segment once from its retained raw frames with hardware transforms off. On a second failure, the clip ends as `failed` and its committed segments stay (C7) | A working encoder on every machine, without a separate probe; no claim that it is the cheapest | E |
| B5 | Time stamps (C8, O3): each encoded frame gets its instant as the time within its segment; its duration is the next new frame's instant minus its own; the last sample runs to the stop time or the segment end. Every segment starts with a keyframe of the picture shown at its start. Repeated frames are not encoded. `write_nv12` takes the instant and the duration | Bytes follow changes, not time; each segment plays on its own; `frames.jsonl` maps global time to segment and local time | r |
| B6 | Keyframe interval in encoded frames: `GOP_FRAMES` = τ · f = 20 while τ = 1 s. No forced keyframe (C9). No B-frames, set and read back (O4), because variable time stamps assume no reordering. The screen-content hint is tried and its acceptance is recorded | Seek cost is bounded by frames decoded; a slowly changing window still gets few keyframes | τ |
| B7 | Quality mode at level q, read back; if refused, bitrate = β · width · height · f, read back | Same quality rule on every machine; Q gets an honest status (C6) | Q |
| B8 | Commit a segment (S6): write `segment-NNNN.mp4.part`, finalize, flush to disk, rename to `segment-NNNN.mp4`, compute its size and SHA-256, record its checkpoint in `clip.json` (temporary name, then rename): file, size, SHA-256, instant range, keyframe instants read from the finished file, encoder, Q status. Only then delete the segment's raw frames | A crash never loses frames that were written; finished segments can be viewed while recording goes on | T |
| B9 | When every segment is committed: write the conditions into `clip.json` and set the state to `ready` | The clip says what it guarantees | conditions |
| B10 | When the service starts: read the active-clip registry and the default clips folder. Every clip in state `recording` or `encoding` enters the queue. A `.part` file is deleted and its segment re-encoded from raw. A clip found in state `recording` becomes `encoding` with stop reason `service_restart`; the instants after its last raw frame are named as lost | A restart never leaves a clip half done or silently short; recovery works from the state, not from which files happen to exist (C10) | t_ready |

Crash loss. A worker crash loses nothing: the segment is encoded again from its raw frames. A service crash during recording loses only the raw frames not yet written; those instants are named in `clip.json`. Durability no longer depends on fragmented MP4.

Keyframe column. The keyframe flag is not known while recording. The worker reads the keyframe instants from each finished segment (B8); the viewer joins them with `frames.jsonl`. The proof compares them with the GOP setting.

### C. Calls an agent makes (operations of the `screenshot` tool)

| Operation | Does | Returns |
|---|---|---|
| `record` | Starts a recording (window or monitor, length 60 s unless given) and returns at once; refused while another runs | clip id, folder, and the `clip.json` written at start, read back: path, size, SHA-256 |
| `record_status` | Reads the state | state (`recording`, `encoding`, `ready`, `failed` with reason), stop reason, seconds recorded, frames waiting, committed segments with size and SHA-256, conditions so far |
| `record_stop` | Ends the recording now; idempotent by clip id | as `record_status` |
| `record_extend` | Adds time to a running recording | new end time |
| `clip_overview`, `clip_frames`, `clip_frame`, `clip_diff` | The viewer, on a ready clip or on its committed segments | text timeline and labelled PNG images |

Contract (O1). `FACADE_TOOL_CONTRACTS` (`tool_profiles.rs:391-396`) gets updated entries for `record`, the three new operations and the viewer: files, read-back and error codes. The `clip.mp4` read-back is replaced by the `clip.json` read-back at start and the per-segment size and SHA-256 at commit.

### D. Viewer changes

| # | Operation | Desired result | Equation |
|---|---|---|---|
| D1 | Read clips made of several segments with frames at uneven times, and with keyframe instants from the segment checkpoints | Long and resized clips can be viewed | T, G |
| D2 | Shrink overview images by averaging all source pixels, never below 75 % of the **window** size (σ · σ_v ≥ 0.75, so σ_v ≥ 0.75 / σ), and never above `VIEW_MAX_LONG_EDGE` per image. Where the ceiling cuts below the floor, the response says so and points to `clip_frame` (S4) | Text stays readable for the fewest tokens; no image is so large that a client downsizes it or the session breaks | σ_v |
| D3 | Enlarge close-ups by whole-number duplication ×2 | Small text becomes readable without blur | σ_v |
| D4 | Refuse with a clear error while the part asked for is not committed, saying how many frames wait | No half-finished clip is shown as complete | t_ready |
| D5 | Detect a short-lived change per changed region, not only when the whole picture returns. Mask the cursor using the logged cursor position. Ignore the blinking caret. Report a continuously animating region as one interval with one frame (C15) | A 50 ms local flash during other animation is found; noise does not flood the timeline | C, viewer |
| D6 | A token budget per call sets N (at most `MAX_IMAGES_PER_CALL`), using the equation's token formula. The timeline is returned on page 1 only (S4) | A call stays inside the client's result cap | N |
| D7 | The decoder's forward-read limit is computed from the keyframe interval, in frames (O2) | Seek stays bounded when τ changes | τ |

## Clip folder

    clip.json                    written at start; state, checkpoints and conditions; replaced atomically
    frames.jsonl                 one line per instant: instant, time, new or repeated, frame size, cursor position
    segment-0001.mp4 ...         one video file per committed segment
    segment-NNNN.mp4.part        the segment being encoded; deleted on recovery
    waiting/                     raw frames not yet committed in a segment (empty when ready)
    views/                       images the viewer exported

Active clips in folders other than the default one are listed in one registry file under the default clips folder.

`clip.json` has a state (`recording`, `encoding`, `ready`, `failed` with reason), a stop reason, the committed segments, and a `conditions` block:

| Condition | Reported | Status |
|---|---|---|
| C | proven losses per instant with cause (slot overflow, privacy discard, disk, load shedding, service restart); longest hold time against T_r; delivery completeness | held when no instant was lost; delivery completeness is "checked only in the proof" |
| G | window size, σ, recorded size per segment | held when recorded size = σ · window size; "padded" for transition-segment instants; not held above the encoder bound |
| Q | quality level q or the bitrate used, encoder name | unverified until q_min is calibrated for the encoder used; held when quality mode at q ≥ q_min was accepted and read back; not held on an uncalibrated bitrate fallback (C6) |
| B | live share, worker cap given and worker share, against 5 %; time until ready | held when the sum ≤ 5 % in every second |
| T | length, bytes, instants lost, stop reason | held when nothing was lost |

For a failed condition, the instants where it failed.

## Parameter map (step 4: every constant is one symbol)

| Symbol | Constant | File | Value | Source of the value |
|---|---|---|---|---|
| f | `RECORD_FPS` | `server/capture_clip.rs` | 20 | base rule |
| f_r | `REFERENCE_REFRESH_HZ` | `server/capture_clip.rs` | 60 | base rule |
| T_r | (computed) | | 1 / f_r = 16.7 ms | C17: period and frequency are separate symbols |
| d_min | (computed) | | 1 / f = 50 ms | requirement |
| requested interval | `REQUESTED_UPDATE_INTERVAL_MS` | `synapse-capture` clip observer | 11 until M3 | research: 11.1 ms gave every refresh at 60 Hz; used only where supported |
| selection lateness | `MAX_SELECTION_LATENESS_INSTANTS` | `synapse-capture` clip observer | from M7 | package 0 |
| slots | `OBSERVE_SLOTS` (computed) | `synapse-capture` clip observer | lateness + 2 | C2 |
| Windows floor | `MIN_WINDOWS_BUILD` | `synapse-capture` clip observer | 18362 (Windows 10 1903) | E5 |
| t_def | `DEFAULT_DURATION_MS` | `server/capture_clip.rs` | 60,000 | requirement |
| segment length | `SEGMENT_MS` | worker | 60,000 | finished segments are viewable while recording goes on; the 30-minute proof case decides whether time segments stay |
| segment raw budget | `SEGMENT_RAW_BUDGET_BYTES` | worker | from M5 | S6; bounds the raw bytes held for one segment |
| resize settle | `RESIZE_SETTLE_MS` | worker | 500 | assumed; checked in the proof |
| h_min | `MIN_LINES` | `server/capture_clip.rs` | 480 | requirement |
| encoder bound | `MAX_ENCODE_MACROBLOCKS`, `MAX_ENCODE_SIDE` | `server/clip_encoder.rs` | 36,864 macroblocks, each side at most 4096 | a chosen bound (C16); the encoder's real acceptance is found when it opens, portrait sizes included |
| disk budget D | (none: storage pressure Level 1) | `synapse-storage/src/pressure.rs:18` | free disk below 2 GB on the clip volume | existing rule; the free-bytes-to-level function is made public there so the threshold has one owner |
| disk check | `DISK_CHECK_MS` | `server/capture_clip.rs` | 1,000 | F7: the 30 s probe is too slow for raw writes |
| finalize reserve | (computed) | `server/capture_clip.rs` | raw bytes of the open segment + their estimated encoded size | C13 |
| excluded-app check | `EXCLUDED_APP_CHECK_MS` | `server/capture_clip.rs` | 1,000 | E6: one window enumeration per second |
| U_max | `CPU_LIMIT_PERCENT` | clip worker mode | 5 | base rule |
| worker cap | (computed every second) | clip worker mode | `CPU_LIMIT_PERCENT` − measured live share | E3; the approved equation's 0.05 · n − a_rt |
| q | `H264_QUALITY_LEVEL` | `server/clip_encoder.rs` (today line 35) | 70 until q_min is measured | built value |
| β | `FALLBACK_BITS_PER_PIXEL_FRAME` | `server/clip_encoder.rs` | 0.10 | equation section 6: 15.0 MB per minute at 1252 × 890, 20 fps, keyframe every 1 s gives 0.090; τ is 1 s again |
| τ | `KEYFRAME_INTERVAL_MS` | `server/clip_encoder.rs` | 1,000 | S5: research's 1 s GOP; raised only after v_dec and seek are measured |
| GOP | `GOP_FRAMES` (computed) | `server/clip_encoder.rs` | τ · f = 20 encoded frames | S5; repeats are not encoded, so keyframes are at least τ apart in time |
| decoder forward-read limit | (computed) | `server/clip_decoder.rs` (today `FORWARD_READ_LIMIT_MS`, line 29) | `GOP_FRAMES` frames | O2 |
| σ_v lower bound | `OVERVIEW_MIN_SCALE` | `server/clip_viewer.rs` | 0.75 of the window size | h_leg / h_text = 9 / 12, assumed; checked by a real agent in the proof |
| image ceiling | `VIEW_MAX_LONG_EDGE` | `server/clip_viewer.rs` (replaces `DEFAULT_LONG_EDGE`, line 49) | 1,568 px | S4: standard-tier Claude models downsize anything larger; a reported 2,000 px client cap can break the session |
| overview budget | `OVERVIEW_IMAGE_TOKEN_BUDGET`, `OVERVIEW_TEXT_TOKEN_BUDGET` | `server/clip_viewer.rs` | about 20,000 and 4,000 | S4; research budget recommendation |
| other calls budget | `CALL_TOKEN_BUDGET` | `server/clip_viewer.rs` | about 10,000 | S4 |
| N | `MAX_IMAGES_PER_CALL` (upper bound) | `server/clip_viewer.rs:46` | at most 8; per call min(8, budget / tokens per image) | equation: tokens = N · w_v · h_v / 770 + 1500 |
| close-up factor | `CLOSEUP_SCALE` | `server/clip_viewer.rs` | 2 | whole-number duplication |
| crop minimum | `MIN_CROP_SHORT_SIDE` | `m1.rs:479` | 200 px | kept from version 1 |
| change test | `BLOCK`, `PIXEL_DELTA`, `MIN_CHANGED_PIXELS` | `server/clip_viewer.rs:56-62` | 8, 32 (largest R, G or B difference), 3 | kept from version 1 (colour-aware, review fix 6); measured against encoder noise in the proof |
| settle, short-lived | `SETTLE_MS`, `SHORT_LIVED_MS` | `server/clip_viewer.rs:64-66` | 200, 500 | kept from version 1; both ≥ d_min |
| cursor mask | `CURSOR_MASK_PX` | `server/clip_viewer.rs` | 48 | research: a 48 × 48 box at the logged cursor position |
| caret rule | `CARET_MAX_WIDTH_PX`, `CARET_PERIOD_MS` | `server/clip_viewer.rs` | 3, 500 | research: thin rectangles that toggle about every 500 ms |

`MAX_FPS`, `DEFAULT_FPS`, `MAX_DURATION_MS` (`capture_clip.rs:52-54`), `FALLBACK_BITRATE_BPS` and `KEYFRAME_INTERVAL_SECONDS` (`clip_encoder.rs:37-38`) are removed. `WORKER_CPU_PERCENT` and `OBSERVE_RING` from version 3 are removed. `MIN_FLASH_MS` and `OBSERVE_INTERVAL_MS` from version 2 are not constants.

## Kept from version 1

The version 1 code does these things right. Each stays, in the new observer, live part, worker or viewer.

| Behaviour | Where today | Stays in |
|---|---|---|
| A minimized window gives no frames: ticks repeat and form a `target_minimized` interval. The window is never restored or focused | `capture_clip.rs:799` (`target_minimized`) | A (repeat lines), `clip.json` |
| An all-black first frame (hidden or secure desktop, protected window) is an error; there is no PrintWindow fallback | `capture_clip.rs:543`, `:827` (`is_all_black`) | A |
| Error codes `CAPTURE_TARGET_INVALID`, `CAPTURE_TARGET_LOST`, `CAPTURE_GRAPHICS_API_UNSUPPORTED`, `CAPTURE_THREAD_FAILED`, each with operation, source of truth, target id and remediation; the stop reasons, including `privacy_paused` and `privacy_excluded_app` | `capture_clip.rs:86` (`StopReason`), `:480` | A, C, `clip.json` |
| The colour-aware change test (largest R, G or B difference) | `clip_viewer.rs:56-62` | D |
| The Graphics Capture backend is pinned, so `SYNAPSE_CAPTURE_FORCE_DXGI` cannot break window clips | `capture_clip.rs:423-424` | O |
| The recorder uses its own capture handle and never switches the shared M1 capture controller | version 1 plan, key facts | O |
| A browser-tab target follows the `screenshot capture` rule and is refused, not mapped to the browser window (issue #286) | `m1_tools/mod.rs:11878` (`perception_window_hwnd`) | A |
| The recorder runs on its own thread, never on a server worker thread (issue #285) | version 1 plan, key facts | A |
| A monotonic start plus a wall-clock anchor in `clip.json` | `capture_clip.rs:1032-1036` (`ClipClock`) | A4 |
| Every image carries the label `Image k (t=mm:ss.mmm, frame n, reason)` and its file path; the label is drawn on the exported copy, the clip stays clean | `clip_viewer.rs:1096-1114` | D |
| A crop is at least 200 px on its short side | `m1.rs:479` | D |
| The folder must not exist when `record` starts | `m1_tools/mod.rs:17242-17245` | A2 |

Replaced, not kept: version 1 records into a hidden temporary folder and renames it into place at the end (`capture_clip.rs:311-334`). Version 4 writes into the clip folder from the start, and the state field of `clip.json` (C10) says how far the clip got.

## What gives way, and known limits

The order is the owner's (E1): time, then length, then named instants. In detail:

- The worker gets only what the live part leaves of 5 %. A weaker machine, a software encoder or a busy live part makes the clip ready later.
- The recording stops when the clip volume reaches storage pressure Level 1 (A11).
- Only then are frames lost: when the disk cannot take the raw writes, when no slot is free, or when the live part alone would pass 5 % (A9). Each lost instant is counted, named with its cause, and condition C is reported as not held for it.

Known limits that stay:

- Raw write volume (E8). Every new frame is written raw before it is encoded. At 1920 × 1080 with typical activity that is about 31 MB per second, about 113 GB per hour. A window that changes on every recorded frame writes 166 MB per second. Mouse movement may push ordinary use toward that figure; package 0 measures it (M4, M5), and the owner decides again on those figures. Storing only the changed rectangle (dirty regions exist from Windows 11 24H2) is the option for that decision.
- In the worst case on the weakest machine with the software encoder, a 2-minute recording needs about 17 GB free.
- Disk writes cost processor time that the equation's a_rt does not contain. Package 0 measures it (c_disk); A9 counts it in the live share.
- On Windows 10 the yellow capture border shows, and frames arrive per composition (up to the display rate) instead of at a set interval.
- The typical rate of 3.8 new frames per second came from one window. The real rate drives both the live share and the write volume, and is unmeasured until package 0.
- The weakest machine's figures remain computed until one run on a real Core i3.

## Not in this build

Other codecs than H.264; choosing an encoder per size (C7: the encoder Media Foundation grants is used and named); colour conversion on the graphics chip; a rolling buffer that keeps only the last minutes; triggers; clicks and key presses in the clip; clip retention; removing the yellow border; storing only changed rectangles (unless the owner chooses it under E8 after package 0); making `screenshot gif` (`server/capture_gif.rs`, its own capture loop, issues #285 and #286) an export of a clip.

## Work packages

Each product package is built by one agent from this plan with a time and tool-call limit. It passes `cargo fmt --all` and `cargo clippy -p synapse-mcp -p synapse-capture -p synapse-storage --all-targets -- -D warnings`, is committed and pushed as `claude`, and is then reviewed by Codex, read-only. Findings are fixed before the next package starts. Packages run one at a time, in this order. Packages 0 and 1 may start once version 4 is committed (E7); package 1 does not depend on package 0's figures beyond the named constants it reads.

| # | Package | Contains |
|---|---|---|
| 0 | Measurements | M1 to M9 in `scripts/clip-equation/`; one results file; `run_equation.py` with c_obs, the real c_read and c_disk. No product code |
| 1 | Observer | O1 to O4 in `synapse-capture`: gated settings, Windows floor, slots by instant, threading rule, identity before read-back; existing screenshot paths unchanged |
| 2 | Live part | A1 to A12; the four `record*` operations; `frames.jsonl` with frame size and cursor position; the waiting folder; `clip.json` at start with state and the registry; the public free-bytes-to-level function in `synapse-storage`; the updated contracts (O1); conditions C, G, T |
| 3a | Worker lifecycle | The long-lived worker mode, its job and processor cap, the queue, segment forming (B2), commit (B8), ready (B9) and recovery (B10) |
| 3b | Encoding | B3 to B7: σ, colours, encoder and retry, time stamps with `write_nv12` taking instant and duration, GOP in frames, no B-frames, screen-content hint, quality and Q status |
| 4 | Viewer | D1 to D7 |
| 5 | Proof and constants | The proof cases below; README and tool-surface tables |

## Proof

After package 4, manual cases run in the real app through Synapse, on the development PC with the new build deployed, plus one Windows 10 virtual machine. Test window: a local HTML page that draws a counter on every screen refresh (requestAnimationFrame) and shows 50 ms flashes at set times (`scripts/clip-equation/flash-page.html`, written in package 0). Each case is read from `clip.json`, `frames.jsonl` and the segment files.

Capture and timing:

1. Flash-phase sweep: 50 ms flashes at phases across one recording interval. Every flash is in the clip, and the counter read from each recorded frame equals the refresh shown at s_k.
2. A 50 ms local flash while another region animates is in the overview picks or named in the timeline (C15).
3. The longest hold time against T_r; proven losses zero on the counter page.
4. Observer settings on a Windows 10 virtual machine (1903 or later): recording starts, the border shows, the encoder found there is named.

Load and disk:

5. The processor share per second: live part including disk writes, worker cap given, worker share, sum at most 5 %.
6. A low-free-space volume: the recording stops by the disk rule, committed segments stay, the stop reason is recorded.
7. Monitor recording while an excluded app is visible but not in front: refused at start; stopped when the app appears (E6). A privacy pause during recording: no frame composed after the pause instant is on disk (C12).

Segments, durability, recovery:

8. A 90 s clip across a segment boundary: two segments, each starts with a keyframe and plays on its own.
9. A 10-minute still window: one encoded frame per segment.
10. A 30-minute clip: seek time within the bound; this case decides whether time segments stay.
11. Kill the worker in the middle of a segment, then kill the service in the middle of a segment, then start the service: the `.part` is discarded, the segment is encoded again from raw, the clip becomes ready, and lost instants (service case) are named.
12. A drag-resize gives one transition segment at the largest size, frames unscaled, G "padded" for those instants; a window below 480 lines is duplicated; recorded size equals σ · window size.

Encoding:

13. GOP of 20 encoded frames and B-picture count 0 read back; the keyframe instants recorded at commit match the files. The screen-content hint: accepted or not, and its byte effect.
14. The decoded text is readable at level 70, and at which lower level it stops being readable (q_min); Q status as reported.
15. Bytes per minute against the equation's figures, which gives K and P for the built encoder; v_dec for τ.

Viewer and agents:

16. A real agent, given `clip_overview`, finds the flash and names its frame.
17. A clip with a planted wrong value: the agent names the frame that shows it.
18. A real agent reads the smallest text from a returned image (h_leg), which checks the 0.75 floor.
19. In Claude Code and in Codex: whether images arrive as image blocks, how they count against the result cap, and whether the client resizes them.
20. The viewer opens a resized clip with several segments; a 4K window shows the "ceiling below floor" note and the pointer to `clip_frame`.

Regression for the ten version 1 review fixes (issue #290, comment 1; C18):

21. Frame zero passes the privacy gate before it is written.
22. A stop or a cancelled call never publishes more than was recorded; the clip state says how it ended.
23. A capture failure after frame one ends with state `failed` or a stop reason, never as a complete clip.
24. Instants start at the first delivered frame; there is no burst of backdated frames.
25. The overview cache is invalidated when the clip files change (a new segment is committed).
26. Change detection sees colour, not only brightness.
27. The final frame's display interval can be requested (the last sample runs to the stop time).
28. The bitrate fallback is reported only after it was read back.
29. Every error exit after `BeginWriting` still finalizes the writer.
30. Storage initialisation never blocks a server worker thread.

Not provable on this PC: the weakest machine's processor costs; they stay the computed ones until one run on a real Core i3.

## Owner decisions still open

None. E1 to E8 were answered on 2026-11-22. E8 comes back to the owner with package 0's measured write volume.

## Changes from version 3

- Header: version 4, link to `docs/clip-recorder-review-v3/`.
- Fixed by the owner: E1 to E8 added with date 2026-11-22. E2 overrides the version 1 decision "no child processes". The three open decisions of version 3 are closed.
- Facts: F10 to F17 added (worker launcher, per-spawn job, record contract, decoder seek assumption, viewer ceiling and transient rule, frame-counter time stamps, memory-copy read-back, `CapturedFrame`). F4 now includes the cursor setting. F6 narrowed to two primitives. F9's consequence changed. Line references corrected: `pressure.rs:18` and `:15` (were 17 and 14), `clip_encoder.rs:187` and `:407` (were 186 and 406), `desktop_worker.rs:1619` (was 1618), `capture_clip.rs:424` (was 423).
- New package 0 and section 0: measurements before any product code (S1, G).
- Section O: settings gated one by one including cursor and dirty regions, Windows 10 1903 floor (S2, E5); slots assigned by instant instead of the 4-texture ring (C2); threading rule (C3); identity before read-back (C4); requested interval measured from 11 ms; the claim "a handler within T_r drops nothing" removed (C1); f_r and T_r separate (C17).
- Section A: no scaling and no letterbox in the live part (C5, E4); privacy before every raw write and the monitor rule (C12, E6); `clip.json` at start with state and a registry (C10); disk rule on the clip volume (C13); worker cap computed every second and load shedding as the last step (S1, E1, E3); states, stop reasons and service ownership (C14); `record` read-back (O1).
- Section B: a new long-lived worker mode reusing only `current_exe` and the job function, package 3 split into 3a and 3b (S3, E2); one worker and a queue (C11); segment forming in the worker with a transition segment (E4); raw frames kept until the segment is committed through a `.part` rename and checkpoint (S6); encoder retry and failure (C7); sample durations and segment-local time (C8, O3); GOP in frames, no forced keyframe, no B-frames, screen-content hint (S5, C9, O4); Q status (C6); recovery from state (C10).
- Section C: `failed` state, stop reason, segment SHA-256, contract update (C14, O1).
- Section D: ceiling `VIEW_MAX_LONG_EDGE` = 1,568 and token budgets that set N, timeline on page 1 only (S4); region-level transients, cursor mask, caret and animation rules (C15); decoder forward-read limit computed (O2).
- Conditions: Q has a status again (C6); C reports proven losses, hold time and delivery completeness separately (C1); G has "padded".
- Parameter map: new and computed constants for the above; τ back to 1 s; change-test constants restored; `WORKER_CPU_PERCENT` and `OBSERVE_RING` removed; the encoder bound named as a chosen bound (C16).
- New section "Kept from version 1" (S8).
- "Known limits" rewritten as the owner's order of what gives way (S9, E1) and the accepted write volume (E8).
- "Not in this build": encoder choice per size (C7).
- Proof rewritten as 30 manual cases (S7, C18).
