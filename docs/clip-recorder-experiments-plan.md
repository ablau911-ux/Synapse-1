# Clip recorder: experiment plan — equations for every quantity

Status 2026-11-22 21:00 IST: active; two ways, Z (zstd alone) and D (zstd then H.264), every option of each, J decides (owner, about 20:57). Times of the owner's decisions are taken from the commits that recorded them. Branch `feature/clip-capture`. Owner: Adam. Work log: issue #290. Edited in place, no version numbers.

## Goal (owner, 2026-11-22)

"Mathematical equations for everything; an absolute and predictable result in everything; run all existing options; one organised table with every result of every experiment, including the equations."

End result wanted: **a small video file, of high quality, whose creation uses as little system resources (RAM + CPU) as possible.**

Fixed by the owner earlier, and not experiment factors: f = 20 fps; flashes of 50 ms or more are in the clip; at most 10 % processor (E10); at least 480 lines; lossless waiting frames (E11); no cursor in the picture (E9); quality before bytes.

## Method (owner, 2026-11-22 about 20:42): the target, every way to it, keep what reaches it, save the best equation

"I want to get to x + y. Find me all the ways to get there and log every way. Try all the ways you found and keep those that produce the outcome x + y. Save the equation that does it better than the others."

1. **Target (x + y).** Owner, 2026-11-22 about 20:47: "forget other variables, we are testing all options". Every way is judged by four numbers only: file size, quality, CPU and RAM. Quality is SSIM-Y, PSNR-Y and R, the share of text read identically. "High quality" is the bar that a way must pass:

       O = [R = 1]

   Flash presence, lost instants, the 10 % processor limit and the 480-line floor are not checks here. They are still measured where a run gives them, and reported beside the four numbers. Fixed and not tested: f = 20 fps, lossless waiting frames, no cursor in the picture.

2. **All the ways.** Every way to turn the captured frames into a stored clip that an agent can view is listed in `docs/clip-recorder-ways.md`, the ways register. A way is any architecture, coder, encoder, setting, or combination of them. Each row records:
   - the way and its source;
   - its equations for bytes, CPU and RAM;
   - whether it can run on this PC, and if not, why;
   - its result on the corpus;
   - kept or dropped, and the reason.

   A way that cannot run is still logged.
3. **Try all, keep those with O = true.** Every runnable way runs on the same corpus (steps 2 and 3). A way with O = false is dropped, and its row names the clause that failed.
4. **The best way.** Among the kept ways, the cost equation is

       J = bytes / bytes* + CPU / CPU* + RAM / RAM*

   where bytes*, CPU* and RAM* are the smallest values among the kept ways, per minute of clip, and RAM is the peak. The way with the least J is the best. Its fitted equations (bytes, CPU, RAM, quality) are saved in the results file and then in the building plan. The Pareto front is shown beside it, so the owner sees every trade-off and decides.

## How "absolute and predictable" is reached

1. **Fixed input.** Each content scenario is captured once into a corpus of lossless frames with their instants. Every configuration encodes the **same** frames, so a difference in a result comes only from the configuration.
2. **Equations before numbers.** Every measured quantity has an equation below, with named symbols. The experiments measure the symbols. The equation then predicts every other combination.
3. **Validation.** About 20 % of the runs (one scenario and one duration per encoder) are held out of the fit and must be predicted. Target: bytes, CPU time and RAM within ±10 %, quality within ±0.005 SSIM. A quantity that misses the target is reported as "not predictable" with its spread; it is never fitted harder to pass.
4. **Repeats.** Every configuration runs 3 times. Results report the mean and the spread. Whether a hardware encoder gives byte-identical output for identical input is itself a result.
5. **Every action has an equation with a known result** (owner, 2026-11-22 about 20:38). Before an action runs, its equation and the value it must give are written down (section "Acceptance"). The check script compares the outcome with that value. A quantity with no exact value yet, such as CPU time, gets a prior prediction from `docs/clip-recorder-equation.md` before its first run, and after the fit it gets the fitted prediction, which a held-out run must meet.

## Two ways compared: zstd alone (Z) and zstd then H.264 (D) (owner, 2026-11-22 about 20:53 and 20:57)

