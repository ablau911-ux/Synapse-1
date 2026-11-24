**Coverage:** every file in the bundle index is now read in full, first line to last: the six named here (frame-selection note, recording-patterns note, video-models note, plan v1, equations 10 and 12) plus everything read in full before. The code was checked at the points the decisions depend on, not read whole.

Still read-only: no file created or edited, HEAD `30392a46`, working tree unchanged.

**What the full read changed** (each is marked in the tables):
- **S1:** package 0 also measures how many new frames mouse movement and a blinking caret produce.
- **S4:** the image ceiling drops from 2,000 px to 1,568 px.
- **S7:** two more proof cases.
- **S8:** five more kept behaviours from plan v1.
- **C15:** caret and animated regions are named.
- **E1:** precedents and the hourly write volume are added.
- **E8:** new owner question on raw write volume.
- **F and G:** one added unmeasured risk and the matching package 0 item.

No decision, Jev agreement or verdict changed.

**Checked in the code myself:**
- `tool_profiles.rs:391-395`: the record contract requires `clip.mp4` path + size + SHA-256.
- `clip_decoder.rs:27-28`: the seek limit assumes a keyframe every second.
- `clip_viewer.rs`: default long edge 1280 (a caller can override it), 8 images per call, no cursor mask, and a change counts as reverted only when the whole picture returns.
- `desktop_worker.rs`: 10 s wait, hidden desktop, one anonymous job per spawn.
- `bench.rs:73` and `run_equation.py`: the read-back cost is a memory copy and c_obs is absent.
- `capture.rs:128-152`: every capture setting is always non-default.
- `graphics_capture_api.rs:134-158`: the crate rejects all five unsupported settings, cursor included.
- `clip_encoder.rs`: time stamps come from a frame counter; no B-frame or screen-content setting.

**Taken from the reviewers, not re-checked:** the UniversalApiContract 8 requirement, the foreground-only privacy check, and `capture.rs:279`.

"settle_in_package" means v4 names the item in one line and the package decides the how and the values.

## Shared findings S1-S9

Jev was not asked about S1-S9, so there is no Jev column here.

| # | Decision | Change to write into v4 | Owner |
|---|---|---|---|
| S1 | fix_in_plan_v4 + proof_item | Remove the fixed `WORKER_CPU_PERCENT` = 4. The job cap is 5 % minus the live share the service measures each second, which is what the approved equation already says (`0.05·n − a_rt`). A new package 0 measures the real path (staging copy + Map, c_obs at the display rate, spool write) before packages 2 and 3. **(changed after full read)** Package 0 also measures the new-frame rate while the mouse moves over the window and while a caret blinks, because Windows delivers a frame for each. | no (the case "live part alone above 5 %" goes to E1) |
| S2 | fix_in_plan_v4 | O1 builds its own settings and asks for cursor, border, secondary windows, interval and dirty regions only where the crate reports each supported. The plan names Windows 10 1903 as the minimum. | yes (E5) |
| S3 | fix_in_plan_v4 | B becomes "a new long-lived worker mode of the same executable". It reuses only `current_exe` and `create_worker_kill_on_close_job`, not the launcher (hidden desktop, HWND, 10 s wait, envelope). Package 3 splits into 3a (lifecycle, cap, queue, recovery) and 3b (encode). | yes (E2, already open) |
| S4 | fix_in_plan_v4 | Keep a ceiling beside the 0.75 floor: a per-image `VIEW_MAX_LONG_EDGE` and a token budget per call that sets N (at most 8). Where the ceiling cuts below the floor, the response says so and points to `clip_frame`. The timeline goes on page 1 only. **(changed after full read)** The ceiling starts at 1,568 px, not 2,000: standard-tier Claude models downsize anything larger, and a reported 2,000 px client cap can break the agent session. Budgets start at about 20,000 image tokens plus 4,000 text for the overview and about 10,000 for the other calls. | no |
| S5 | fix_in_plan_v4 | Count the keyframe interval in encoded frames: GOP = τ·f = 20 frames, τ = 1 s until v_dec is measured. Seek cost is bounded by frames decoded, so this keeps the decoder's assumption. With repeats not encoded, a slowly changing window still gets few keyframes. | no |
| S6 | fix_in_plan_v4 + proof_item | Raw frames of a segment are deleted only after the segment file is finalized, flushed, renamed from `.part` and checkpointed. A crash discards the `.part` and re-encodes from raw, so durability no longer rests on fragmented MP4. A segment also closes when its retained raw bytes reach a named budget. | no |
| S7 | proof_item | Rewrite the proof as manual Synapse cases: a 90 s clip across a segment boundary, a 10-minute still hold, a 30-minute seek check, kill worker and service mid-segment, a low-free-space volume, a flash-phase sweep, a real agent finding the flash, and O1 on a Windows 10 virtual machine. **(changed after full read)** Add a clip with a planted wrong value that the agent must name by frame. Add a measurement in Claude Code and Codex of whether images arrive as image blocks, how they count against the result cap, and whether the client resizes them. | no |
| S8 | fix_in_plan_v4 | Add a "Kept from version 1" section: minimized intervals without restore or focus, the all-black first-frame error, the error codes and stop reasons, the colour-aware change test, the pinned WGC backend, the own capture handle, the browser-tab rule. **(changed after full read)** Also: the monotonic start plus wall-clock anchor in `clip.json`, the label and file path on every image, the 200 px minimum crop, and "the folder must not exist". State that the v1 temp-folder-then-rename publish is replaced by the state field of C10. | no |
| S9 | fix_in_plan_v4 after the owner answers | Replace the "known limit" at line 149 with the owner's explicit order of what gives way, and report each lost instant. | yes (E1) |

