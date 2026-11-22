# Clip recorder: experiment plan — equations for every quantity

Status 2026-11-22 20:45 IST: active again on the owner's instruction (20:40), after all code fixes landed. Branch `feature/clip-capture`. Owner: Adam. Work log: issue #290. Edited in place, no version numbers.

## Goal (owner, 2026-11-22)

"Mathematical equations for everything; an absolute and predictable result in everything; run all existing options; one organised table with every result of every experiment, including the equations."

End result wanted: **a small video file, of high quality, whose creation uses as little system resources (RAM + CPU) as possible.**

Fixed by the owner earlier, and not experiment factors: f = 20 fps; flashes of 50 ms or more are in the clip; at most 10 % processor (E10); at least 480 lines; lossless waiting frames (E11); no cursor in the picture (E9); quality before bytes.

## Method (owner, 2026-11-22 21:08): the target, every way to it, keep what reaches it, save the best equation

"I want to get to x + y. Find me all the ways to get there and log every way. Try all the ways you found and keep those that produce the outcome x + y. Save the equation that does it better than the others."

1. **Target (x + y).** Owner, 2026-11-22 21:15: "forget other variables, we are testing all options". Every way is judged by four numbers only: file size, quality, CPU and RAM. Quality is SSIM-Y, PSNR-Y and R, the share of text read identically. "High quality" is the bar that a way must pass:

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
5. **Every action has an equation with a known result** (owner, 2026-11-22 20:58). Before an action runs, its equation and the value it must give are written down (section "Acceptance"). The check script compares the outcome with that value. A quantity with no exact value yet, such as CPU time, gets a prior prediction from `docs/clip-recorder-equation.md` before its first run, and after the fit it gets the fitted prediction, which a held-out run must meet.

## Equations

Symbols: w × h = recorded size in pixels (px = w · h); f = 20 fps; n_new = new frames per second; t = clip length; τ_f = GOP length in encoded frames; E = encoder; q = quality level; c = content scenario.

### 1. File size

    bytes(t) = t · r
    r = n_new · P(E, q, px, c) + (n_new / τ_f) · (K(E, q, px, c) − P(E, q, px, c))      [repeats are not encoded, ρ = 0]
    P(E, q, px, c) = p_E,c · px · exp(a_E · q)
    K(E, q, px, c) = k_E,c · px · exp(b_E · q)

Measured: p, k, a and b per encoder and content. Check: P and K scale with px (one exponent per E, fitted, expected near 1).

### 2. Quality

    Q_ssim(E, q, c) = 1 − s_E,c · exp(−g_E · q)            SSIM-Y of the decoded frames against the corpus
    Q_psnr(E, q, c) = m_E,c + l_E · q                       PSNR-Y in dB
    R(E, q, c) = text readability: share of text characters read identically from decoded and source frames (Windows OCR on the text regions)
    q_min(E, px) = the lowest q with R = 1 on every text scenario

Quality constraint: q ≥ q_min. "High quality" means R = 1; SSIM and PSNR are reported beside it.

### 3. Processor time

    Live part, per second:     a_rt = f_o · c_obs + n_new · px · (c_read + c_comp(L)) + n_new · c_file + n_new · px · c_write
    Worker, per second:        a_w  = n_new · px · (c_dec_w + c_conv) + n_new · c_enc(E, q, px)
    Live encode (option L2):   a_L  = f_o · c_obs + n_new · px · c_conv_live + n_new · c_enc(E, q, px)       [no waiting files]
    Processor share:           U = (a_rt + a_w) / (n_threads · 10^9 ns)        must be ≤ 0.10 on the weakest machine
    c_enc(E, q, px) = e0_E + e1_E · px · (1 + e2_E · q)                          ns per encoded frame

Measured as process CPU time (user plus kernel, `GetProcessTimes`) per stage, not wall time. That makes the figures robust against other load on the PC. The weakest machine is scaled as in equation section 6 and is marked as an estimate.

### 4. Memory

    M_live   = M0_live + S · px · 4 + B_w · px · 3                                 S = OBSERVE_SLOTS, B_w = write buffers
    M_worker = M0_worker + px · (4 + 3 + 1.5) · d + M_enc(E, px)                   d = frames held by the decoder and converter
    M_enc(E, px) = m0_E + m1_E · px
    M_total(peak) = M_live + M_worker                                               [option L2: M_live + M_enc, no worker]

Measured: the peak working set and peak private bytes of every process (`GetProcessMemoryInfo`), sampled every 50 ms.

### 5. Time until ready

    t_ready = max( t_clip ,  t_clip · a_w / (0.10 · n_threads · 10^9 − a_rt) )

Measured as wall time from the stop to `ready`; compared with the equation.

