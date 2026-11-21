# Clip recorder: building plan

Status 2026-11-21 16:40 IST. Branch `feature/clip-capture`. Owner: Adam. Builders: Claude agents; review: Codex.

## Goal

Agents can see clips of bugs while an app is in active use. Two parts:

1. **Recorder**: records a short video clip of one window or one monitor.
2. **Viewer**: lets an agent look at a clip as a text timeline plus a few chosen still frames.

## Sources fused

| # | Source | What it contributed |
|---|---|---|
| S1 | Research document `reports/Bug clip video - full research results.md` (14 sections, academic and technical sources) | Architecture, encoder choice, clip folder format, frame selection, viewer tools, defaults |
| S2 | Measurements on this PC, 2026-11-21 (`clipbench`, H.264 only) | Real numbers: sizes, CPU, colour accuracy, 20 fps feasibility |
| S3 | Code inspection of the current tree (capture crate, GIF recorder, screenshot facade) | What to reuse, what not to copy |
| S4 | Review of the 51 upstream documents (README, CONTRIBUTING, systemdocs 01-18, ADRs, capability matrix) | Project rules, tool-surface limits, privacy and storage rules |

## Owner decisions (fixed)

- Target: one window or one monitor. Maximum clip length 2 minutes. 20 fps default, 30 fps maximum.
- Small files; MP4 was only an example, the encoding is chosen on evidence.
- Windows' yellow capture border is accepted for now.
- Viewers are the existing AI agents only (no local model, no Gemini).
- Recording on request first. Rolling buffer and triggers come later.
- Simple, maintainable by one person; reuse existing code; no ffmpeg, no child processes.
- No automated tests (upstream policy #1699).

## Key facts that drive the design

| Fact | Source | Consequence |
|---|---|---|
| Windows.Graphics.Capture is the only API that records a window behind other windows | S1 §5.1 | Use WGC through the existing `synapse-capture` stream |
| WGC delivers frames only when content changes; its own rate control is imprecise | S1 §5.2, S2 | The recorder owns a constant-rate ticker and repeats the last frame; repeats cost almost nothing in H.264 |
| A fully static window is a still picture, not a clip | S2 (Finance Manager runs: 1 new frame in 600) | Flag new vs repeated per frame so the viewer skips repeats |
| Media Foundation H.264 is built into Windows; hardware use must be requested and verified | S1 §5.3 | Own MF Sink Writer encoder; record which encoder was really used |
| The ready-made `windows-capture` encoder: bitrate only, no keyframe control, colour tint | S1 §5.3, S2 (colour error up to 21/255 vs 1-3) | Do not use it |
| Quality-based encoding is far smaller than fixed bitrate for UI content | S2 (x264 quality 23/28: 10 / 7.2 MB/min on changing text; fixed 6 Mbps: 43.5 MB/min) | Quality rate-control mode, bitrate only as fallback |
| 20 fps was held in every run, finalize under 200 ms | S2 | 20 fps default is safe |
| Normal MP4 is unreadable after a crash | S1 §5.3 | Fragmented MP4 |
| Agents and MCP results cannot carry video | S1 §8.1 | Viewer returns chosen PNG frames plus a text timeline |
| Scene-cut detection misses UI changes (0.05-0.2 % of the frame) | S1 §8.2 | Block-level (8x8) change test for frame choice |
| The public tool list is full at 40 | S4 | New operations on the existing `screenshot` tool, not a new tool |
| The GIF recorder blocks a tokio worker for the whole capture | S4 (bug in `capture_gif.rs`) | Recorder runs on its own thread |
| The shared M1 capture controller feeds `observe` | S3, S4 | Recorder uses its own `CaptureHandle`; never switches the shared one |
| `SYNAPSE_CAPTURE_FORCE_DXGI` makes window capture impossible | S4 | Pin the WGC backend |
| Capture channel holds 2 frames, drops the oldest | S4 | Drain continuously; report dropped frames |
| Privacy pause and app exclusions gate every recorder | S4 (`TimelineControl::suppress_reason`) | Recorder refuses or stops when paused or excluded |
| Raw media belongs on disk, not in RocksDB; no file retention exists yet | S4 | Clip folders under `%LOCALAPPDATA%\synapse\clips\`; retention is later work |
| No tool returns images today; Claude Code caps a tool result near 25,000 tokens | S4, S1 §8.4 | Viewer pages at most 8 images per call, PNG, file path with each |

## Part 1: Recorder (`screenshot operation=record`)

**Parameters**: `window_hwnd` or `monitor_index` (else the session window target, resolved like `screenshot capture`); `duration_ms` 100..=120000; `fps` 1..=30, default 20; `folder` optional (default `%LOCALAPPDATA%\synapse\clips\<clip_id>`, must not exist).

**Pipeline** (one dedicated thread):

1. Check privacy (`suppress_reason`) and disk pressure (refuse at levels 3/4).
2. Start an own `spawn_capture_loop` handle: target window or monitor, backend pinned to WGC, `min_update_interval_ms = 1000/fps`, `dirty_region_only = false`, cursor on.
3. Ticker at 1000/fps ms: take the newest frame (drain the channel), convert with `captured_frame_region_to_bgra_bitmap`, encode; when nothing new arrived repeat the last frame and mark it repeated.
4. Encoder: own Media Foundation Sink Writer, H.264, NV12 or RGB32 input, keyframe every 1 s, quality rate-control mode (fallback 6 Mbps), hardware transforms requested and the actual encoder recorded, fragmented MP4 (fallback normal MP4, recorded).
5. Size fixed at clip start (even dimensions); a resized window is scaled with letterboxing. A minimized window: keep repeating, mark the interval. Never restore or focus a window.
6. Every tick re-check privacy; stop early with the reason recorded.
7. Write into a temp folder, rename into place at the end; remove the temp folder on error; always stop the capture handle.

**Clip folder**: `clip.mp4`; `frames.jsonl` (frame number, time ms, new or repeated, keyframe); `clip.json` (schema version, clip id, Synapse version, monotonic start plus wall-clock anchor, target details, encode details incl. encoder actually used and hardware flag, health: frames new / repeated / dropped by the capture channel / achieved fps / minimized intervals / resizes / early-stop reason).

**Response**: folder and file paths, `clip.mp4` size and SHA-256, duration, size, frame counts, encoder, hardware flag, container.

**Errors**: `CAPTURE_TARGET_INVALID`, `CAPTURE_TARGET_LOST` (window closed), `CAPTURE_GRAPHICS_API_UNSUPPORTED`, `CAPTURE_THREAD_FAILED`; each with operation, source of truth, target id and remediation. All-black frames (hidden desktop) are an error, never a silent PrintWindow fallback.

**Wiring**: `ScreenshotOperation` / `ScreenshotParams` (`m1.rs`), params and response structs next to the GIF ones, facade match and per-operation validator (`server/m1_tools/mod.rs`), `FACADE_TOOL_CONTRACTS` entry with the readback (path + size + SHA-256), same profile and permission as `gif`, tool description, README and `docs/SYNAPSE_40_TOOL_SURFACE_MIGRATION.md` operation tables.

**Files**: `server/capture_clip.rs` (recorder), `server/clip_encoder.rs` (Media Foundation encoder; one type owns it).

## Part 2: Viewer (operations on `screenshot`)

Built after the recorder, on the clip folder format above.

| Operation | Returns |
|---|---|
| `clip_overview` | Text timeline (changes, repeats skipped, gaps stated), then up to 8 labelled key frames per call with paging |
| `clip_frames` | Frames in a time range: at most 8 images, at most 15 fps |
| `clip_frame` | One frame at native pixels, optional crop (at least 200 px short side) and 1-2x scale |
| `clip_diff` | Changed regions between two times as text, plus one before/after crop |

**Decoding**: Media Foundation Source Reader (built into Windows), RGB32 output, sample timestamps; `frames.jsonl` gives new/repeated flags without decoding.

**Frame choice** (S1 §8.2): block-level 8x8 change test with a small mask around the cursor; settled after 200 ms without change; changes that revert within 500 ms flagged as short-lived; pick last-before and first-settled-after each change, the start of each short-lived change, one frame every 10 s as a floor, first and last frames.

**Every image**: PNG; label `Image k (t=mm:ss.mmm, frame n, reason)` in the text and drawn on the exported copy (the clip stays clean); file path returned with each image.

**Not in this part**: `clip_find`, `clip_transients`, events (clicks, keys, logs) joined into the clip, local captions.

## Later (not in this build)

Rolling buffer with triggers (hotkey, agent call, error signals); events in the clip folder; clip retention by count, age and total size; yellow border removal (packaged identity); optional local vision captions.

## Proof

Per item: `cargo fmt --all` and `cargo clippy -p synapse-mcp --all-targets -- -D warnings`. After deploy: one real recording of a window and one viewer call through the MCP client, with the clip folder read back. Codex (gpt-6.1-sol, high) reviews each part.
