# Fable 5.2 logic review of the clip recorder at c00ff762 (2026-11-22)

Read-only headless review (claude -p --model claude-fable-5-2, Serena read tools and git only) of a detached snapshot at c00ff762. Prompt: every component against docs/clip-recorder-plan.md. Triage by claude is in issue #290.

**Verdict:** the recording path matches the plan and the lock around `clip.json` is sound, but the worker lifecycle and recovery have two ways to leave a clip stuck or to overwrite a finished one. Fix findings 1 and 2 before the proof cases.

Nothing was built or run; every finding is from reading the code at c00ff762. "Unverified" means the logic is in the code but how often it happens needs a measurement.

## Findings

**P1 — `clip_worker.rs:743` — the host trusts the worker's exit code instead of `clip.json`.**
- Scenario: `taskkill /F` or Task Manager ends the worker mid-segment (exit code 1, proof case 11), or the worker returns 1 because `worker.lock` could not be opened. `clip.json` still says `recording` or `encoding`, the queue is empty, and `finish` (`capture_clip.rs:1194`) never enqueues. The clip stays stuck until the next service start.
- Fix: after every worker exit, read the state; if it is still `recording` or `encoding`, go to `after_crash`. Also call `enqueue` from `finish` (it is already once-only).

**P2 (P1 if more than one server process runs) — `clip_worker.rs:646`, `main.rs:484` — recovery treats every `recording` clip as dead, in every stdio and http process.**
- Scenario: a second synapse-mcp process starts while the first records. It marks the live clip `encoding / service_restart`, its worker encodes with the clip closed, sets `ready` and empties `waiting/`. The first recorder's `finish` then overwrites `ready` with `encoding` (`capture_clip.rs:1269` guards only `failed`), and the later frames are gone.
- Fix: the recorder holds an exclusive `recorder.lock` for its life (same pattern as `worker.lock`); recovery skips a clip whose lock is held; `finish` changes the state only from `recording`.

**P2 — `clip_worker.rs:759` — one crash without a new committed segment fails the clip.**
- Scenario: the clip is `encoding`, the worker dies with an access violation during the first or a long segment. The clip becomes `failed` although every raw frame is on disk; B10 says the segment is encoded again.
- Fix: count no-progress crashes per clip in the host and fail on the second, as B4 does for the encoder.

**P2 — `clip_worker.rs:377` and `:302` — any I/O error in the worker is terminal (frequency unverified).**
- Scenario: Defender or the indexer holds `segment-0001.mp4.part` or `clip.json` for a moment, so the rename or the unlocked read fails. `run` returns an error, the clip becomes `failed`, and the live recording is stopped.
- Fix: one helper that retries rename and replace a few times with a short sleep; keep `failed` for the encoder's second failure.

**P2 — `clip_observer.rs:356` — a slot overflow drops the newest frame.**
- Scenario: the recorder thread stalls 200 ms or more (disk) during a burst, the last frame of the burst finds no slot, and the window then goes still. Windows sends nothing more, so every later instant repeats the older picture while only one instant is named lost.
- Fix: when no slot is free, put the new frame in the newest unread pinned slot and name that older instant lost.

**P2 — `capture_clip.rs:1145` — the cursor is sampled when the instant is handled, 50 to 100 ms or more after the frame was composed.**
- Scenario: Windows 10 1903 or 1909 (cursor drawn into the picture), mouse moving at 1000 px/s. The logged position is 50 to 100 px from the drawn cursor, so the 48 px mask misses it.
- Fix: read `GetCursorPos` in the observer handler and keep it with the frame.

**P2 — `clip_worker.rs:835` — B is `held` after recovery without any live figure.**
- Scenario: the service is killed while recording. `conditions.b` is empty, `report_b` starts from the default and writes `held` when the worker stayed under its cap.
- Fix: write `held` only when the live detail (`max_live_share_percent`) is present, otherwise `unverified`. This sits next to the known C gap; check that the queued fix covers B.

**P3 — `capture_clip.rs:1226` — the recorder writes G as `held` before any segment exists.**
- Scenario: a clip ends `failed` with a padded or above-bound committed segment; only `ready()` would have corrected G, so it still says `held`.
- Fix: the recorder writes `pending_worker`, as it does for B.