### 6. Disk

    D_wait(t) = t · n_new · z(L, c) · px · 3            z = compressed ratio of the waiting coder at level L (package 0b)
    D_clip(t) = bytes(t)
    Architecture Z:  bytes_Z(t) = t · n_new · z(L, c) · px · 3,   a_w = 0,   M_worker = 0,   the viewer decodes with c_dec_z(L) ns per pixel

## Factors: every existing option

The harness first enumerates every Media Foundation video encoder on this PC (`MFTEnumEx`, hardware and software, H.264, HEVC and AV1) and records which ones exist. Each existing one enters the matrix.

| # | Factor | Levels |
|---|---|---|
| F1 | Architecture | **D**: deferred, waiting frames zstd −3, worker encodes H.264/HEVC/AV1 (built). **D0**: deferred with raw waiting frames (reference). **L2**: live encode in the service with the hardware encoder, no waiting files, no worker. **Z**: no video encoder at all; the zstd frames themselves are the clip, at zstd levels −3, 1, 3 and 9 (lossless, so R = 1 and SSIM = 1). Only repeated frames are skipped, as today. Agents see the clip only through the viewer's PNG images, so a standard player is not required (owner question, 2026-11-22: "we said zstd, why H.264?") |
| F2 | Encoder E | every encoder found: e.g. Intel Quick Sync H.264, Microsoft software H.264, Quick Sync HEVC, Quick Sync AV1 |
| F3 | Rate control | quality mode q ∈ {30, 40, 50, 60, 70, 80, 90, 100}; plus the bitrate fallback β ∈ {0.05, 0.10, 0.20} bit per pixel per frame |
| F4 | GOP τ_f | 20, 40, 100, 200 encoded frames |
| F5 | Screen-content hint | on, off |
| F6 | Recorded size | 1920 × 1080; 1280 × 720; a 640 × 360 window at σ = 2 (1280 × 720 recorded); a 506 × 333 window at σ = 3 |
| F7 | Content c | typing, flash page, scroll, video-like canvas, still window, UI demo (menus, list selection, a dialog fade, a spinner next to a changing label) |
| F8 | Low-latency mode | on, off (where the encoder accepts it) |

Fixed: f = 20; B-frames 0; NV12 input; variable time stamps (only new frames are encoded).

Run design (owner decision, 2026-11-22 21:15: every option runs at every level; encoder × quality × content × size in full; the other options one at a time; the equations predict every other combination and held-out runs prove it). The full product of every option, about 67,000 runs or 90 hours, is not run. Every way of `docs/clip-recorder-ways.md` that can run here enters the grids like an encoder.
- **Grid A (full):** E × q × c × size at τ_f 20, hint on, latency off: about 4 × 8 × 6 × 4 = 768 encodes per architecture D.
- **Grid B (one factor at a time)** around the baseline (E = Quick Sync H.264, q = 70, τ_f 20, hint on, 1080p) for F3 β, F4, F5, F8, for every E and c.
- **Grid C (architecture):** D0, D and L2 at the baseline and at the chosen q of every E, for every c; Z at zstd levels −3, 1, 3 and 9 for every c and every size of F6.
- Each configuration runs 3 times. Expected machine time: 2 to 3 hours. The runs are headless after the corpus capture.

## Procedure

1. **Corpus.**
   - Scenario pages under `scripts/clip-equation/` drive themselves by frame count with fixed seeds, never by input to other apps.
   - Each is rendered once for 20 s by headless Chrome through the DevTools protocol: virtual time advances 50 ms per instant, and one screenshot is taken at each instant at the window size. No window appears on the owner's screen. A frame equal in every pixel to the previous one is a repeat. (Changed 2026-11-22 21:25: a visible Chrome `--app` window took the keyboard focus, against the owner's window rule, and its title bar made repeated captures differ in 50 of 160 new frames.)
   - The capture path's own costs (c_obs, c_read) come from package 0 and the live tests, not from this corpus.
   - Frames are stored losslessly (zstd) with their instants. Corpus size: about 0.5 to 2 GB.
2. **Encode.** One probe subprocess per run reads the corpus and encodes with the configuration. It uses a verbatim copy of the product's encoder path (as package 0b copied `Nv12Canvas::draw`), named in the results. The subprocess reports CPU and memory of itself.
3. **Live option L2.** Replayed from the corpus at the real 20 fps pace through the same GPU path as the observer: upload, convert on the GPU where available, then the hardware encoder. It measures the live process's CPU and memory.
4. **Decode and quality.** Decoding uses the product's decoder path. PSNR-Y and SSIM-Y are computed per frame against the corpus. Text readability uses Windows OCR on fixed text regions of the typing, scroll and UI demo scenarios. Each flash's presence is checked by a pixel test in the flash box.
5. **Fit and validate** the equations; report residuals and the held-out prediction errors.
6. **Results.**
   - `docs/clip-recorder-experiments-results.md`: one table with a row per configuration (mean and spread of the 3 repeats), every equation with its fitted coefficients, the validation table, and the Pareto front of bytes against quality against CPU against RAM.
   - Raw CSV beside it in `scripts/clip-equation/results/`.

