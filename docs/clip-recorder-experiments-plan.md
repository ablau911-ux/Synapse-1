# Clip recorder: experiment plan — equations for every quantity

Status 2026-11-22 16:51 IST. Branch `feature/clip-capture`. Owner: Adam. Work log: issue #290. Edited in place, no version numbers.

## Goal (owner, 2026-11-22)

"Mathematical equations for everything; an absolute and predictable result in everything; run all existing options; one organised table with every result of every experiment, including the equations."

End result wanted: **a small video file, of high quality, whose creation uses as little system resources (RAM + CPU) as possible.**

Fixed by the owner earlier, and not experiment factors: f = 20 fps; flashes of 50 ms or more are in the clip; at most 10 % processor (E10); at least 480 lines; lossless waiting frames (E11); no cursor in the picture (E9); quality before bytes.

## How "absolute and predictable" is reached

1. **Fixed input.** Each content scenario is captured once into a corpus of lossless frames with their instants. Every configuration encodes the **same** frames, so a difference in a result comes only from the configuration.
2. **Equations before numbers.** Every measured quantity has an equation below, with named symbols. The experiments measure the symbols. The equation then predicts every other combination.
3. **Validation.** About 20 % of the runs (one scenario and one duration per encoder) are held out of the fit and must be predicted. Target: bytes, CPU time and RAM within ±10 %, quality within ±0.005 SSIM. A quantity that misses the target is reported as "not predictable" with its spread; it is never fitted harder to pass.
4. **Repeats.** Every configuration runs 3 times. Results report the mean and the spread. Whether a hardware encoder gives byte-identical output for identical input is itself a result.

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
    q_min(E, px) = the lowest q with R = 1 on every text scenario, and every flash present in the decoded clip

Quality constraint: q ≥ q_min. "High quality" means R = 1 and every flash present; SSIM and PSNR are reported beside it.

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

## Factors: every existing option

The harness first enumerates every Media Foundation video encoder on this PC (`MFTEnumEx`, hardware and software, H.264, HEVC and AV1) and records which ones exist. Each existing one enters the matrix.

| # | Factor | Levels |
|---|---|---|
| F1 | Architecture | **D**: deferred, waiting frames zstd −3, worker encodes (built). **D0**: deferred with raw waiting frames (reference). **L2**: live encode in the service with the hardware encoder, no waiting files, no worker |
| F2 | Encoder E | every encoder found: e.g. Intel Quick Sync H.264, Microsoft software H.264, Quick Sync HEVC, Quick Sync AV1 |
| F3 | Rate control | quality mode q ∈ {30, 40, 50, 60, 70, 80, 90, 100}; plus the bitrate fallback β ∈ {0.05, 0.10, 0.20} bit per pixel per frame |
| F4 | GOP τ_f | 20, 40, 100, 200 encoded frames |
| F5 | Screen-content hint | on, off |
| F6 | Recorded size | 1920 × 1080; 1280 × 720; a 640 × 360 window at σ = 2 (1280 × 720 recorded); a 506 × 333 window at σ = 3 |
| F7 | Content c | typing, flash page, scroll, video-like canvas, still window, UI demo (menus, list selection, a dialog fade, a spinner next to a changing label) |
| F8 | Low-latency mode | on, off (where the encoder accepts it) |

Fixed: f = 20; B-frames 0; NV12 input; variable time stamps (only new frames are encoded).

Run design (the full product is 3 × 4 × 11 × 4 × 2 × 4 × 6 × 2 ≈ 25,000 runs, too many for one PC):
- **Grid A (full):** E × q × c × size at τ_f 20, hint on, latency off: about 4 × 8 × 6 × 4 = 768 encodes per architecture D.
- **Grid B (one factor at a time)** around the baseline (E = Quick Sync H.264, q = 70, τ_f 20, hint on, 1080p) for F3 β, F4, F5, F8, for every E and c.
- **Grid C (architecture):** D0, D and L2 at the baseline and at the chosen q of every E, for every c.
- Each configuration runs 3 times. Expected machine time: 2 to 3 hours. The runs are headless after the corpus capture.

## Procedure

1. **Corpus.**
   - Scenario pages under `scripts/clip-equation/` drive themselves by frame count with fixed seeds, never by input to other apps.
   - Each is captured once for 20 s in a Chrome `--app` window behind the owner's windows, with the occlusion flags. The window is closed by the same script.
   - Frames are stored losslessly (zstd) with their instants. Corpus size: about 0.5 to 2 GB.
2. **Encode.** One probe subprocess per run reads the corpus and encodes with the configuration. It uses a verbatim copy of the product's encoder path (as package 0b copied `Nv12Canvas::draw`), named in the results. The subprocess reports CPU and memory of itself.
3. **Live option L2.** Replayed from the corpus at the real 20 fps pace through the same GPU path as the observer: upload, convert on the GPU where available, then the hardware encoder. It measures the live process's CPU and memory.
4. **Decode and quality.** Decoding uses the product's decoder path. PSNR-Y and SSIM-Y are computed per frame against the corpus. Text readability uses Windows OCR on fixed text regions of the typing, scroll and UI demo scenarios. Each flash's presence is checked by a pixel test in the flash box.
5. **Fit and validate** the equations; report residuals and the held-out prediction errors.
6. **Results.**
   - `docs/clip-recorder-experiments-results.md`: one table with a row per configuration (mean and spread of the 3 repeats), every equation with its fitted coefficients, the validation table, and the Pareto front of bytes against quality against CPU against RAM.
   - Raw CSV beside it in `scripts/clip-equation/results/`.

## Choosing the configuration

1. **Hard constraints:** R = 1 and every flash present (quality); U ≤ 10 % on the weakest-machine estimate; no lost instants.
2. Among the configurations that pass: the **Pareto front** of bytes per minute, CPU time per minute and peak RAM.
3. **Recommendation:** the front point with the least CPU plus RAM whose bytes are within 10 % of the smallest passing file. The owner sees the whole front and decides. The product changes only after that decision.

## Out of scope here

The viewer's change analysis (D5) gets its own equation next. This plan covers the recording and encoding pipeline only.