**P3 — `clip_folder.rs:355`, `clip_viewer.rs:261` — σ is computed per segment but not stored; the viewer uses the σ of frame zero.**
- Scenario: a 640 × 360 window (σ = 2) is maximized to 1080 lines (σ = 1). For the second segment the cursor mask, the caret width and the 75 % floor are computed with σ = 2.
- Fix: store `scale` in `SegmentCheckpoint` and read it per segment.

**P3 — `clip_worker.rs:320` — a recovered clip with no lines becomes `ready` with zero segments.**
- Scenario: the service dies during the first-frame wait (`clip.json` exists, `frames.jsonl` is empty). After restart the clip is `ready`, G is `held`, and the viewer answers "not committed yet, retry" forever.
- Fix: in `ready()`, no segments means `failed` with "no frame recorded".

**P3 — `clip_worker.rs:818` — the cap follows the previous second's live share (unverified).**
- Scenario: on the weakest machine the worker runs at its cap; the live share jumps from 2 % to 6 %, and that second sums to 14 %. B is honestly `not_held`, but it will rarely hold where the cap binds.
- Fix: cap = 10 % minus the largest live share of the last few seconds.

**P3 — `capture_clip.rs:709` — the live share comes from `GetThreadTimes` (unverified).**
- Scenario: Windows charges thread time per clock tick, so short timer-driven bursts (handler 60 per second, recorder 20 per second) can be under-counted. A real 6 % measured as 2 % gives the worker 8 %, and B says `held` at 14 %.
- Fix: compare once against `QueryThreadCycleTime`; switch if they differ.

**P3 — `capture_clip.rs:388` — nothing releases the recorder on a panic.**
- Scenario: a panic in the recorder thread leaves the "one recording" claim and state `recording`; every later `record` is refused until restart.
- Fix: a drop guard that releases the claim and clears the live share.

## Ownership (item 7)

- **Condition B has two merge functions:** `merge_live_b` and `report_b`, in two files, each guessing the other's state from status strings and the presence of the key `seconds_over_limit`. Put one merge on `Conditions` and make the status an enum.
- **Lost instants are merged in three places:** `finish` (by hand), `add_lost`, and `mark_service_restart` (plain push, which skips the C and T sync).
- **The worker's outcome has two sources of truth:** the exit code and `clip.json` (finding 1).
- **Duplicated rules:**
  - `ClipTarget` and `ClipSource` are the same enum.
  - `ObserverGrant` copies `ObserverSettings` field by field.
  - The Windows build check is in both `Recording::start` and `ClipObserver::start`.
  - The share formula is in `LiveMeter::tick` and `share_percent`.
  - The thread count is read in two places.
- **Dead variant:** `LossCause::PrivacyDiscard` is never produced.

## Plan section → status

| Section | Status |
|---|---|
| O1, O2, O4 | held |
| O3 | gap: overflow drops the newest frame |
| A1 to A7, A9 to A12 | held (A9 measurement unverified) |
| A8 | gap: cursor position is late |
| Ownership (C14) | gap: no enqueue at finish, no panic guard |
| B1 to B3, B5 to B8 | held |
| B4 | held for the encoder; other I/O errors are also terminal |
| B9 | gap: ready with zero segments; B after recovery |
| B10 | gap: stuck on exit code 1; no liveness check; one crash fails the clip (plus the known items) |
| C (calls and contracts) | held |
| D1 | gap: per-segment σ |
| D2, D3, D4, D6, D7 | held |
| D5 | not judged (in rework) |
| Conditions C, T | held, apart from the known recovery gap |
| Condition G | gap: `held` before the worker's verdict |
| Condition Q | held; never claims `held` (always `unverified` or `not_held`) |
| Condition B | gap: recovery; lag and tick accounting unverified |
| Shared `clip.json` | held: every write is under the lock with atomic replace; readers take no lock |
| Windows 10 1903 | gated settings held; cursor logging gap |

## What I read

- **In full:** the plan, the equation, `clip_observer.rs`, `capture_clip.rs`, `clip_folder.rs`, `clip_worker.rs`, `clip_encoder.rs`, `clip_decoder.rs`.
- **Viewer:** constants, `ViewedClip` and all of `ClipView` in full; `Analyzer` and `BlockMap` (D5) not read.
- **Wiring, sampled:** the `screenshot` dispatch and the two record validators in `m1_tools/mod.rs`, the contract entries in `tool_profiles.rs`, and the start-up part of `main.rs`.
- **Not read:** the windows-capture crate source and the Rust standard library's rename behaviour on Windows, which is why the I/O finding is marked unverified.
