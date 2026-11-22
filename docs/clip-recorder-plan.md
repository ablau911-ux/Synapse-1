# Clip recorder: building plan (version 4.1)

Status 2026-11-22 16:30 IST. Branch `feature/clip-capture`. Owner: Adam. Builders: Claude agents. Review: Codex (read-only), plus Fable 5.2 for component logic. Work log: issue #290. The plan is edited in place; there are no new version numbers (owner, 2026-11-22).

This is step 3 of the owner's work order: the equation in `docs/clip-recorder-equation.md` turned into an order of operations, each with its desired real-world result. Step 4 is the code, in which every tunable is a named constant that stands for one symbol of the equation, or is computed from such constants.

History. Version 1 (commit ad0047d1) was built as commits 7709f70c, 7e4cf3eb and be2447f8. Version 2 (f354496d) was reviewed against the code (issue #290). Version 3 (30392a46) folded in that review and was itself reviewed by Codex and Opus, consolidated, and decided by Fable. The full record is in `docs/clip-recorder-review-v3/` (`1-codex-gpt-6-astra.md`, `2-opus-4-8.md`, `3-consolidation-and-jev.md`, `4-fable-decisions.md`; the last one is binding). Decision numbers below (S1 to S9, C1 to C18, O1 to O5, E1 to E8) refer to `4-fable-decisions.md`.

## Where the build stands (2026-11-22 18:00 IST)

- **Built:** packages 0, 0b, 1, 2, 3a, 3b and 4. Package 5 (proof) is partly done: see "Proof results so far" under Proof.
- **Reviews:** Codex reviewed every fix round; Fable 5.2 reviewed the component logic (`docs/clip-recorder-review-build/fable-5-2-c00ff762.md`) and the new D5 analyzer. The review findings that are accepted are fixed or queued; the rest are listed under "Open items".
- **Live tests** (release build, a separate stdio instance, the proof page):
  - Recordings of 30 s and 90 s end `ready` with C, G, B and T held and 0 lost instants.
  - The processor sum stays at most 8.1 %.
  - Waiting frames average 37 KB instead of 6.29 MB raw.
  - A resize gives one segment per stable size.
  - `record_stop` is idempotent.
  - After a service kill, the clip is recovered to `ready`.
- **Owner decisions taken during the build:**
  - E9: no cursor in the picture.
  - E10: 10 % processor limit.
  - E11: waiting frames are compressed with zstd −3 on the whole frame.
- History of the plan versions and the full change list are at the end.

## Fixed by the owner

- Flashes from 0.050 s on screen are in the clip. 60 Hz is the reference screen.
- Never above 20 fps and never above 10 % of the processor time, on any machine (E10, 2026-11-22; it was 5 % until then). A weaker machine takes longer; nothing else gives way except as E1 below states.
- Every machine from a Core i3 2.2 GHz with two cores, 4 GB memory and Intel graphics. Such a machine runs Windows 10, so every Windows feature used must have a Windows 10 path.
- 60 s by default, longer on demand. At least 480 lines. The size follows the window. Quality before bytes.
- Viewers are the existing agents, which see still images. No automated tests (upstream policy #1699; the test surface was removed again in 3f972db5).
- Disk: the recorder stops at storage pressure Level 1 (free disk below 2 GB), the rule that already exists in `synapse-storage` (owner, 2026-11-21).

Answered on 2026-11-22 (the owner said "continue" after Fable's recommendations, so each question takes Fable's recommendation):

| # | Question | Answer (2026-11-22) |
|---|---|---|
| E1 | What gives way when a machine cannot meet every condition | Time first (the clip is ready later). Then length: the recording stops at storage pressure Level 1, the existing rule. Only after both, frames for named instants are lost, counted and reported per instant. Never the processor cap (10 % since E10), never the size, never the quality. |
| E2 | The worker as a separate process | Yes. This overrides the version 1 owner decision "no child processes". A job object is the only hard limit that also covers the encoder's own threads. |
| E3 | Worker share of the processor | 10 % minus the live part's measured share, recomputed every second. Not a fixed 4 %. |
| E4 | Frames captured while a window is dragged to a new size | One transition segment whose canvas is the largest size seen during the drag; frames are placed on it unscaled. G is reported as "padded" for those instants. |
| E5 | Minimum Windows version | Windows 10 version 1903 (build 18362). Windows capture does not exist before 1803 and windows-capture 2.0.0 needs 1903. On Windows 10 the yellow border always shows. |
| E6 | Monitor recording while an excluded app is visible but not in front | Recording refuses to start, or stops, when an excluded app is visible anywhere on the recorded monitor, even if it is not in front. |
| E7 | Go | Packages 0 and 1 may start once version 4 is committed. |
| E8 | Raw write volume through the disk | First accepted for this build (about 31 MB/s at 1080p). **Settled by E11:** the owner raised that raw waiting frames break R9, and the waiting frames are now compressed. |
| E9 | The mouse cursor inside the recorded frames (asked after package 0, M4: a drawn cursor makes 20 new frames per second while the mouse moves, 166 MB/s at 1080p) | The cursor is not drawn into the frames (owner, 2026-11-22 03:35). Its position is still logged per instant in `frames.jsonl`. Where Windows cannot leave it out (Windows 10 before 2004, build 19041, has no cursor setting), it stays in the picture and the viewer masks it (D5). |
| E10 | Processor limit (asked after package 0, M9: on the weakest machine the live part alone is estimated at 5.2 % at 720p and 9.1 % at 1080p) | Up to 10 % on the weakest machine is accepted (owner, 2026-11-22 03:35). `CPU_LIMIT_PERCENT` becomes 10; the worker cap is 10 % minus the live share. A stronger machine uses less for the same work. |
| E11 | Form of the waiting frames (R9: files as small as possible; asked after the owner noticed the raw writes) | Lossless compression of each waiting frame: first "make it simple" (whole frame, no tiles), then, after the full comparison of 31 methods (`ca46079d`), **zstd at level −3 on the whole frame, alpha dropped** (owner, 2026-11-22). Video-like content does not compress and passes 10 % with every coder; E1 applies there. |

## What the code had to be corrected on (facts found in the reviews)

These facts decided the shape of packages 0 to 4. Each was read in the code before the build; the line numbers were checked with Serena on 2026-11-22 at 02:45 (unless marked "crate") and refer to the code as it was then. Packages 1 to 4 have changed these files since, so use the symbol names, not the line numbers.

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

The work is split by the 10 % rule. What cannot wait runs in the Synapse service while the screen is recorded (sections O and A). What can wait runs in a worker that Windows itself limits (section B). Package 0 measures the costs of both before either is built.

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

### 0b. Compression of the waiting frames (package 0b, in `scripts/clip-equation/probe/`)

Added 2026-11-22 09:20 IST. Rule R9 (equation line 27) and equation line 249 require the waiting frames in a cheaper form than raw; version 4 kept them raw (E8). The owner asked for the best method found online, then chose to measure the two finalists and decide on the numbers (2026-11-22). Research: `docs/clip-recorder-waiting-compression-research.md` (commit `660f9573`).

Package 0b changes no product code. It adds one probe subcommand and appends one section to `scripts/clip-equation/results-package0.md`.

| # | Measurement | Decides |
|---|---|---|
| M10 | Real frames: capture the new frames of windows the probe opens itself (no input to any other app), each at least 20 s: the flash page `scripts/clip-equation/flash-page.html` with and without `?anim=1`, a page that scrolls text by itself, a page that types text into a text box by itself, a playing video or a canvas animation, and one still window. At least one scenario at σ = 2 and one at σ = 3 (a small window) | the input of M11 to M13 |
| M11 | Per scenario, single thread, release build: bytes per frame and ns per pixel for compression and decompression of (a) raw, (b) `lz4_flex` on the whole frame, (c) changed tiles + `lz4_flex`, (d) changed tiles + zstd level −1 and 1, each with tiles of 32, 64 and 128 px, alpha kept and dropped; a full keyframe every `WAITING_KEYFRAME_FRAMES` = 16 frames and after a size change | method, tile size, alpha |
| M12 | Write cost per frame with the compressed sizes: create a `.tmp`, write, rename, against the raw write | c_disk with compression |
| M13 | Exactness: decoded B, G, R equal the source byte for byte; the NV12 produced by the existing `Nv12Canvas::draw` from decoded frames equals the one from the source for σ = 1, 2, 3 | lossless proof |
| M14 | The live share at the measured figures, for the development PC and the weakest-machine estimate (equation section 6 scaling), at 1080p with the measured new-frame rates | B within 10 % |

Choice rule: the method with the fewest bytes whose compression keeps the live share within its part of `CPU_LIMIT_PERCENT` on the weakest-machine estimate at the M4 worst rate; when both qualify, the one with the fewer bytes. The owner sees the figures before the choice goes into section A (A8).

Result (commit `a0d02e6e`, `results-package0.md` section "Package 0b"). All 176 runs were lossless. Tiles + zstd made typing and flash frames 800 to 4,600 times smaller. Video content did not compress (1.5 times), and every coder then passed 10 % on the weakest-machine estimate (12.5 to 21 %). First owner decision (2026-11-22; replaced by zstd in the next paragraph, E11): "make it simple", so the whole frame goes through `lz4_flex` with alpha dropped. It needs no tiles, no keyframes and no state between files. Measured: typing 182 times smaller, flash 79, scroll 26, video 1.5. On the weakest-machine estimate this stays within 10 % except for video (12.5 %), where E1 applies: time gives way first, and lost instants are named last.

Full comparison (commit `ca46079d`, 31 methods, 12 scenarios including real 1920 × 1080, all lossless). Whole-frame zstd gives about 3 times fewer bytes than whole-frame lz4 for 0.2 to 1.1 points more processor time. Video passes 10 % with every coder (lz4 14.9 %, zstd −3 15.9 %). Owner decision (2026-11-22): zstd is the coder, at level −3. At 1080p it gives 8.6 KB (typing), 25 KB (flash) and 123 KB (scroll) per frame at 6.8 to 8.7 %. That replaces `lz4_flex`.

### O. Observer, in `synapse-capture` (new, for clips only)

The existing screenshot paths stay as they are. The clip recorder gets its own frame handler and its own capture handle.

| # | Operation | Desired result | Equation |
|---|---|---|---|
| O1 | Refuse to start below Windows 10 1903 (`MIN_WINDOWS_BUILD`). Build the session settings for the clip: ask for no cursor in the picture (E9), "no border", secondary windows, the minimum update interval and dirty regions each only where windows-capture reports that setting supported; use Default otherwise. Where the interval is supported, request `REQUESTED_UPDATE_INTERVAL_MS` | Recording starts on Windows 10 1903 and later and on Windows 11. On Windows 10 the yellow border shows and frames arrive per composition, up to the display rate | R10 |
| O2 | In the frame handler: read the composition time (`Frame::timestamp()`), compute the instant k it belongs to, and copy the frame on the graphics chip into the candidate slot of the open instant. When a frame for a later instant arrives, pin the candidate slot: it is the frame for s_k. Store the composition time and the dirty region when given. Return. The handler only issues GPU copies and never maps | The handler holds the one-buffer pool for as short a time as possible; the chosen frame per instant is fixed by composition time, not by when the reader gets to it | C (2), C (3) |
| O3 | Measure the longest hold time (composition time to frame release) and count pinned-slot overflows. Each overflow is a lost instant, recorded with its cause | The clip reports proven losses and the hold time against T_r | conditions |
| O4 | On request for instant s_k: return the identity (composition time) of the pinned frame first. Read back to the processor only when the caller asks for it after its repeat check, on the reader thread | A repeated frame costs no read-back; the only read-back is one per new recorded frame | C (2), a_rt |

Slots. `OBSERVE_SLOTS` = `MAX_SELECTION_LATENESS_INSTANTS` + 2: one candidate slot for the open instant, one pinned slot per instant the reader may still be behind, and one spare. A pinned slot is never overwritten while the reader may be reading it. When no slot is free, the newest frame takes the newest unread pinned slot and that older instant is recorded as lost (`989aaebb`), so a window that then goes still shows its last picture. The lateness bound comes from package 0 (M7).

Threading rule (C3). Multithread protection is on for the capture device. The handler only issues GPU copies and never maps. The reader thread never holds the device context across a wait. The mechanism (package 0 proposes `Map` with "do not wait" and a retry) is confirmed by M6.

Requested interval (C1). `REQUESTED_UPDATE_INTERVAL_MS` starts at 11 ms, because the research measured about 45 fps at a 16.7 ms request and about 60 fps at 11.1 ms on a 60 Hz display (windows-capture issue #190). Package 0 confirms or replaces the value. Where the interval is unsupported (Windows 10 and Windows 11 before 24H2), Windows delivers per composition and the slots absorb rates above 60 per second.

What a clip can and cannot claim (C1). A handler that returns quickly does not prove that nothing was lost: Windows' delivery, scheduling and pool recreation happen outside it. Each clip therefore reports three separate things: proven losses (overflow, disk, load shedding, service restart, unreadable or missing waiting frame), the longest hold time against T_r, and delivery completeness as "checked only in the proof". Clip frames are not routed through `CapturedFrame` (F17).

### A. Live part, in the Synapse service

The live part does no scaling and no encoding. It stores every new frame at its captured size (C5).

| # | Operation | Desired result | Equation |
|---|---|---|---|
| A1 | Read the machine: thread count n, Windows build, and the free space of the volume that holds the clip folder | No constant of one PC in the code | R10 |
| A2 | Refuse to start when: the privacy gate pauses; the target app is excluded; for a monitor, an excluded app is visible anywhere on it (E6); storage pressure on the clip volume is Level 1 or worse; another recording is running; the folder exists; Windows is older than 1903 | Nothing is recorded that must not be; the 10 % rule holds because there is only one recorder | existing rules, B |
| A3 | Read the window size, compute σ and the recorded size for the report and for the worker. The live part itself does not scale | The worker knows the size; the live part stays one copy per new frame | G |
| A4 | Write `clip.json` at once with state `recording` (written to a temporary name, then renamed), list the clip in the active-clip registry when its folder is not the default one, and read the file back for the `record` response | A crash at any point leaves a state to recover from; `record` returns something it has verified (O1) | t_ready, T |
| A5 | Start the observer (O1) | Every screen refresh that Windows delivers is seen | C (1) |
| A6 | For each instant s_k (every 1000 / f ms): ask the observer for the identity of the frame for s_k (O4) | A late recorder cannot miss a flash | C (2) |
| A7 | If the identity equals the last recorded one: write only its line in `frames.jsonl` | A repeated frame costs nothing | B |
| A8 | If it is new: check the privacy gate before the write, frame zero included (C12). When the gate pauses, frames composed after the pause instant are discarded, also those already in a slot. Otherwise read the frame back (O4), drop the alpha byte (B, G, R stay), compress the whole frame with zstd at `WAITING_ZSTD_LEVEL` = −3 (one reused context), write it into `waiting/` as one self-contained file at its captured size (`.tmp`, then rename), then write its line with the frame size and the cursor position (C15) | Nothing private reaches the disk; the waiting files are lossless and small (R9) | a_rt |
| A9 | Measure the live part's processor share every second, disk writes included. Set the worker's cap to `CPU_LIMIT_PERCENT` minus that share (E3). If the live part alone reaches its share of 10 %, it skips the read-back of new frames for the rest of that second; those instants are lost and named (E1) | The sum never exceeds 10 %; time gives way first, frames only last | B |
| A10 | Count, per clip: proven losses with their cause, the longest hold time against T_r, the live share, and the worker cap given | The clip can state which conditions held | conditions |
| A11 | Every `DISK_CHECK_MS`, on the volume that holds the clip folder: free space as measured, minus the bytes written since the measurement, minus a reserve for the segment being finalized (C13). Stop when that falls below the storage Level 1 threshold (F7) | The disk keeps the existing reserve | T |
| A12 | Stop at the requested length, on a stop call, when the window closes, when privacy pauses, under the disk rule, or on a write failure. `record_extend` moves the end. A write failure is a recoverable stop that keeps committed segments. State becomes `encoding` with a stop reason (C14) | Length is 60 s by default and can be extended; no stop loses committed work | T |

Ownership (C14). The recording belongs to the service, not to the MCP call that started it, and runs to its requested end. `record_stop` is idempotent by clip id. Committed segments always stay.

Segments are not formed live. The worker forms them from `frames.jsonl` (B2), so a window drag costs the live part nothing.

### B. Worker: a new long-lived worker mode of the same executable

The worker is a separate process (E2), started from `current_exe` (`desktop_worker.rs:1619`) inside a job made by `create_worker_kill_on_close_job` (`desktop_worker.rs:983`). It reuses those two primitives only. It does not use the desktop worker's launcher: no hidden desktop, no window handle, no 10 s wait, no one-shot envelope (F10).

The job gets a hard processor limit (`JOB_OBJECT_CPU_RATE_CONTROL_ENABLE` with `HARD_CAP`). The service sets that limit every second to 10 % minus the live share it measured (A9). When the remainder is zero, the worker waits; the clip is ready later. The worker runs at below-normal priority and without a window. The limit also covers the encoder's own threads. Each spawn has its own job (F11), so the cap touches no other worker.

One clip worker runs at a time for the whole service, with a first-in first-out queue that recovered clips also enter (C11). The job is kill-on-close, so the worker ends with the service; B10 resumes the work.

Package 3a builds the process, the cap, the queue and recovery (B1, B2, B8 to B10). Package 3b builds the encoding (B3 to B7).

| # | Operation | Desired result | Equation |
|---|---|---|---|
| B1 | Take the next clip from the queue; read its `clip.json` state and committed segments | Work resumes where it stopped | t_ready |
| B2 | Form segments from `frames.jsonl`: close a segment every `SEGMENT_MS`, when its retained waiting bytes (the compressed files on disk) reach `SEGMENT_RAW_BUDGET_BYTES`, when the window size has been stable for `RESIZE_SETTLE_MS` at a new size, and at stop. Frames captured during a drag form one transition segment whose canvas is the largest size seen during the drag; frames are placed on it unscaled (E4) | The size follows the window without one segment per drag frame; pixels stay exact | G, T |
| B3 | Duplicate pixels by σ and convert the colours. Above the encoder bound, use the largest size within the bound at the same aspect ratio | Recorded size as computed in A3; above the bound, G reported not held | G |
| B4 | Encode with H.264 through Media Foundation with hardware transforms on (F8). The encoder is the one Media Foundation grants, named in the clip. On any encoder failure, restart the segment once from its retained waiting frames with hardware transforms off. On a second failure, the clip ends as `failed` and its committed segments stay (C7) | A working encoder on every machine, without a separate probe; no claim that it is the cheapest | E |
| B5 | Time stamps (C8, O3): each encoded frame gets its instant as the time within its segment; its duration is the next new frame's instant minus its own; the last sample runs to the stop time or the segment end. Every segment starts with a keyframe of the picture shown at its start. Repeated frames are not encoded. `write_nv12` takes the instant and the duration | Bytes follow changes, not time; each segment plays on its own; `frames.jsonl` maps global time to segment and local time | r |
| B6 | Keyframe interval in encoded frames: `GOP_FRAMES` = τ · f = 20 while τ = 1 s. No forced keyframe (C9). No B-frames, set and read back (O4), because variable time stamps assume no reordering. The screen-content hint is tried and its acceptance is recorded | Seek cost is bounded by frames decoded; a slowly changing window still gets few keyframes | τ |
| B7 | Quality mode at level q, read back; if refused, bitrate = β · width · height · f, read back | Same quality rule on every machine; Q gets an honest status (C6) | Q |
| B8 | Commit a segment (S6): write `segment-NNNN.mp4.part`, finalize, flush to disk, rename to `segment-NNNN.mp4`, compute its size and SHA-256, record its checkpoint in `clip.json` (temporary name, then rename): file, size, SHA-256, instant range, keyframe instants read from the finished file, encoder, Q status. Only then delete the segment's waiting frames, except the last picture drawn, which seeds the next segment | A crash never loses frames that were written; finished segments can be viewed while recording goes on | T |
| B9 | When every segment is committed: write the conditions into `clip.json` and set the state to `ready` | The clip says what it guarantees | conditions |
| B10 | When the service starts: read the active-clip registry and the default clips folder. Every clip in state `recording` or `encoding` enters the queue. A `.part` file is deleted and its segment re-encoded from its waiting frames. A clip found in state `recording` becomes `encoding` with stop reason `service_restart`; the instants after its last `frames.jsonl` line up to its end are named as lost | A restart never leaves a clip half done or silently short; recovery works from the state, not from which files happen to exist (C10) | t_ready |

Crash loss. A worker crash loses nothing: the segment is encoded again from its waiting frames. A service crash during recording loses only the frames not yet written; those instants are named in `clip.json`. Durability no longer depends on fragmented MP4.

Keyframe column. The keyframe flag is not known while recording. The worker reads the keyframe instants from each finished segment (B8); the viewer joins them with `frames.jsonl`. The proof compares them with the GOP setting.

### C. Calls an agent makes (operations of the `screenshot` tool)

| Operation | Does | Returns |
|---|---|---|
| `record` | Starts a recording (window or monitor; `duration_ms` 60,000 unless given, no upper limit; there is no `fps` parameter, f is `RECORD_FPS`). It returns once frame zero is on disk (at most 3 s; a target that delivers no frame in that time is refused as `CAPTURE_TARGET_INVALID`, and is never restored). Refused while another recording runs | clip id, folder, and the `clip.json` written at start, read back: path, size, SHA-256 |
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

### D5 as an equation (2026-11-22; replaces the invented rules)

Owner decision (2026-11-22): D5 gets an equation first. Its earlier rules were invented by the builder and failed on ordinary UIs. Sources: `docs/clip-recorder-change-detection-research.md` (anchor comparison, two thresholds), `docs/clip-recorder-review-build/` (the Fable D5 review), and the measured encoder noise.

**Measured noise (quality 70, GOP 20, proof clip).** In one 8 × 8 block of static content, the number of pixels whose largest R, G or B difference to the previous frame exceeds δ:

| δ | 32 | 48 | 64 |
|---|---|---|---|
| noise maximum ν(δ), static text | 7 | 6 | 1 |
| noise maximum, blank area | 0 | 0 | 0 |

**1. Block test.** For a block b and two pictures X and Y, n_δ(b; X, Y) = the number of pixels of b whose largest channel difference between X and Y exceeds δ.

    DIFF(b; X, Y)  ⇔  n_δhi ≥ N_hi  or  n_δlo ≥ N_lo
    SAME(b; X, Y)  ⇔  n_δhi ≤ ν(δ_hi)  and  n_δlo ≤ ν(δ_lo)
    otherwise: undecided

Thresholds follow from the noise with a safety factor of 2 and from the smallest change that must be seen:

    N_hi > 2 · ν(δ_hi)                    δ_hi = 64:  N_hi = 3        (text strokes, contrast above 64)
    2 · ν(δ_lo) < N_lo ≤ 32               δ_lo = 32:  N_lo = 16       (a flat fill over half a block)

What is guaranteed to be seen:
- a change whose contrast exceeds 64 on at least 3 pixels of a block;
- a change whose contrast exceeds 32 on at least 16 pixels of a block. Examples: a list selection #cce8ff on white (51) and a grey fill #d0d0d0 on white (47).

Named limit: a change with contrast of 32 or less (a hover tint #e5f3ff on white, 26) is not detected.

No false change follows while the noise stays within ν.

**2. Anchor.** Each block keeps an anchor A_b, its last settled picture. Frames are compared with the anchor, not with the previous frame.

    quiet → active      when DIFF(b; F_t, A_b); the start is the first instant that was not SAME
    active → settled    when SAME(b; F_t, F_t−k) for every k = 1 … K_s,   K_s = SETTLE_MS · f / 1000 = 4
    at settle:   DIFF(b; F_t, A_b)   →  CHANGED, and A_b := F_t
                 SAME(b; F_t, A_b)   →  RETURNED: a FLASH if end − start ≤ SHORT_LIVED_MS, otherwise REVERTED
                 undecided           →  A_b := F_t, no event (drift and noise)

Consequence: a fade of any length is detected once its total contrast passes the block test, because the difference to the anchor accumulates. A flash of one recorded instant (50 ms) is a FLASH. At the end of the clip and at a resize, every active block is closed with the clip time; "settled" then means quiet for at least `SETTLE_MS`.

**3. Animation.** The activity of a block is the share of frames in the last W_a = `SHORT_LIVED_MS` in which DIFF(b; F_t, F_t−1) held.

    animating(b, t)  ⇔  activity ≥ ANIMATING_CHANGE_SHARE = 0.5

- While a block animates, its anchor is frozen and it makes no event.
- When it stops, it is compared with its anchor like any settling block. So a checkmark that replaces a spinner is a CHANGED event.
- As built (`ea3e8514`): that change starts at the block's last motion, not at the start of the animation, so "checkmark replaces spinner" starts within one instant of the replacement.
- Owner decision (2026-11-22 about 20:42, issue #293): a block that animated and ends back at its anchor makes no FLASH or REVERTED event for that animated stretch; the animation event already covers it. Only that stretch is exempt: a later return in the same block is classified as usual. Without this, a playhead would give a short-lived change in every block it crosses.
- Connected animating blocks form one animation interval.
- There is no absorption: a block that is not animating is never swallowed by a neighbour.

**4. Events.** Blocks with the same outcome (CHANGED, FLASH, REVERTED, animation) whose intervals overlap and that are 8-neighbours, a gap of `REGION_GAP_BLOCKS` allowed, form one event with one bounding box. A flash beside a change therefore gives two events.

**5. Caret and cursor.**
- A block change is a caret when all of these hold:
  - its footprint is at most `CARET_MAX_WIDTH_PX` wide and 8 to 48 px tall;
  - it alternates between exactly two pictures, one of them the anchor;
  - it does so for at least 2 cycles in one place;
  - its half-period is 400 to 700 ms (the Windows default is 530 ms).
  A playhead or a meter fails this and is reported.
- The cursor box `CURSOR_MASK_PX` at the logged position is masked, as before.

**6. Picks within the budget (D6).** One image per event:
- CHANGED: the settle frame;
- FLASH or REVERTED: the frame with the most changed blocks;
- animation: its first frame, and its end frame when it stops before the clip ends.

A stable interval that an animation overlaps is not listed as "no change".

**7. Proof of the equation.** The page `scripts/clip-equation/ui-events-page.html` plays a fixed schedule of known events:
- a list selection;
- a grey fill;
- a 300 ms fade;
- a label that changes beside a spinner;
- a checkmark that replaces a spinner;
- a 50 ms flash beside a change;
- a playhead;
- typing with a caret.

It prints the schedule. Accepted when:
- every scheduled event is reported with the right type, its start within one instant;
- a still clip gives no event;
- a lone spinner gives exactly one animation;
- the keyframe instants give no event;
- the flash proof clip gives one FLASH per flash.

The noise table is measured again on these clips. If ν is exceeded, the thresholds are recomputed from the formulas above, not hand-tuned.

## Clip folder

    clip.json                    written at start; state, checkpoints and conditions; replaced atomically
    frames.jsonl                 one line per instant: instant, time, new or repeated, frame size, cursor position
    segment-0001.mp4 ...         one video file per committed segment
    segment-NNNN.mp4.part        the segment being encoded; deleted on recovery
    waiting/                     frames not yet committed in a segment, each one file: B, G, R compressed with zstd (empty when ready)
    views/                       images the viewer exported

Active clips in folders other than the default one are listed in one registry file under the default clips folder.

`clip.json` has a state (`recording`, `encoding`, `ready`, `failed` with reason), a stop reason, the committed segments, and a `conditions` block:

| Condition | Reported | Status |
|---|---|---|
| C | proven losses per instant with cause (slot overflow, disk, load shedding, service restart, `waiting_unreadable`, `waiting_missing`); longest hold time against T_r; delivery completeness. There is no privacy-discard cause: a privacy pause stops the recording before the instant is handled, so no instant inside the clip is discarded (`185fb5a4`) | held when no instant was lost; delivery completeness is "checked only in the proof" |
| G | window size, σ, recorded size per segment | held when recorded size = σ · window size; "padded" for transition-segment instants; not held above the encoder bound |
| Q | quality level q or the bitrate used, encoder name | unverified until q_min is calibrated for the encoder used; held when quality mode at q ≥ q_min was accepted and read back; not held on an uncalibrated bitrate fallback (C6) |
| B | live share, worker cap given and worker share, against 10 %; time until ready | held when the sum ≤ 10 % in every second |
| T | length, bytes, instants lost, stop reason | held when nothing was lost |

For a failed condition, the instants where it failed.

## Parameter map (step 4: every constant is one symbol)

| Symbol | Constant | File | Value | Source of the value |
|---|---|---|---|---|
| f | `RECORD_FPS` | `server/capture_clip.rs` | 20 | base rule |
| f_r | `REFERENCE_REFRESH_HZ` | `server/capture_clip.rs` | 60 | base rule |
| T_r | (computed) | | 1 / f_r = 16.7 ms | C17: period and frequency are separate symbols |
| d_min | (computed) | | 1 / f = 50 ms | requirement |
| requested interval | `REQUESTED_UPDATE_INTERVAL_MS` | `synapse-capture` clip observer | 11 | package 0, M3: 72 frames/s on the 144 Hz development panel, longest gap 13.9 ms (below T_r); used only where supported |
| selection lateness | `MAX_SELECTION_LATENESS_INSTANTS` | `synapse-capture` clip observer | 2 | package 0, M7: 0 instants late on the development PC under full load; 1 estimated on the weakest machine, plus 1 margin |
| slots | `OBSERVE_SLOTS` (computed) | `synapse-capture` clip observer | lateness + 3 = 5 | package 0, M7: at most 3 slots in use; lateness + 2 holds only if a slot is freed as soon as it is read and a newer frame is pinned |
| Windows floor | `MIN_WINDOWS_BUILD` | `synapse-capture` clip observer | 18362 (Windows 10 1903) | E5 |
| t_def | `DEFAULT_DURATION_MS` | `server/capture_clip.rs` | 60,000 | requirement |
| segment length | `SEGMENT_MS` | worker | 60,000 | finished segments are viewable while recording goes on; the 30-minute proof case decides whether time segments stay |
| segment raw budget | `SEGMENT_RAW_BUDGET_BYTES` | worker | 1 GiB of retained waiting files on disk (compressed bytes) | first a proposal from package 0 M4 and M5 (S6). Since `c00ff762` it counts the compressed files on disk: counting decompressed w·h·3 bytes closed segments after 227 frames at 1584 × 992 in the live test. With compression, `SEGMENT_MS` normally closes the segment first |
| map retry, worker tick, text tokens | `MAP_RETRY_SLEEP_MS`, `WORKER_TICK_MS`, `TEXT_CHARS_PER_TOKEN` | observer, worker, viewer | 1 ms; as built; as built | added by the builder in packages 1 to 4 (reported deviations): the M6 map retry sleep, the worker's control tick, and the characters-per-token estimate for the text budget |
| waiting-frame compression | `WAITING_ZSTD_LEVEL` | `server/clip_folder.rs` | −3 | package 0b full comparison (`ca46079d`); owner decision 2026-11-22 |
| resize settle | `RESIZE_SETTLE_MS` | worker | 500 | assumed; checked in the proof |
| h_min | `MIN_LINES` | `server/capture_clip.rs` | 480 | requirement |
| encoder bound | `MAX_ENCODE_MACROBLOCKS`, `MAX_ENCODE_SIDE` | `server/clip_encoder.rs` | 36,864 macroblocks, each side at most 4096 | a chosen bound (C16); the encoder's real acceptance is found when it opens, portrait sizes included |
| disk budget D | (none: storage pressure Level 1) | `synapse-storage/src/pressure.rs:18` | free disk below 2 GB on the clip volume | existing rule; the free-bytes-to-level function is made public there so the threshold has one owner |
| disk check | `DISK_CHECK_MS` | `server/capture_clip.rs` | 1,000 | F7: the 30 s probe is too slow for raw writes |
| finalize reserve | (computed) | `server/capture_clip.rs` | raw bytes of the open segment + their estimated encoded size | C13 |
| excluded-app check | `EXCLUDED_APP_CHECK_MS` | `server/capture_clip.rs` | 1,000 | E6: one window enumeration per second |
| U_max | `CPU_LIMIT_PERCENT` | clip worker mode | 10 | E10 (owner, 2026-11-22; was 5) |
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
| block test (D5 step 1) | `BLOCK`, `DELTA_LO`, `DELTA_HI`, `NOISE_LO`, `NOISE_HI`, `N_LO`, `N_HI` | `server/clip_viewer.rs` | 8; 32, 64 (largest R, G or B difference); ν = 7, 1; 16, 3 | `ea3e8514`. ν measured at quality 70 on the proof clip (`cb70a9f7`): static text blocks had at most 7 pixels above 32 and 1 above 64; blank areas had none. N_hi > 2 · ν(δ_hi) and 2 · ν(δ_lo) < N_lo ≤ 32 are checked when the code compiles. The D5 proof clips measure ν again; if ν(32) is 8, N_lo becomes 17 |
| animation and regions (D5 steps 3, 4) | `ANIMATING_CHANGE_SHARE`, `REGION_GAP_BLOCKS` | `server/clip_viewer.rs` | 0.5; (24, 2) | `ea3e8514`. A block animates while it changed against the previous frame in at least half of the instants of the last `SHORT_LIVED_MS`. No absorption. Events join across 24 blocks in a row and 2 between rows, so one text line is one event |
| settle, short-lived | `SETTLE_MS`, `SHORT_LIVED_MS` | `server/clip_viewer.rs` | 200, 500 | kept from version 1; both ≥ d_min. K_s = `SETTLE_MS` · f / 1000 = 4; `SHORT_LIVED_MS` is also W_a |
| cursor mask | `CURSOR_MASK_PX` | `server/clip_viewer.rs` | 48 | research: a 48 × 48 box at the logged cursor position |
| caret rule (D5 step 5) | `CARET_MAX_WIDTH_PX`, `CARET_HEIGHT_PX`, `CARET_MIN_CYCLES`, `CARET_HALF_PERIOD_MS` | `server/clip_viewer.rs` | 3; 8 to 48; 2; 400 to 700 ms | research: thin rectangles that toggle with a half-period of about 530 ms (the Windows default) |

Measured mechanisms from package 0 (`scripts/clip-equation/results-package0.md`): the reader maps with "do not wait", sleeps 1 ms and retries (M6: a yield loop burns 2.2 ms of processor per 720p read); multithread protection is on. The H.264 screen hint is `CODECAPI_AVScenarioInfo` = DisplayRemoting (M8: accepted and read back by the software encoder and Quick Sync). `Frame::timestamp()` is about 5.5 ms after the frame arrives (M3, all 3,742 frames), so the hold time (O3) is measured from arrival, and the frame chosen for an instant is the one on screen at that instant. The encoder does not report a friendly name; the hardware vendor id is reported instead (F8).

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

- The worker gets only what the live part leaves of 10 %. A weaker machine, a software encoder or a busy live part makes the clip ready later.
- The recording stops when the clip volume reaches storage pressure Level 1 (A11).
- Only then are frames lost: when the disk cannot take the raw writes, when no slot is free, or when the live part alone would pass 10 % (A9). Each lost instant is counted, named with its cause, and condition C is reported as not held for it.

Known limits that stay:

- Waiting-frame write volume (E8, settled by compression on 2026-11-22). Every new frame is written as one zstd −3 file of its B, G, R before it is encoded. Measured:
  - At 1080p in package 0b: 8.6 KB per frame typing, 25 KB flash, 123 KB scroll.
  - In the live test of 2026-11-22: 37 KB per frame on the proof page at 1584 × 992 (raw: 6.29 MB), 22.9 MB for 30 s.
  - Video-like content does not compress: about 5.6 MB per 1080p frame.
- In the worst case (video-like content, weakest machine, software encoder), a 2-minute recording can still need about 15 GB free (the earlier raw estimate of 17 GB scaled by 5.6 / 6.2; an estimate). Ordinary content needs well under 1 GB.
- Disk writes cost processor time that the equation's a_rt does not contain. Package 0 M5 and package 0b M12 measured it; A9 counts it in the live share.
- On Windows 10 the yellow capture border shows, and frames arrive per composition (up to the display rate) instead of at a set interval.
- The new-frame rate drives both the live share and the write volume. Package 0 M4 measured 8 per second while typing, and 20 per second while the mouse moves with the cursor drawn in; since E9 the cursor is not drawn in.
- The weakest machine's figures remain computed until one run on a real Core i3.

## Not in this build

Other codecs than H.264; choosing an encoder per size (C7: the encoder Media Foundation grants is used and named); colour conversion on the graphics chip; a rolling buffer that keeps only the last minutes; triggers; clicks and key presses in the clip; clip retention; removing the yellow border; storing only changed rectangles (unless the owner chooses it under E8 after package 0); making `screenshot gif` (`server/capture_gif.rs`, its own capture loop, issues #285 and #286) an export of a clip.

## Work packages

Each product package is built by one agent from this plan with a time and tool-call limit. It passes `cargo fmt --all` and `cargo clippy -p synapse-mcp -p synapse-capture -p synapse-storage --all-targets -- -D warnings`, is committed and pushed as `claude`, and is then reviewed by Codex, read-only. Findings are fixed before the next package starts. Packages run one at a time, in this order. Packages 0 and 1 may start once version 4 is committed (E7); package 1 does not depend on package 0's figures beyond the named constants it reads.

| # | Package | Contains | Status (2026-11-22) |
|---|---|---|---|
| 0 | Measurements | M1 to M9 in `scripts/clip-equation/`; one results file; `run_equation.py` with c_obs, the real c_read and c_disk. No product code | done (`996dc08d`) |
| 0b | Compression measurements | M10 to M14 in the probe; one section in `results-package0.md`. No product code | done (`a0d02e6e`, full comparison `ca46079d`) |
| 1 | Observer | O1 to O4 in `synapse-capture`: gated settings, Windows floor, slots by instant, threading rule, identity before read-back; existing screenshot paths unchanged | built (`7d1b6a34`) |
| 2 | Live part | A1 to A12; the four `record*` operations; `frames.jsonl` with frame size and cursor position; the waiting folder; `clip.json` at start with state and the registry; the public free-bytes-to-level function in `synapse-storage`; the updated contracts (O1); conditions C, G, T | built (`804644c7`); waiting frames compressed (E11, `fc5a061d`) |
| 3a | Worker lifecycle | The long-lived worker mode, its job and processor cap, the queue, segment forming (B2), commit (B8), ready (B9) and recovery (B10) | built (`4fd24043`); recovery and worker-outcome fixes from the Fable review in progress |
| 3b | Encoding | B3 to B7: σ, colours, encoder and retry, time stamps with `write_nv12` taking instant and duration, GOP in frames, no B-frames, screen-content hint, quality and Q status | built (`4f72002a`) |
| 4 | Viewer | D1 to D7 | built (`31c32615`); D5 rebuilt from the equation (`ea3e8514`, proof page `3077349b`), not yet run live |
| 5 | Proof and constants | The proof cases below; README and tool-surface tables | partly: see "Proof results so far"; README and tool-surface tables not done |

## Proof

After package 4, manual cases run in the real app through Synapse, on the development PC with the new build, plus one Windows 10 virtual machine. Test window: a local HTML page that draws a counter on every screen refresh (requestAnimationFrame) and shows a 50 ms flash in a small box once per second, its phase stepping by 5 ms across one recording interval (`scripts/clip-equation/flash-page.html`; it was missing from package 0 and was added in `660f9573`). Each case is read from `clip.json`, `frames.jsonl` and the segment files.

How the live cases run:
- The release build runs as a separate stdio MCP instance, with its own `--db` and `SYNAPSE_SHELL_JOB_ROOT`, so it does not collide with the running service.
- The test window is a Chrome `--app` window with `--disable-backgrounding-occluded-windows` and `--disable-renderer-backgrounding`. It is placed behind the owner's windows, and the test script opens and closes it around each recording (owner, 2026-11-22).
- A brand-new window that is covered before its first paint delivers no frame. `record` then refuses correctly.

### Proof results so far

| Case | Result | Build |
|---|---|---|
| 1 flash sweep | 28 of about 30 flashes found as short-lived by the viewer; the counter check per frame is not done yet | `cb70a9f7` (viewer) |
| 2 local flash during animation | held on the proof page: the counter and the clock are 2 animations; the flash box is reported separately | `cb70a9f7` |
| 3 hold time, losses | max hold 0.82 to 0.85 ms against 16.67 ms; 0 proven losses | `f5dd30bb`, `c00ff762` |
| 5 processor share | sum at most 8.1 % (worker at most 7.7 %, live part at most 1.4 %), 0 seconds over 10 % | `f5dd30bb`, `c00ff762` |
| 8 across a segment boundary | not reached: in the 90 s test the resize closed the segments before 60 s; segment budget now counts compressed bytes (834 frames in one segment) | `c00ff762` |
| 11 kill and restart | service kill at 12.4 s: recovered to `ready`, `service_restart`, 247 frames encoded from `waiting/`; the instants up to the requested end were not named lost and C was missing (fix in progress) | `c00ff762` |
| 12 resize | 3 segments at 1584 × 992, 1234 × 868, 1584 × 992; G held | `c00ff762` |
| 13 GOP | GOP 20, B-frames 0, screen hint accepted (Intel hardware encoder); 13 keyframes per 227 frames | `f5dd30bb` |
| 22 stop | `record_stop` at 10 s: `ready`, `stop_call`; a second stop returns the same state | `c00ff762` |
| waiting frames (E11) | 37 KB per frame instead of 6.29 MB; `waiting/` empty at `ready` | `f5dd30bb` |

Not run yet: 4, 6, 7, 9, 10, 14 to 21, 23 to 30.

Capture and timing:

1. Flash-phase sweep: 50 ms flashes at phases across one recording interval. Every flash is in the clip, and the counter read from each recorded frame equals the refresh shown at s_k.
2. A 50 ms local flash while another region animates is in the overview picks or named in the timeline (C15).
3. The longest hold time against T_r; proven losses zero on the counter page.
4. Observer settings on a Windows 10 virtual machine (1903 or later): recording starts, the border shows, the encoder found there is named.

Load and disk:

5. The processor share per second: live part including disk writes, worker cap given, worker share, sum at most 10 %.
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

None. E1 to E11 are answered (2026-11-22); E8 is settled by E11.

## Open items (known, not fixed in this build)

From the Fable 5.2 logic review and the live tests. Each one is either unverified or rare; they are recorded so that they are not lost:

| Item | Why it is open |
|---|---|
| The share measurement uses `GetThreadTimes`, which may under-count short bursts | unverified; needs one comparison with `QueryThreadCycleTime` |
| 28 of about 30 flashes were found as short-lived on the proof clip | likely the page: some flashes lasted 48.6 ms at 144 Hz; the page is fixed (`b608bce4`) and the case is checked again with the D5 rebuild |
| The weakest machine's figures | still computed estimates until a run on a real Core i3 |

## Changes on 2026-11-22, 18:00 to 20:35 IST

- Last code fixes: the cursor per instant on repeated lines (`7472ea12`), retry on a brief file hold for the waiting-frame rename and the unlocked `clip.json` read (`891c280d`, `7d2cb399`), the worker cap rule in the module doc (`a5e9a801`), recovery keeps the losses named in `frames.jsonl` (`bf5930e1`).
- D5 rebuilt from the equation (`ea3e8514`) with the proof page (`3077349b`). Two rules added as built, in D5 step 3. The parameter map lists the D5 constants as built.
- All code fixes are done.
- About 20:33, owner: the experiments run now (`docs/clip-recorder-experiments-plan.md`): equations for everything, every option, one results table, and this plan corrected from the results. Codex reviews the fixes meanwhile, read-only.

## Changes on 2026-11-22, 16:30 to 18:00 IST

- Owner's order: all code fixes first, then one verification round (Codex review, one build, live tests), then the experiments (`docs/clip-recorder-experiments-plan.md`, paused).
- D5 written as an equation (section D) and being rebuilt from it.
- Recovery and worker fixes from the Fable review (`b42b3c7b`, `a8f14db9`, `34ffd5a9`, `3178ff93`) and consolidations (`3c77c48a`, `2f998548`, `44278279`, `5e8e2662`, `185fb5a4`, `662a9fe2`).
- Open items closed in code (not yet run live): one lock-file open (`d1f060de`), waiting-frame names (`a6978277`), retry on a brief file hold (`51dd54b5`), slot overflow keeps the newest frame (`989aaebb`), cursor position sampled with the frame (`5204cbd1`), σ per segment (`d989e139`), worker cap from the largest live share of the last 3 s (`0b0450e9`), a recorder that ends without `finish` marks its clip `failed` (`80ece2de`).

## Changes on 2026-11-22, 14:30 to 16:30 IST

- Live tests on the release builds `f5dd30bb` and `c00ff762`: results in "Proof results so far".
- Segment budget counts compressed waiting bytes (`c00ff762`). D5 reworked: per-region short-lived changes, block-activity animations, keyframe noise, `PIXEL_DELTA` 64 (`cb70a9f7`).
- Fable 5.2 logic review (`abe511b3`). Accepted findings (worker outcome from `clip.json`, recorder lock, recovery losses and conditions, crash counting, G and B before the worker's verdict, panic guard, one merge for B, one path for lost instants, duplicated types and checks) are being fixed. The rest is under "Open items".
- Plan edits: "Where the build stands", E8 settled by the new E11, the `record` contract as built, package status, proof results, open items, outdated raw-frame and pre-measurement wording removed.

## Changes on 2026-11-22, 09:20 to 14:30 IST

- Package 0b added and measured: compression of the waiting frames (R9, equation line 249). Research `docs/clip-recorder-waiting-compression-research.md`; figures in `results-package0.md` (`a0d02e6e`, full comparison `ca46079d`).
- A8 and the clip folder: waiting frames are B, G, R, one self-contained zstd file each at `WAITING_ZSTD_LEVEL` = −3 (owner decision after the full comparison; `lz4_flex` came first and was replaced). Parameter map row added.
- Built: `5b4ed09d` (lz4), `fc5a061d` (zstd, plus losses named `waiting_unreadable`, segment seed from the last drawn picture, finalize reserve from retained bytes), `f5dd30bb` (missing files named `waiting_missing`; repeats of an unreadable picture are named lost too).

## Changes in version 4.1 (2026-11-22)

- E9: no cursor in the picture; position still logged. E10: processor limit 10 % (was 5 %); every "5 %" processor reference changed to 10 %.
- Parameter map filled from package 0: interval 11, lateness 2, slots lateness + 3 = 5, segment raw budget 1 GiB, Map mechanism, screen hint, time stamp meaning, encoder identity.

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