## Codex-only findings C1-C18

| # | Decision | Jev | Change to write into v4 | Owner |
|---|---|---|---|---|
| C1 | fix_in_plan_v4 | agree | Drop "a handler within T_r can drop nothing". Report three things: proven losses, the longest hold time (composition time to frame release) against T_r, and delivery completeness as "checked only in the proof". The requested interval becomes a measured value starting at 11 ms, because research measured about 45 fps at 16.7 ms. | no |
| C2 | fix_in_plan_v4 | agree | Replace the 4-texture ring with slots assigned by instant: the handler computes k from the composition time, keeps one candidate slot for the open instant and pins it when a later frame arrives. Slot count = longest allowed selection lateness in instants + 2. A pinned slot is never overwritten; overflow is recorded as a lost instant. | no |
| C3 | fix_in_plan_v4 | agree | State the rules: multithread protection on the capture device, the handler only issues GPU copies and never maps, the reader thread never holds the context across a wait. The mechanism (a non-waiting Map with retry is my proposal) is confirmed in package 0. | no |
| C4 | fix_in_plan_v4 | agree | O4 returns the frame identity (composition time) first; A6 compares it with the last recorded one; read-back happens only for a new frame. | no |
| C5 | fix_in_plan_v4 | agree, including owner | The live part stores every frame at its captured size and does no scaling. The worker forms segments. My recommendation for frames during a drag is one transition segment whose canvas is the largest size in the drag, frames placed unscaled. | yes (E4) |
| C6 | fix_in_plan_v4 | agree | Q gets a status again: unverified until q_min is calibrated for the encoder actually used, held when quality mode at q ≥ q_min was accepted and read back, not held on an uncalibrated bitrate fallback. | no |
| C7 | fix_in_plan_v4 | agree | B3 says "the encoder Media Foundation grants, named in the clip"; choosing an encoder per size moves to "Not in this build". Any encoder failure restarts the segment once from its retained raw frames with hardware transforms off; a second failure ends the clip as failed with committed segments kept. | no |
| C8 | fix_in_plan_v4 | agree | B4 states: sample duration = next new frame's instant minus its own, the last sample runs to the stop time, every segment starts with a keyframe of the picture then shown, and `frames.jsonl` maps global time to segment and local time. Proof item 4 becomes "one encoded frame per segment for a still window". | no |
| C9 | fix_in_plan_v4 (drop the forced keyframe) | overrule (Jev: settle_in_package) | With S5 the keyframe interval is a GOP size in frames, so B5 no longer needs `CODECAPI_AVEncVideoForceKeyFrame`. Removing an unverified dependency is better than deferring its validation. The keyframe column in `frames.jsonl` is read from the finished segment, and the proof compares it. | no |
| C10 | fix_in_plan_v4 | agree | `clip.json` is written at start with a state field and updated atomically at each segment checkpoint. Active clips in custom folders are listed in one registry file. Recovery works from the state, not from which files exist. | no |
| C11 | fix_in_plan_v4 | agree | One clip worker at a time for the whole service, with a first-in first-out queue that recovered clips also enter. | no |
| C12 | fix_in_plan_v4 | agree on the decision; owner yes for one half | A7 checks the privacy gate before every raw write, frame zero included; on pause, frames composed after the pause instant are discarded. The monitor rule for a visible excluded app is E6. | yes (E6) |
| C13 | fix_in_plan_v4 | agree | Replace "the disk never fills" with a rule on the volume that holds the clip folder: free space measured every second minus bytes written since, with a reserve for the segment being finalized. A write failure is a recoverable stop that keeps committed segments. | no |
| C14 | fix_in_plan_v4 | agree | States: recording, encoding, ready, failed (with reason), plus a stop reason. The recording belongs to the service and runs to its requested end; stop is idempotent by clip id; committed segments always stay. | no |
| C15 | settle_in_package (4, with one field in 2) | agree | D5: a short-lived change is detected per changed region, not only when the whole picture returns. Package 2 logs the cursor position per instant so the viewer can mask it. Proof gate: a 50 ms local flash during other animation is in the overview picks or named in the timeline. **(changed after full read)** D5 also names the two other noise rules from the research: ignore the blinking caret, and report a continuously animating region as one interval with one frame. | no |
| C16 | fix_in_plan_v4 | agree | Call it a chosen bound (36,864 macroblocks, each side at most 4096). The encoder's real acceptance is found at open, portrait sizes included. | no |
| C17 | fix_in_plan_v4 | agree | Two symbols: f_r = 60 Hz (`REFERENCE_REFRESH_HZ`) and T_r = 1 / f_r. | no |
| C18 | proof_item | agree | Add a real-agent reading of the smallest text (h_leg) and a regression check for each of the ten v1 review fixes. | no |

