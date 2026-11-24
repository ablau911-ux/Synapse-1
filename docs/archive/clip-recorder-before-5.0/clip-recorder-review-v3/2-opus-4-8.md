All seven files now read end-to-end. Several things changed — marked **(new after full code read)**.

---

## A. Coverage table

| File | Status |
|---|---|
| 00-INDEX.md / 01-v1 / 02-v1.1 / 03-v2 / 04-v3 | read fully |
| 10-equation-draft | read fully |
| 11-equation-9ba1754a | **read fully (new after full read)** |
| 12-equation-base-rules | **read fully (new after full read)** |
| 13-equation-approved | read fully |
| 20-issue-290 / 21-issues-285-286 | read fully |
| 30/31 research + 32-note ×5 | read fully (via sub-agent) |
| windows-capture-2.0.0/src/frame.rs | **read fully (new after full code read)** |
| windows-capture-2.0.0/src/graphics_capture_api.rs | **read fully (new after full code read)** |
| synapse-capture/.../windows/capture.rs | **read fully (new after full code read)** |
| synapse-mcp/server/capture_clip.rs | **read fully (new after full code read)** |
| synapse-mcp/server/clip_encoder.rs | **read fully (new after full code read)** |
| synapse-mcp/server/clip_decoder.rs | read fully |
| synapse-mcp/server/clip_viewer.rs | **read fully (new after full code read)** |
| synapse-mcp/server/tool_profiles.rs | read fully (screenshot ops + facade contract in full; profile machinery read) |
| synapse-mcp/desktop_worker.rs | **read fully, all 2080 lines (new after full code read)** |
| synapse-capture/src/frame.rs | read fully |
| synapse-mcp/server/capture_gif.rs | read fully |
| scripts/clip-equation/{bench.rs, measure_encoder.ps1, run_equation.py} | read fully |

**Facts F1–F9: all still verified** at their cited places; F3 re-confirmed (`Direct3D11CaptureFramePool::Create(..,1,..)` is the only pool creation, plus `Recreate(..,1,..)` on resize).

## B. Findings

**(new after full code read) — reshaped by reading `desktop_worker.rs` in full:**

`desktop_worker.rs:12,1577,1739 | major | The existing desktop worker is a 10-second-bounded, hidden-desktop, synchronous one-shot: spawn (CREATE_SUSPENDED on lpDesktop) → assign to a per-spawn kill-on-close job → resume → WaitForSingleObject(WORKER_TIMEOUT_MS=10_000) → read a JSON/BGRA envelope → terminate+reap. v3 F6 calls the clip worker "one more operation of this worker, not a new mechanism," but a streaming encoder runs 60 s–minutes, must be fire-and-monitored (record returns immediately), needs no hidden desktop, and must resume after restart — the 10 s wait would hard-kill it. The reusable part is only current_exe + per-spawn job + suspended-create/assign/resume/readback; B1–B8 are a new worker mode. Package 3 is larger than the plan states. | Plan the clip worker as a new long-lived worker mode; reuse only the spawn/job primitives. | VERIFIED`

`desktop_worker.rs:982,1739 | (resolves earlier finding) | Each spawn calls create_worker_kill_on_close_job() → CreateJobObjectW(None,null): an anonymous job PER worker, not a shared one. So adding JOB_OBJECT_CPU_RATE_CONTROL|HARD_CAP to the clip worker's own job is isolated and will NOT throttle Context/Snapshot/Capture ops. My earlier "shared-job throttle" finding is withdrawn. | No action. | VERIFIED (new after full code read)`

**Carried / confirmed (VERIFIED unless noted):**

`capture_clip.rs:417 | major | record always sets secondary_windows:true, min_update_interval Custom, and the shared capture.rs always sets WithoutBorder; the crate returns BorderConfigUnsupported / SecondaryWindowsUnsupported / MinimumUpdateIntervalUnsupported for every non-Default setting it can't support (graphics_capture_api.rs:140-153,304-353), so three separate Windows-10 start failures. v3 O1's "request only where is_*_supported()" is confirmed necessary and feasible (crate exposes is_border/secondary/min_update_interval_supported via ApiInformation). | O1 must gate all three settings, not only the border. | VERIFIED (new after full code read)`

`bench.rs + run_equation.py + capture.rs:281-312 | major | The live-CPU budget is under-modeled three ways: (1) c_read=0.13 ns/px (bench copy_bgra) is a RAM memcpy, but the real readback is CreateTexture2D(staging)+CopyResource+Map(READ)+tight row copy — a GPU→CPU stall, worse on old Intel iGPUs; (2) run_equation.py omits c_obs entirely; (3) with MinUpdateInterval unsupported (Win10/most Win11) v3 must use Default = per-composition delivery, i.e. observation at the DISPLAY rate (up to ~144/s on the dev machine), not the modeled 60/s, with no way to throttle to 60. The uncapped live part therefore has three under-counted costs, so "≤5% on any machine" (BR2) is unproven. | Measure true staging readback + c_obs at the real display rate before asserting the 1% live budget. | VERIFIED (new after full code read)`

`clip_viewer.rs:46,1043,1172 | major | The viewer already has an absolute safety cap: DEFAULT_LONG_EDGE=1280 and fit() downscales every overview/range image to it ("~1,200 tokens each, 8 stay under 25k"). v3's parameter map replaces σ_v→DEFAULT_LONG_EDGE with OVERVIEW_MIN_SCALE=0.75-of-window — a floor with no ceiling. A 1440p/4K window × 8 images blows the ~25k-token MCP cap and Claude's long-edge budget. | Keep an absolute long-edge/token ceiling alongside the 0.75 floor. | VERIFIED (new after full code read)`