At about 20:53 the owner said: "We said zstd is the only encoder we need." Asked why the built design encodes H.264 after zstd, he was given the logged reasons:
1. The 10 % processor limit on the weakest machine. Writing frames with zstd −3 while recording is cheap (1.4 % measured). Encoding H.264 live would pass the limit, so a worker encodes afterwards within what is left (`docs/clip-recorder-equation.md`, t_ready; E3, E10).
2. Small files (R9). H.264 is lossy and much smaller on changing content: changing text measured 10 to 15 MB per minute (`docs/clip-recorder-equation.md`, section 6). zstd −3 at 1080p measured 8.6 KB per frame for typing, 25 KB for a flash and 123 KB for scrolling (E11), which is about 148 MB per minute for scrolling at 20 new frames per second.

He then decided (about 20:57) that both ways run on the same corpus and J decides:
- **Z: zstd alone.** The zstd frames are the clip. There is no worker. Every zstd option is tested (factors Z1 to Z7). The coder is lossless, so SSIM-Y = 1, PSNR-Y = ∞ and R = 1 for every option. σ = 1, because σ only protects text from a video encoder's colour loss.
- **D: zstd, then H.264.** Waiting frames are zstd −3 (E11), and a worker encodes H.264 afterwards. Every H.264 encoder found on this PC is tested (factors D1 to D5). σ as in the product.

Agents see a clip only through the viewer's PNG images, so no standard player is needed for either way. If Z is chosen, the D5 noise ν becomes 0, and the D5 thresholds are recomputed when the building plan is corrected.

## Equations

Symbols: w × h = window size in pixels, px = w · h; f = 20 fps; t = clip length; N_new = new frames, n_new = N_new / t; L = zstd level; m = frame mode; c = content; W = 2^windowLog; K_z = keyframe interval of the delta and stream modes, in new frames. Every frame is B, G, R: px · 3 bytes raw.

### 1. File size

    b_k     = ζ(L, m, c) · px · 3 + h                         bytes of new frame k; h = zstd frame header
    bytes(t) = Σ_k b_k + t · f · ℓ                            ℓ = bytes of one frames.jsonl line
    B_min   = 60 · n_new · (ζ · px · 3 + h) + 60 · f · ℓ      bytes per minute

ζ is measured for every L, m, c and size. Prediction across sizes: ζ(px) = ζ(px_0) · (px / px_0)^(η − 1), one η per mode, fitted, expected near 1. Exact check: bytes equals the file sizes on disk, and zstd is deterministic, so a repeat run gives the same bytes.

### 2. Quality

    decoded_k = frame_k for every k   ⇒   SSIM-Y = 1, PSNR-Y = ∞, R = 1

Checked as Σ |decoded − corpus| = 0 over every pixel of every frame.

### 3. Processor time (process CPU, user plus kernel, `GetProcessTimes`)

    C_comp = N_new · (c_0(m) + px · κ_c(L, m, c))              ns to compress a clip
    U      = n_new · (c_0 + px · κ_c) / (n_threads · 10^9)       share of the PC while recording
    C_dec  = N_dec · px · κ_d(L, m)                              ns for the viewer to decode N_dec frames
    N_dec(seek) = 1 for independent frames;  ≤ K_z for the delta and stream modes

