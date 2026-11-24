# Clip recorder plan v3: consolidated review (2026-11-21)

Reviewers: Codex gpt-6-astra (high, read-only) — VERDICT not ready; Opus 4.8 (read-only, every file read fully after two follow-up rounds) — VERDICT ready after fixes.
Full reports: codex_review_full.md and opus_review_final.md in this folder.

## Part 1: findings both reviewers share (consolidated)

S1 | live processor budget unproven | Both: c_read in bench.rs:72 / run_equation.py:14 is a RAM memcpy, not a D3D staging CreateTexture2D+CopyResource+Map; c_obs is missing. Codex: a fixed 4 % worker + assumed 1 % live cannot enforce BR2 (20 new 4K frames/s need about 2.1 % for read-back alone). Opus: on Windows 10 (and Windows 11 before 24H2) the minimum update interval is unsupported, so observation runs at the display rate (up to 144/s), not 60/s. | Proposed fix: measure the real capture-to-spool path and c_obs at the display rate; set the worker cap to 5 % minus the measured live share instead of a fixed 4 %.
S2 | Windows 10 settings gating incomplete | Both: every non-Default setting fails on Windows 10 (graphics_capture_api.rs:140-153, 304-353). Codex adds: explicit WithCursor is also rejected where unsupported, and the base support check needs UniversalApiContract 8. | Fix: O1 gates border, secondary windows, interval, dirty regions and cursor; plan names the minimum Windows 10 build.
S3 | desktop worker reuse understated | Both: desktop_worker.rs (lines 12, 1577-1618, 1739) is a synchronous one-shot: hidden desktop, needs an HWND, WaitForSingleObject 10 s, then terminate. A clip encoder runs minutes, fire-and-monitor, survives restart. Opus: each spawn has its own anonymous kill-on-close job, so a CPU cap is isolated from other worker ops. | Fix: new long-lived worker mode reusing only the spawn/job primitives; package 3 sized accordingly.
S4 | viewer image budget | Codex: fixed N = 8 conflicts with N from token budget; eight 75 %-scale 4K images ~ 50,000 tokens; the timeline repeats on every page. Opus: v3 drops the absolute DEFAULT_LONG_EDGE = 1280 cap (clip_viewer.rs:46, 1043, 1172); a floor with no ceiling overflows the MCP image limit. | Fix: keep an absolute long-edge/token ceiling with the 75 % floor; page images by token budget.
S5 | keyframe interval τ = 5 s unvalidated | Codex: removing the crash bound left no durability bound; forced-keyframe placement unverified. Opus: τ = 5 s contradicts the research's 1 s GOP; clip_decoder.rs:31 FORWARD_READ_LIMIT_MS = 1000 assumes 1 s keyframes. | Fix: hold τ = 1 s (or derive from measured seek) until measured; derive decoder seek limit from τ.
S6 | crash durability | Codex: B1 deletes raw input when handed to the encoder, before durable output. Opus: the proof never kills mid-fragment; fragmented-MP4 crash readability unconfirmed. | Fix: keep raw frames until the segment (or fragment) is durably committed; add a kill-during-encode check to the proof.
S7 | proof too narrow | Codex: two 30 s clips cannot exercise 60 s boundaries, long static holds, slow disk, recovery, failure paths, Windows 10; opening the viewer does not prove the agent sees the flash. Opus: no 30-minute seek check, no mid-fragment kill. | Fix: widen the proof (manual Synapse cases), sweep flash phase, real-agent check.
S8 | v1 behaviours not carried forward | Both: minimized-window intervals, all-black first frame error, capture error taxonomy / stop reasons, viewer change-test constants (colour-aware, Codex fix 6) are in the v1 code but absent from v3. | Fix: v4 lists them as kept behaviour.
S9 | slow disk loses frames: owner never asked | Codex (blocker): v3 line 149 accepts frame loss on a slow disk, contradicting C and BR4 ("only time gives way"). Opus: the owner's open question "which condition gives way first when a machine cannot meet all" (11-equation:135) was answered silently with C. | Fix: put the choice to the owner.