## Acceptance (BDD): every action, its equation and its known result

Owner, 2026-11-22: test scripts are allowed, TDD and BDD, and every action shows an equation whose result is known. Each step's scenarios live in `scripts/clip-equation/spec/<step>.feature`, checked by one runner, `scripts/clip-equation/spec/run-spec.mjs <step>`. The runner computes each equation's value from its inputs, compares it with the outcome, prints both, and exits non-zero on the first miss. The builder of a step writes the feature file and its checks first, shows them failing, then builds until they pass. No `tests/` or `benches/` path and no Rust test attributes, so the push gate stays green.

Symbols as in section "Equations"; t = 20 s, f = 20 fps; w × h = recorded size = σ · window size.

**Step 1, corpus** (per capture folder, 6 scenarios × 4 sizes = 24 folders):

| Action | Equation | Known result |
|---|---|---|
| list encoders | unique encoders = entries with distinct (codec, name, vendor); each has a name and a hardware flag; each one activates (`IMFActivate::ActivateObject`) | 10 unique (first run, `1e156ca9`: 13 entries, Intel's 3 listed twice); every one activates. 3 have no CLSID (the 2 DX12 encoders and HEVCVideoExtensionEncoder); they are activated from their enumeration object |
| capture instants | N_inst = t · f | 400 per folder |
| lost instants | N_lost = N_inst − N_new − N_rep | 0 |
| frame size | stored at the window size: bytes(frame) = w_win · h_win · 3; σ is recorded and the encode step scales to σ · window, as the product's worker does | 1920 × 1080: 6,220,800; 1280 × 720: 2,764,800; 640 × 360: 691,200; 506 × 333: 505,494 |
| encoded size | (w, h) = σ · (w_win, h_win) | 640 × 360 at σ = 2: 1280 × 720; 506 × 333 at σ = 3: 1518 × 999 |
| repeats | stored files = N_new; each repeat line names an earlier stored frame | files = N_new |
| still page | N_new(still) = 1 | 1 |
| flash page | flashes = t · 1 per s | 20, each in one or more new frames |
| repeatability | differing frames between two renders of typing at 1280 × 720 | 0 (seeded pages, virtual time, no window). The visible-window run of `d9d11c26` gave 50 of 160, all in the title bar |
| waiting bytes | D_wait = Σ zstd(L = −3) bytes of the stored frames | equals the folder's file sizes summed, to the byte |

**Step 2, encoding** (per configuration, 3 runs):

| Action | Equation | Known result |
|---|---|---|
| frames encoded | N_enc = N_new of the corpus folder | exact |
| keyframes | N_key = ⌈N_enc / τ_f⌉ | exact for τ_f ∈ {20, 40, 100, 200} |
| B-frames | count(B) | 0 |
| file bytes, D and L2 | bytes = Σ P + N_key · (K − P) (equation 1) | before the fit: the prior from `docs/clip-recorder-equation.md`, reported with its error; after the fit: within ±10 % on held-out rows |
| file bytes, Z | bytes_Z(L) = Σ zstd(L) bytes of the N_new frames | exact, to the byte (zstd is deterministic for one level and version) |
| Z round trip | Σ \|decoded − corpus\| over all pixels | 0 |
| repeat identity, software encoder | bytes of run 1 = run 2 = run 3 | identical |
| CPU and RAM | equations 3 and 4 | prior reported with its error; after the fit within ±10 % on held-out rows |

**Step 3, quality:**

| Action | Equation | Known result |
|---|---|---|
| meter self-check | SSIM-Y(x, x); PSNR-Y(x, x); R(x, x) | 1; ∞; 1 |
| known distortion | Y + d on every pixel, no clipping: MSE = d², PSNR = 20 · log10(255 / d) | d = 4: 36.0896 dB within 0.01 dB |
| Z quality | SSIM-Y; R | 1; 1 (lossless) |
| quality curve | equation 2 | after the fit, held-out SSIM within ±0.005 |

**Step 4, equations:** every equation of this plan has its fitted coefficients and residuals. Each held-out row is predicted: bytes, CPU time and RAM within ±10 %, SSIM within ±0.005. A quantity that misses is marked "not predictable" with its spread, and it is never refitted to pass.

**Step 5, results:** rows in `docs/clip-recorder-experiments-results.md` = configurations run = (grid A + grid B + grid C) × scenarios, counted from `results/encoders.json`; every row is in the CSV in `scripts/clip-equation/results/`; every equation is listed with its coefficients beside the validation table and the Pareto front.

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