### 4. Memory

    M_comp = M_0 + M_cctx(L, W) + B_c · px · 3                   B_c = frames held: 1 (whole), 2 (XOR, tiles, rectangles)
    M_dec  = M_0 + M_dctx(W) + B_d · px · 3
    M_cctx, M_dctx: zstd's own estimates (`ZSTD_estimateCCtxSize_usingCParams`, `ZSTD_estimateDCtxSize`)
    stream ways:   W_eff = min(2^windowLog, px · 3) when each frame is its own zstd frame with a known size (K_z = 1),
                   W_eff = 2^windowLog otherwise                                       (zstd's rule in ZSTD_resetCCtx_internal)
    nbWorkers > 0: M_cctx = ZSTD_sizeof_CCtx after the last frame                    (the estimates cover one thread only; the workspace is allocated at the first frame)
    viewer:        M_dec gains b_max, the largest clip file, which the viewer holds while it decodes

Corrections from the step 2a pilot (`65fe9549`), each checked by its own row in `spec/step2z.mjs`. CPU is read from the process cycle counter (user plus kernel, all threads), because `GetProcessTimes` advances in 15.6 ms ticks and read 0.5 to 0.8 of the true time.

Measured: peak private bytes and peak working set of the run's process (`GetProcessMemoryInfo`, every 50 ms).

### 5. Time until ready

    t_ready = t_close                                            closing the last file and clip.json; no worker

### 6. Run time of the experiment itself

    T = Σ_runs N_new · px · 3 / v(L, m) · repeats               v = compress speed in bytes per second

A pilot on one folder measures v for every level; T is computed and reported before the full run starts.

### 7. Way D (zstd, then H.264)

Symbols as above, plus: E = H.264 encoder, q = quality level, τ_f = GOP in encoded frames, P and K = bytes of a predicted frame and of a keyframe, (w, h) = σ · window.

    bytes_D(t) = t · (n_new · P + (n_new / τ_f) · (K − P))
    P(E, q, px, c) = p_E,c · px · exp(a_E · q),   K(E, q, px, c) = k_E,c · px · exp(b_E · q)
    Q_ssim(E, q, c) = 1 − s_E,c · exp(−g_E · q);  Q_psnr(E, q, c) = m_E,c + l_E · q;  R = share of text read identically (Windows OCR)
    q_min(E, px) = the lowest q with R = 1 on every text scenario
    CPU:  a_rt = n_new · px · c_comp(−3) + n_new · c_file   (live part)
          a_w  = n_new · px · (c_dec(−3) + c_conv) + n_new · c_enc(E, q, px),   c_enc = e0_E + e1_E · px · (1 + e2_E · q)
    RAM:  M_D = M_live + M_worker,   M_worker = M0_w + px · (3 + 1.5) · d + M_enc(E, px),   M_enc = m0_E + m1_E · px
    t_ready = max(t_clip, t_clip · a_w / (0.10 · n_threads · 10^9 − a_rt))

D passes O only with q ≥ q_min. Its J uses bytes_D, CPU = a_rt + a_w over the clip, and RAM = the peak of M_D.

## Factors: every option of both ways

| # | Factor | Levels |
|---|---|---|
| D1 | H.264 encoder E (way D) | every H.264 encoder found in `results/encoders.json`, counted unique by name and vendor (first run: Intel Quick Sync, NVIDIA, Microsoft software, Microsoft AVC DX12); each must activate |
| D2 | Quality q (way D) | 30, 40, 50, 60, 70, 80, 90, 100 |
| D3 | GOP τ_f (way D) | 20, 40, 100, 200 encoded frames |
| D4 | Screen-content hint (way D) | on, off |
| D5 | Bitrate fallback β (way D) | 0.05, 0.10, 0.20 bit per pixel per frame |
| Z1 | Level L | −1 to −7, −10, −20, −50, −100, and 1 to 22 (33 levels) |
| Z2 | Frame mode m | whole frame; XOR against the previous new frame; changed tiles of 32, 64 and 128 px (tile positions stored); the changed rectangle (bounding box of the change) |
| Z3 | Context | independent frames; one zstd stream across frames with windowLog 23, 24, 25 and 27 (a 1920 × 1080 frame is 6.2 MB, so windowLog ≥ 23); long-distance matching off and on |
| Z4 | Dictionary | none; trained on the first 20 new frames (110 KB and 1 MB) |
| Z5 | Layout | B, G, R interleaved; planar B, G, R; lossless YCoCg-R planar |
| Z6 | Threads | nbWorkers 0, 1, 2 |
| Z7 | Keyframe interval K_z (XOR and stream modes) | 1, 20, 100, whole clip |
| F6 | Window size | 1920 × 1080; 1280 × 720; 640 × 360; 506 × 333. Way Z: σ = 1. Way D: σ as in the product (2 for 640 × 360, 3 for 506 × 333) |
| F7 | Content c | typing, flash page, scroll, video-like canvas, still window, UI demo |

Fixed: f = 20; only new frames are stored; repeats are lines in `frames.jsonl`.

Run design (owner decision, 2026-11-22 about 20:47: every option at every level; the main factors in full, the others one at a time; the equations predict the rest and held-out runs prove it):
- **Grid A-Z (full):** Z1 × Z2 × F6 × F7 = 33 × 6 × 4 × 6 = 4,752 runs, with independent frames, no dictionary, interleaved layout, nbWorkers 0.
- **Grid A-D (full):** D1 × D2 × F6 × F7 = 4 × 8 × 4 × 6 = 768 encodes, at τ_f 20, hint on, quality mode.
- **Grid B (one at a time):** Z3, Z4, Z5, Z6 and Z7, each level, around the Z baseline (L = −3, whole frame); D3, D4 and D5, each level, around the D baseline (E = Intel Quick Sync, q = 70); for every F6 and F7.
- Bytes are deterministic, so each configuration's bytes come from 1 run. CPU and RAM come from 3 runs.
- T (equation 6) is computed from the pilot before the full run. The step 2a pilot (`65fe9549`) gave T = 34.8 h for grids A-Z and B with 3 repeats; levels 19 to 22 at 1920 × 1080 and 1280 × 720 alone took about 20 h. Run list v2 (21:40), chosen by equation 6:
  1. Coverage floor: every runnable way × every level × every content at 640 × 360, so every option runs at every level. The context ways ZW12 to ZW15 get the full level sweep, because they lead the pilot (scroll 1920 × 1080 at L −3: prefix + ldm 8.9 KB per frame, stream W27 10.2 KB, whole frame 124 KB).
  2. The other sizes are added cell by cell, cheapest predicted cell first, until T_total reaches 12 h.
  3. Cells left out are predicted by the size equations (η in equation 1, px-linear κ_c in equation 3) and marked "predicted".
  4. 20 % of the cells that run at more than one size are held out with a fixed seed, so step 4 proves the size prediction.
  5. The list and T_total are in `scripts/clip-equation/results/z-runlist.json`. Way D's full run and way Z's run one after the other, never at the same time, so neither disturbs the other's CPU figures.
  6. As built (`1cfd8ed2`, 21:42): the coverage floor alone is T = 16.05 h (levels 19 to 22 take 10.16 h of it), so at 12 h no larger cell fits and nothing can be held out. Decision: the budget is floor + 4 h, T_total ≈ 20 h, with the 1,242 cheapest larger cells, 20 % of them held out (mulberry32, seed 20261122). Reason: the owner's order of costs is tokens, then PC, then time; this choice needs no new code, keeps every option at every level, and proves the size prediction. Order: way D's full run first (≈ 5 h, results sooner), then way Z.
- Every runnable way of `docs/clip-recorder-ways.md` that is not one of these factors is added as a row of grid B.

## Measurement rules, the same for both ways (2026-11-22 22:20, from the harness review #295)

The way D full run was stopped at 22:18 after 425 of 2,808 runs, because the review found that Z and D were not measured the same way. These rules are binding for every run; a run that breaks one is not a result.

- **M1 Input.** Every way receives each new frame as B, G, R, A, the format Windows capture delivers. It is expanded from the corpus before the timer starts. Any conversion a way needs is part of its timed work: B, G, R, A to B, G, R for the zstd ways that drop alpha, and to NV12 for way D.
- **M2 CPU.** The process cycle counter delta covers all of a way's work for one clip, including one-time setup:
  - Z: conversion, compression, writing, context creation and dictionary training.
  - D: the live process (conversion, zstd −3, writing), plus the worker process (reading, decoding, σ scaling, NV12, encoding, writing).
  - Corpus decoding and verification are never counted.
- **M3 RAM.** Each measured process prepares its input, then takes M_0. Its RAM is the OS peak of private bytes (`PeakPagefileUsage`) after the work, minus M_0. Way D's live part and worker run as two separate processes, as in the product (E2), and RAM_D = M_live + M_worker (equation 7).
- **M4 Bytes.** Bytes count every file the product would keep for the clip:
  - Z: the frame files, `frames.jsonl` and `clip.json`.
  - D: the MP4 segments, `frames.jsonl` and `clip.json`. The waiting files are deleted at `ready`, so they are not counted.
  - Both ways write `frames.jsonl` in the same format.
- **M5 Settings.** A run records the requested and the read-back settings (q, rate mode, GOP, hint, ldm, window). If they differ, the run is recorded under its actual settings with status `setting_refused:<name>` and does not count as the requested configuration. Quality mode keeps the product's fallback β = `FALLBACK_BITS_PER_PIXEL_FRAME` (0.10).
- **M6 Frame mapping (D).** A decoded frame is matched to a corpus new frame by presentation time, within half an instant. A missing, extra or mistimed frame fails the run with status `frame_map`.
- **M7 Quality.** PSNR-Y is computed from the clip's total MSE (∞ when MSE = 0). SSIM-Y is the mean over frames. R is measured on fixed text regions per scenario, defined in `corpus/corpus.json`. If the source text read in a region is empty, R = n/a for that scenario and it is not used for O. Text and non-text rows pass O only at the q actually run.
- **M8 Ways as registered.** ldm is set explicitly on or off in every run. ZW9 is the register's list of merged changed-tile rectangles, with its merge gap. A single global bounding box is a separate way, ZW9b.
- **M9 Runner.** A run key has its defaults resolved the same way at planning, lookup and append, and duplicate keys are rejected. The run list (floor + 4 h, 20 % hold-out) is regenerated and checked before `--full`.
- **M10 Fit.**
  - Splits are made first, and coefficients come only from training rows. A configuration needs all of its repeats to succeed.
  - Failure counts are kept. FULL is printed only when the unique keys match both run plans.
  - Predicted cells appear with their uncertainty. The published equations come from the fitted models themselves.
  - D's P and K are fitted separately, and the prediction uses the GOP.
  - The best-way check is computed independently, with fixed test cases.

## Procedure

1. **Corpus.**
   - Scenario pages under `scripts/clip-equation/` drive themselves by frame count with fixed seeds, never by input to other apps.
   - Each is rendered once for 20 s by headless Chrome through the DevTools protocol: virtual time advances 50 ms per instant, and one screenshot is taken at each instant at the window size. No window appears on the owner's screen. A frame equal in every pixel to the previous one is a repeat. (Changed 2026-11-22 20:51: a visible Chrome `--app` window took the keyboard focus, against the owner's window rule, and its title bar made repeated captures differ in 50 of 160 new frames.)
   - The capture path's own costs (c_obs, c_read) come from package 0 and the live tests, not from this corpus.
   - Frames are stored losslessly (zstd −3) with their instants.
