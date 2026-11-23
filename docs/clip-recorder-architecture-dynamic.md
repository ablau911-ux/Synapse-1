# Dynamic clip recorder: architecture (way DYN1)

Created 2026-11-23 11:33 IST. Branch `feature/clip-capture`. Architect: Claude Fable 5.2. Builder: Codex (GPT-6 astra).

This document is the building spec for section 12 of `docs/clip-recorder-plan-4.2.md` (W6 to W9, plus what W1 to W5 become). The builder implements exactly what is written here and reports every deviation. Where this document and plan 4.2 differ, section 0.2 names the difference and the reason.

Line numbers are 1-based and were read on 2026-11-23 at commit `1c4c15aa`. If a file has moved on, use the symbol name.

## 0. Starting point and decisions

### 0.1 What the code is today

Plan 4.2 W1 to W5 are not built. The code is still the 4.1 design:

- The recorder writes one whole zstd frame per new picture into `waiting/` (`capture_clip.rs:1188-1195`, `clip_folder.rs:87-135`).
- A worker process encodes H.264 segments from those files (`clip_worker.rs:286`, `clip_encoder.rs:110-121`). The recorder queues the clip for it (`capture_clip.rs:668`, `capture_clip.rs:1327`).
- The viewer decodes `segment-NNNN.mp4` files (`clip_viewer.rs:2005-2027`, `clip_decoder.rs:53`).

The dynamic design replaces the waiting files, the worker and the whole-window H.264 segments. It keeps capture, the instant loop, `recorder.lock`, `clip.json` handling, the cursor rule and D5.

### 0.2 Decisions made in this document

Each one is needed to make the owner's requirements buildable. D1 to D4 go beyond the text of plan 4.2 section 12 and need the owner's confirmation (section 9).

| # | Decision | Why |
|---|---|---|
| D1 | The moving rule has a cost term. An area becomes MOVING only when it keeps changing **and** the exact coder is expensive on it. | The corpus scroll scene scrolls on every instant for 20 s (`scripts/clip-equation/scroll-page.html:21`, `:55`). D5 item 3 alone would call it animation. W6 requires "scroll gives none". zstd stores scroll at 9.4 MB/min, exact (`results/test-1080-bigwindow-summary.txt:2`). |
| D2 | The recorder's change test uses δ = 0 (any byte differs). The viewer keeps δ_lo = 32 for events. Same equation, two values of δ. | With δ = 32, quiet parts of a video are not "changed", stay EXACT and are stored losslessly on every frame. The recorder decides cost, so every changed byte counts. |
| D3 | One moving area at a time: the bounding box of the largest connected group. Other moving groups stay EXACT. | One encoder bounds RAM. A union box would store static text between two videos lossily. "Quality before bytes" (`docs/clip-recorder-plan.md:31`). |
| D4 | σ = 1. The clip stores the window's own pixels. `MIN_LINES` and pixel duplication go. | The exact layer is lossless. Duplicating pixels adds bytes and no information. The viewer already has a ×2 close-up (`clip_viewer.rs:72`). |
| D5 | H.264 is encoded live on its own thread. There is no worker process and no `encoding` state. | Plan 4.2 section 2: "no worker". Deferred encoding would need lossless video frames on disk (about 99 MB/s at 1080p, `test-1080-bigwindow-summary.txt:2`). |
| D6 | Inside a moving interval every instant gives exactly one H.264 sample. Sample n shows instant `first_instant + n`. | The mapping is an equation. No gaps exist, so bug #296 cannot occur (section 5.4). |
| D7 | At ν = 0 the block test is one line: DIFF ⇔ some pixel differs by more than δ. `DELTA_HI`, `N_HI`, `N_LO`, `NOISE_*` and "undecided" drop out. | Plan 4.2 section 5 sets N_hi = N_lo = 1 and ν = 0. Then n_δhi ≥ 1 implies n_δlo ≥ 1, so only δ_lo decides. Same behaviour, fewer constants. |

## 1. Data flow

### 1.1 Picture

    WGC capture thread          recorder thread                           moving thread
    (ClipObserver, kept)        "synapse-clip-recorder" (kept)            "synapse-clip-moving" (new)

    frame arrives               every instant k (1/f = 50 ms):
      GPU copy into a slot  →     frame identity for s_k
                                  repeat?  → ClipLayers::repeat(k)
                                  new?     → privacy gate, read back (BGRA)
                                             ClipLayers::picture(k, bgra)
                                               1 SourcePictures: BGRA → B,G,R; keep previous
                                               2 ChangeMap: which 8×8 blocks changed
                                               3 MovingAreas: M(k), the moving box or none
                                               4 MovingCoder::follow: crop M(k) ────jobs────→  B,G,R box → NV12
                                               5 ExactStream::write: changed rows                H.264 Quick Sync q90
                                                 outside M(k) → zstd stream                     moving/seg-NNNN.mp4.part
                                               6 MovingAreas::exact_written (cost)     ←─committed segments, spare buffers
                                  one line in frames.jsonl, flushed
                                  clip.json when a segment is committed

    clip folder  →  viewer (blocking thread per call): ExactReader + ClipDecoder → ClipPictures → D5, images

### 1.2 Threads

| Thread | Exists today | Does | Never does |
|---|---|---|---|
| Capture thread (WGC) | yes, inside `ClipObserver` | Copies each delivered frame into a GPU slot, by composition time | Map, read back, wait |
| Recorder thread `synapse-clip-recorder` | yes (`capture_clip.rs:248`) | The instant loop, privacy gate, read back, steps 1 to 6 above, `frames.jsonl`, `clip.json` | Media Foundation calls |
| Moving thread `synapse-clip-moving` | new | Owns Media Foundation for its life. Creates one encoder per moving interval, converts boxes to NV12, writes samples, finalizes and verifies segments | Touch `clip.json` or `frames.jsonl` |
| Viewer call | yes (`clip_viewer.rs:137`, `spawn_blocking`) | Reads the folder, decodes, composites | Write clip files other than `views/` |

The recorder thread is started as today (`capture_clip.rs:236-263`) and runs `live::record` (`capture_clip.rs:653-678`). The moving thread is started by `ClipLayers::start` and joined by `ClipLayers::finish`.

Media Foundation must stay on the thread that created it (`clip_encoder.rs:19-20`). That is why the encoder has its own thread: creating an encoder can take longer than one instant, and the instant loop must not wait for it. The observer tolerates the reader being only 2 instants late (`clip_observer.rs:69`).

### 1.3 Queues

| Queue | From → to | Carries | Bound | When full |
|---|---|---|---|---|
| `jobs` (`std::sync::mpsc::channel`) | recorder → moving | `MovingJob::{Begin, Frame, Repeat, End}` | `Frame` jobs are counted in `queued: Arc<AtomicUsize>`. At most `MOVING_QUEUE_FRAMES` = 8 `Frame` jobs wait. `Begin`, `Repeat`, `End` carry no pixels and are always sent | The recorder sends `Repeat` instead of `Frame`. The instant is named lost with cause `moving_behind` |
| `spare` (`mpsc::channel`) | moving → recorder | Used box buffers (`Vec<u8>`) to fill again | At most `MOVING_QUEUE_FRAMES` + 1 buffers exist | The recorder allocates a new buffer only when none is spare and fewer than the bound exist |
| `committed` (`mpsc::channel`) | moving → recorder | One `MovingSegment` per finished segment | One per segment | Not bounded; a segment lasts up to 60 s |

There is no queue between the capture thread and the recorder thread. The GPU slots are that queue (`clip_observer.rs:73`, 5 slots).

### 1.4 Buffer ownership

| Buffer | Owner | Lifetime |
|---|---|---|
| GPU slots and staging texture | `ClipObserver` | The recording |
| Read-back picture, B,G,R,A (`ObservedPixels.bgra`, `clip_observer.rs:169-173`) | Recorder thread, local to one `handle_instant` | One instant. Dropped after step 1 |
| `current` and `previous` source pictures, B,G,R | `SourcePictures` | The recording; swapped on each new picture |
| Exact picture E, B,G,R | `ExactStream` | The recording |
| zstd compression context, payload and output buffers | `ExactStream` | The recording |
| Block windows (one `MotionWindow` per block) | `MovingAreas` | The recording |
| Box buffers, B,G,R of the moving box | Created by `MovingCoder` on the recorder thread, moved to the moving thread inside `Frame`, returned through `spare` | Recycled |
| NV12 box | `Nv12Box`, on the moving thread | One moving interval |
| Encoder (Sink Writer) | `ClipEncoder`, on the moving thread | One moving interval |

No buffer is shared between threads. A box buffer has one owner at any time.

### 1.5 RAM budget at 1920 × 1080

px = 2,073,600. One B,G,R picture = 6.2 MB. One B,G,R,A picture = 8.3 MB. RAM is counted as the plan's rule M3: OS peak of private bytes minus M_0, the value just before `record` (`docs/clip-recorder-experiments-plan.md:176`).

| Component | Owner | MB | Source |
|---|---|---|---|
| Frame pool buffer, 5 GPU slots, staging texture (7 × 8.3) | `ClipObserver` | 58 | `clip_observer.rs:73`; on Intel graphics GPU memory is system memory, so it is counted |
| Read-back picture | recorder thread | 8.3 | `clip_observer.rs:169-173` |
| `current` + `previous` | `SourcePictures` | 12.4 | 2 × 6.2 |
| Exact picture E | `ExactStream` | 6.2 | |
| zstd stream context, level 1, window 2^27 | `ExactStream` | 141 | measured for the whole run: `results/test-1080-bigwindow-summary.txt:2` |
| Payload buffer + compressed output buffer | `ExactStream` | 12.5 | one picture + its compress bound |
| Block windows, change map, targets | `MovingAreas` | 3 | 32,400 blocks |
| Box buffers, worst case box = whole window (8 + 1) × 6.2 | `MovingCoder` | 56 | `MOVING_QUEUE_FRAMES` |
| NV12 box, worst case | `Nv12Box` | 3.1 | px × 1.5 |
| Media Foundation + Quick Sync encoder | `ClipEncoder` | 172 | measured for the whole H.264 worker at full 1080p: `results/test-1080-summary.txt:11` |
| `frames.jsonl` writer, `clip.json`, lost list | recorder | 1 | |
| **Sum** | | **473.5** | ≤ 700 (owner raised the limit from 500 to 700 MB on 2026-11-23) |

Both measured figures (141 and 172) are whole-process figures that already contain frame buffers, so the sum counts some bytes twice. It is an upper bound.

The viewer is not part of this budget. It needs about 135 MB for the zstd window plus one picture and the H.264 decoder. Viewing while a 1080p recording runs can pass 500 MB in the same process (risk R9).

### 1.6 CPU rule

The 10 % rule stays (`docs/clip-recorder-plan.md:29`, A9 at `:150`). `LiveMeter` measures the recorder thread plus the capture thread today (`capture_clip.rs:726-729`). It now adds the moving thread's processor time. When the live part alone reaches the limit within a second, read-backs are skipped until the second ends and those instants are named lost (`capture_clip.rs:731-738`, `capture_clip.rs:1100-1102`). Nothing else changes in that rule.

The rule's owner moves. `CPU_LIMIT_PERCENT`, `share_percent`, `logical_processors` and `live_alone_reached` live in `clip_worker.rs:27`, `:123`, `:129`, `:117` today. That file is deleted. They move into `LiveMeter` in `capture_clip.rs`. The worker cap (`ProcessorBudget::set_live_share`, `clip_worker.rs:77`) is deleted with the worker.

## 2. Types

Code rules: one type owns one rule. No pass-through layers. Named constants only. No `#[cfg(test)]` code (upstream policy, `docs/clip-recorder-plan.md:32`). All files below are in `crates/synapse-mcp/src/server/`.

`clip_motion.rs`, `clip_exact.rs`, `clip_encoder.rs`, `clip_decoder.rs`, `clip_moving.rs` and `clip_layers.rs` must compile with only `std`, `serde`, `serde_json`, `zstd`, `windows` and each other (`super::clip_…`). They must not use `clip_folder.rs`, `capture_clip.rs`, `m1` or `rmcp`. Reason: the probe includes these files directly with `#[path]` (section 7), so the checks run the product code, not a copy.

### 2.1 New file `clip_motion.rs`: the moving-area rule

| Type | The one rule it owns | Fields | Public methods |
|---|---|---|---|
| `PixelRect` | A rectangle in picture pixels. Serialised as `[x, y, w, h]` | `x, y, w, h: u32` | `right()`, `bottom()`, `area()`, `contains(&PixelRect) -> bool`, `union(&PixelRect) -> PixelRect`, `intersect(&PixelRect) -> Option<PixelRect>`, `aligned(step: u32, width: u32, height: u32) -> PixelRect` |
| `BlockGrid` | The 8 × 8 block layout of a picture, and the block test. This is `Grid`, moved from `clip_viewer.rs:455-519` | `width, height, cols, rows: usize` | `new(width, height)`, `blocks()`, `place(b)`, `span(b)`, `rows_of(b)`, `near(a, b, gap)`, `percent(count)` as today; `bbox(blocks) -> Option<PixelRect>`; new `differs(a: &[u8], b: &[u8], block: usize, delta: u8) -> bool` (section 3.1); new `groups(flags: &[bool]) -> Vec<Vec<usize>>`, the 8-connected groups, body taken from `BlockMap::regions` (`clip_viewer.rs:626-661`) |
| `MotionTiming` | D5's times in instants at f | `activity: u64` (W_a), `animating: usize` (A), `hold: u64` (H) | `of(fps: u32) -> Self` (the arithmetic of `Instants::of`, `clip_viewer.rs:673-684`) |
| `MotionWindow` | "A block animates when it changed in at least A of the last W_a instants" (D5 item 3). This is the `moves` field and `animating_until` of `BlockState` (`clip_viewer.rs:777`, `:791-796`), made one shared type | `moves: VecDeque<u64>` (at most A instants) | `moved(&mut self, instant, &MotionTiming)`, `animating_until(&self, &MotionTiming) -> u64`, `animating(&self, instant, &MotionTiming) -> bool`, `first_move(&self) -> Option<u64>` |
| `ChangeMap` | Which blocks differ between two source pictures | `changed: Vec<bool>`, `count: usize` | `none(&BlockGrid) -> Self`, `between(previous: &[u8], current: &[u8], &BlockGrid, delta: u8) -> Self`, `is_changed(b) -> bool`, `count_outside(&BlockGrid, Option<PixelRect>) -> u64` |
| `CostWindow` (private) | "The exact coder is expensive here": compressed bytes per changed block byte over the last W_a instants (section 3.3) | `records: VecDeque<(u64, u64, u64)>` (instant, bytes, changed blocks) | `add(instant, bytes, blocks)`, `costly(instant, &MotionTiming) -> bool` |
| `MovingAreas` | M(k): the one moving box at instant k (section 3) | `grid: BlockGrid`, `timing: MotionTiming`, `windows: Vec<MotionWindow>`, `targets: VecDeque<(u64, PixelRect)>`, `cost: CostWindow`, `current: Option<PixelRect>`, `enabled: bool`, `fits_encoder: fn(u32, u32) -> bool` | `new(width, height, fps, fits_encoder) -> Self`, `step(&mut self, instant, changes: Option<&ChangeMap>) -> Option<PixelRect>`, `exact_written(&mut self, instant, bytes: u64, changed_blocks: u64)`, `disable(&mut self)` |