## Opus-only findings O1-O5

| # | Decision | Jev | Change to write into v4 | Owner |
|---|---|---|---|---|
| O1 | fix_in_plan_v4 | agree | `record` reads back the folder, the clip id and the `clip.json` written at start (path, size, SHA-256). Each segment's size and SHA-256 are recorded at commit and returned by `record_status`. `FACADE_TOOL_CONTRACTS` gets updated entries for record, the three new operations and the viewer. | no |
| O2 | settle_in_package (4) | agree | One parameter-map row: the decoder's forward-read limit is computed from the keyframe interval, not a constant of its own. Segment reading is already D1. | no |
| O3 | settle_in_package (3b) | agree | `write_nv12` takes the instant and the duration; covered by the C8 text. | no |
| O4 | settle_in_package (3b) | agree | B4 adds "no B-frames, set and read back", because the variable time stamps assume no reordering. The screen-content hint is tried, and its acceptance and byte effect go into the proof. | no |
| O5 | drop | agree | Already implied by section O, which gives the clip its own handler and frame time. At most one clause in O2. | no |

## E. Questions only the owner can answer

| # | Question | Options | Recommendation |
|---|---|---|---|
| E1 | When a machine cannot meet every condition (the disk cannot absorb the raw frames, or the live part alone needs more than 5 %), what gives way? v3 answered this without asking. | (a) Frames are lost for those instants, counted and reported. (b) The live part may exceed 5 %. (c) Size or quality drops. (d) The recording stops. | (a), in this order: time first, then length (stop at disk Level 1, the existing rule), then the named instants of C. Never the 5 %, size or quality. **(changed after full read)** The research has precedents for both steps: Steam stops recording on low disk space, and Jam sheds load on heavy targets. |
| E2 | The worker as a separate process. This overrides the v1 decision "no child processes". | Yes / no. | Yes. A job object is the only hard limit that also covers the encoder's threads; without it the 5 % can only be measured, not enforced. |
| E3 | Worker share. | Fixed 4 % / 5 % minus the measured live share. | The second. It is what the approved equation says and it replaces the open "4 %" decision. |
| E4 | Frames captured while a window is being dragged to a new size. | (a) One transition segment, canvas = largest size in the drag, frames unscaled. (b) Letterbox into the current size, as v3. (c) One segment per size. | (a). It keeps the pixels exact and costs nothing in the live part; G is reported as "padded" for those instants. |
| E5 | Minimum Windows 10 version. | Windows 10 1903 and later / something else. | Accept 1903. Windows capture does not exist before 1803 and the crate needs 1903. On Windows 10 the yellow border always shows. |
| E6 | Monitor recording while an excluded app is visible but not in front. | (a) Refuse or stop. (b) Keep the existing foreground-only rule and log an issue. | (a). The check is one window enumeration per second. It is a new privacy behaviour, so it needs your yes. |
| E7 | Go. | Start package 0 and package 1 once v4 is committed / wait. | Start. |
| E8 **(changed after full read)** | Raw write volume through the disk. Deferring the encode to hold the 5 % means every new frame is first written raw. | (a) Accept for this build and state the figure in the plan. (b) Bring "store only the changed rectangle" into this build; Windows reports it only from Windows 11 24H2. (c) Limit how long a recording may run. | (a), then decide again on package 0's measured figures. |

