# Clip recorder plan 4.2: way Z (zstd)

Created 2026-11-23 06:28 IST. This plan replaces `docs/clip-recorder-plan.md` for the build. Rules that still hold are kept short here and cite the old plan's section. Only what changes for way Z is spelled out.

## 1. Decision

- **2026-11-23 06:06, owner:** the clip is zstd (way Z). H.264 (way D) is dropped because no D configuration reached O = [R = 1]. In the full D run (2,808 runs) the best R per text folder was 0.9888–0.9996. Sources: `scripts/clip-equation/results/d-runs.csv`, commit `2ff3db5a`, #290.
- **2026-11-23 06:10, owner:** the format is the one the live recorder already writes, zstd level −3, one whole frame per file (old plan E11). These frames become the clip, and the H.264 worker leaves the clip path.
- **Follow-up:** the Z run resumes after the build. It stopped at 6,195 of 48,330 rows and covers 506×333 only. If it finds a better zstd format, the format is swapped then. No pluggable format layer is built now.

## 2. Architecture

    capture (WGC) → live recorder → waiting/frame-NNNNNNNN.zst (zstd −3, whole frame) → recording ends → clip folder → viewer

There is no worker, no MP4, no encode step. The old plan's sections A and B still hold for capture, frame pacing, the cursor sampled with the frame, `recorder.lock` liveness and panic → clip failed. The worker parts do not hold: worker start and cap, segments, sigma per segment, and judging worker exit by clip.json.

## 3. Clip folder

| file | rule |
|---|---|
| `frames/frame-NNNNNNNN.zst` | One zstd frame (level −3) per new picture, the same bytes the live recorder wrote to `waiting/`. |
| `frames.jsonl` | One line per instant (50 ms). Each line says whether a new frame starts at that instant and names the frame it shows. |
| `clip.json` | Way `ZW1`, w, h, the frame count, the condition C/G/B/T and the lost instants (`ClipJson::add_lost`, exactly once). |

**End of recording:** the live recorder closes the last frame, renames `waiting/` to `frames/` (one rename, with the same file-hold retry as the old plan) and writes `clip.json` complete.

    t_ready = t_close ≤ 50 ms    (no encode; experiments plan eq. 5, step2z "time until ready" passes on every row)

**Recovery (old B10):** a clip whose recorder died is promoted the same way. Its lost instants are named once and its condition is written. There is no encode to resume.

**Bytes (eq. 1, M4):**

    B = Σ frame files + frames.jsonl + clip.json

## 4. Quality

Lossless:

    Σ |decoded − source| = 0  ⇒  SSIM = 1, PSNR = ∞, R = 1

Every Z row of the run checks this (step2z "round trip"; 6,105 of 6,105 rows that ran).

## 5. Viewer

- **Reader:** reads `frames/*.zst` through `frames.jsonl`. Every frame is whole, so a seek decodes one frame: N_dec = 1 (step2z "seek cost").
- **D5 (old plan "### D5 as an equation"):** stays as written, with the encoder noise set to the lossless value, ν(δ) = 0 for every δ. The thresholds follow from the same lines (old plan lines 227–229):

      N_hi > 2 · ν(δ_hi) = 0     ⇒  N_hi = 1     (was 3)
      2 · ν(δ_lo) < N_lo ≤ 32    ⇒  N_lo = 1     (was 16)
      SAME ⇔ n_δhi ≤ 0 and n_δlo ≤ 0;  DIFF ⇔ n_δhi ≥ 1 or n_δlo ≥ 1

  `NOISE_HI` 1 → 0, `NOISE_LO` 7 → 0, `N_HI` 3 → 1, `N_LO` 16 → 1. With these values DIFF and SAME cover every block, so the "undecided" state can no longer occur. Its code path goes, and `ANALYSIS_VERSION` goes up by one.
- **Open item O1:** δ_lo = 32 was chosen above the H.264 noise. Without noise, a lower δ_lo would also catch a hover tint (contrast 26, old plan "Named limit"). Keep 32, or lower it? The owner decides.

## 6. Parameter map (changes only; the rest is the old plan's map)

| constant | value | source |
|---|---|---|
| zstd level of a frame | −3 | old plan E11; owner 06:10 |
| `NOISE_HI`, `NOISE_LO` | 0, 0 | §5 (ν = 0) |
| `N_HI`, `N_LO` | 1, 1 | §5, old plan lines 228–229 |
| H.264 q, τ_f, β, hint, worker cap, sigma | removed | §1 |

## 7. Equations (experiments plan, ZW1 L = −3)

| equation | status |
|---|---|
| Bytes: B as in §3 | exact (M4) |
| CPU: C = N_new · (c_0 + px · κ_c) | κ_c partial: 506×333 rows only; no ZW1 L = −3 row ran yet |
| RAM compress: M = M_0 + M_cctx(−3) + 1 · px · 3 | ±10 % on every one-thread row (step2z) |
| RAM view: M_dec = M_0 + M_dctx + px · 3 + b_max + m_d0 | m_d0 fitted on the pilot, partial |
| t_ready ≤ 50 ms | exact |

## 8. Work packages

| id | what | files | proof |
|---|---|---|---|
| W1 | End of recording promotes waiting/ to the clip (§3); no worker start | `server/capture_clip.rs`, `server/clip_folder.rs` | check script: clip folder has exactly the §3 files, B = Σ files, t_ready ≤ 50 ms; live recording |
| W2 | Recovery promotes waiting/ the same way (§3) | `server/clip_folder.rs` | check script: a killed recorder → clip with its lost instants named once |
| W3 | Viewer reads zstd frames; seek N_dec = 1 | `server/clip_decoder.rs`, `server/clip_viewer.rs` | check script: decoded = source for every frame; live view |
| W4 | D5 constants for ν = 0, undecided path removed, `ANALYSIS_VERSION` + 1 | `server/clip_viewer.rs`, `scripts/clip-equation/spec/d5.*` | d5 spec red → green; `d5-proof.mjs` live |
| W5 | Remove the dead H.264 code: worker, encoder, Media Foundation use, keyframe rule | `server/clip_worker.rs`, `server/clip_encoder.rs`, `desktop_worker.rs` (worker launch), Cargo features | cargo build and clippy clean; no reference left |

Code rules: one type owns one rule, no pass-through layers, named constants only.

## 9. Verification round

1. Codex review, read-only.
2. cargo fmt, clippy, then ONE release build (`-j 4`, Idle priority).
3. Live tests: one recording plus view, then `node scripts/clip-equation/live-test/d5-proof.mjs <release exe> <scratch dir>`.

## 10. Issues

- #292 (worker), #296 (Quick Sync time stamps) and #297 (extra keyframe) are H.264 only. Proposal: close them as superseded by way Z.
- #293 (D5) stays open: W4 and the live proof.

## 11. Open items for the owner

- O1: δ_lo, keep 32 or lower it (§5).