`ChangeMap::between` must be cheap. Compare each pixel row as one slice first. Only for rows that differ, compare the 24-byte slice of each block column. Unchanged rows cost one `memcmp`.

### 2.2 New file `clip_exact.rs`: the exact layer

| Type | The one rule it owns | Fields | Public methods |
|---|---|---|---|
| `ExactRef` | Where one exact record is. Serialised as `[group, offset, bytes, raw]` | `group: u32`, `offset: u64`, `bytes: u32`, `raw: u32` | `is_key(&self) -> bool` (`offset == 0`) |
| `ExactStream` | The exact layer's write format (section 4.2): E follows the source outside M; only changed rows are coded; one zstd frame per group of 20 records | `dir: PathBuf`, `width, height: u32`, `picture: Vec<u8>` (E), `cctx: zstd_safe::CCtx<'static>`, `file: Option<File>`, `group: u32`, `records_in_group: u32`, `offset: u64`, `payload: Vec<u8>`, `out: Vec<u8>`, `summary: ExactSummary` | `create(folder: &Path, width, height) -> Result<Self, String>`, `write(&mut self, source: &[u8], moving: Option<PixelRect>) -> Result<Option<ExactRef>, String>`, `resize(&mut self, width, height) -> Result<(), String>`, `finish(&mut self) -> Result<ExactSummary, String>` |
| `ExactSummary` | Totals of the exact layer for `clip.json` | `groups: u32`, `records: u64`, `bytes: u64` | fields only |
| `ExactEntry` | One index row for the reader | `instant: u64`, `exact: ExactRef`, `width, height: u32` | fields only |
| `ExactReader` | The exact layer's read side: E at any instant, decoding at most one group | `dir: PathBuf`, `entries: Vec<ExactEntry>`, `dctx: zstd_safe::DCtx<'static>`, `picture: Vec<u8>`, `size: (u32, u32)`, `position: Option<usize>` (index of the last entry applied), `compressed: Vec<u8>`, `payload: Vec<u8>` | `open(folder: &Path, entries: Vec<ExactEntry>) -> Result<Self, String>`, `picture_at(&mut self, instant: u64) -> Result<(&[u8], u32, u32), String>` |

Writer and reader are in one file because they share one format. A change to the format changes both.

### 2.3 Changed file `clip_encoder.rs`

| Type | Kept, changed or new | The rule it owns | Notes |
|---|---|---|---|
| `ClipEncoder` | changed | One H.264 file through the Sink Writer, hardware only, quality mode at q, settings read back | `create(path, width, height, fps) -> Result<Self, String>` (no `hardware` argument; always on). `grant(&self) -> &EncoderGrant`. `write_nv12(&mut self, nv12, time_hns, duration_hns)` as today (`clip_encoder.rs:404-432`). `finish(self) -> Result<(), String>`. New `accepts(width: u32, height: u32) -> bool`: the bound of `clip_encoder.rs:59-61`. It no longer owns a `MediaFoundationRuntime`; the moving thread holds one for its life |
| `EncoderGrant` | new (replaces `EncoderReadback`, `clip_encoder.rs:333-337`, and the `QualityReport` it carried) | What Media Foundation granted | `name: String`, `hardware: bool`, `quality_level: u32`, `gop_frames: u32`, `b_frames: u32`, `screen_content_hint: bool`. Derives `Serialize`, `Deserialize`, `Clone` |
| `Nv12Box` | new (replaces `Nv12Canvas`, `clip_encoder.rs:251-330`) | One B,G,R box as NV12: BT.709 limited range, chroma averaged over 2 × 2, odd sides padded by repeating the last column or row | `new(box_width: u32, box_height: u32) -> Self`, `draw(&mut self, bgr: &[u8])`, `bytes(&self) -> &[u8]`, `coded_size(&self) -> (u32, u32)` |
| `MediaFoundationRuntime` | kept (`clip_encoder.rs:797-830`) | COM and Media Foundation for one thread | unchanged |
| `open_writer`, `video_type`, `encoder_params`, `set_and_read`, `codec_api`, `encoder_identity`, `hr`, `even_up` | kept | | `open_writer` always sets hardware transforms on (`clip_encoder.rs:480-487`) |
| `confirm_settings` | changed | Read-back of GOP, B-frames, hint, quality | The bitrate fallback (`clip_encoder.rs:649-666`) is deleted. Quality mode refused → `Err` |
| `SegmentSample`, `SegmentInput`, `EncodedSegment`, `encode_segment`, `encode_once`, `RecordedSize`, `even_down`, `Nv12Canvas`, `EncoderReadback` | deleted | | The worker's inputs and the σ scaling |

Constants: `H264_QUALITY_LEVEL` 70 → 90 (`clip_encoder.rs:50`). `GOP_FRAMES` stays 20 (`clip_encoder.rs:57`); it now takes `fps` from `ClipEncoder::create` instead of `capture_clip::RECORD_FPS`. `FALLBACK_BITS_PER_PIXEL_FRAME` (`clip_encoder.rs:52`) is renamed `NOMINAL_BITS_PER_PIXEL_FRAME` and becomes private: it only fills `MF_MT_AVG_BITRATE`, which the output type needs (`clip_encoder.rs:495-497`). `MAX_ENCODE_MACROBLOCKS`, `MAX_ENCODE_SIDE` stay.

### 2.4 Changed file `clip_decoder.rs`

| Type | Kept, changed or new | The rule it owns | Notes |
|---|---|---|---|
| `SampleTable` | new, from `keyframe_times_hns` (`clip_decoder.rs:243-277`) | What an MP4 file holds: its samples in order, each with its file time and keyframe flag | `read(path: &Path) -> Result<Self, String>`, `len() -> u32`, `key_frames() -> Vec<KeyFrame>` |
| `KeyFrame` | new | One keyframe of a segment. Serialised as `[frame, t_hns]` | `frame: u32`, `t_hns: i64` |
| `ClipDecoder` | changed | Frame number n of one segment, decoded from the keyframe at or before it, counted in decode order | `open(path)` kept. New `frame_number(&mut self, frame: u32, keys: &[KeyFrame]) -> Result<DecodedFrame, String>` (section 6.2). `frame_at` and `FORWARD_READ_LIMIT_FRAMES` are deleted (`clip_decoder.rs:129-161`, `:31`). `next_frame`, `seek`, `copy_pixels` become private. Field `last` becomes `last: Option<(u32, DecodedFrame)>` (frame number and picture) |
| `DecodedFrame` | kept (`clip_decoder.rs:37-45`) | One decoded H.264 frame, top-down B,G,R,A | `t_ms()` is deleted |
| `keyframe_times_hns` | deleted | | Replaced by `SampleTable` |

### 2.5 New file `clip_moving.rs`: the moving layer

| Type | The one rule it owns | Fields | Public methods |
|---|---|---|---|
| `MovingRef` | Which H.264 frame an instant shows. Serialised as `[segment, frame]` | `segment: u32`, `frame: u32` | fields only |
| `MovingSegment` | "A moving segment is what its file holds": the rectangle, the instants and the verified sample table | `index: u32`, `file: String`, `first_instant: u64`, `last_instant: u64`, `rect: PixelRect`, `coded_width, coded_height: u32`, `frames: u32`, `key_frames: Vec<KeyFrame>`, `complete: bool` | `from_file(folder: &Path, index: u32, rect: PixelRect, first_instant: u64, expected_frames: u32) -> Result<Option<Self>, String>`. It reads the `.part` or `.mp4` with `SampleTable::read`, renames `.part` to `.mp4`, and returns `None` when no sample is readable. Used by the moving thread and by recovery |
| `MovingAvailability` | Whether hardware H.264 at quality 90 works in this service process | process-wide `static` holding `Untried`, `Available` or `Unavailable(reason)` | `get() -> Self`, `set_available()`, `set_unavailable(reason: String)` |
| `MovingCoder` | One encoder per moving interval; one sample per instant; a bounded queue; a segment is at most 60 s | `jobs: Sender<MovingJob>`, `spare: Receiver<Vec<u8>>`, `committed: Receiver<MovingSegment>`, `queued: Arc<AtomicUsize>`, `cpu_hns: Arc<AtomicI64>`, `buffers: usize`, `open: Option<OpenSegment>`, `next_index: u32`, `fps: u32`, `thread: Option<JoinHandle<MovingSummary>>` | `start(folder: &Path, fps: u32) -> Result<Self, String>`, `follow(&mut self, instant: u64, rect: Option<PixelRect>, source: &[u8], source_width: u32, new_picture: bool) -> MovingStep`, `take_committed(&mut self) -> Vec<MovingSegment>`, `cpu_hns(&self) -> i64`, `finish(self) -> MovingSummary` |
| `MovingStep` | What `follow` did at one instant | `reference: Option<MovingRef>`, `began: Option<PixelRect>`, `behind: bool` | fields only |
| `MovingSummary` | The moving layer's totals at the end | `committed: Vec<MovingSegment>` (those not yet taken), `short: Vec<ShortSegment>` (`segment`, `first_instant`, `expected`, `held`), `grant: Option<EncoderGrant>`, `unavailable: Option<String>` | fields only |
| `MovingJob`, `OpenSegment`, `MovingWorker` (private) | The thread protocol and the thread's state | section 5.6 | |

### 2.6 New file `clip_layers.rs`: the split

| Type | The one rule it owns | Fields | Public methods |
|---|---|---|---|
| `SourcePictures` | The current and the previous source picture, as tightly packed B,G,R (alpha dropped, as A8 does today, `clip_folder.rs:113-118`) | `width, height: u32`, `current: Vec<u8>`, `previous: Vec<u8>`, `has_previous: bool` | `accept(&mut self, bgra: &[u8], width, height) -> bool` (returns `true` when the size changed), `current(&self) -> &[u8]`, `previous(&self) -> Option<&[u8]>` |
| `ClipLayers` | "Every pixel of every instant is stored by exactly one layer": the order rule → moving → exact, on each instant | `source: SourcePictures`, `grid: BlockGrid`, `areas: MovingAreas`, `exact: ExactStream`, `moving: MovingCoder` | `start(folder: &Path, width, height, fps) -> Result<Self, String>`, `picture(&mut self, instant, bgra: &[u8], width, height) -> Result<Written, String>`, `repeat(&mut self, instant) -> Result<Written, String>`, `cpu_hns(&self) -> i64`, `finish(self) -> Result<LayersSummary, String>` |
| `Written` | What one instant wrote, for its `frames.jsonl` line | `exact: Option<ExactRef>`, `moving: Option<MovingRef>`, `moving_rect: Option<PixelRect>` (only on the first instant of a segment), `moving_behind: bool`, `committed: Vec<MovingSegment>`, `bytes: u64` | fields only |
| `LayersSummary` | Totals for `clip.json` | `exact: ExactSummary`, `moving: MovingSummary` | fields only |

`ClipLayers::picture`, in this order:

1. `resized = source.accept(bgra, width, height)`. On a resize: `exact.resize`, a new `BlockGrid`, a new `MovingAreas`, and `moving.follow(instant, None, …)` to end an open interval.
2. `changes = ChangeMap::between(previous, current, &grid, MOVING_DELTA)`. For the first picture of a size: `ChangeMap::none`.
3. `rect = areas.step(instant, Some(&changes))`. When `MovingAvailability::get()` is `Unavailable`: call `areas.disable()` once; `step` then always returns `None`.
4. `step = moving.follow(instant, rect, source.current(), width, true)`.
5. `record = exact.write(source.current(), rect)`.
6. When `record` is `Some`, is not a key record, and `rect` equals the box of the instant before: `areas.exact_written(instant, record.bytes, changes.count_outside(&grid, rect))`.
7. Return `Written` with `committed = moving.take_committed()`.

`ClipLayers::repeat` (an instant with no new picture: a repeat, a lost instant, a minimized window): steps 3, 4 (with `new_picture = false`), 5 and 7. Step 5 writes a record only when M changed, because only then E changes.

### 2.7 New file `clip_recovery.rs`

| Type | The one rule it owns | Public |
|---|---|---|
| `ClipRecovery` | B10 for DYN1: a clip whose recorder died becomes `ready`, from what its files hold (section 4.6) | `recover_clips_on_start()` (moved from `clip_worker.rs:653-683`), `ClipRecovery::run(folder: &Path) -> Result<(), String>` |

### 2.8 Changed file `clip_folder.rs`

Kept unchanged: `clips_root` (`:48`), `ClipClock`, `ClipTargetInfo`, `ObserverGrant`, `ConditionStatus`, `LostInstant`, `ClipJson::read`, `create`, `update`, `add_lost` (`:627-691`), `FolderLock`, `write_atomically` (`:730-748`), `retry_brief_hold` (`:755-776`), `RecorderLock` (`:781-816`), `file_size_and_sha256` (`:823`), `ClipRegistry` (`:858`), `read_frame_lines` (`:307-317`).

Changed:

- `CLIP_SCHEMA_VERSION` 2 → 3 (`:16`). `ClipJson::read` already refuses other versions (`:633-639`), so older clips are refused with a clear message.
- `FrameLine` (`:285`) gets three optional fields (section 4.4) and one method: `shows_new_picture(&self) -> bool` = `new || exact.is_some()`.
- `ClipJson` (`:599`) loses `segments` and gets `way`, `exact`, `moving` (section 4.5).
- `ClipState` (`:218`) loses `Encoding`.
- `LossCause` (`:270`) loses `WaitingUnreadable` and `WaitingMissing` and gets `MovingBehind` and `MovingFailed`.
- `ClipGeometry` (`:345`): `scale` is always 1; `recorded_*` equal `window_*`.
- `Conditions` (`:428`): `merge_b`, `BReport`, `BStatus` (`:442-587`) are deleted. B is written once, by the recorder (section 4.5).

New: `CommittedSegment` (the `clip.json` entry of a moving segment: the `MovingSegment` flattened, plus `bytes` and `sha256` read back from the file; `CommittedSegment::read_back(folder: &Path, segment: MovingSegment) -> Result<Self, String>`), `ExactInfo`, `MovingInfo` (section 4.5).

Deleted: `WAITING_DIR`, `WAITING_FRAME_EXTENSION`, `WAITING_ZSTD_LEVEL`, `WAITING_BYTES_PER_PIXEL`, `PART_SUFFIX` use for segments, `segment_file_name`, `segment_part_name`, `is_part_file`, `waiting_frame_path`, `is_waiting_frame`, `WaitingFrameWriter`, `WaitingReadError`, `WaitingFrameReader`, `SegmentCheckpoint`, `QualityReport`.

### 2.9 Changed file `capture_clip.rs`