Context for E1 and E8:
- At the equation's "typical" activity (3.8 new frames per second) a 1920 × 1080 window writes about 31 MB of raw frames per second, about 113 GB per hour.
- The research cites 20 GB per hour for another recorder as a disk-wear concern.
- A window that changes on every recorded frame writes 166 MB per second.
- Mouse movement over the window may push ordinary use toward that worst-case figure; this is unmeasured and is a package 0 item.
- In the worst case on the weakest machine with the software encoder, a 2-minute recording needs about 17 GB free.

## F. Verdict

**Ready after fixes**, unchanged by the full read. No package may start from v3 as written. The two reviewers found the same facts and differ only in the label. Codex is right on severity:
- Package 1 would be built to a wrong ring and threading spec (C2-C4).
- The 5 % budget and the no-loss claim are stated as proven when they are not (S1, C1).
- Raw frames are deleted before the output is safe (S6).
- An owner-level conflict was answered silently (S9).

Opus is right on the remedy: every finding is closed by one plan revision, one measurement run and the owner's answers. Nothing overturns the equation or the split into observer, live part, capped worker and viewer.

Three limits remain:
- My estimate that the live part stays under 5 % at 1920 × 1080 is unmeasured until package 0.
- The Core i3 figures stay computed until a run on a real one.
- **(changed after full read)** The "typical" 3.8 new frames per second came from one window. The real rate with mouse movement and a blinking caret is unmeasured, and it drives both the live share and the write volume.

## G. Order of work

Before any build, v4 gets:
1. The owner's answers to E1-E6 and E8.
2. A new package 0: measurements only, in `scripts/clip-equation/`.
3. Section O rewritten (S2, C1-C4, C17).
4. Sections A and B rewritten (S1, S3, S6, C5-C14, O1).
5. Section D extended (S4, C15, O2).
6. The "Kept from version 1" section (S8).
7. The proof rewritten (S7, C18).

v4 states the worker share and the requested capture interval as formulas, so package 0's numbers fill the parameter map without another plan version.

Package 0 starts first. It measures:
- the real read-back and observation cost;
- whether Windows delivers every refresh;
- the threading rule;
- **(changed after full read)** the new-frame rate under mouse movement and a blinking caret.

Package 1 (observer) follows and does not depend on the E answers, so it can run while those are pending. Then, one at a time: 2 (live part, durable clip state, contract), 3a (worker lifecycle, queue, recovery), 3b (encoding), 4 (viewer), 5 (proof).