## Part 2: findings from one reviewer only, with Jev direction (advisory; jev-1.14.0, triage preset; choices: fix_in_plan / settle_in_package / proof_item / drop; owner = needs owner decision)

C1 (codex, blocker) plan v3 A8/O3 (line 62): a frame handler that always returns within T_r with a one-buffer pool is claimed to prove no frame loss. Codex: Windows delivery, scheduling, frame acquisition and pool recreation happen outside the handler; research measured about 45 fps for a 16.66 ms request. Fix: drop the sufficient-condition claim; report proven loss, unknown delivery completeness, and measured controlled-case coverage separately.
C2 (codex, blocker) plan v3 O ring (line 49): 4 GPU textures assume bounded arrival rate and zero selection delay; a stalled consumer can lose the needed frame while callbacks stay short; Windows 10 per-composition delivery and cursor movement can exceed 60/s. Fix: specify delivery watermark, max selection lateness, pinned-slot ownership, overflow handling, ring size derived from those bounds.
C3 (codex, major) plan v3 O2/O4 (line 45): GPU ring copy and readback threading unspecified; a D3D11 immediate context cannot be used from two threads at once; a lock held across a blocking Map can stall capture. Fix: define context ownership, copy ordering, staging completion and non-overwrite rules.
C4 (codex, major) plan v3 O4 (line 47) reads the chosen frame back before A6 checks whether it is a repeat, contradicting "a repeated frame costs no read-back". Fix: select by frame identity first, read back only after the repeat check.
C5 (codex, major) plan v3 A9 (line 63): letterboxing during resize settle re-introduces letterboxing that the approved equation removed; continuous dragging can break G indefinitely; CPU letterbox contradicts A7 "only one copy". Fix: keep native-size captured frames and handle resize in the deferred worker, or amend G and count live scaling cost.
C6 (codex, major) plan v3 conditions (line 116) removed the pass/fail for Q, contradicting V = C·G·Q·B·T and the approved per-condition report. Fix: calibrate q_min and fallback readability per supported path; report Q as established / failed / unverified against that calibration.
C7 (codex, major) plan v3 B3 (line 74): letting Media Foundation pick the encoder neither proves "cheapest encoder" nor the minimum-byte E(w,h); only size refusal gets a software retry, other hardware failures are not handled. Fix: keep H.264, define supported-path selection and a bounded software replay from retained inputs; soften the optimisation claim.
C8 (codex, major) plan v3 B4 (line 75): time stamps alone do not define variable-frame-rate display durations, the final hold, or segment-local timing; "no encoder work after the first frame" conflicts with independently playable 60 s static segments. Fix: specify sample durations through stop, global-to-segment time mapping, and an initial sample per segment.
C9 (codex, major (candidate)) plan v3 B5 (line 76): CODECAPI_AVEncVideoForceKeyFrame applies to the next MFT input, not necessarily the intended Sink Writer sample when buffering intervenes. Fix: validate keyframe placement on software and Intel encoders; derive the keyframe index from actual output.
C10 (codex, major) plan v3 B8 (line 79): restart scan misses the state "last raw file deleted, clip.json not written"; no durable initial descriptor, segment commit protocol or custom-folder discovery; viewing finished segments needs metadata before clip.json. Fix: persist initial clip state and atomic segment checkpoints; recover all incomplete states idempotently.
C11 (codex, major) plan v3 A2 (line 56): one active recording does not mean one encoding worker; earlier clips and restart recovery can overlap, each with its own 4 % job. Fix: one global encoding queue or one shared capped job, including recovered work.
C12 (codex, major) plan v3 A2/A10 (line 55): start gate plus "stop when privacy pauses" loses the per-frame privacy gate before writing (frame zero included) fixed in the v1 code review; monitor recording only checks the foreground app, visible excluded background windows are not handled. Fix: define privacy checks before persistence, queue cut-off on pause, and monitor-visible exclusion rules.
C13 (codex, major) plan v3 A10 (line 64): a once-per-second free-space check cannot guarantee "the disk never fills"; output volume choice, raw backlog, other writers and finalisation failures unspecified. Fix: probe the actual spool/output volume, account for pending writes and finalisation, define recoverable disk-full behaviour.
C14 (codex, major) plan v3 C ops (line 88): states recording/encoding/ready cannot express worker failure, interrupted capture, failed recovery or disk-full stop; session ownership and cancellation changed without a contract. Fix: define terminal failure/interrupted states, ownership at start, idempotent stop, keep committed segments.
C15 (codex, major) clip_viewer.rs:593 overview picks a transient onset only when the whole episode reverts; a 50 ms local flash during unrelated animation can be omitted; the cursor mask promised in plan v1 is absent (clip_viewer.rs:637). Fix: validate transient selection under unrelated motion; keep local transient candidates; implement cursor/noise handling.
C16 (codex, minor) plan v3 parameter map (line 134): 4096x2304 presented as a universal H.264 level 5.1 limit; level limits and what an encoder accepts differ, portrait sizes too. Fix: call it a chosen bound and validate encoder acceptance.
C17 (codex, minor) plan v3 parameter map (line 127): T_r is seconds per refresh in the equation but Hz in the map; the ring formula mixes units. Fix: separate refresh frequency and period symbols.
C18 (codex, major) plan v3 proof: no real-agent check of the smallest readable text h_leg (equation section 3), and no regression acceptance for the ten v1 code-review fixes (cancellation, backend failure, startup timing, cache invalidation, final-frame interval, encoder finalisation). Fix: add both to the proof.
O1 (opus, major) tool_profiles.rs:391: the record facade contract requires a synchronous read-back of clip.mp4 path + size + SHA-256 (honoured by capture_clip.rs today); plan v3 returns at once with no media and writes segment-*.mp4, so the contract breaks and is not updated. Fix: redefine the record read-back (folder + id + status; SHA-256 per segment when ready) and update the contract.
O2 (opus, major) clip_decoder.rs:31,54: FORWARD_READ_LIMIT_MS = 1000 assumes a keyframe every second; the decoder opens a single clip.mp4, while plan v3 D1 needs several segments. Fix: derive the seek limit from the real τ; extend the decoder to segments.
O3 (opus, minor) clip_encoder.rs:120,470 write_nv12 time-stamps by frame counter (constant rate); plan v3 B4 needs s_k time stamps. Fix: pass s_k into write_nv12 (SetSampleTime/Duration).
O4 (opus, minor (candidate)) clip_encoder.rs sets no B-frame disable and no screen-content hint (research: film fundamentals section 6.3, Windows capture note). Fix: set B-picture count 0 and the screen-content hint.
O5 (opus, minor) synapse-capture frame.rs: CapturedFrame has only captured_at: Instant, no composition time; the new observer must keep SystemRelativeTime itself. Fix: do not route clip frames through CapturedFrame.