2. **Compress.** One probe subprocess per run reads a corpus folder, decompresses it to raw frames in memory before the timer starts, then compresses with the configuration and writes the clip to a scratch folder. It reports its own CPU time and peak memory, and the bytes.
   Way D: one probe subprocess per run plays the live part (zstd −3 of each new frame, timed apart) and then the worker: it decodes the waiting frames, scales by σ, converts to NV12 and encodes with a verbatim copy of the product's encoder path (`clip_encoder.rs`), named in the results. It reports the live and worker CPU and the peak memory of each part.
3. **Decompress and verify.** A second subprocess decodes the clip and reports the decode CPU per frame and the seek cost (N_dec for the worst frame). Way Z: it compares every pixel with the corpus. Way D: it computes PSNR-Y and SSIM-Y per frame against the corpus, R with Windows OCR on fixed text regions of the typing, scroll and UI demo scenarios, and the flash count by a pixel test in the flash box (reported).
4. **Fit and validate** the equations; report residuals and the held-out prediction errors.
5. **Results.**
   - `docs/clip-recorder-experiments-results.md`: one table with a row per configuration (bytes, CPU mean and spread, RAM mean and spread, decode CPU, seek cost), every equation with its fitted coefficients, the validation table, J for every row, and the Pareto front of bytes, CPU and RAM.
   - Raw CSV beside it in `scripts/clip-equation/results/`.

