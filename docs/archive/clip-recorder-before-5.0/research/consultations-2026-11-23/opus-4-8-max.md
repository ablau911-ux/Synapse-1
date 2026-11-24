**Model: Claude Opus 4.8.** Grounded in plan 4.2, the two research docs, and the e1/ocr-noise JSONs.

**1. Best way? No — the proposal over-reaches while plan 4.2 under-reaches, and both miss the sweet spot your own data already names.** `our-measurements` concludes (L17, L404): lossless **change-only** zstd (prefix/XOR/rect) is **7–12× smaller than whole frames**, still R=1, low CPU, 2.8 MB. Plan 4.2 discarded that for whole frames — which is exactly why you get 142 scroll / 6,668 video MB/min. The proposal then piles a two-codec RDP-style hybrid on top. The right answer sits between: **change-only zstd, one codec, lossless.** The measured winner was "ZW12 prefix L3" (0.27–0.80 s, 2.8 MB).

**2. Simplest design, most benefit.** Keep plan 4.2 verbatim — single process, no worker, no MP4, lossless RGB, one frame-file per picture, viewer seeks one file — but write each frame as the **delta vs. the previous frame (the measured "prefix" span)** instead of the whole frame, and **skip identical frames**. That's a one-function change to the frame writer: no tiling, no hashing, no H.264, no muxing, no new container, no two-codec mixing. RAM stays ~12 MB + one prev-frame (~8 MB); fidelity stays pixel-exact (no fidelity metric needed — lossless by construction); you capture the 7–12× on UI/scroll. Strictly less code than the proposal, barely more than plan 4.2.

**3. Drop / change / risks.**
- **Drop** (from proposal): per-region spatial mixing, settle/refresh logic, tile-hash machinery, H.264 in the live path, OCR as a fidelity metric.
- **Change** (plan 4.2): whole-frame → change-only frame. Single highest-value edit.
- **Keep H.264 only as a later, optional *export-time* transcode** for clips that are genuinely mostly video — gated by data (#2 below), never in capture. A lossy encoder in the hot path costs the RAM (164 vs 12 MB) and CPU you're trying to save.
- **Risks:** (a) WGC per-window gives **no** dirty rects — `GetFrameDirtyRects` is DDA (full output), a different API; so you diff frames yourself or crop DDA. Resolve this explicitly — the proposal glosses it. (b) "Prefix" assumes contiguous change; cursor + a distant UI edit spans the whole frame, so dirty-rects/tiles may beat prefix there — measure. (c) True full-motion video stays ~6,668 MB/min under *any* lossless scheme; if real clips contain much video you *will* need lossy eventually — don't assume it away. (d) H.264 was dropped for failing "R=1," but your own `ocr-noise.json` shows even the **source** scores R 0.974–0.9998 under invisible ±1 noise — that bar is noise. Re-justify dropping H.264 as "lossless already gives R=1 for free," not "H.264 fails R=1" (and `e1-nv12.json` confirms NV12/4:2:0 alone drops 1080p typing to 0.965 — text genuinely wants lossless RGB).

**4. Measure next (quick, extends the existing runner).**
1. **Resume the paused Z run's change-only formats (prefix/XOR/rect) at 1920×1080** on the four scenes — it stopped at 6,195/48,330, covering only 506×333. Confirms the 7–12× holds at full res and gives real CPU/RAM *including* the diff. Everything rests on this one number.
2. **Dirty-fraction histogram on 5–10 real agent recordings** (offline-diff, plot changed-area % per frame) → how often the video pathology actually occurs → whether lossy is ever needed at all.
3. **WGC dirty-rect reality check** + 1080p frame-diff cost → decides prefix-vs-tiles and the true RAM budget.
4. Only if (2) shows much video: H.264 q-sweep, SSIM/PSNR on text crops, to pick export quality.

Net: ship change-only lossless zstd; let data, not architecture, decide if H.264 ever returns — at export only.