Jev results:
C1 fix_in_plan 0.87 owner:no/0.25
C10 fix_in_plan 0.73 owner:no/0.25
C11 fix_in_plan 0.77 owner:no/0.35
C12 fix_in_plan 0.73 owner:no/0.16
C13 fix_in_plan 0.78 owner:no/0.44
C14 fix_in_plan 0.95 owner:no/0.34
C15 settle_in_package 0.66 owner:no/0.30
C16 fix_in_plan 0.58 owner:no/0.19
C17 fix_in_plan 0.67 owner:no/0.10
C18 proof_item 0.64 owner:no/0.14
C2 fix_in_plan 0.65 owner:no/0.40
C3 fix_in_plan 0.73 owner:no/0.16
C4 fix_in_plan 0.58 owner:no/0.11
C5 fix_in_plan 0.87 owner:yes/0.52
C6 fix_in_plan 0.52 owner:no/0.49
C7 fix_in_plan 0.86 owner:no/0.48
C8 fix_in_plan 0.81 owner:no/0.20
C9 settle_in_package 0.46 owner:no/0.23
O1 fix_in_plan 0.92 owner:no/0.44
O2 settle_in_package 0.43 owner:no/0.23
O3 settle_in_package 0.67 owner:no/0.09
O4 settle_in_package 0.60 owner:no/0.28
O5 drop 0.40 owner:no/0.17

## Part 3: verdict disagreement
Codex: not ready. Opus: ready after fixes.
