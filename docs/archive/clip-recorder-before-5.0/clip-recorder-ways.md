# Clip recorder: the ways register (zstd only)

Status 2026-11-22 18:59 IST, written by claude. Research only: nothing was built or run for this file. Branch `feature/clip-capture`. Plan: `docs/clip-recorder-experiments-plan.md`, section "Method", step 2. Edited in place, no version numbers.

**Scope (owner, 2026-11-22, through the manager): "zstd is the only encoder we need."** The clip is the zstd-compressed frames themselves (ways ZW0 to ZW20). **One more architecture stays (owner's correction):** way D, zstd −3 waiting frames and then a worker that encodes H.264 with every H.264 Media Foundation encoder found (ways DW1 to DW4, section 3b). Every other coder and encoder is logged once in section 6, "Dropped by the owner: zstd only", without research. Way ids here are ZW and DW so they do not collide with the plan's factor names D1 to D5 and Z1 to Z7.

The target, as the plan states it: **O = [R = 1]**. Every way is judged by file size, quality, CPU and RAM; among the ways with O = true the least J = bytes / bytes* + CPU / CPU* + RAM / RAM* wins. Every zstd way is lossless, so **R = 1, SSIM-Y = 1 and PSNR-Y = ∞ are known before running**, and every way below meets O by construction. The runs decide only bytes, CPU and RAM, and they check the round trip Σ|decoded − corpus| = 0.

A **way** is one route from captured frames to a stored clip that the viewer turns into PNG images: what is compressed (whole frame, delta, tiles, rectangles), in which byte layout, in which zstd mode (independent frames, prefix, one stream, dictionary), at which level and with which threads. The columns "result on the corpus" and "kept or dropped" are filled by steps 2 to 4 of the plan.

## 1. Sources

A number that is a measurement cites one of these. **[calc]** = arithmetic on cited numbers. **[I]** = inference, not a source.

| Key | Source |
|---|---|
| [P0] | `scripts/clip-equation/results-package0.md`, package 0 (M1 to M9), measured on this PC |
| [P0b] | same file, "Package 0b: compression of the waiting frames" (720p windows, ns per pixel) |
| [P0b-1080] | same file, "Package 0b, full comparison at 1920 × 1080": bytes per frame for typing / flash / scroll / video, alpha dropped, and U_weak = live share on the weakest-machine **estimate** at 20 new frames per second |
| [CORPUS] | `scripts/clip-equation/results/corpus.md` (step 1, partial run of `d9d11c26`) |
| [EXP] | `docs/clip-recorder-experiments-plan.md` |
| [WCR] | `docs/clip-recorder-waiting-compression-research.md` |
| [LOCK] | `Cargo.lock` of the Synapse workspace: zstd 0.13.3, zstd-safe 7.2.4, zstd-sys 2.0.16+zstd.1.5.7 |
| [PROBE] | `scripts/clip-equation/probe/Cargo.toml`: zstd 0.14.4 with `zstdmt` (zstd 1.5.7 per [P0b]) |
| [ZSRC] | zstd v1.5.7 source: `lib/compress/clevels.h` (level table), `lib/compress/zstd_compress.c` (`ZSTD_sizeof_matchState`, `ZSTD_estimateCCtxSize_usingCCtxParams_internal`, `ZSTD_estimateCCtxSize_internal`), `lib/decompress/zstd_decompress.c` (`ZSTD_estimateDCtxSize`, `ZSTD_decodingBufferSize_internal`, `ZSTD_estimateDStreamSize`), `lib/zstd.h` (`ZSTD_c_windowLog`, `ZSTD_c_enableLongDistanceMatching`, `ZSTD_c_nbWorkers`, `ZSTD_CCtx_refPrefix`); https://github.com/facebook/zstd/tree/v1.5.7/lib |
| [ZRS] | docs.rs, read 2026-11-22: zstd-safe 7.2.4 `CCtx` and `DCtx` have `ref_prefix`, `load_dictionary`, `sizeof`; `CParameter` has `WindowLog`, `HashLog`, `ChainLog`, `Strategy`, `TargetLength`, `EnableLongDistanceMatching`, `LdmHashLog`, `NbWorkers`, `JobSize`; zstd 0.13.3 `stream::write::Encoder` has `window_log`, `long_distance_matching`, `multithread`; features `zdict_builder`, `experimental`, `zstdmt` exist in zstd and zstd-sys. The `ZSTD_estimate*` functions are not in the safe API; they are in zstd-sys with `experimental` |
| [ZSTD] | https://github.com/facebook/zstd README: dictionary training; "gains are mostly effective in the first few KB" |
| [ZPATCH] | https://github.com/facebook/zstd/wiki/Zstandard-as-a-patching-engine (`--patch-from`: a reference used as the match window) |
| [WEBP] | https://developers.google.com/speed/webp/docs/webp_lossless_bitstream_specification (reversible predictor and subtract-green transforms) |
| [YCOCG] | H. S. Malvar, G. J. Sullivan, "YCoCg-R: A Color Space with RGB Reversibility and Low Dynamic Range", JVT-I014, 2003 (not opened in this sweep) |
| [WGC] | https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession (`DirtyRegionMode`; from Windows 11 24H2 only, per [WCR]) |
| [NVCOMP] | https://docs.nvidia.com/cuda/nvcomp/ (zstd on a CUDA GPU) |

This PC [P0]: Intel Core i7-13620H, 16 threads, 32 GB; Intel UHD Graphics is the capture adapter; NVIDIA RTX 4050 Laptop GPU beside it.

## 2. Symbols and the zstd equations

Plan symbols: px = w · h, n_new, f_o, c_obs, c_read, S (slots), M0_live. Added:

- **L** = zstd level (negative = fast levels, down to `ZSTD_minCLevel()`; up to 22). **m** = the way's mode and layout.
- **z_m(L, c)** = compressed bytes ÷ (px · 3) for content c. **h_m** = bytes per frame outside the zstd frame (tile bitmap, rectangle list, header).
- **c_pre(m)**, **d_pre(m)** = ns per pixel of the pass before compress and after decompress (repack, XOR, tile compare, colour transform). Measured for the alpha-drop repack: 0.15 to 0.25 ns/px [P0b].
- **c_z(L, m, c)**, **d_z(L, m, c)** = zstd compress and decompress ns per source pixel.
- **c_file(b)** = 308.6 µs + 0.277 µs per KB of b, one file per frame: create, write, rename (fit over 176 points, [P0b] M14).
- **ν** = 1 when the way keeps the previous frame (delta, tiles, prefix, stream), else 0. **τ_k** = new frames between independent frames (keyframes). **δ** = share of pixels inside changed tiles or rectangles. **W** = windowLog.
- **U** = C / (60 · 16 · 10^9) here; **U_weak** ≈ 3.68 · C / (60 · 4 · 10^9), the weakest-machine estimate of [P0].

**Bytes per minute**

    B = 60 · n_new · (px · 3 · z_m(L, c) + h_m)
    with keyframes:  z_m = z_Δ + (z_key − z_Δ) / τ_k,   z_key = the whole-frame z at the same L

**CPU per minute: compress (recording) and decompress (viewer)**

    C_rec  = 60 · [f_o · c_obs + n_new · px · (c_read + c_pre(m) + c_z(L, m, c)) + n_new · c_file(B / (60 · n_new))]
    C_view = 60 · n_new · px · (d_z(L, m, c) + d_pre(m))            decoding the whole minute (change analysis)
    seek   = mean frames decoded to show one frame at a random instant: 1 for independent frames, (τ_k + 1) / 2 for delta, prefix and stream ways

**RAM (peak)**

    M_rec  = M0_live + S · px · 4 + (1 + ν) · px · 3 + CCtx(L, m)
    M_view = M0_view + (1 + ν) · px · 3 + DCtx(m)

**CCtx exactly** ([ZSRC], `ZSTD_estimateCCtxSize_usingCCtxParams_internal`; the run checks it against `CCtx::sizeof()` [ZRS]):

    CCtx = matchState + tokenSpace + tmpWorkSpace + blockStateSpace + ldmSpace + ldmSeqSpace + bufferSpace (+ sizeof(ZSTD_CCtx))
    matchState = 4 · (2^chainLog · [the strategy allocates a chain table] + 2^hashLog + 2^min(17, W) · [minMatch = 3])
                 + 2^hashLog · [row match finder: greedy, lazy, lazy2]  + optSpace · [strategy ≥ btopt] + slack
    windowSize = min(2^W, source size);  blockSize = min(128 KB, windowSize);  maxNbSeq = blockSize / (minMatch = 3 ? 3 : 4)
    tokenSpace = (32 + blockSize) + 8 · maxNbSeq + 3 · maxNbSeq        ≈ 491,552 B for minMatch ≥ 4, ≈ 611,694 B for minMatch = 3 [calc]
    bufferSpace = 0 for one-shot compression (whole, delta, tiles, prefix, dictionary)
                = (windowSize + blockSize) + (ZSTD_compressBound(blockSize) + 1) for one stream across frames
    ldmSpace, ldmSeqSpace = 0 unless long-distance matching is on (then `ZSTD_ldm_getTableSize`, exact by the API)
    cParams(L, px · 3) = the row of `clevels.h` for sources above 256 KB, reduced by `ZSTD_adjustCParams` when the source is smaller than 2^W

**DCtx exactly** ([ZSRC]):

    one-shot:  DCtx = ZSTD_estimateDCtxSize() = sizeof(ZSTD_DCtx)
    stream:    DCtx = sizeof(ZSTD_DCtx) + min(2^W, 128 KB) + [2^W + 2 · min(2^W, 128 KB) + 64]        (ZSTD_estimateDStreamSize)

**Level table at 1920 × 1080** (source px · 3 = 6,220,800 B, so ⌈log2⌉ = 23; [ZSRC] `clevels.h`, table bytes [calc] by the matchState formula; the row match finder for greedy to lazy2 is assumed on, as on x86-64 [I]):

| L | W | chainLog | hashLog | minMatch | strategy | hash + chain + tag tables |
|---|---|---|---|---|---|---|
| < 0 | 19 | 12 | 13 | 6 | fast (targetLength = −L) | 32 KB |
| 1 | 19 | 13 | 14 | 7 | fast | 64 KB |
| 2 | 20 | 15 | 16 | 6 | fast | 256 KB |
| 3 | 21 | 16 | 17 | 5 | dfast | 768 KB |
| 4 | 21 | 18 | 18 | 5 | dfast | 2 MB |
| 5, 6 | 21 | 18 | 19 | 5 | greedy, lazy | 2.5 MB |
| 7, 8 | 21 | 19 | 20 | 5 | lazy, lazy2 | 5 MB |
| 9 | 22 | 20 | 21 | 5 | lazy2 | 10 MB |
| 10, 11 | 22 | 21 | 22 | 5 | lazy2 | 20 MB |
| 12 | 22 | 22 | 23 | 5 | lazy2 | 40 MB |
| 13 | 22 | 22 | 22 | 5 | btlazy2 | 32 MB |
| 14 | 22 | 22 | 23 | 5 | btlazy2 | 48 MB |
| 15 | 22 | 23 | 23 | 5 | btlazy2 | 64 MB |
| 16 | 22 | 22 | 22 | 5 | btopt | 32 MB + optSpace |
| 17 | 23 | 23 | 22 | 4 | btopt | 48 MB + optSpace |
| 18 | 23 | 23 | 22 | 3 | btultra | 48.5 MB + optSpace |
| 19 | 23 | 24 | 22 | 3 | btultra2 | 80.5 MB + optSpace |
| 20 to 22 | 25 to 27 | 25 to 27 | 23 to 25 | 3 | btultra2 | reduced by `ZSTD_adjustCParams` to the 6.2 MB source; exact only by the API |

What this means for J [I]: at L ≤ 2 the zstd context is under 1 MB, far below the frame buffers (px · 3 = 6.2 MB per frame at 1080p, S · px · 4 for the slots), so RAM differs between ways mainly by ν (one more frame), by the stream window (2^W) and by levels of 4 and above.

### Priors measured before this register

- Front end: c_obs = 0.373 ms per delivered frame; c_read = 0.307 to 0.349 ns/px at 1080p; f_o = 72 per second here; n_new = 1.67 (caret), 8.0 (typing), 20.0 (every instant changes) [P0] M1 to M4.
- Corpus typing, whole-frame zstd −3, 8.00 new frames per second: 5,589 B per frame at 1280 × 720 and 6,101 B at 1920 × 1080 [CORPUS].
- Alpha dropped gives 8 to 25 % fewer bytes than alpha kept in every scenario at about the same cost [P0b].
- 720p tile coders, compress / decompress ns/px, zstd 1 tile 32: typing 0.31 / 0.06, flash 0.35 / 0.11, scroll-small 2.06 / 1.32; zstd −1 tile 32: typing 0.32 / 0.06, flash 0.35 / 0.10 [P0b] M11.
- Round trip: all 176 method × scenario runs of package 0b decoded equal to their source, 0 mismatches [P0b] M13.

## 3. The register: zstd ways

"Runnable": **yes** = the crate and its call exist ([LOCK], [PROBE], [ZRS]); **to confirm** = one named fact is unverified; **no** = cannot run here. Every way: R = 1, SSIM-Y = 1, round trip 0 (lossless). Measured cells give bytes per 1080p frame for typing / flash / scroll / video, then U_weak for the same four, from [P0b-1080].

| Id | Way | Source | Bytes (B) | CPU (C_rec, C_view) | RAM | Known before running | Runnable | Levels to try |
|---|---|---|---|---|---|---|---|---|
| ZW0 | Raw B, G, R per new frame (reference, no zstd) | [P0b-1080], [EXP] D0 | z = 1, h = 0 | c_z = d_z = 0 | ν = 0, CCtx = 0 | 6.2 MB; 9.8 / 8.6 / 8.9 / 9.9 %. 7.46 GB per minute at 20 new frames per second [calc] | yes | reference row only |
| ZW1 | Whole frame, independent zstd frames, B, G, R interleaved, alpha dropped | [P0b-1080], [CORPUS], [EXP] Z | z_whole(L, c) | c_pre = repack; seek 1 | ν = 0; one-shot CCtx(L) | L −7: 10.9 KB / 35.4 KB / 155.1 KB / 5.7 MB, 7.3 / 6.7 / 8.8 / 15.2 %. L −5: 9.9 KB / 32.4 KB / 146.0 KB / 5.6 MB, 7.6 / 6.8 / 8.6 / 15.7 %. L −3: 8.6 KB / 25.0 KB / 123.4 KB / 5.6 MB, 8.0 / 6.8 / 8.7 / 15.9 %. L −1: 8.0 KB / 23.8 KB / 118.0 KB / 5.6 MB, 7.4 / 6.9 / 8.7 / 18.0 %. L 1: 7.5 KB / 21.2 KB / 108.0 KB / 5.0 MB, 7.8 / 6.6 / 8.4 / 25.4 %. L 3: 7.3 KB / 20.7 KB / 102.4 KB / 4.7 MB, 7.7 / 7.2 / 8.9 / 71.6 %. L 6: 6.5 KB / 17.5 KB / 80.9 KB / 4.3 MB, 10.9 / 9.4 / 16.0 / 162.9 %. B at L −3 [calc]: typing 4.1, flash 30, scroll 148, video 6,720 MB per minute | yes | L ∈ {−50, −20, −10, −7, −5, −3, −1, 1, 2, 3, 4, 6, 9, 12, 15, 19, 22}; every c and size |
| ZW2 | Whole frame, B, G, R, A interleaved (alpha kept) | [P0b] | z with 4 bytes per pixel | no repack | as ZW1 + px | 8 to 25 % more bytes than ZW1 at about the same cost [P0b]: known worse than ZW1 | yes | L −3, 1 (to confirm the measured gap only) |
| ZW3 | Whole frame, planar: all B, then all G, then all R | [I] | z_planar(L, c) | c_pre = plane split; d_pre = merge | ν = 0; + px · 3 for the planar copy | not measured | yes | L −3, −1, 1, 3 |
| ZW4 | Whole frame, subtract green: R − G and B − G modulo 256, interleaved or planar | [WEBP] (subtract-green transform) | z_sg(L, c) | c_pre = 2 subtractions per pixel | as ZW1 or ZW3 | reversible, so lossless; not measured | yes | interleaved, planar × L −3, 1 |
| ZW5 | Whole frame, lossless YCoCg-R by lifting, modulo 256 so each plane stays 8 bits, planar | [YCOCG], [I] (the modulo form) | z_ycocg(L, c) | c_pre = 4 adds and 2 shifts per pixel | as ZW3 | each lifting step is invertible modulo 256, so lossless [I]; the round trip proves it | yes | L −3, −1, 1, 3 |
| ZW6 | Whole frame, spatial predictor before zstd: left or up difference modulo 256 per channel | [WEBP] (predictor transform), [I] | z_pred(L, c) | c_pre = 1 subtraction per byte | as ZW1 | lossless; not measured | yes | left, up × interleaved, planar × L −3, 1 |
| ZW7 | Delta against the previous frame: XOR, or subtraction modulo 256, whole frame, then zstd; keyframe every τ_k | [P0b-1080] (XOR measured) | z = z_Δ + (z_key − z_Δ) / τ_k | c_pre = 1 op per byte; seek (τ_k + 1) / 2 | ν = 1; one-shot CCtx(L) | XOR + zstd −1: 0.9 KB / 7.0 KB / 161.7 KB / 5.7 MB, 8.9 / 8.0 / 12.8 / 17.1 %. XOR + zstd 1: 0.9 KB / 6.9 KB / 140.1 KB / 4.1 MB, 9.6 / 8.5 / 12.6 / 24.9 %. Subtraction not measured | yes | XOR, SUB × L −5, −3, −1, 1 × τ_k 16, 20, 100 |
| ZW8 | Changed tiles: one bit per tile, the changed tiles' rows in one zstd frame; keyframe every τ_k | [P0b-1080], [P0b] | z = (bitmap + zstd(changed rows)) / (px · 3) | c_pre = tile compare on every pixel; zstd on δ · px; seek (τ_k + 1) / 2 | ν = 1 | zstd 1 tile 32: 1.0 KB / 4.5 KB / 108.9 KB / 4.8 MB, 5.6 / 5.3 / 11.3 / 26.0 %. zstd −1 tile 32: 1.1 KB / 4.9 KB / 118.7 KB / 5.5 MB, 5.8 / 5.5 / 10.6 / 23.1 %. Tiles 64 and 128 in [P0b-1080]; tile 32 gives the fewest bytes on every content except video [P0b] | yes | tile 32, 64, 128 × L −3, −1, 1, 3 × τ_k 16, 20, 100 |
| ZW9 | Changed rectangles: changed tiles merged into bounding rectangles, a list of (x, y, w, h), the rectangles' rows in one zstd frame | [I] from ZW8 | z = (h_rect + zstd(rows in rectangles)) / (px · 3) | as ZW8, zstd on the rectangles' area | ν = 1 | not measured; on typing the rectangle may hold unchanged pixels that tiles skip [I] | yes | merge gap 0, 1, 2 tiles × L −3, 1 |
| ZW10 | Dirty rectangles from Windows (`DirtyRegionMode`): read back and compress only those | [WGC], [WCR], [P0] (dirty regions reported supported by windows-capture 2.0.0 here) | as ZW9 with Windows' rectangles | C_rec = 60 · [f_o · c_obs + n_new · δ · px · (c_read + c_z) + n_new · c_file]: no compare, less read-back | ν = 1 (to rebuild full frames) | lossless only if the rectangles cover every change; check Σ\|rebuilt − full read-back\| = 0 | to confirm: the headless corpus has no dirty regions; needs a live capture | coder of ZW8 or ZW1 inside the rectangles |
| ZW11 | Change mask on the GPU (compute shader compares the slot with the previous frame), read back only changed tiles, then zstd | [I], [WCR] 4 | as ZW8 | c_read on δ · px; the compare moves to the GPU | ν = 1 on the GPU | not measured | to confirm: a new compute shader in the probe | tile 32, 64 |
| ZW12 | Previous frame as zstd prefix (`ZSTD_CCtx_refPrefix`, the library form of `--patch-from`); every frame its own zstd frame, decoded with the same prefix | [ZSRC] zstd.h ("similar in outcome as performing a diff and compressing it, but performs much faster"), [ZPATCH], [ZRS] (`CCtx::ref_prefix`, `DCtx::ref_prefix`) | z_prefix(L, c) | c_z includes indexing the prefix; seek (τ_k + 1) / 2 | ν = 1; CCtx(L) with W ≥ ⌈log2(2 · px · 3)⌉ | not measured. [I]: at low levels the hash table (2^13 to 2^17 entries) is far smaller than a 6.2 MB prefix, so many matches one frame back may be lost; ZW15 adds long-distance matching for that | yes | L −3, −1, 1, 3, 6 × τ_k 16, 20, 100 |
| ZW13 | One zstd stream across frames: each frame flushed (`ZSTD_e_flush`), the window holds earlier frames; a new zstd frame (keyframe) every τ_k frames | [ZSRC] (`ZSTD_c_windowLog`), [ZRS] (`Encoder::window_log`) | z_stream(L, W, c) | seek: decode from the last keyframe, (τ_k + 1) / 2 | ν = 1 (the window); CCtx with bufferSpace = 2^W + 128 KB + compressBound(128 KB) + 1; DCtx stream form with 2^W | not measured. W must reach one frame back: at 1080p W ≥ 23 (6,220,800 < 2^23); at 720p W ≥ 22 [calc]. Same hash-table caveat as ZW12 [I] | yes | W 23, 24, 25 × L −3, 1, 3 × τ_k 20, 100, 400 |
| ZW14 | ZW13 with long-distance matching | [ZSRC] zstd.h ("designed to improve compression ratio for large inputs, by finding large matches at long distance"; raises the default window to 128 MB unless set) | z_stream+ldm(L, W, c) | + LDM hashing per byte | + ldmSpace, ldmSeqSpace (exact by the API); W set explicitly to 23 to 25 so the window does not grow to 2^27 | not measured | yes | LdmHashLog default and W − 7 [I] × W 23, 24 × L −3, 1 |
| ZW15 | ZW12 with long-distance matching (zstd.h: refPrefix "is compatible with LDM") | [ZSRC] | z_prefix+ldm(L, c) | as ZW12 + LDM | as ZW12 + ldmSpace | not measured | yes | L −3, 1, 3 × τ_k 16, 100 |
| ZW16 | Trained dictionary: train on the first N new frames of a window (or of the session), store it once with the clip, compress every frame independently with it | [ZSTD], [ZRS] (`zdict_builder`, `CCtx::load_dictionary`) | B = 60 · n_new · px · 3 · z_dict + dictionary bytes / clip length | + training once (`ZDICT_trainFromBuffer`); seek 1 | + dictionary + CDict (`ZSTD_estimateCDictSize`, exact by the API) | [ZSTD]: gains mostly in the first few KB of an input, so a gain is expected only on frames of a few KB (typing at 1080p: 6 to 9 KB per frame) [I] | yes (feature `zdict_builder`) | dictionary 16, 64, 112 KB × N 20, 100 frames × L −3, 1, 3 |
| ZW17 | Multithreaded zstd (`nbWorkers`) | [P0b-1080], [ZSRC] (`ZSTD_c_nbWorkers`) | as ZW1 | total CPU ≥ one thread: zstd 1 × 2 threads 7.4 KB / 21.3 KB / 108.3 KB / 5.1 MB at 8.6 / 7.9 / 9.8 / 26.1 % against one thread's 7.8 / 6.6 / 8.4 / 25.4 % | + one CCtx and job buffers per worker (exact by `ZSTD_estimateCStreamSize_usingCCtxParams`) | measured: more total processor time than one thread [P0b-1080], so known worse on CPU; only wall time can gain | yes (`zstdmt`, [PROBE]) | nbWorkers 0, 1, 2, 4 × L 1, 3 |
| ZW18 | Per-frame switch: ZW8 or ZW7 when the changed share δ is below θ, ZW1 otherwise | [P0b-1080] (tiles and XOR smallest on typing and flash; whole frame cheaper on scroll) | B = 60 · n_new · px · 3 · [(1 − κ_θ) · z_tile + κ_θ · z_whole], κ_θ = share of frames with δ ≥ θ | C = ZW8's compare on every frame + (1 − κ_θ) · c_tile + κ_θ · c_whole | ν = 1 | not measured; bytes and CPU can be predicted from per-frame results of ZW1 and ZW8 before a run [I] | yes | θ 10, 25, 50 % × tile 32 × L −3, 1 |
| ZW19 | Combinations of a layout (ZW3 to ZW6) with a temporal mode (ZW7, ZW8, ZW12, ZW13) | [I] | product of the two z factors, measured not assumed | sum of the c_pre terms | larger ν and CCtx of the two | not measured | yes | the best layout of ZW3 to ZW6 with the best two temporal modes |
| ZW20 | zstd on the NVIDIA GPU with nvCOMP | [NVCOMP] | as ZW1 | c_z moves to the GPU; adds a cross-adapter copy | GPU memory | – | **no**: the capture adapter is the Intel GPU [P0]; needs CUDA and the CUDA toolkit; no crate | – |

Count of zstd ways: 21. **Yes 18** (ZW0 to ZW9, ZW12 to ZW19), **to confirm 2** (ZW10, ZW11), **no 1** (ZW20).

## 3b. Way D: zstd −3 waiting frames, then a worker encodes H.264 (owner's correction)

Architecture D of the plan (F1, built): the live side stores each new frame as whole-frame zstd −3 (ZW1 at L −3); a worker decodes it, converts it to NV12 with `Nv12Canvas::draw` and encodes it with one H.264 Media Foundation encoder. Lossy, so R = 1 is **not** known before running: the run finds q_min (plan equation 2), and only rows with q ≥ q_min meet O.

**Equations** (plan equations 1, 3 and 4):

    B = 60 · [n_new · P(E, q, px, c) + (n_new / τ_f) · (K(E, q, px, c) − P(E, q, px, c))]
    C = 60 · (a_rt + a_w)
        a_rt = f_o · c_obs + n_new · px · (c_read + c_z(−3, c)) + n_new · c_file        (the waiting frames: ZW1 at L −3)
        a_w  = n_new · px · (d_z(−3, c) + c_conv) + n_new · c_enc(E, q, px)
    M = M_live + M_worker,  M_live includes one-shot CCtx(−3);  M_worker = M0_worker + px · (4 + 3 + 1.5) · d + DCtx + m0_E + m1_E · px
    Disk while waiting: D_wait = t · n_new · z(−3, c) · px · 3 (not part of the clip)
    Known before running: N_enc = N_new, N_key = ⌈N_enc / τ_f⌉, 0 B-frames, waiting round trip 0.

**Options of way D** (the plan's D factors): quality q ∈ {30, 40, 50, 60, 70, 80, 90, 100} (`CODECAPI_AVEncCommonQuality`, quality mode); GOP τ_f ∈ {20, 40, 100, 200} encoded frames; screen hint off or `CODECAPI_AVScenarioInfo` = DisplayRemoting; bitrate fallback β ∈ {0.05, 0.10, 0.20} bit per pixel per frame; plus content F7 and size F6.

**Priors** ([P0] M8, `docs/clip-recorder-equation.md` section 6): c_enc of the Microsoft software encoder 4.66 to 4.87 ns/px on synthetic 720p, 7.0 ns/px (measured 4.8 to 7.4) on real screen clips; Quick Sync processor part 1.35 to 1.45 ns/px synthetic, 2.0 (1.2 to 2.3) real. c_conv 1.65 ns/px as built. Bytes at q 70: software 15.0 MB per minute on changing small text at 1252 × 890 (keyframe every 1 s), 9.0 (every 5 s); still window 2.5 and 0.5; Quick Sync 10.3 (changing text) and 3.7 (still), keyframe every 1 s. 100 synthetic 720p frames at GOP 20: software 1,048,732 B, Quick Sync 724,299 B (723,975 B with the hint). GOP 20, 0 B-frames, quality mode 70 and the hint were accepted and read back on both [P0] M8.

| Id | Encoder E | Source | Known before running | Runnable | Needs |
|---|---|---|---|---|---|
| DW1 | Intel Quick Sync Video H.264 Encoder MFT (VEN_8086; listed twice) | `scripts/clip-equation/results/encoders.json` (commit `1e156ca9`), [P0] M8 | the built path; settings accepted and read back [P0] M8 | yes | nothing new |
| DW2 | Microsoft software H.264 Encoder MFT | encoders.json, [P0] M8 | spends as much on a repeated frame as on a new one (8.2 against 8.0 ms at 1252 × 890, equation doc section 6), so repeats must never reach it; in D they do not | yes | nothing new |
| DW3 | NVIDIA H.264 Encoder MFT (VEN_10DE) | encoders.json, [EXP] step 1 (every encoder activates) | the worker reads frames from disk, so the encoder's GPU need not be the capture adapter [I]; no NVIDIA GPU on the weakest machine | yes | nothing new |
| DW4 | Microsoft AVC DX12 Encoder (VEN_1414; no CLSID, activated from its enumeration object) | encoders.json, [EXP] step 1 | activates; output not yet seen | to confirm: whether it takes system-memory NV12 samples without a D3D12 device manager | – |

Count of D ways: 4. **Yes 3**, **to confirm 1** (DW4).

### Factors shared by several ways

| Factor | Levels | Applies to | Equation it enters |
|---|---|---|---|
| Level L | −50, −20, −10, −7, −5, −3, −1, 1, 2, 3, 4, 6, 9, 12, 15, 19, 22 | every way | z(L), c_z(L), d_z(L), CCtx(L) by the level table |
| Byte layout | interleaved BGR, BGRA, planar, subtract green, YCoCg-R, left / up predictor | ZW1 to ZW6, ZW19 | z, c_pre, d_pre |
| Keyframe interval τ_k | 16, 20, 100, 400 new frames | ZW7, ZW8, ZW9, ZW12 to ZW15, ZW18 | z = z_Δ + (z_key − z_Δ) / τ_k; seek (τ_k + 1) / 2 |
| windowLog W | 22 to 25 | ZW12 to ZW15 | CCtx bufferSpace 2^W + 128 KB + …; DCtx 2^W + … |
| Long-distance matching | off, on | ZW12 to ZW15 | ldmSpace, ldmSeqSpace |
| Dictionary size | 16, 64, 112 KB | ZW16 | B (dictionary stored once), CDict RAM |
| nbWorkers | 0, 1, 2, 4 | ZW1, ZW17 | CPU total, RAM per worker |
| Tile size, merge gap, θ | 32, 64, 128; 0 to 2 tiles; 10 to 50 % | ZW8, ZW9, ZW18 | h_m, δ, κ_θ |
| Content c, size | F7 scenarios; F6 sizes | every way | z(L, c), px |

## 4. Ways not runnable here

- **ZW20, zstd on the GPU with nvCOMP:** CUDA only. The capture adapter is the Intel GPU, so frames would cross to the NVIDIA GPU first, and the weakest machine has Intel graphics only.
- **ZW10, Windows dirty rectangles (to confirm):** the headless corpus has no capture metadata; it needs a live capture with `DirtyRegionMode` (Windows 11 24H2 and later).
- **ZW11, change mask on the GPU (to confirm):** needs a compute shader that does not exist yet.

## 5. How each way maps to the experiment grids

The plan's step 2 says every runnable way "enters the grids like an encoder". All zstd ways run in the **Z runner path**: the probe reads each corpus frame, applies the layout and the temporal mode, compresses, writes one file per frame, then decodes and compares (bytes exact, round trip 0, CPU of compress and decompress, `CCtx::sizeof()` and peak RAM).

| Runner path | Ways | Grid |
|---|---|---|
| Z, independent frames | ZW0, ZW1 to ZW6, ZW16, ZW17 | Grid C: the plan's zstd levels −3, 1, 3, 9 for every c and size, then the levels of section 3 one factor at a time (Grid B style) |
| Z, with the previous frame | ZW7, ZW8, ZW9, ZW12, ZW15, ZW18 | Grid C × τ_k; ZW18 first computed from the per-frame results of ZW1 and ZW8, then one confirming run |
| Z, one stream | ZW13, ZW14 | Grid C × W × τ_k |
| Z, combinations | ZW19 | after the single ways, with the best layout and the best two temporal modes |
| Live only | ZW10, ZW11 | a separate live capture after the corpus runs |

| D: decode the zstd −3 waiting frames, convert with the copied `Nv12Canvas::draw`, encode with the H.264 MFT | DW1 to DW4 | Grid A (E × q × c × size) and Grid B (τ_f, hint, β, one at a time) for every H.264 encoder that encodes; quality by the plan's step 3 to find q_min |

The plan's live path L2 and every non-H.264 encoder no longer apply under the owner's scope.

## 6. Dropped by the owner: zstd only

Logged, not researched further, not run.

| Id | Way |
|---|---|
| X1 | H.264 encoded live in the service (architecture L2, no waiting files); H.264 stays only in way D (section 3b) |
| X2 | HEVC through Media Foundation: Intel, NVIDIA, HEVCVideoExtensionEncoder, Microsoft HEVC DX12 |
| X3 | AV1 through Media Foundation: Intel, NVIDIA |
| X4 | HEVC screen-content coding (palette, intra block copy) on Quick Sync |
| X5 | Live encode with GPU colour conversion (Video Processor MFT) or RGB input to the encoder |
| X6 | Ring of encoded packets in RAM (replay-buffer model) |
| X7 | x264 lossy and lossless (libx264, libx264rgb) |
| X8 | x265 lossy and lossless |
| X9 | NVENC lossless (H.264, HEVC) |
| X10 | AV1 software: libaom, SVT-AV1 with screen-content mode, rav1e |
| X11 | VP9 lossless and screen tuning |
| X12 | VVC (H.266) |
| X13 | FFV1 |
| X14 | UT Video, MagicYUV |
| X15 | ZMBV, Flash Screen Video |
| X16 | Motion JPEG |
| X17 | D3D12 video encode through ffmpeg |
| X18 | JPEG XL lossless |
| X19 | WebP lossless and animated WebP |
| X20 | PNG, APNG, fpng / fpnge |
| X21 | QOI |
| X22 | GIF |
| X23 | lz4 and lz4 HC (whole frame, tiles, XOR) and the adaptive raw fallback |
| X24 | Snappy |
| X25 | Deflate (miniz_oxide, zlib-rs), zenflate |
| X26 | RDP planar codec, QOV |
| X27 | OpenH264, AMD AMF, VAAPI |
| X28 | Windows AppRecording / Game Bar recording |
| X29 | Hybrid of zstd frames with a video encoder for video-like frames |

## 7. Most promising zstd ways for J (before any run)

All zstd ways meet O = [R = 1], so J alone decides. The bytes term dominates J: the measured spread of bytes between zstd ways is up to about 10 times on text content, while RAM differs mainly by one frame buffer.

1. **ZW8, changed tiles + zstd 1, tile 32.** Fewest bytes on flash (4.5 KB per 1080p frame) and near-fewest on typing (1.0 KB), lowest U_weak on text (5.3 to 5.6 %) [P0b-1080]. B = 60 · n_new · (px · 3 · z_tile + h_bitmap); ν = 1.
2. **ZW7, XOR delta + zstd −1 or 1.** Fewest bytes on typing (0.9 KB) [P0b-1080]; weaker on scroll (140 to 162 KB against ZW1's 108 to 123 KB). Subtraction instead of XOR is unmeasured.
3. **ZW18, switch between ZW8 and ZW1 by changed share.** Takes the smaller measured row per frame; predictable from ZW1 and ZW8 per-frame results before a run. B = 60 · n_new · px · 3 · [(1 − κ_θ) · z_tile + κ_θ · z_whole].
4. **ZW12 / ZW15, previous frame as prefix, with or without long-distance matching.** The one zstd-native way that could match delta bytes on text and beat XOR on scrolled content (rows shifted by 15 px per frame at 300 px/s); unmeasured, with the hash-table caveat above.
5. **ZW1, whole frame at L −3 to 1.** The simplest way: independent frames (seek 1, no previous frame in RAM), CCtx under 1 MB, 8.6 / 25.0 / 123.4 KB per 1080p frame at −3 [P0b-1080]. Layout transforms ZW3 to ZW6 may lower its z on every content; they are unmeasured.

Open for every zstd way: video-like content stays at 4.1 to 5.8 MB per 1080p frame with every measured zstd way [P0b-1080], 5 to 7 GB per minute at 20 new frames per second [calc].

Way D (DW1 to DW3) is the comparator for that case: its bytes stay small on changing content (Quick Sync at q 70: 10.3 MB per minute on changing small text at 1252 × 890, equation doc section 6), but R = 1 holds only from q_min, which the run finds, and its CPU and RAM add the worker (decode, conversion, encoder) to the zstd −3 live side.