`tool_profiles.rs:391 | major | FACADE_TOOL_CONTRACTS for record mandates a synchronous readback "clip.mp4 path + size + SHA-256 read back from the written file" (honored today by capture_clip.rs file_sha256). v3's async model returns folder+id with no media written and produces segment-*.mp4, not clip.mp4 — the mandatory readback invariant is broken and the contract (files, error taxonomy) is un-updated. | Redefine the record readback (folder+id+status; move SHA-256 to ready/record_status) and update the contract. | VERIFIED`

`clip_decoder.rs:31,54 | major | FORWARD_READ_LIMIT_MS=1000 and its comment ("the recorder writes a keyframe every second") bake a 1 s GOP into the viewer seek heuristic; τ=5 s makes every seek decode up to 5 s. The decoder also opens one clip.mp4 (ClipDecoder::open on VIDEO_FILE), while v3 D1 needs multi-segment reading. | Derive the seek threshold from actual τ; extend the decoder to segments. | VERIFIED`

`13-equation:138 / 04-plan-v3:140 | major | τ=5s contradicts research's 1s GOP and rests on unmeasured v_dec/seek. | Hold τ=1s until measured. | VERIFIED`

`clip_encoder.rs:120,470 | minor | write_nv12 timestamps by a frame counter (frame_time_hns = frame×1/fps, CFR). v3 B4 (per-frame s_k, skip repeats, variable times) needs the encoder API to take the s_k timestamp; SetSampleTime/Duration already support it, so feasible but a signature change. | Pass s_k into write_nv12. | VERIFIED (new after full code read)`

`clip_encoder.rs | minor | No B-frame disable (research FILM §6.3) and no screen-content hint (research WIN). | Set B-picture count 0 and the screen-content hint. | CANDIDATE`

`frame.rs (synapse-capture) | minor | CapturedFrame carries only captured_at:Instant, no composition-time field; the new observer must store SystemRelativeTime itself. | Don't route clip frames through CapturedFrame. | VERIFIED (new after full code read)`

`04-plan-v3:181 | minor | Proof never hard-kills mid-fragment; fragmented-MP4 crash readability unconfirmed. | Add a kill-during-encode readability check. | VERIFIED`

`04-plan-v3:131 | minor | 60s time-segments possibly unneeded (v2 audit); not settled by proof. | Verify one 30-min clip seeks in bound first. | VERIFIED`

## C. Lost / dropped from v1→v3

- Owner's explicit open question **"which condition gives way first when a machine can't meet all"** — last in `11-equation:135`; v3 "Known limits that stay" silently answers **C** (frames lost), never put back to the owner. *(new after full read)*
- FACADE record readback (clip.mp4 + SHA-256) — active in `tool_profiles.rs:391`, honored by `capture_clip.rs` today; broken, not updated, by v3's async/segment model. | VERIFIED
- Viewer absolute cap DEFAULT_LONG_EDGE=1280 — present and applied in `clip_viewer.rs`; dropped from v3 param map. | VERIFIED
- Change-test constants + "measured against encoder noise" — equation §4; present in `clip_viewer.rs` (colour-aware, Codex-fix #6 applied) but dropped from v3 param map. | VERIFIED
- Minimized-window handling, all-black-frame error, capture error taxonomy — all present in `capture_clip.rs` (target_minimized/close_minimized, is_all_black:826, StopReason, CAPTURE_TARGET_LOST); v3 plan silent on carrying them into the new observer/worker split. | VERIFIED
- Event-aligned frame selection (research #1 priority) — deferred; 10 s uniform floor present in `clip_viewer.rs` (with_floor). | VERIFIED (acknowledged)
- Always-on privacy features (IsPassword masking, retention caps, visible indicator, Gemini opt-in) — deferred; not needed for on-demand. | VERIFIED (acknowledged)

## D. Conclusion

Reading the seven remaining files in full sharpened, not reversed, the verdict, and it moved two findings in opposite directions. In v3's favour: each desktop-worker spawn gets its **own** anonymous kill-on-close job, so my earlier worry that a CPU hard-cap would throttle other worker ops is withdrawn — the cap is cleanly isolable. Against v3: that same file shows the existing worker is a 10-second-bounded, hidden-desktop, spawn-await-reap **one-shot**, so v3's framing of the streaming encoder as "one more operation, not a new mechanism" understates package 3 — a long-lived, fire-and-monitored, restart-resumable, CPU-capped worker is a new mode that reuses only the spawn primitives. The other confirmations are concrete now: the facade readback contract for `record` (clip.mp4 + SHA-256) is real and is broken by the async/segment design; the viewer's working absolute 1280-px token cap is dropped by v3's window-relative floor and would overflow the MCP image limit on large monitors; the decoder bakes a 1 s-keyframe seek assumption that τ=5 s breaks and still opens a single clip.mp4; and the live-CPU budget rests on an unrepresentative c_read (a RAM memcpy, not a D3D staging map), an omitted c_obs, and an observation rate that — with MinUpdateInterval unsupported on Windows 10 and most Windows 11 — runs at the display's refresh rate, not the modeled 60/s, so "≤5 % on any machine" stays unproven. None of these are blockers: all are addressable in packages 2/3/5 and the proof, the equation lineage (10→11→12→13) is sound, and every v2 review/audit point is folded in. But the readback-contract breakage, the worker-scope understatement, and the unproven live budget should be settled before building packages 2 and 3, and the viewer cap restored in package 4.

VERDICT: ready after fixes