## Acceptance (BDD): every action, its equation and its known result

Owner, 2026-11-22: test scripts are allowed, TDD and BDD, and every action shows an equation whose result is known. Each step's scenarios live in `scripts/clip-equation/spec/<step>.feature`, checked by one runner, `scripts/clip-equation/spec/run-spec.mjs <step>`. The runner computes each equation's value from its inputs, compares it with the outcome, prints both, and exits non-zero on the first miss. The builder of a step writes the feature file and its checks first, shows them failing, then builds until they pass. No `tests/` or `benches/` path and no Rust test attributes, so the push gate stays green.

Symbols as in section "Equations"; t = 20 s, f = 20 fps.

**Step 1, corpus** (6 scenarios × 4 sizes = 24 folders):

| Action | Equation | Known result |
|---|---|---|
| list encoders | checked in step 2 for way D (activation of every H.264 encoder) | – |
| capture instants | N_inst = t · f | 400 per folder |
| lost instants | N_lost = N_inst − N_new − N_rep | 0 |
| frame size | bytes(frame) = w · h · 3 at the window size, σ = 1 | 1920 × 1080: 6,220,800; 1280 × 720: 2,764,800; 640 × 360: 691,200; 506 × 333: 505,494 |
| repeats | stored files = N_new; each repeat line names an earlier stored frame | files = N_new |
| still page | N_new(still) = 1 | 1 |
| flash page | flashes = t · 1 per s | 20, each in one or more new frames |
| repeatability | differing frames between two renders of typing at 1280 × 720 | 0 (seeded pages, virtual time, no window). The visible-window run of `d9d11c26` gave 50 of 160, all in the title bar |
| waiting bytes | D_wait = Σ zstd(L = −3) bytes of the stored frames | equals the folder's file sizes summed, to the byte |