Kept: `RecordingControl`, `Recorders`, `ClipPlan`, `capture_clip`, `clip_status`, `clip_stop`, `clip_extend`, `run_recorder_thread`, `capture_clip_plan`, `with_clip_context`, the error helpers, and in `live`: `record`, `fail_start`, `Machine`, `Recording::start` up to the observer start (`:869-924`), `run` (`:1028-1060`), `stop_condition` (`:1062-1085`), the line builders and the cursor rule (`:1131-1178`), `write_line` (`:1180-1186`), `wait_for_first_frame`, `sleep_until`.

Changed:

- `Recording` (`:848-866`): field `waiting: WaitingFrameWriter` is replaced by `layers: ClipLayers`. `ClipLayers::start` is called in `write_frame_zero`, once the first frame's size is known.
- `write_frame_zero` (`:926-986`): the privacy check and the all-black check stay. `scale` is 1. `write_waiting(0, …)` becomes `layers.picture(0, …)`.
- `handle_instant` (`:1087-1119`): a repeat calls `layers.repeat(k)`; a new frame calls `layers.picture(k, …)`. The line gets `exact`, `moving`, `moving_rect` from `Written`. `moving_behind` adds a `LostInstant` with cause `MovingBehind` and sets `line.lost`. Each `committed` segment is written to `clip.json` with `CommittedSegment::read_back` inside one `ClipJson::update`.
- `write_lost` (`:1121-1129`): also calls `layers.repeat(k)`.
- `LiveMeter` (`:703-757`): owns `CPU_LIMIT_PERCENT` and the share arithmetic (section 1.6). `cpu()` adds `layers.cpu_hns()`. `max_worker_cap_given` is deleted.
- `DiskGuard` (`:760-845`): `retained`, `retained_bytes`, `retained_pixels`, `forget_committed`, `finalize_reserve` are deleted. `wrote(bytes)` takes `Written.bytes`. The moving files are seen by the free-space measurement every `DISK_CHECK_MS` (`:831-837`).
- `finish` (`:1218-1328`): section 4.6, "End of recording".
- `status_of` (`:335-374`): `frames_waiting` is deleted; `segments` lists the moving segments.
- `RecorderClaim::drop` (`:421-435`): the `ProcessorBudget` call (`:431`) is deleted.

Deleted: `MIN_LINES` (`:50`), `scale_for_height` (`:608-610`), `stop_failed_recording` (`:176-184`), `write_waiting`, both `ClipWorkerHost::enqueue` calls (`:668`, `:1327`), the `Encoding` state.

### 2.10 Changed file `clip_viewer.rs`

Kept: all four operations, the token budgets, `Recent`, `Activity`, `BlockEpisode`, `Event`, the caret rule, the picks, the image helpers.

Changed: section 6. In short: `ViewedClip` reads the new folder; a new type `ClipPictures` composites; `Grid` moves to `clip_motion.rs` as `BlockGrid`; `BlockState` uses `MotionWindow`; `Verdict` becomes a `bool` test at ν = 0; the analyzer treats blocks inside a stored moving rectangle as animating; `ANALYSIS_VERSION` 13 → 14 (`:128`).

### 2.11 Deleted and other files

- `clip_worker.rs`: deleted whole. `recover_clips_on_start` and the loss naming of `mark_service_restart` (`:685-753`) move to `clip_recovery.rs`.
- `server.rs:249-257`: drop `mod clip_worker`; add `clip_exact`, `clip_layers`, `clip_motion`, `clip_moving`, `clip_recovery`; re-export `clip_recovery::recover_clips_on_start` only.
- `main.rs`: delete `Mode::ClipWorker` and `--clip-worker-folder` (`:113`, `:216`, `:398-404`, `:504`). The call at `:484` stays.
- `m1.rs`: `ClipStatusResponse` (`:672`) loses `frames_waiting` (`:685`) and gets `exact_bytes: u64`; `ClipSegmentInfo` (`:693`) describes a moving segment. Update the `record` contract text (`tool_profiles.rs`, named in `docs/clip-recorder-plan.md:196`).
- `desktop_worker.rs`: no change. It holds no clip code (searched for "clip": no match). The clip worker only reused two of its primitives.
- `crates/synapse-mcp/Cargo.toml:68`: the Media Foundation features stay, because the encoder stays.
- `synapse-capture`: no change. The observer is used as it is.

## 3. The moving-area rule

One equation. The recorder computes it live. The check scripts compute it again from source frames. The viewer shares its block layer (`BlockGrid`, `MotionWindow`) and reads its result (the stored rectangles) instead of computing it again (section 6.3).

### 3.1 Symbols and constants

| Symbol | Constant | Value | Source |
|---|---|---|---|
| block | `BLOCK` | 8 px, on the picture's own pixels | `clip_viewer.rs:75`; moved to `clip_motion.rs` |
| δ | `MOVING_DELTA` | 0 | decision D2 |
| W_a | `SHORT_LIVED_MS` · f / 1000 | 500 ms = 10 instants | `clip_viewer.rs:100`; moved to `clip_motion.rs` |
| share | `ANIMATING_CHANGE_SHARE` | 0.5 | `clip_viewer.rs:103`; moved to `clip_motion.rs` |
| A | ⌈W_a · share⌉ | 5 | `clip_viewer.rs:673-684` |
| N_min | `MOVING_MIN_BLOCKS` | 64 blocks | plan 4.2 O2; start value |
| s_min | `MOVING_MIN_SIDE_PX` | 64 px | start value; checked on the encoder in step 2 |
| a | `MOVING_ALIGN_PX` | 16 px | one H.264 macroblock |
| H | `MOVING_HOLD_MS` · f / 1000 | 500 ms = 10 instants | start value |
| θ | `MOVING_COST_SHARE` | 0.25 | start value; section 3.3 |
| block bytes | (computed) | 8 · 8 · 3 = 192 | |

All are named constants in `clip_motion.rs`. Plan 4.2 section 12.5 calls them start values to be confirmed by the proof.

### 3.2 The equation

S_k is the source picture at instant k (B,G,R). k⁻ is the instant of the new picture before k. Only instants with a new picture can change a block.

**1. Block test (D5 item 1 at ν = 0).**

    changed(b, k)  ⇔  some pixel of block b has  max(|ΔB|, |ΔG|, |ΔR|) > δ  between S_k and S_k⁻

With δ = 0 this is "the bytes of the block differ". This is `BlockGrid::differs`.

**2. Animating block (D5 item 3, unchanged).**

    animating(b, k)  ⇔  #{ j ∈ (k − W_a, k] : changed(b, j) }  ≥  A

In code this is exactly today's rule: keep the last A change instants of the block; the block animates while k < (oldest of them) + W_a (`clip_viewer.rs:791-796`). At f = 20: five changes within 500 ms.

**3. Cost of the exact coder (decision D1).** J(k) is the set of exact records written at instants in (k − W_a, k − 1], without key records and without records of an instant on which the box changed (M(j) ≠ M(j − 1)). Those two kinds hold whole areas, not changes, and would distort the ratio. For record j, z_j is its compressed bytes and c_j is the number of changed blocks outside the moving box at that instant.

    ρ(k) = Σ_{j ∈ J(k)} z_j  /  ( 192 · Σ_{j ∈ J(k)} c_j )
    costly(k)  ⇔  J(k) ≠ ∅  and  Σ c_j > 0  and  ρ(k) ≥ θ

**4. Admitted blocks.** A block inside the current box stays admitted while it animates. A block outside is admitted only when the exact coder is expensive.

    X(k) = { b : animating(b, k)  and  ( b lies inside M(k − 1)  or  costly(k) ) }

**5. Target.** G(k) is the largest 8-connected group of X(k) (most blocks; ties: the group with the lowest block index).

    T(k) = bounding box of G(k)   when |G(k)| ≥ N_min
    T(k) = ∅                      otherwise

**6. Box with hysteresis.**

    U(k) = bounding box of  ⋃_{j = k − H + 1}^{k} T(j)
    R(k) = U(k) aligned outward to multiples of a, clamped to the picture:
           x0 = ⌊x0 / a⌋ · a,  y0 = ⌊y0 / a⌋ · a,  x1 = min(⌈x1 / a⌉ · a, w),  y1 = min(⌈y1 / a⌉ · a, h)
    M(k) = R(k)   when both sides of R(k) ≥ s_min and the encoder bound accepts it
    M(k) = ∅      otherwise

Everything outside M(k) is the EXACT area.

### 3.3 What each part gives

- **Block grid and window:** steps 1 and 2 are D5's, on the same grid and the same W_a and share as the viewer.
- **Share threshold:** `ANIMATING_CHANGE_SHARE`. A block that changes in fewer than half of the instants never moves (a caret, typing, a 5 fps indicator).
- **Hysteresis on start:** a block needs A = 5 changes, so a box opens at the earliest on the fifth changed instant. A single change, a flash and a short burst stay EXACT. The cost term adds a second condition: it needs at least one exact record of the changing area.
- **Hysteresis on stop:** a block animates for 5 more instants after its last change (step 2). The box then holds for H − 1 = 9 more instants (step 6). A video that stalls for up to about 500 ms keeps its box: it must reach five new changes before instant L + 15.
- **Grow fast, shrink slow:** U(k) is a union over H instants. A new target enlarges the box at once. The box shrinks only when the larger target has been gone for H instants.
- **Bounding box:** steps 5 and 6. A static part inside the box (a logo in a video) is inside M. A static part outside the box (black bars above and below a film) stays EXACT and costs nothing.
- **Minimum area:** N_min blocks and s_min per side. A spinner, a playhead and a thin progress bar stay EXACT.
- **Cost term:** scrolling text changes on every instant but the zstd stream finds each row again at another offset. Measured ρ on the scroll scene is about 0.003 (7.8 KB per frame against about 2.5 MB of changed blocks, from `test-1080-bigwindow-summary.txt:2`). Video does not compress: ρ is about 0.8 (5,949 of about 7,400 MB/min raw). θ = 0.25 lies far from both.

### 3.4 Worked times (f = 20)

A video starts changing at instant F and changes on every instant. It makes its last change at instant L.

| Instant | What holds | What is stored |
|---|---|---|
| F … F + 3 | 1 to 4 changes: not animating | The area is in the exact stream (4 lossless pictures of the area) |
| F + 4 | 5 changes: animating. J = the records of F … F + 3, ρ ≥ θ. M(F + 4) opens | First H.264 sample (a keyframe). E freezes inside M |
| F + 5 … L | animating | One sample per instant |
| L + 1 … L + 5 | still animating (k < L − 4 + 10) | One sample per instant (repeats) |
| L + 6 … L + 14 | T = ∅, but U still holds T(L + 5) | One sample per instant (repeats) |
| L + 15 | M = ∅. The segment ends at L + 14 | One exact picture of the area (its changed rows) |

So: first moving instant = F + 4. Last moving instant = L + 14. First exact picture after the stop = L + 15. The checks in section 7 use these three numbers, computed from the constants.

### 3.5 Known behaviour of the rule

- **A scroll that moves a playing video.** The video's new place is outside the old box and is costly, so the box grows. All animating blocks are connected, so the box becomes the scrolled area. About 0.75 s after the scroll ends, the box shrinks to the video's new place and the text is stored exactly. During that time the text is H.264 at quality 90.
- **A scroll beside a video that does not move.** The scrolled blocks animate but are not costly. They are not admitted. The box stays on the video.
- **Two videos.** Only the larger group moves. The other stays EXACT and is large (decision D3).
- **Slow animation.** Content that changes in fewer than 10 of 20 instants per second (an 8 fps animation) is never MOVING and is stored exactly on every change (risk R3).

## 4. On-disk format

### 4.1 The clip folder

    clip.json                      state, geometry, layers, conditions; replaced atomically
    clip.json.lock                 lock for clip.json updates (kept)
    recorder.lock                  held by the live recorder (kept)
    frames.jsonl                   one line per instant
    frames/group-NNNNNN.zst        the exact layer: one zstd frame per group of 20 records
    moving/seg-NNNN.mp4            the moving layer: one H.264 file per moving interval
    moving/seg-NNNN.mp4.part       the segment being written
    views/                         images the viewer exported (kept)

`NNNNNN` is the group index, six digits from 000000. `NNNN` is the segment index, four digits from 0000. There is no `waiting/` folder and no `segment-NNNN.mp4`.