**Step 2, compress** (per configuration):

| Action | Equation | Known result |
|---|---|---|
| frames compressed | N_comp = N_new of the corpus folder | exact |
| bytes | bytes = Σ output sizes | equals the clip's file sizes on disk, to the byte |
| baseline equals the corpus | whole frame, L = −3, independent, interleaved, with the corpus writer's call (streamed, 16 KB output drain): bytes = D_wait of the folder | exact, to the byte (same libzstd 1.5.7, same call) |
| one-shot call | the same frames with one `ZSTD_compress2` call per frame, as the product writes them | a known difference from D_wait caused by the block splits of the call (pilot: 49,650,183 against 49,677,690 B on scroll 1920 × 1080); reported, not a pass/fail |
| determinism | bytes of run 1 = run 2 = run 3 | identical |
| RAM | M_comp = M_0 + M_cctx(L, W) + B_c · px · 3 (equation 4) | within ±10 % of the measured peak private bytes |
| CPU | equation 3 with κ_c from the fit set | held-out rows within ±10 % |
| run time | T (equation 6) from the pilot | the full run's wall time within ±20 % |
| D: encoders | unique H.264 encoders that activate | 4 (Intel Quick Sync, NVIDIA, Microsoft software, Microsoft AVC DX12); one that does not activate is reported with its error |
| D: frames encoded | N_enc = N_new of the corpus folder | exact |
| D: keyframes | N_key = ⌈N_enc / τ_f⌉ | exact for τ_f ∈ {20, 40, 100, 200} |
| D: B-frames | count(B) | 0 |
| D: bytes | equation 7 | before the fit: the prior from `docs/clip-recorder-equation.md`, reported with its error; after the fit: held-out rows within ±10 % |
| D: CPU and RAM | equation 7 | prior reported with its error; after the fit, held-out rows within ±10 % |

**Step 3, decompress and verify:**

| Action | Equation | Known result |
|---|---|---|
| round trip | Σ \|decoded − corpus\| over every pixel | 0 for every configuration |
| seek cost | N_dec(worst frame) | 1 for independent frames; ≤ K_z for the delta and stream modes |
| decoder RAM | M_dec (equation 4) | within ±10 % of the measured peak private bytes |
| time until ready | Z: t_ready = t_close; D: equation 7 | Z: at most one instant, 50 ms; D: within ±10 % of equation 7 after the fit |
| D: meter self-check | SSIM-Y(x, x); PSNR-Y(x, x); R(x, x) | 1; ∞; 1 |
| D: known distortion | Y + d on every pixel, no clipping: MSE = d², PSNR = 20 · log10(255 / d) | d = 4: 36.0896 dB within 0.01 dB |
| D: quality curve | equation 7 | after the fit, held-out SSIM within ±0.005 |

**Step 4, equations:** every equation of this plan has its fitted coefficients and residuals. Each held-out row is predicted: bytes, CPU time and RAM within ±10 %. A quantity that misses is marked "not predictable" with its spread, and it is never refitted to pass.

**Step 5, results:** rows in `docs/clip-recorder-experiments-results.md` = grid A-Z (4,752) + grid A-D (768, with the H.264 encoders that activate) + grid B, counted from the run list; every row is in the CSV in `scripts/clip-equation/results/`; J is given for every row; every equation is listed with its coefficients beside the validation table and the Pareto front.

## Choosing the configuration

1. **Hard constraint:** O, that is R = 1 (section "Method").
2. Among the configurations that pass: the **Pareto front** of bytes per minute, CPU time per minute and peak RAM.
3. **Recommendation:** the kept way with the least J (section "Method"). The owner sees the whole front and decides. The product changes only after that decision.

## Correcting the building plan from the results

After the owner's decision, `docs/clip-recorder-plan.md` is edited in place:
- the architecture (D, L2 or Z) and the encoder and its settings that were chosen;
- every constant of the parameter map that the results set (q, τ_f, β, the hint, the waiting coder level), with the result row it comes from;
- the equations of this plan with their fitted coefficients replace the estimates in `docs/clip-recorder-equation.md`;
- the work packages that the change needs, then one verification round (Codex review, one release build, live tests) on the corrected product.

## Out of scope here

The viewer's change analysis (D5) has its own equation in the building plan, section "D5 as an equation". This plan covers the recording and encoding pipeline only.