Bytes of a clip (plan 4.2 section 3):

    B = Σ frames/* + Σ moving/*.mp4 + frames.jsonl + clip.json

### 4.2 The exact layer

**The exact picture E.** E has the picture's size, B,G,R, 3 bytes per pixel, rows top to bottom. It starts as zeros. On every `write(source, moving)`:

    E[p] := source[p]   for every pixel p outside `moving`
    E[p] stays          for every pixel p inside `moving`

**Changed rows.** A row is changed when any of its bytes changed in that step. Consecutive changed rows form a band.

**Record.** When at least one row changed, one record is written. Its uncompressed payload is:

    u16 LE   band_count
    band_count × ( u16 LE first_row, u16 LE row_count )
    then, band after band: row_count × width × 3 bytes of E (after the update)

**Key record.** The first record of every group holds the whole picture: `band_count = 1`, `first_row = 0`, `row_count = height`. This holds also when only a few rows changed. So a group never needs an earlier group.

**Group.** A group is one zstd frame in one file. It holds at most `EXACT_GROUP_PICTURES` = 20 records. A new group starts with the first record, after 20 records, and after a resize.

**Compression.** One zstd compression context for the recording, with these parameters, set once:

| Parameter | Constant | Value |
|---|---|---|
| compression level | `EXACT_ZSTD_LEVEL` | 1 |
| window log | `EXACT_WINDOW_LOG` | 27 |
| long-distance matching | (fixed) | off, set explicitly (as the probe does, `scripts/clip-equation/probe/src/exp_z.rs:487`) |
| content checksum | (fixed) | on |

Each record's payload is compressed with `compress_stream2` and the directive `Flush`. The bytes that call returns are appended to the group file. They are the record: `offset` is the file length before, `bytes` the number appended, `raw` the payload length. This is the probe's stream way (`exp_z.rs:565-590`, `:877`).

**Closing a group.** After the 20th record, at a resize and at the end of the recording, `compress_stream2` is called with empty input and the directive `End`. Its few bytes (end of frame and checksum) are appended to the file. No line refers to them. The file is then a complete zstd frame; any zstd tool can decompress it to the concatenated payloads.

**Reading.** To apply record j of a group, feed exactly its `bytes` to a streaming decompression context and expect exactly `raw` bytes out (`exp_z.rs:652-670`, `:994-999`). Then copy each band into E. Records of a group must be applied in order from the group's first record. The decompression context is reset at each group start, with window log limit 27.

**Use the streaming API of `zstd::zstd_safe`** (`CCtx`, `DCtx`, `InBuffer`, `OutBuffer`, `CParameter`, `DParameter`, `EndDirective`). The product has zstd 0.13 (`crates/synapse-mcp/Cargo.toml:65`), the probe 0.14.4 (`scripts/clip-equation/probe/Cargo.toml:18`). Use only calls present in both. Check the exact names in the crate documentation before writing them. Do not use the `experimental` feature.

**Nothing changes → nothing written.** When no row of E changed, `write` returns `None`, no byte is written, and the line has no `x` field.

### 4.3 The moving layer

One file per moving interval: `moving/seg-NNNN.mp4`. H.264 High profile, 4:2:0, no B-frames, a keyframe every `GOP_FRAMES` = 20 samples, quality mode at 90, hardware encoder (section 5).

- The picture of the file is the box M, cropped. Its coded size is the box size with each odd side raised by one (section 5.2).
- Sample n has time n / f and duration 1 / f. The file has exactly one sample per instant of the interval: `frames = last_instant − first_instant + 1`.
- The rectangle and the instants are in `clip.json` (`moving.segments`) and in `frames.jsonl` (`mv`, `mv_rect`).
- A segment ends when M(k) differs from the segment's rectangle, when it reaches `MOVING_SEGMENT_MS` = 60,000 ms (then a new one starts with the same rectangle), at a resize, and at the end of the recording.

### 4.4 `frames.jsonl`

One line per instant, as today (`clip_folder.rs:285-303`). Each line is written and flushed when its instant is handled (`capture_clip.rs:1180-1186`). A torn last line is skipped by the reader (`clip_folder.rs:307-317`).

| Field | Type | Kept or new | Meaning |
|---|---|---|---|
| `instant` | integer | kept | k |
| `t_ms` | integer | kept | k · 1000 / f |
| `new` | bool | kept, meaning sharpened | `true` when a new source picture arrived at this instant (its composition time differs from the last recorded one, `capture_clip.rs:1092-1094`) |
| `width`, `height` | integer | kept | Captured size of the picture shown |
| `cursor` | `[x, y]`, optional | kept | Cursor in frame pixels (`capture_clip.rs:1154-1178`) |
| `composed_hns` | integer, optional | kept | Composition time |
| `lost` | string, optional | kept | Loss cause for this instant |
| `minimized` | bool, omitted when false | kept | The window was minimized |
| `x` | `[group, offset, bytes, raw]`, optional | new | An exact record was written at this instant. It is a key record when `offset` is 0 |
| `mv` | `[segment, frame]`, optional | new | This instant lies in a moving interval. `frame` = `instant − first_instant` of that segment |
| `mv_rect` | `[x, y, w, h]`, optional | new | Only on the first instant of a segment: its rectangle. Recovery uses it when the segment is not yet in `clip.json` |

In Rust the three new fields are `exact: Option<ExactRef>`, `moving: Option<MovingRef>`, `moving_rect: Option<PixelRect>`, renamed with serde to `x`, `mv`, `mv_rect`, each skipped when `None`. The short names keep the file small: it has 1,200 lines per minute.

Rules that follow:

- A line with `new: false` can still carry `x`. That happens when a moving box ends or shrinks on an instant without a new picture: the freed area is then stored exactly.
- A line with `lost: "moving_behind"` has its `mv`, and the exact layer of that instant is complete. Only the moving box repeats its previous picture.
- The picture can differ from the previous instant only when `new` is true or `x` is present (`FrameLine::shows_new_picture`).

### 4.5 `clip.json`

`schema_version` 3. Written atomically under the folder lock, as today (`clip_folder.rs:644-660`, `:730-748`).

| Field | Kept or new | Meaning |
|---|---|---|
| `schema_version`, `clip_id`, `synapse_version` | kept | |
| `way` | new | `"DYN1"` |
| `state` | changed | `recording`, `ready` or `failed` |
| `failure`, `stop_reason` | kept | `stop_reason` values unchanged (`clip_folder.rs:239-249`) |
| `fps`, `end_ms`, `recorded_ms` | kept | |
| `target`, `clock`, `observer` | kept | |
| `geometry` | changed | `window_width`, `window_height` of frame zero; `scale` = 1; `recorded_width`, `recorded_height` = the window size. The fields stay because the `record` response and `d5-proof.mjs` read them |
| `lost` | kept | Instants with a cause. New causes: `moving_behind`, `moving_failed`. Removed: `waiting_unreadable`, `waiting_missing` |
| `exact` | new, optional until the end | `{ "level": 1, "window_log": 27, "group_pictures": 20, "groups": n, "records": n, "bytes": n }` |
| `moving` | new | `{ "status": "not_used" | "available" | "unavailable", "reason": text or absent, "encoder": EncoderGrant or absent, "segments": [ … ] }` |
| `moving.segments[i]` | new | `index`, `file` (`"moving/seg-0003.mp4"`), `bytes`, `sha256`, `first_instant`, `last_instant`, `rect` `[x, y, w, h]`, `coded_width`, `coded_height`, `frames`, `key_frames` `[[frame, t_hns], …]`, `complete` |
| `conditions` | changed | below |
| `segments` | deleted | |

`moving.segments` grows by one entry each time the moving thread commits a segment. The other new fields are written at the end of the recording or by recovery.

Conditions:

| Condition | Status | Detail |
|---|---|---|
| C | `held` when `lost` is empty, else `not_held` with the instants (`ClipJson::add_lost`, unchanged) | `max_hold_ms`, `reference_period_ms`, `hold_within_reference`, `frames_delivered`, `delivery_completeness` as today (`capture_clip.rs:1243-1253`) |
| G | `held` | `window_width`, `window_height`, `scale: 1`. The stored pixels are the window's pixels |
| Q | `exact` when there is no moving segment. `held` when every segment came from a hardware encoder in quality mode with level 90 read back | `quality_level`, `encoder`, `hardware`, `moving_segments`, `moving_instants`, and `moving_unavailable: reason` when the encoder could not be used |
| B | `held` when no second reached the limit, else `not_held` | `threads`, `max_live_share_percent`, `live_seconds_at_limit`, `limit_percent` |
| T | `held` when nothing was lost | `length_ms`, `bytes` (B of section 4.1), `instants_lost`, `stop_reason` |

Q has no `unverified` state any more. Either the pixels are exact, or the encoder confirmed quality 90.

### 4.6 Crash safety, end of recording, recovery

**Write order on each instant.** (1) Exact bytes are appended to the group file. (2) The moving job is sent. (3) The line is written and flushed. A line never refers to bytes that were not written before it.

**What a crash of the service loses.**

| Thing | After a crash | Lost |
|---|---|---|
| Exact layer | Every record with a line is in its file. The open group has no end mark; it is still read record by record | Nothing that has a line |
| `frames.jsonl` | Complete up to the last flushed line | The torn last line, if any |
| Committed moving segments | In `clip.json` | Nothing |
| The open moving segment (`.part`) | Kept as far as its file can be read | The samples the file does not hold; at most `MOVING_SEGMENT_MS` |
| `clip.json` | The last atomic version | Nothing |

"Crash" means the process ends. Lines and stream bytes are flushed to the operating system, not forced to the disk, as today.

**End of recording** (`Recording::finish`, replacing `capture_clip.rs:1218-1328`):

1. Flush `frames.jsonl`.
2. `layers.finish()`: write the end mark of the open exact group; send `End`; join the moving thread, which finalizes and verifies the open segment.
3. `observer.stop()` for the statistics, as today.
4. One `ClipJson::update`: when the state is still `recording`, set `ready` and the stop reason; set `recorded_ms`, `exact`, `moving`, the conditions; add the lost instants (the recorder's own, the observer's slot overflows as today at `:1227-1238`, and for each short segment the instants `first_instant + held … first_instant + expected − 1` with cause `moving_failed`).

There is no encode step after the stop. The clip is `ready` when step 4 returns.

A recorder thread that ends without `finish` (a panic) still marks the clip `failed` (`capture_clip.rs:421-435`). That stays.

**Recovery at service start (B10).** `recover_clips_on_start` walks `ClipRegistry::all_folders()` on its own thread and skips a folder whose `recorder.lock` is held (today `clip_worker.rs:653-683`, `clip_folder.rs:800-815`). For a clip in state `recording` it runs `ClipRecovery::run`:

1. Read the lines.
2. Exact layer: find the last line with `x` whose group file has at least `offset + bytes` bytes. When that group is open (fewer than 20 records have a line), truncate its file to this length; a complete group keeps its end mark. Lines after it that carry an `x` beyond the file are named lost with cause `service_restart` (this can only happen after a power loss). The open group has no end mark and never gets one; readers apply it record by record.
3. Moving layer: for every segment index named by an `mv` field but absent from `clip.json`: take its rectangle from `mv_rect` and its first instant from its first line; call `MovingSegment::from_file`. A readable file is renamed to `.mp4` and recorded with the frames it holds and `complete: false` when it holds fewer than its lines. Instants of the segment without a frame are named lost with cause `service_restart`. An unreadable file is deleted and all of its instants are named lost.
4. The instants after the last line up to the planned end are named lost with cause `service_restart` (the rule of `clip_worker.rs:701-711`).
5. One `ClipJson::update`: `recorded_ms` from the last line; state `ready`; stop reason `service_restart`; `exact` totals from the files; C and T created when missing (as `clip_worker.rs:717-741`) and `add_lost` with the losses the lines name first, so no instant is named twice (`clip_worker.rs:742-751`).

A clip in state `ready` or `failed` is not touched. There is no `encoding` state to resume.

For a moving interval that lost samples, the viewer shows what the exact layer holds inside the box: the last exact picture from before the interval. Those instants are in `lost`.

## 5. The Quick Sync path

### 5.1 Cropping

The recorder thread copies the box out of the current source picture: rows `y … y + h`, bytes `x · 3 … (x + w) · 3` of each row, into a box buffer of `w · h · 3` bytes. The buffer goes to the moving thread inside `MovingJob::Frame`. This is the only copy the recorder thread makes for the moving layer.

The box is always cut from the **current source picture**. When a segment begins on an instant without a new picture (the box shrank after its hold), the first sample is cut from the last source picture.

### 5.2 Even dimensions

The rule gives a box aligned to 16 px (section 3.2, step 6). So its sides are multiples of 16, except where the box is clamped to a picture whose width or height is not a multiple of 16. There a side can be odd.

    coded_width  = w rounded up to even     (`even_up`, `clip_encoder.rs:237-239`)
    coded_height = h rounded up to even

`Nv12Box` fills the extra column or row by repeating the last pixel column or row. The viewer cuts the decoded picture back to w × h. The rectangle in `clip.json` is always the true w × h; `coded_width` and `coded_height` are stored beside it.

A box is refused by the rule when a side is under 64 px or when `ClipEncoder::accepts(coded_width, coded_height)` is false (more than 36,864 macroblocks or a side over 4,096 px, `clip_encoder.rs:59-61`). The area then stays EXACT.

### 5.3 NV12 conversion

`Nv12Box::draw` uses the formulas the encoder uses today (`clip_encoder.rs:286-330`), BT.709 limited range:

    Y = ((47 R + 157 G + 16 B + 128) >> 8) + 16                      for every pixel
    U = ((−26 R − 87 G + 112 B + 128) >> 8) + 128                    for the mean R, G, B of each 2 × 2 block
    V = ((112 R − 102 G − 10 B + 128) >> 8) + 128

The media types stay BT.709, range 16 to 235, progressive, square pixels (`clip_encoder.rs:534-573`). Write the conversion as plain loops over rows. The old per-pixel closure with a column table existed for σ scaling and is not needed.

The conversion runs on the moving thread. For a `Repeat` job the NV12 box is not redrawn; the same bytes are written again.

### 5.4 Time stamps and gaps (bug #296)

What went wrong before: frames were written with their real times and repeated frames were skipped, so the stream had gaps (B5, `docs/clip-recorder-plan.md:175`). Quick Sync then wrote other times into the file than the ones given. 228 runs failed frame mapping, all of them Quick Sync on content with gaps (`docs/research/clip-video-compression-research.en.md:260`). The probe saw it as differing file times (`scripts/clip-equation/probe/src/exp_d.rs:594-603`).

The fix has two parts.

1. **No gaps are ever given to the encoder.** Inside a moving interval every instant gives one sample:

       time(n) = n · 10,000,000 / f     duration = 10,000,000 / f      (100 ns units)

   A new picture gives a `Frame`. An instant without a new picture, a lost instant, and an instant on which the queue was full give a `Repeat`. The encoder sees a constant frame rate, which is what it assumes.

2. **The viewer never trusts sample times.** The mapping is by count:

       frame number of instant k  =  k − first_instant

   After the file is finalized, the moving thread reads its sample table (`SampleTable::read`). It stores `frames` (the number of samples) and `key_frames` (the frame number and the file's own time of every keyframe). The viewer seeks to a keyframe by the file's own time and counts decoded frames from there (section 6.2). B-frames are off and read back as 0 (`clip_encoder.rs:625-631`), so decode order is display order.

   When the file holds fewer samples than instants, the segment is stored with what it holds, `complete: false`, and the missing instants are named lost (`moving_failed`).

An extra keyframe (#297) does no harm: `key_frames` comes from the file, not from the GOP setting.

### 5.5 When Quick Sync is unavailable

"Unavailable" means any of: no hardware H.264 encoder is granted (`encoder_identity` reports `hardware == false`, `clip_encoder.rs:703-760`); quality mode or level 90 is not read back (`clip_encoder.rs:634-648`); GOP or zero B-frames are not read back; creating the writer fails; writing a sample fails.

**Changed 2026-11-23:** the owner raised the RAM limit to 700 MB and approved a software fallback. Its plan comes after the R1 fix (see Builder notes). The text below is the earlier rule. There is no software fallback. The Microsoft software encoder measured about 671 MB of RAM (`docs/research/consultations-2026-11-23/brief.md:11`), which breaks the 500 MB limit, and a bitrate fallback breaks quality 90. Any hardware H.264 encoder is accepted, not only Intel's; its name and vendor are stored in `moving.encoder`.

What happens:

1. The moving thread calls `MovingAvailability::set_unavailable(reason)`. It finalizes what the open file holds with `MovingSegment::from_file` (kept when readable, deleted when not).
2. On its next instant the recorder sees `Unavailable`, calls `MovingAreas::disable()`, and `follow` ends the interval. From that instant M = ∅: the area is stored exactly, with one exact picture of the former box first.
3. The instants of that interval whose samples were not kept are named lost with cause `moving_failed`. This is bounded by the time it takes to see the failure (a few instants at the first creation).
4. `MovingAvailability` is process-wide. Later recordings in the same service process start with M disabled and lose nothing. `clip.json` says `moving.status: "unavailable"` with the reason, and Q is `exact`.

The cost of being unavailable is size and CPU on video content: lossless video is large and may pass the 10 % rule, where frames are shed and named (E1, `docs/clip-recorder-plan.md:39`). That is reported, not hidden.

### 5.6 Encoder lifetime per moving interval

The thread protocol:

    enum MovingJob {
        Begin { segment: u32, rect: PixelRect, first_instant: u64 },
        Frame { bgr: Vec<u8> },       // the box of the current source picture
        Repeat,                       // encode the last box again
        End,
    }

`MovingCoder::follow(instant, rect, source, source_width, new_picture)` on the recorder thread:

| Open segment | `rect` | Action |
|---|---|---|
| none | `None` | nothing; `reference = None` |
| none | `Some(r)` | `Begin { next_index, r, instant }`, then `Frame` (box of the current source); `began = Some(r)` |
| `s` | `Some(r)`, `r == s.rect`, age < `MOVING_SEGMENT_MS` | `Frame` when `new_picture`, else `Repeat` |
| `s` | `Some(r)`, `r != s.rect` or age reached | `End`; then as "none, `Some(r)`" |
| `s` | `None` | `End`; `reference = None` |

A `Frame` is sent only when `queued` < `MOVING_QUEUE_FRAMES`. Otherwise `Repeat` is sent and `behind = true`. `reference` is `[segment, instant − first_instant]`.

On the moving thread (`MovingWorker`):

- **Thread start:** `MediaFoundationRuntime::start()` once. It lives until the thread ends.
- **`Begin`:** create `ClipEncoder` on `moving/seg-NNNN.mp4.part` with the coded size, and an `Nv12Box`. Check the grant (hardware, quality 90, GOP, B-frames). On success with `Untried`: `set_available()`. On failure: section 5.5.
- **`Frame`:** `Nv12Box::draw`, `write_nv12(time(n), duration)`, n += 1, return the buffer through `spare`, decrease `queued`.
- **`Repeat`:** `write_nv12` with the unchanged NV12 box, n += 1.
- **`End`:** `ClipEncoder::finish()` (Finalize, `clip_encoder.rs:436-445`), then `MovingSegment::from_file(…, expected_frames = n)`, send the segment through `committed`. The encoder is dropped. The next interval gets a new one.
- **After each job:** store the thread's processor time in `cpu_hns` (`synapse_capture::current_thread_cpu_hns`, used today at `capture_clip.rs:727`).
- **Channel closed:** finalize an open segment the same way and return the `MovingSummary`.

So one encoder lives from `Begin` to `End`: one moving interval with one rectangle, at most 60 s. Nothing of the encoder is kept between intervals except Media Foundation itself.

The container stays as today: fragmented MP4 first, plain MP4 when refused (`clip_encoder.rs:352-383`).

## 6. The viewer

### 6.1 Reading the folder

`ViewedClip::open` (`clip_viewer.rs:202-229`) reads `clip.json` and the lines as today. It no longer requires committed segments. It builds:

- the exact index: one `ExactEntry` per line with `x`, in order;
- the moving index: `clip.json` `moving.segments` by `index`.

**D4, what can be viewed.** An instant is viewable when its line exists and, if it has `mv`, its segment is in `clip.json`. For a `ready` clip that is every line up to `recorded_ms`. For a `recording` clip it is every line before the first instant of the open moving segment. `last_frame` (`clip_viewer.rs:237-243`) returns that bound. A time past it is refused with the existing error shape (`clip_viewer.rs:307-326`), saying how many instants are not viewable yet.

`largest_recorded` (`clip_viewer.rs:367-371`) takes the largest `width` and `height` of the lines. `segment_of`, `segment_scale`, `scale_at`, `scale` are deleted; every use takes scale 1. `fingerprint` (`clip_viewer.rs:330-351`) covers `frames.jsonl` and `clip.json`; both change whenever the clip grows.

### 6.2 Composition

New type in `clip_viewer.rs`:

| Type | The one rule it owns | Fields | Methods |
|---|---|---|---|
| `ClipPictures` | The picture at instant k = the exact picture, with the moving frame of that instant drawn over its rectangle | `exact: ExactReader`, `moving: Option<(u32, ClipDecoder)>` (segment index and its decoder), `folder: PathBuf` | `open(&ViewedClip) -> Result<Self, ErrorData>`, `at(&mut self, clip: &ViewedClip, instant: u64) -> Result<ClipPicture, ErrorData>` |
| `ClipPicture` | One composed picture, top-down B,G,R,A | `width, height: u32`, `bgra: Vec<u8>` | fields only |

`ClipPictures::at(k)`:

1. `(bgr, w, h) = exact.picture_at(k)`. Expand to B,G,R,A.
2. When line k has `mv = [s, n]` and segment s is in `clip.json` and n < its `frames`: open the decoder of segment s unless it is the open one; `frame = decoder.frame_number(n, &segment.key_frames)`; copy the top-left `rect.w × rect.h` pixels of `frame` to `(rect.x, rect.y)`.
3. When line k has `mv` but the segment or the frame is missing: leave the exact picture. The instant is in `lost`.

`ClipDecoder::frame_number(n, keys)`:

1. When the last decoded frame number is n: return it.
2. Take the last key `(n_key, t_key)` with `n_key ≤ n`.
3. When the last decoded frame number m satisfies `n_key ≤ m < n`: read on, `n − m` frames.
4. Otherwise `seek(t_key)` (`clip_decoder.rs:164-181`), then read `n − n_key + 1` frames. The first frame after the seek must carry time `t_key` within half a sample; if not, return an error naming the segment.

`ClipView` (`clip_viewer.rs:1661-1671`) holds one `ClipPictures` instead of `decoder: Option<(u32, ClipDecoder)>`. `ClipView::decode` (`clip_viewer.rs:2005-2027`) becomes a call to `ClipPictures::at`. `Picture::new` and `to_rgba` (`clip_viewer.rs:531-541`, `:2107`) take a `ClipPicture`; both only read `width`, `height` and `bgra`.

### 6.3 Seek cost

To show instant k from nothing:

    N_exact(k) ≤ EXACT_GROUP_PICTURES = 20      records of one group, from its key record
    N_h264(k)  ≤ GOP_FRAMES = 20                frames from the keyframe at or before k, when k is in a moving interval
    N_dec(k)   = N_exact(k) + N_h264(k) ≤ 40

Both bounds follow from the files: a key record every 20 records, a keyframe every 20 samples (read back in `key_frames`). Stepping forward costs one record or one frame per step, because both readers keep their position.

The exact reader's RAM is about 135 MB at any picture size (the zstd window of 2^27) plus one picture.

### 6.4 D5 on the combined picture

D5 stays as written in the old plan (`docs/clip-recorder-plan.md:210-299`) with three changes.

**1. ν = 0 (plan 4.2 section 5, decision D7).** Outside moving rectangles the picture is exact, so there is no encoder noise.

    DIFF(b; X, Y)  ⇔  some pixel of b differs by more than δ_lo between X and Y
    SAME(b; X, Y)  ⇔  not DIFF

`Verdict` (`clip_viewer.rs:426-450`) becomes `BlockGrid::differs(x, y, b, DELTA_LO)`. `DELTA_HI`, `NOISE_LO`, `NOISE_HI`, `N_HI`, `N_LO` and their compile-time check (`clip_viewer.rs:79-95`) are deleted. The `Undecided` arms go: in `Analyzer::step` (`pending`, `clip_viewer.rs:942-946`) every picture is SAME or DIFF, and in `end_change` the last arm (`clip_viewer.rs:1111`) is removed. `DELTA_LO` stays 32 (open item O1 of plan 4.2).

**2. The analyzer walks instants, not segment files.** `Analyzer::run` (`clip_viewer.rs:857-886`) loops over the viewable instants in order. For each instant whose line `shows_new_picture()`, it takes `ClipPictures::at(instant)` and calls `step`. Inside `step`, the test `if !self.clip.is_new(instant) { return; }` (`clip_viewer.rs:911`) uses `shows_new_picture` too. `same_picture` (`clip_viewer.rs:266-268`) likewise.

**3. A block inside a stored moving rectangle animates by record.** In `Analyzer::step`, the blocks that intersect the `mv` rectangle of the instant are found the way `cursor_blocks` finds its blocks (`clip_viewer.rs:1117-1136`). For those blocks:

- no pixel test is made (their pixels are H.264 and carry noise);
- `moved = true` and `animating = true` for this instant;
- their anchor stays frozen and they make no event, as D5 item 3 says for animating blocks.

For all other blocks, D5 item 3 is computed from pixels as today, through the shared `MotionWindow`. So an animation the recorder left EXACT (a spinner, an 8 fps animation) is still reported as one animation.

When a moving interval ends, the block's next picture is exact. It is then compared with its anchor like any settling block (D5 item 3, second bullet): a video that stopped on a new picture gives one `change` that starts at its last motion.

Results: one moving interval gives one `animating` timeline entry whose box is its rectangle (connected animating blocks join into one event, `clip_viewer.rs:1277`). Key records and H.264 keyframes give no event: the first are lossless, the second lie inside blocks that are not tested.

`BlockState` (`clip_viewer.rs:774-806`) holds a `MotionWindow` instead of `moves`. `Instants` (`clip_viewer.rs:666-685`) is built from `MotionTiming::of(fps)` plus `settle`. `ANALYSIS_VERSION` becomes 14, so cached analyses are recomputed (`clip_viewer.rs:1986-2002`).

`clip_diff` (`BlockMap::between`, `clip_viewer.rs:596`) uses the same one-line test. When one of its two instants lies in a moving interval, that rectangle is reported as changed; that is correct, the area was moving.

## 7. Build order

Rules for every step:

- No product code is written before its check script exists and has been run **red**. Save the red output and the green output as `scripts/clip-equation/results/<check>-red.txt` and `<check>-green.txt` (the existing convention, for example `results/step2z-red.txt`).
- A check is a `.feature` file plus a `.mjs` file in `scripts/clip-equation/spec/`, run as `node scripts/clip-equation/spec/run-spec.mjs <check> <args>`. Add one dispatch line per check next to the existing ones (`spec/run-spec.mjs:305-309`). Each scenario prints `action | equation | known | measured | result`. Node built-ins only.
- Never loosen a threshold to get green. If a check cannot pass, stop and report: one problem, three options, one recommendation.
- A check takes its constants from what the probe prints (`"constants": { … }`), not from numbers typed into the script. The `.feature` file states the equation.
- No `cargo test`, no `#[cfg(test)]`. The probe is the test surface.
- One cargo build at a time, `-j 4`, at Idle priority. This holds for the probe too.
- The probe includes product files by path, for example in `scripts/clip-equation/probe/src/main.rs`: `#[path = "../../../../crates/synapse-mcp/src/server/clip_exact.rs"] mod clip_exact;`. No product file is copied. The files of steps 1 to 3 are compiled only by the probe until step 4 adds them to `server.rs`.
- After each green step: commit on `feature/clip-capture` and push.

The corpus is at `C:\Users\Adam\clip-corpus` (`scripts/clip-equation/corpus/corpus.json:3`): six scenes, 20 s each, at four sizes. Its reader exists in the probe (`exp_z.rs:1065`, `Corpus`).

### Step 1. The exact layer (W7)

- **Files:** new `server/clip_motion.rs` with `PixelRect` only (the rest comes in step 3); new `server/clip_exact.rs`; new `probe/src/exp_dyn.rs` with `clip-probe exp-dyn mode=exact folder=<corpus folder> out=<dir>`; `probe/src/main.rs` (one dispatch line, next to `:86-87`).
- **What the probe does:** feeds every new corpus frame to `ExactStream::write(frame, None)`, writes `frames.jsonl` lines with `x`, calls `finish`, then reads every instant back with `ExactReader` and compares with the corpus frame. Prints one JSON line: constants, `records`, `groups`, `bytes`, `cpu_ns`, `ram_peak`, `reader_mismatches`.
- **Check:** `spec/dyn-exact.feature`, `spec/dyn-exact.mjs`. The script decodes the group files itself with `zlib.zstdDecompressSync` (as `spec/run-spec.mjs:97-102` does for corpus frames), splits the output by the `raw` lengths, applies the bands and compares with the corpus.
  - Round trip: mismatching bytes = 0 on all six 1920 × 1080 scenes and on `typing-506x333`. Known: 0.
  - Reader: `reader_mismatches` = 0.
  - Groups: every group file is one complete zstd frame; records per group ≤ 20; the first record of each group has `offset` 0 and `raw` = 2 + 4 + w · h · 3.
  - Idle: the still scene has exactly 1 record.
  - Rows: on the typing scene, mean `raw` of non-key records < 5 % of one picture.
  - Size, per scene (typing, ui, scroll, flash): exact retained bytes must be no greater than the whole-frame stream baseline measured in the same run, with the same compression level, window, group length, and input. The historical rounded MB/min figures are not acceptance limits. When changed rows cover more than `EXACT_WHOLE_FRAME_ROW_PERCENT` of the picture, write one whole-picture band; see Builder notes. Fewer uncompressed bytes do not guarantee fewer compressed bytes.
  - RAM: `ram_peak` ≤ 141 MB + 10 %.
  - The 60 s mixed clip of #290: total ≤ 1.10 × 3.24 MB (plan 4.2 W7). Its folder is not in the repo; take it from issue #290. If it cannot be found, report the scenario as blocked, not as passed.
- **Done:** `dyn-exact` all passed; red and green outputs saved.

### Step 2. The Quick Sync path (W8)

- **Files:** `server/clip_encoder.rs`, `server/clip_decoder.rs` (changed as in sections 2.3, 2.4), new `server/clip_moving.rs`; `probe/src/exp_dyn.rs` gets `mode=moving rect=x,y,w,h [pause=from-to] [force=unavailable]`. The old probe copies `probe/src/clip_encoder.rs` and `clip_decoder.rs` stay for `exp-d`; the new mode includes the product files under other module names.
- **What the probe does:** opens a `MovingCoder`, calls `follow` on every instant with the fixed rectangle (`new_picture` from the corpus line; `pause` makes instants repeats), finishes, then decodes every frame by number with `ClipDecoder::frame_number` and compares with the source box. Prints: the grant, each `MovingSegment`, per frame SSIM-Y against the source box of instants n − 1, n, n + 1, encoder creation time, queue high-water mark, `cpu_ns`, `ram_peak`.
- **Check:** `spec/dyn-moving.feature`, `spec/dyn-moving.mjs`, on `video-1920x1080`.
  - Grant: `hardware` = true, `quality_level` = 90, `b_frames` = 0, `gop_frames` = 20.
  - One sample per instant: `frames` = instants of the interval, for `rect` = whole picture, `rect` = 400,304,640,368, and `rect` = 64 × 64 (the minimum side).
  - Frame mapping: for every frame n, SSIM-Y against instant n is the highest of the three. Known: 100 % of frames.
  - Mapping with gaps (#296): the same with `pause=100-140` and `pause=200-203`; the paused instants decode to the picture before the pause.
  - Quality: mean SSIM-Y on the box ≥ 0.95 (plan 4.2 W8).
  - Odd box: on `video-506x333` with `rect` = 0,0,506,333: `coded_width` = 506, `coded_height` = 334, decoded and cut to 506 × 333, mapping exact.
  - Keyframes: `key_frames[0]` has frame 0; the spacing between keyframes is ≤ 20 frames. (The 60 s segment limit is checked live in step 4.)
  - Queue: high-water mark ≤ 8; `moving_behind` instants = 0. Reported: encoder creation time in ms.
  - Unavailable: with `force=unavailable` (the probe calls `MovingAvailability::set_unavailable` before the run) there is no segment file and the summary carries the reason.
- **Done:** `dyn-moving` all passed; the creation time is written into the green output.

### Step 3. The rule and the split (W6)

- **Files:** `server/clip_motion.rs` (complete), new `server/clip_layers.rs`; `probe/src/exp_dyn.rs` gets `mode=layers`; new `scripts/clip-equation/spec/dyn-scenes.mjs`, which writes the synthetic corpus folders below in the corpus format (`frame-NNNN.zst`, `frames.jsonl`, `meta.json`, as `corpus/render.mjs` writes them).
- **What the probe does:** runs the real `ClipLayers` (`picture` / `repeat`) over a corpus folder and writes a complete clip folder without `clip.json`'s recorder parts: `frames/`, `moving/`, `frames.jsonl`, and a `layers.json` with the summaries and the constants. Then it composes every instant (exact reader plus decoder) and reports, per instant, mismatching bytes outside the box and SSIM-Y inside it.
- **Synthetic scenes** (each 1920 × 1080, 200 instants, static text background; "noise" = new random pixels on every instant from 40 to 120):
  1. noise box 320 × 240 at (400, 304);
  2. noise box 48 × 48;
  3. noise bar 512 × 16;
  4. a 320 × 240 box that alternates between two flat colours on every instant;
  5. noise boxes 320 × 240 and 160 × 128, far apart;
  6. noise box 321 × 243 at (403, 301);
  7. noise box at the bottom-right corner of a 1001 × 701 picture;
  8. noise box 320 × 240 that changes only on every third instant.
- **Check:** `spec/dyn-motion.feature`, `spec/dyn-motion.mjs`.
  - Corpus (plan 4.2 W6): `video-1920x1080` gives one moving area that covers ≥ 99 % of the picture from instant ≤ F + 4; typing, ui, scroll, still and flash give 0 moving instants.
  - Scene 1: one segment; rectangle = `[400, 304, 320, 240]`; first moving instant = F + 4 = 44; last = L + 14 = 134; an exact record at 135 whose bands cover rows 304 to 543 (section 3.4).
  - Scenes 2, 3, 4, 8: 0 moving instants (minimum blocks, minimum side, cost term, share threshold).
  - Scene 5: every moving rectangle equals the larger box.
  - Scene 6: rectangle = `[400, 288, 336, 256]` (aligned outward to 16).
  - Scene 7: the rectangle ends at x = 1001, y = 701; coded sizes are even.
  - Exactness outside the box: mismatching bytes = 0 on every instant of every scene.
  - Inside the box: mean SSIM-Y ≥ 0.95 on scene 1 and on the video scene.
  - Frozen rows: on scene 1, the exact bytes written during instants 44 to 134 are 0 (the text does not change, the box is not coded exactly).
  - Start cost: on scene 1, exact records exist at instants 40 to 43 and their `raw` sum ≤ 4 · 240 rows · 1920 · 3 + headers.
- **Done:** `dyn-motion` all passed. If the corpus video or scroll scenario fails with the start values, do not tune by hand: report the measured ρ and animating counts and stop.

### Step 4. The recorder and the folder (W1, W5)

- **Files:** `server/clip_folder.rs`, `server/capture_clip.rs`, `server.rs`, `main.rs`, `m1.rs`, `tool_profiles.rs` (contract text), delete `server/clip_worker.rs`. The viewer does not compile against the new folder types yet, so this step and step 6 are built together and checked one after the other. Do not leave the tree in a state that does not build.
- **Check:** `spec/dyn-folder.feature`, `spec/dyn-folder.mjs <clip folder> <log.json>`, on live recordings made with the existing driver `live-test/record-drive.mjs` (`live-test/README.md`). Three clips: 20 s of `ui-events-page.html`, 20 s of `video-page.html`, and 70 s of `video-page.html`. The test window is opened and closed in the same script that records.
  - Files: the folder holds exactly the names of section 4.1; no `waiting`, no `segment-*.mp4`, no `.part`.
  - Lines: instants are 0 … n − 1 without a gap; every `x` lies inside its group file; within a group, offsets rise and the first is 0; every `mv` names a segment of `clip.json` and `frame` = `instant − first_instant`; `mv_rect` is on the first instant of each segment and equals its `rect`.
  - `clip.json`: `way` = DYN1, `schema_version` = 3, `state` = ready, `geometry.scale` = 1, C, G, B and T held, Q `exact` (ui clip) and `held` (video clip), `lost` empty.
  - Bytes: `conditions.t.detail.bytes` = Σ files of section 4.1.
  - Ready time: the time from the last line to `state: ready` (the driver polls `record_status`) ≤ 1,000 ms. Reported: the measured value.
  - Status: `record_status` has no `frames_waiting`; `record_stop` twice returns the same state.
  - Segment length: the 70 s clip has two segments with the same `rect`; the first has `frames` = `MOVING_SEGMENT_MS` · f / 1000 = 1,200; the second starts at the next instant.
- **Done:** `dyn-folder` all passed on the three clips; `cargo clippy` clean; no reference to the worker is left.

### Step 5. Recovery (W2)

- **Files:** new `server/clip_recovery.rs`, `server.rs`, `main.rs`.
- **Check:** `spec/dyn-recovery.feature`, `spec/dyn-recovery.mjs <clip folder> <log.json>`. The recording is made with `record-drive.mjs` options `kill_server_at_ms` and `restart_after_kill` (`live-test/README.md`) on `video-page.html`, killed at 8,000 ms of a 20,000 ms clip.
  - After the restart: `state` = ready, `stop_reason` = service_restart.
  - Each lost instant is named once; instants from the last line + 1 to 399 are named with cause `service_restart`.
  - No `.part` file is left.
  - The moving segment that was open: either recorded with `complete: false` and `frames` ≥ 1, or absent with all of its instants lost. Report which, and how many frames survived (this measures risk R7).
  - The exact layer: every record that has a line decodes. The last group has no end mark, so the check reads it with Node's streaming zstd decompression (`zlib.createZstdDecompress`), not with the whole-file call.
  - `clip_overview` on the recovered clip succeeds (after step 6).
- **Done:** `dyn-recovery` all passed; the survived-frames figure is in the green output.

### Step 6. Viewer composition and seek (W3, W9)

- **Files:** `server/clip_viewer.rs`, `live-test/view-call.mjs` if it needs a field.
- **Check:** `spec/dyn-viewer.feature`, `spec/dyn-viewer.mjs`. Inputs: the clip folders the probe made in step 3 (scene 1 and `video-1920x1080`), completed with a `clip.json` by the probe (`mode=layers clipjson=1`), and viewed through the stdio server with `live-test/view-call.mjs`.
  - Exact outside the box: `clip_frame` at 10 instants of scene 1, each with a `region` outside the box and below y = 40 (the label is drawn at the top-left corner, `clip_viewer.rs:2285-2318`); the PNG equals the source crop byte for byte.
  - Composed inside the box: `clip_frame` with a `region` inside the box at instants 44, 90, 134: SSIM-Y against the source ≥ 0.95, and the picture differs between the three instants.
  - Exact after the stop: at instant 135 the box region equals the source byte for byte.
  - Missing frame: with one segment removed from `clip.json`, `clip_frame` inside the box succeeds and shows the exact layer's picture.
  - Seek bound, from the files: for every instant, records to apply ≤ 20 and H.264 frames to decode ≤ 20.
  - Seek time: 20 `clip_frame` calls at random instants; reported: the longest. Known bound: none; the figure goes into the green output.
- **Done:** `dyn-viewer` all passed.

### Step 7. D5 on the combined picture (W4)

- **Files:** `server/clip_viewer.rs`, `spec/d5.feature`, `spec/d5.mjs`, `live-test/d5-proof.mjs`, `scripts/clip-equation/d5-scenes.html` (a `video` scene).
- **Changes to the proof driver:** `ClipFiles` reads keyframe instants from `clip.json` `segments[].keyframe_instants` today (`live-test/d5-proof.mjs:310`). It now takes the instants of lines whose `x` has offset 0, plus `first_instant + frame` for every `key_frames` entry. `waitReady` (`:344`) needs no change. Add a clip `video`: a canvas of 640 × 360 that animates beside static text for 20 s.
- **Check:** the existing `d5` check, with these edits to `spec/d5.feature`:
  - every existing scenario stays and must pass (still clip, lone spinner = exactly one animation, flash clip, boundary clips, "keyframe instants give no event");
  - new scenario "a moving area is one animation": the `video` clip has exactly one `animating` entry, its box equals the stored rectangle, and no `change` or `short_lived` entry lies inside the box while it moves;
  - new scenario "a moving area that stops gives one change": after the canvas stops on a new picture, one `change` starts within one instant of its last motion.
  - Run: `node scripts/clip-equation/live-test/d5-proof.mjs <release exe> <scratch dir>`.
- **Done:** `d5` all passed on a live run; `ANALYSIS_VERSION` is 14.

### Step 8. The YouTube live proof

- **Files:** new `live-test/test-window.mjs` (`Cdp`, `TestWindow`, `until`, `alive`, `CleanupError`, moved out of `d5-proof.mjs:130`, `:148`, `:190`, `:203` so both drivers share them), `live-test/d5-proof.mjs` (imports them), new `live-test/youtube-proof.mjs`, new `spec/dyn-youtube.feature`, `spec/dyn-youtube.mjs`, `live-test/README.md`.
- **Check and done criterion:** section 8.

### Step 9. Verification round

1. Codex review of the whole change, read-only.
2. `cargo fmt`, `cargo clippy`, then one release build (`-j 4`, Idle priority).
3. On the release build, in this order: `dyn-exact`, `dyn-moving`, `dyn-motion` (probe); `dyn-folder`, `dyn-recovery`, `dyn-viewer` (live); `d5-proof.mjs`; `youtube-proof.mjs`.
4. Report every figure against the budgets of section 1.5 and 1.6, and every deviation from this document.

## 8. The YouTube live test

### 8.1 What it is

One real recording of 60 s of a YouTube watch page: a video plays next to text; the page is scrolled once; a search is typed. The recording and all viewing go through a Synapse MCP server started by the script. Then a check script judges the clip.

    node scripts/clip-equation/live-test/youtube-proof.mjs <synapse-mcp exe> <scratch dir> [--url <watch url>]

It needs Node 22 or later, Chrome and a network connection. No npm packages.

### 8.2 Parts it reuses

- `McpServer` (`live-test/mcp-stdio.mjs:15`): the build under test as a separate stdio instance with its own `--db` and `SYNAPSE_SHELL_JOB_ROOT`. Its `call` runs one `screenshot` operation and saves returned images (`:71`).
- `User32` (`mcp-stdio.mjs:110-127`): show a window without activating it and put it behind all others.
- `TestWindow` (`live-test/d5-proof.mjs:208`, `:262`, `:275`): a Chrome `--app` window with its own profile and DevTools port; `close` ends the process tree and verifies that no process of the profile is left, or throws `CleanupError`.
- The Chrome flags that keep a covered window rendering (`d5-proof.mjs:40-41`).

The recording, the status and the viewer calls go through the Synapse MCP server. The scroll and the typing are sent to the page over the DevTools protocol, not as real mouse and keyboard input. Reason: the drivers in this folder never send input to any app and never activate a window (`mcp-stdio.mjs:1-3`), so the test does not take the owner's mouse, keyboard or foreground.

### 8.3 Configuration (`CONFIG` at the top of the script)

| Name | Value | Meaning |
|---|---|---|
| `url` | `https://www.youtube.com/watch?v=aqz-KE-bpKQ` (Big Buck Bunny, Blender Foundation); `--url` replaces it | The watch page. The script does not trust the id: it checks that a video plays (step 3) |
| `window` | x 0, y 0, 1920 × 1080 | So the RAM figure is a 1080p figure |
| extra Chrome flags | `--autoplay-policy=no-user-gesture-required`, `--mute-audio`, `--force-color-profile=srgb` | The video starts alone, makes no sound, and colours match the DevTools screenshot |
| `durationMs` | 60,000 | |
| `scrollAtMs` | 12,000; `scrollDeltaY` 400 | One wheel event |
| `typeAtMs` | 30,000; `typeText` `"synapse clip"`; `typeGapMs` 250 | 12 keys |
| `pauseAtMs` | 48,000 | The video is paused |
| `shotAtMs` | 54,000 | The independent screenshot |
| `sampleMs` | 1,000 | RAM and CPU sampling |
| `playWaitMs` | 30,000 | How long to wait for a playing video |

### 8.4 Steps

Everything from step 1 on runs inside `try { … } finally { await win.close(); await server.close(); }`.

1. Start the stdio server (`McpServer`, `init`).
2. Open the test window with `TestWindow.open(url, profileDir)`: minimized, not activated. Connect to its page target (the target predicate becomes a parameter; today it looks for a `file:` URL).
3. Make the page ready:
   - If the page is a consent page (`location.host` is `consent.youtube.com`), click its reject button and wait for the watch page.
   - Wait up to `playWaitMs` until `document.querySelector('video')` exists, `readyState ≥ 3`, `paused` is false and `currentTime` rose between two reads 500 ms apart. If the video is paused, call `video.play()` once.
   - If no video plays in time: stop with the message "no playing video", exit code 1. No clip is judged.
4. `placeBehind()`: show without activating, let it paint, put it behind all windows.
5. Read from the page: `devicePixelRatio`, `innerWidth`, `innerHeight`, and the boxes (`getBoundingClientRect`) of the video element, the search input, the title-and-description block (`#above-the-fold`) and the right column (`#secondary`).
6. Sample M_0 and the processor time of the server process (`Get-Process -Id <pid>`: `PrivateMemorySize64`, `TotalProcessorTime`). Start the sampler: the same two values every `sampleMs` until the clip is ready.
7. `screenshot operation=record` with `window_hwnd`, `duration_ms: 60000` and a folder in the scratch dir. Wait for `clock.wall_clock_unix_ms_at_s0` in `clip.json` (as `d5-proof.mjs:330-336`). All later times are converted to clip time with s_0.
8. At `scrollAtMs`: `Input.dispatchMouseEvent` with `type: "mouseWheel"`, `deltaY: 400`, at the centre of the page. Record the wall time. Two seconds later read the video box again.
9. At `typeAtMs`: focus the search input with `Runtime.evaluate`. For each character: `Input.dispatchKeyEvent` `keyDown` with `text`, then `keyUp`; record the wall time; wait `typeGapMs`. Do not press Enter. After the last key, blur the input (a blinking caret would keep the page changing).
10. At `pauseAtMs`: `video.pause()`; record the wall time and read `video.paused`.
11. At `shotAtMs`: `Page.captureScreenshot` with `format: "png"`; record the wall times before and after; save the PNG.
12. Wait until `record_status` says `ready` or `failed`. Record the wall time of the first `ready`.
13. Stop the sampler. Close the browser window (`win.close()`). Verify that no process of the profile is left. A window that cannot be closed makes the script exit 1 with `CleanupError`, whatever the clip says (owner rule: a test window is open only while it is recorded).
14. With the window closed, through the same server: `clip_overview`; `clip_frame` at 5,000, 20,000, 40,000 and 57,000 ms; `clip_frame` at the screenshot time with `region` = the title-and-description block; `clip_diff` from 29,000 to 34,000 ms. Save every image in the scratch dir.
15. Write `<scratch>/youtube-run.json`: the configuration, the page boxes, all event times, the samples, the folder path, the viewer outputs. Run `node spec/run-spec.mjs dyn-youtube <scratch>/youtube-run.json` and exit with its status.

Page pixels become picture pixels as the D5 driver does it (`viewport`, `d5-proof.mjs:109`): `x_pic = side + x_css · dpr`, `y_pic = top + y_css · dpr`, where `side` and `top` are the window borders computed from the captured size and the page's inner size.

### 8.5 What is measured

| Quantity | How |
|---|---|
| Captured size | `clip.json` `geometry` |
| RAM | max of the `PrivateMemorySize64` samples during the recording, minus M_0 |
| CPU | (processor time at ready − at record start) / (recorded seconds × logical processors) |
| Bytes | Σ `frames/*`, Σ `moving/*.mp4`, `frames.jsonl`, `clip.json`; and MB per minute per layer |
| Layers in time | from `frames.jsonl`: instants with `mv`, instants with `x`, instants with neither |
| Rectangles | `clip.json` `moving.segments[].rect` against the video boxes read from the page |
| Time until ready | first `ready` minus s_0 − 60,000 ms |
| Exact records | decoded by the script (the decoder of `spec/dyn-exact.mjs`): bands per record |
| Viewer | timeline entries of `clip_overview`; the images |

### 8.6 Pass criteria (`spec/dyn-youtube.feature`)

Times are clip times. f = 20. V is the video's box in picture pixels before the scroll, V′ after it. "IoU" is intersection over union.

| # | Scenario | Equation | Known |
|---|---|---|---|
| 1 | The clip is complete | `state` = ready; `stop_reason` = completed; `recorded_ms` = 60,000; lines = 1,200 | as stated |
| 2 | Nothing was lost | `lost` is empty; C and T held | 0 |
| 3 | The test ran at 1080p | captured width ≥ 1,900 and height ≥ 1,000; otherwise scenarios 4 and 5 are reported as blocked, not passed | as stated |
| 4 | RAM | max(private bytes) − M_0 ≤ 700 MB | 700 MB (owner decision 2026-11-23; was 500) |
| 5 | CPU | process CPU share ≤ `CPU_LIMIT_PERCENT`; B held | 10 % |
| 6 | The encoder | `moving.status` = available; `hardware` = true; `quality_level` = 90; Q held | as stated |
| 7 | The video moves | of the instants in [1,000 ms, `scrollAtMs`), at least 95 % have `mv` | ≥ 95 % |
| 8 | The box is the video | for the segment that covers 5,000 ms: IoU(rect, V) ≥ 0.80, and rect lies inside V grown by `MOVING_ALIGN_PX` + `BLOCK` = 24 px on each side | as stated |
| 9 | Text beside the video is exact | for every instant in [1,000 ms, `scrollAtMs`): the moving rectangle does not intersect the right-column box | 0 intersections |
| 10 | The scroll is stored | an exact record lies in [scroll time, scroll time + 2,000 ms] | ≥ 1 |
| 11 | After the scroll the box is the video again | at scroll time + 3,000 ms: IoU(rect, V′) ≥ 0.80 | as stated |
| 12 | Typing is exact and does not move | for each of the 12 keys: an exact record within [key time, key time + 300 ms] with a band that meets the search input's rows; and the moving rectangle is the same at `typeAtMs` − 1,000 and at the last key + 1,000 | 12 of 12; unchanged |
| 13 | The stop | last instant with `mv` ≤ pause time + (5 + H − 1 + 5) / f = pause time + 950 ms (5 instants of slack for the pause to take effect) | ≤ 950 ms |
| 14 | One exact picture after the stop | the instant after the last `mv` has an exact record whose bands cover all rows of the last rectangle | covered |
| 15 | Nothing changes, nothing written | every line with `new: false` that is not the first instant after a rectangle change has no `x`; reported also: exact bytes written in the last 2 s | 0 violations |
| 16 | Sample count | for every segment: `frames` = `last_instant − first_instant + 1`, and `complete` is true | as stated |
| 17 | Size bound | B ≤ B_max; B_max is written out below the table | computed from the clip |
| 18 | The picture is the screen | the `clip_frame` crop of the title-and-description block at the screenshot time equals the DevTools screenshot crop, byte for byte, below the label box | max difference 0 |
| 19 | The viewer sees it | `clip_overview` succeeds; at least one `animating` entry overlaps V; at least one `change` entry starts in [scroll time − 50 ms, scroll time + 2,000 ms]; at least one `change` entry meets the search input's box during the typing | as stated |
| 20 | The video is in the clip | the `clip_frame` images at 5,000 and 20,000 ms differ inside V, and V is not one flat colour in either | as stated |
| 21 | Ready time | reported; ≤ 1,000 ms | 1,000 ms |
| 22 | The window is closed | no process of the test profile is left | 0 |

Scenario 17, written out. The bound is built only from measured figures:

    B_max =  535 MB/min · Σ_s ( area(rect_s) / 2,073,600 ) · ( frames_s / 1,200 )        moving layer at the measured worst rate
           + 9.4 MB/min · 1 min                                                          exact layer at the worst measured non-video rate
           + Σ_s 4 · area(rect_s) · 3 bytes                                              four lossless pictures of the box at each start
           + 0.3 MB                                                                      frames.jsonl and clip.json

535 MB/min is 9 % of the lossless video rate at 1080p (plan 4.2 section 1, `test-1080-bigwindow-summary.txt:2`). 9.4 MB/min is the scroll scene (same file). The bound is loose on purpose: it catches a broken rule (video stored losslessly), not a tuning question. The actual MB per minute of each layer is printed beside it for the owner.

The test passes when every scenario passes and the script exits 0. The run's folder, `youtube-run.json` and the images stay in the scratch dir for the owner and for an agent to look at.

### 8.7 Things the test cannot fix by itself

- A consent page or an advertisement can look different from day to day. An advertisement is also a playing video, so the test accepts it. If no video plays within 30 s the test stops before recording.
- If scenario 18 fails while every other scenario passes, the difference may come from colour handling between the DevTools screenshot and Windows capture, not from the recorder. Do not change the criterion. Report the difference image and the largest difference (risk R14).

## 9. Risks and open items

### 9.1 For the owner to decide

| # | Item | Recommendation |
|---|---|---|
| O1 | δ_lo for the viewer, 32 or lower (plan 4.2 O1). Independent of the recorder's δ = 0 | Keep 32 until the D5 proof asks for less |
| O2 | The cost term in the moving rule (decision D1) and its value θ = 0.25 | Accept. Without it W6's scroll scene fails by construction |
| O3 | σ = 1: the rule "at least 480 lines" (`docs/clip-recorder-plan.md:31`) no longer duplicates pixels (decision D4) | Accept. The stored pixels are exact; duplication adds only bytes |
| O4 | A box that is still moving when the recording ends has a lossy last picture | Leave as is. An exact last picture costs up to one lossless picture of the box per clip. Say so if the last frame matters |
| O5 | Viewing a clip while a 1080p recording runs can pass 500 MB (the viewer's zstd window is 135 MB) | Accept and document, or refuse viewer calls while a recording runs. Cheapest: accept |
| O6 | One moving area only (decision D3) | Accept for this build |
| O7 | Issues: #296 is fixed by section 5.4; #292 (worker) is obsolete; #297 (extra keyframe) is harmless here | Close #292 and #297, close #296 after step 2 is green |

### 9.2 Risks

| # | Risk | Effect | How the build finds out, what to do |
|---|---|---|---|
| R1 | Encoder creation takes longer than 8 instants | `moving_behind` losses at the start of an interval | Happened: measured up to 1,854 ms. Fix plan in the Builder notes "2026-11-23: R1 fix plan, encoder start" (owner decision: warm-up at record start plus a queue sized from the measured warm start) |
| R2 | Four lossless pictures of the box at every moving start | A few MB per start (up to 4 · box bytes) | Reported by step 3 "start cost" and by the YouTube size table. A delay line would remove it at about 60 MB of RAM; not built now |
| R3 | The share threshold 0.5 misses slow animation (under 10 changes per second) | Such content is stored exactly on every change and is large | The YouTube run and the corpus show the exact-layer rate. If real content needs it, lower `ANIMATING_CHANGE_SHARE` by the proof, in recorder and viewer together |
| R4 | A scroll that moves a playing video makes the whole scrolled area move for about 0.75 s | Text is quality 90 for that time, then exact | Scenario 11 checks that the box returns. Accepted behaviour (section 3.5) |
| R5 | Quick Sync refuses a 64 px side, or an odd coded size near the picture edge | The interval fails and the encoder is marked unavailable for the process | Step 2 tests 64 × 64 and the odd case. If refused, raise `MOVING_MIN_SIDE_PX` to the smallest accepted size |
| R6 | The encoder writes fewer samples than it was given | Missing frames at the end of a segment | Sample count is verified at every commit and named as loss; scenario 16 |
| R7 | A `.part` file cannot be read after a kill (it is not finalized) | The open moving segment is lost at a crash, at most 60 s of the moving area; the text stays | Step 5 measures what survives. If nothing survives and that matters, lower `MOVING_SEGMENT_MS` |
| R8 | The zstd window of 2^27 costs 141 MB even for small windows | RAM does not shrink with the window size | Accepted by the owner's choice of W = 27. The value is one constant |
| R9 | Viewer and recorder in one process | Over 500 MB while both run at 1080p | Open item O5 |
| R10 | Video on a weak machine passes 10 % CPU | Frames are shed and named (E1) | The CPU share is in condition B and in scenario 5. Quick Sync's driver threads are not in `LiveMeter`; scenario 5 measures the whole process |
| R11 | The start values of the rule (N_min, s_min, H, θ) are estimates | Wrong boxes on real content | Steps 3 and 8. Values change only through the proof, never by hand to pass a check |
| R12 | Windows capture shows a hardware video overlay as black | The moving area is black in the clip | Scenario 20 |
| R13 | The corpus is synthetic; real scrolling may match worse | The exact layer is larger than measured | The YouTube size table shows the real rate |
| R14 | The DevTools screenshot differs from Windows capture for reasons outside the recorder | Scenario 18 fails though the clip is right | Section 8.7. Report, do not loosen |
| R15 | The product has zstd 0.13, the probe 0.14.4 | An API name differs | Use only calls in both; the probe compiles the product file, so a mismatch fails the build at once |
| R16 | The 60 s mixed clip of #290 is not in the repo | One size scenario of step 1 cannot run | Take it from the issue; else report it as blocked |
| R17 | Clips made before this build have schema 2 | They are refused by the viewer (`clip_folder.rs:633-639`) | Accepted: the format is new. Say so in the release note |
| R18 | A monitor target | Works the same way; the rule sees the whole monitor as one picture | Not part of the proof. The requirement is one application window |
| R19 | `Recording` grows | Harder to maintain alone | `ClipLayers` takes everything about pictures; `Recording` keeps instants, lines and stops. Do not add picture logic to `Recording` |

### 9.3 What this document does not change

Capture and the observer; the instant grid and `RECORD_FPS` = 20; the privacy gate before every write; the all-black first frame rule; the cursor rule (not drawn, logged per line); `recorder.lock`; the registry; atomic `clip.json`; the disk rule; the 10 % rule; the `record`, `record_status`, `record_stop`, `record_extend` operations and the four viewer operations; D5 items 2, 4, 5 and 6.

## Builder notes

### 2026-11-23: R1 fix plan, encoder start (written by Claude 2026-11-23 16:30 IST; this section is the builder's spec)

**Facts (sources):**
- F1. `final-dyn-moving-{1,2,3}.txt`, row "bounded queue" `[name, high_water, behind, creation_ms]`. Creation of the moving encoder took 1,719–1,854 ms in 11 of 18 cases, 715–722 ms in 2 and 221–232 ms in 5. A 64 × 64 box took 1,719–1,724 ms, so the cost does not come from box size. Every case with creation ≥ 715 ms has `behind` 7–30. Every case with `behind` = 0 passes mapping and quality (SSIM 0.970 and 0.968).
- F2. `final-dyn-motion-3.txt`: noise SSIM 0.692. About 28 of the 91 noise instants are `moving_behind` repeats that score near 0 against new noise. 63 × 0.97 / 91 ≈ 0.67, which matches.
- F3. `MovingCoder::start(folder, fps)` needs no picture size (`clip_moving.rs`). It is called inside `ClipLayers::start` (`clip_layers.rs:79`), which runs after the first captured frame (`capture_clip.rs:860`). Creating the encoder there would block the recorder thread.
- F4. Encoder creation = `MFCreateSinkWriterFromURL` with hardware transforms enabled, plus the media types (`clip_encoder.rs` `open_writer`). Each case of the probe is a new process, so each one pays any one-time start cost.
- F5. RAM: on 2026-11-23 the owner raised the limit from 500 to 700 MB for every recording. The budget (section 1.5) now has 226.5 MB of headroom, as an upper bound. Each queue frame at 1080p B,G,R = 6.2 MB.
- F6. There is no software fallback (section 5, line 591: the Microsoft software encoder measured about 671 MB). The owner confirmed on 2026-11-23: Quick Sync, or any hardware H.264 encoder, only.

**Owner decision (2026-11-23):** both parts. (A) Warm the encoder once at record start, before capture starts. (B) Size `MOVING_QUEUE_FRAMES` from the measured warm start time, within the RAM budget.

**Parts, in order:**

1. **M1: measure (probe only, no product change).** Add the probe command `clip-probe exp-encoder-start`. In one process, create and finish `ClipEncoder`s one after another:
   - sizes: 64×64, 640×368, 1920×1080, 64×64, 1920×1080, 640×368;
   - each encoder writes 3 frames before it finishes;
   - print each creation time in ms;
   - variant `keep=1` holds the first encoder open until all the others finish; variant `keep=0` drops each encoder before creating the next;
   - variant `thread=other` creates the first encoder on another thread, which then ends, and the rest on the main thread.

   Run each variant in 3 separate processes. Save the output to `scripts/clip-equation/results/r1-start-*.txt`.
   - Result W = the largest creation time of encoders 2–6 across all runs of the chosen variant.
   - Choose the cheapest variant whose W is smallest. Prefer `keep=0` if it is within 100 ms of `keep=1`.
2. **A: warm-up.**
   - `MovingCoder::start` creates one 64 × 64 warm-up encoder on the moving worker thread, writing to `moving/warm.mp4.part`. It keeps it open or drops it according to M1. On drop, it deletes the file.
   - `start` returns only after the warm-up has finished or failed. It returns the warm-up time in ms (`warm_ms` in `MovingSummary`).
   - A warm-up failure calls `MovingAvailability::set_unavailable(reason)`. The recording continues with the exact layer only, as with any other creation failure.
   - Move the call out of `ClipLayers::start`: the recorder calls `MovingCoder::start` after the clip folder exists and before the observer captures the first frame, then passes the coder into `ClipLayers::start`.
   - The probe's `exp-dyn` and the dyn-moving probe call `start` before they feed instant 0. This matches the product.
   - The warm file is never left behind: it is deleted on `finish`, and recovery deletes a stale `moving/warm.mp4.part`.
3. **B: queue size.**
   - `MOVING_QUEUE_FRAMES` = max(8, ⌈W × `RECORD_FPS` / 1000⌉ + 2).
   - RAM check: (Q + 1) × 6.2 MB replaces the 56 MB row of section 1.5, and the new sum must stay ≤ 700. Also measure the real peak with dyn-live `video` at 1080p (rule M3) and report it.
   - If Q does not fit the budget, STOP and report to Claude with the numbers. Do not change the queue's picture format and do not lower any gate.
4. **Bug: silent probe crash.** In dyn-motion run 3, scene `slow`, `clip-probe exp-dyn` exited non-zero with empty stderr, and the spec printed `FAIL slow: undefined` (Gitea #290, comment 1369).
   - First make `dyn-motion.mjs` print `run.status`, `run.signal` and stdout's last line when a probe fails.
   - Then run scene `slow` 20 times in a loop to reproduce it. Fix the cause in the probe or the product.
   - If it does not reproduce in 20 runs, report that with the loop output. Do not mark it fixed.

**Proof (all on one release build; `-j 4`, process priority Idle; one build at a time):**
- `dyn-moving` 3/3 passed, with `behind` = 0 in every case. The output shows `warm_ms` and the creation time of each segment.
- `dyn-motion` 3/3 passed, including noise and video quality ≥ 0.95, and `slow` passes.
- `dyn-exact` 3/3 passed. fmt and clippy (`-D warnings`) are clean.
- `r1-start-*.txt` committed. Section 1.5's RAM table updated with the measured peak.
- No gate, threshold, timeout or assertion is loosened. A check that stays red is reported red, with its output.

**Out of scope here:** a software encoder (the owner approved one on 2026-11-23; it gets its own plan after this fix lands), a different queue picture format, any change to quality 90, the GOP, or the segment length.

### 2026-11-23: step 9, decoder seek and recovery edges

The Windows fragmented MP4 source did not reliably land on the requested stored keyframe. A reverse pixel-hash check failed at frame 79 and returned the pixels of frame 59; the initial strict timestamp check had already failed at keyframe 80 (40,000,000 hns requested, 39,000,000 hns returned). Flushing and seeking inside the key sample still failed at the scene-cut keyframe 6. Native compressed reads also returned shifted timestamps, so using their times did not establish the landing frame.

The moving encoder now uses ordinary MP4, the existing fallback container, for all intervals. With that container, all 91 reverse seeks through the noise fixture matched sequential decode byte for byte. The decoder flushes before a seek and seeks inside the stored key sample; mapping remains by sample count. This is a container deviation from the previous preference for fragmented MP4. An unfinalized ordinary MP4 may have no recoverable moving samples; step 5 explicitly permits that outcome with every affected instant named as lost. Exact records remain independently recoverable.

Recovery now creates an empty frame index when a crash precedes the first picture. The added check changed from exit 1 (missing file, despite 200 losses already published) to exit 0, ready/service_restart, with all 200 planned instants named. A salvaged uncommitted moving interval is marked incomplete even when all indexed samples survive.

The reviewer owns `live-test/` in this continuation. Builder checks and the local UI/video/restart driver live under `spec/`; no file under `live-test/` was changed or executed for YouTube.

### 2026-11-23: continuation 3, viewer and live verification

The composed viewer builds with the recorder and recovery code. Viewer PNGs retain their existing top-left labels; independent pixel checks compare rows below the 40-row label area. The D5 moving-stop check uses the last instant of the stored moving interval as "last recorded motion", consistent with treating the rectangle as moving by record through its hold interval.

Live-driver files are untouched under the latest owner instruction. `dyn-folder`, `dyn-recovery`, `dyn-viewer`, and D5 must not be called green until their live evidence exists. The red scripts and per-step Cargo diagnostics are retained even where the numbered order temporarily required later modules to compile.

### 2026-11-23: continuation 3, step 3 acceptance gaps

The first combined-layer run passed the exact-byte and geometry checks: scene 1 opened at 44, ended at 134, restored exact rows at 135, and wrote no exact record while the box stayed open. The minimum size, minimum side, flat-colour cost, slow-change share, largest-group, odd alignment, corner clamp, typing and UI checks passed.

Two quality gates remain open: scene 1 measured mean SSIM-Y 0.6920073488, with 28 `moving_behind` instants (52–79); video measured 0.9421424366, with 27 `moving_behind` instants (13–39). The hardware grant was quality 90, GOP 20, zero B-frames in both. The combined path's initial queue saturation needs investigation; the isolated step-2 path had no queue losses. The queue bound and quality requirement are unchanged. Per the continuation instruction, implementation proceeds with these measured failures recorded.

`Recording` initializes its layers after the first frame reveals its dimensions, so the field is an `Option<ClipLayers>` until frame zero is accepted. The first-picture and repeat paths fail if initialization is absent. This resolves the specification's field/initialization ordering without an extra pass-through type.

### 2026-11-23: continuation 3, step 2 integration and seek

The inherited step-2 code builds in the probe. The workspace check still references the deleted encoder API in the old worker and the old timestamp API in the viewer; those callers are replaced in steps 4 and 6. The step-2 check is therefore not a workspace-green claim. Its diagnostics are saved in `results/step2-check.stderr.txt`.

The first resumed moving check failed in all six encoding cases: seeking to keyframe 6 at 3,000,000 hns in fragmented MP4 landed at 0 hns. Sequential decoding now reads across keyframe boundaries without seeking. Random seek remains subject to step 6's strict landing and cost checks; no timestamp tolerance was relaxed.

The latest owner instruction reserves all of `scripts/clip-equation/live-test/` for the reviewer. Steps 7 and 8 will update only their spec files and product code; live driver edits and YouTube execution remain reviewer work.

### 2026-11-23: continuation 2, framing equation

The owner's continuation authorizes proceeding through steps 2–9 even when an acceptance gate remains open, superseding section 7's stop rule. Commits stay local; the reviewer runs the YouTube proof.

The step 1 gate now subtracts the counted logical record headers, physical zstd frame/block headers and checksum bytes, and the exact index file length. Record headers are counted from each decoded record's actual band count (including keys); zstd boundaries are parsed from each written group, not estimated from compression ratios. The logical record-header count is not a measurement of those headers' individual compressed contribution. This interpretation keeps the requested equation explicit and reproducible.

The same-run scroll equation is `(3,329,582 - 108,541) = 3,221,041 <= 3,298,546` bytes. Framing consists of 2,400 record-header bytes, 63,695 zstd framing bytes, and 42,446 index bytes. All nine checks pass; every input and backward seek has zero mismatching bytes. Evidence: `scripts/clip-equation/results/dyn-exact-green.txt`; preceding red evidence remains in `dyn-exact-failed.txt`.

### 2026-11-23: continuation, measured baseline and broad-row fallback

The owner's continuation replaces the incorrectly rounded absolute size gates with a same-run, per-scene comparison. `dyn-exact.mjs` now runs the existing `exp-z` whole-frame stream baseline with the exact writer's reported level, window log, and group length. It requires the baseline's round trip to pass and saves its measurements beside the exact measurements. The acceptance scope remains the four scenes named in step 1: typing, UI, scroll, and flash. All eight inputs retain the independent round-trip, reader, format, and RAM checks; the mixed clip also retains its separate size gate. Size includes the exact group files and index; the baseline includes all retained files, as the existing harness reports them.

`ExactStream` now owns `EXACT_WHOLE_FRAME_ROW_PERCENT = 75`. Above three quarters of the picture, preserving whole-frame dictionary alignment takes precedence over omitting at most a quarter of the rows. This separates the measured scroll case (about 88% changed rows) from typing and small UI edits, while retaining the specified row-band format, level 1, window log 27, no long-distance matching, and groups of 20. The stored picture is still the exact-layer picture, so any moving box remains frozen. The check reads the constant from the probe and validates that broad records contain one full-height band.

The red run measured scroll at 3,559,626 retained bytes against a same-run baseline of 3,298,546. The requested whole-frame fallback reduces it to 3,329,582 bytes, but this is still 31,036 bytes (about 0.94%) above the baseline. Round trips remain exact. Separate header/pixel flushes and 64-row flush chunks were tried locally; neither met the retained-byte gate, and neither experiment remains in the product. No gate or compression parameter was relaxed. The requested fallback alone is therefore insufficient to establish the corrected size requirement. Section 7 says, "If a check cannot pass, stop and report"; steps 2–9 are not started in this continuation.

Possible next decisions: (1) authorize a measured compression-parameter search while preserving the exact format and the current byte gate (recommended); (2) authorize a different exact record format and repeat the independent reader checks; (3) defer the dynamic build. This continuation is WIP until the same-run byte gate passes. The earlier Builder notes below describe the preceding run; their Git-permission and network limitations are historical.

Final corpus verification: eight of nine scenarios pass; only the size comparison fails. All eight inputs have zero independent-decoder and product-reader byte mismatches. The broad-row rule has zero violations. Peak writer RAM is 160,436,224 bytes, below the unchanged 155.1 MiB allowance; the mixed clip is 2,823,850 bytes, below its unchanged 3.564 MiB allowance. Evidence: `results/dyn-exact-red.txt`, `results/dyn-exact-failed.txt`, `results/dyn-exact-metrics.json`, and `results/dyn-exact-baselines.json`. There is no green result. Only `.runs/dyn-exact-final` remains from these trials. Workspace formatting, separate formatting checks for the changed Rust files, strict workspace clippy, and the final probe release build pass. Synapse health is OK; this is not dynamic-recorder UX verification because the integration remains unbuilt. The YouTube proof was neither written nor run because step 8 was not reached.

The final workspace `cargo build --release -j 4` passed in 7m 30s after clean formatting and strict clippy. It ran alone at Idle priority. The context-mode client response timed out at 300 seconds, but the original build continued; its process exit and final `Finished release` log were checked without starting another build. The earlier ONNX linking failure did not recur. Logs are `results/dyn-continuation-{fmt,clippy,build}.{stdout,stderr}.txt` and `results/dyn-step1-probe-build.{stdout,stderr}.txt`. `REVIEW.md` records the work and remains outside the commit.

### 2026-11-23: step 1 size gate failed

The owner approved the cost term, one moving area, and exact initial pictures. The build follows the numbered steps, uses check scripts before product edits, commits locally, and does not push. The owner's explicit instruction also excludes running the YouTube proof.

Step 1 adds `clip_exact.rs`, the `PixelRect` part of `clip_motion.rs`, and the `exp-dyn mode=exact` probe. The files are included directly by the probe and are not yet wired into the server, as section 7 requires. The check is available through `node scripts/clip-equation/spec/run-spec.mjs dyn-exact out=<fresh directory>`.

Readings used to resolve small ambiguities:

- The measurements cited as MB in this document use MiB in their CSV and runner (`test-1080-bigwindow.csv`, `run-1080.mjs`). The checks therefore use 1,048,576 bytes per unit. The acceptance numbers themselves are unchanged.
- The safe zstd crate has no `EndDirective` export. The code aliases `zstd::zstd_safe::zstd_sys::ZSTD_EndDirective` and passes its flush/end variants to the safe streaming API. It uses no experimental API and no unsafe block.
- Corpus lines omit `t_ms`; the probe computes it from `instant` and `meta.json`'s `instant_ms`. The mixed input was located at `C:\Users\Adam\clip-corpus\mix-1920x1080`, confirmed by issue #290 comment 1355.
- A finished exact writer rejects later writes/resizes; repeated `finish` returns the existing summary. Group creation uses `create_new` so a previous recording cannot be overwritten.

The first complete run passed round trips, product-reader seeks, group format, idle, changed-row size, RAM, and mixed-clip size. All eight inputs have zero mismatching bytes, including `typing-506x333` and the 60 s mixed clip. Typing non-key records average 1.383% of a 1080p picture (4.532% at 506×333). The mixed clip occupies 2,912,626 bytes (2.778 MiB), below 3.564 MiB. Maximum measured writer RAM was 160,432,128 bytes (153.000 MiB), below 155.1 MiB.

The remaining failure is scroll size: **10.1841716766 MiB/min versus the required 9.4**. Its group files alone contain 3,517,163 bytes, or 10.0626840591 MiB/min, so index overhead does not explain the failure. A fresh run of the existing whole-frame `exp-z` baseline with level 1, window log 27, no long-distance matching, and groups of 20 gives **9.4372158051 MiB/min**, zero mismatches, and a 20-record seek bound (`results/dyn-scroll-baseline.json`). That agrees with the rounded historical 9.4 figure. Removing unchanged rows changes the byte sequence presented to zstd; a smaller uncompressed input does not guarantee a smaller compressed output. The sentence “Changed rows can only be smaller” is therefore not established by this implementation's measured output.

No compression level, window, group length, row-band format, or acceptance threshold was changed to get green. Section 7's instruction “If a check cannot pass, stop and report” blocks progression to step 2. Options for the architect/owner are: (1) retain the specified format and revise the scroll budget using these measurements; (2) retain 9.4 and revise the exact coding format/parameters, then repeat step 1; (3) defer DYN1 and keep the existing recorder. Recommendation: option 1 if the measured 8.34% increase over the stated scroll limit is acceptable, otherwise option 2. This is a WIP, not a completed step, and there is no green result file.

Commit is independently blocked by the session's filesystem permissions: `git add` cannot create `C:/Users/Adam/Synapse/.git/worktrees/Synapse-clip/index.lock` (`Permission denied`). This checkout's Git metadata lies outside the writable `C:/Users/Adam/Synapse-clip` root; approval is unavailable in this run. No commit or push was made. The changes remain in the checkout, and `REVIEW.md` remains uncommitted as requested.

Verification: workspace `cargo fmt --all -- --check` and `cargo clippy -j 4 --offline -- -D warnings` passed. The probe entry point and all new Rust modules pass formatting. Probe strict clippy reports 11 findings in existing `exp_corpus.rs`, `exp_meter.rs`, the old copied `clip_encoder.rs`, `exp_d.rs`, and `exp_z.rs`; none are in the added modules. The probe release build passed. The workspace `cargo build --release -j 4`, run after formatting and clean workspace clippy at Idle priority, failed at link time: `OrtGetApiBase` is undefined because ONNX Runtime is not linked. There is no ONNX Runtime library in this checkout's target directory; no unrelated build configuration was changed. Logs are in `scripts/clip-equation/results/dyn-*.txt`.

The final check through `run-spec.mjs dyn-exact` again gives seven passes and the same scroll-size failure (`dyn-exact-failed.txt`); per-scene metrics are in `dyn-exact-metrics.json`. No green file exists. Steps 2–9, dead-worker removal, server integration, recovery, viewer/D5, and `youtube-proof.mjs` are not implemented. Synapse's health check passed, but no live UX verification of DYN1 was possible because its recorder integration has not been built. The YouTube live test was not run.
