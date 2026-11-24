# Clip recorder: compression for the `waiting/` frames (research)

Written 2026-11-22 08:58 (system clock, JST). Research only: web sources plus reading
`crates/synapse-mcp/src/server/clip_encoder.rs`. Nothing was run on this PC.
Each number is marked **[S]** when a source measured it, or **[E]** when it is my estimate.

## 1. Load and budget (all [E], derived from the brief)

- Load: 1920 × 1080 × 8 new frames/s = 16.6 Mpx/s = 66.4 MB/s of BGRA.
- Budget: 10 % of total processor time on the weakest machine (i3, 2.2 GHz, 2 cores / 4 threads)
  is 0.4 thread-seconds each second. That is 24 ns per pixel for the whole pipeline.
- Rule of thumb used below: **% of total CPU = 0.415 × (ns per pixel on the i3)**.
- Speed scaling from a benchmark machine to the i3 (single thread, clock × IPC) [E]:
  i7-9700K at 4.9 GHz ÷ 2.5; i7-6700K at 4.0 GHz ÷ 2.0; Ryzen 5900HX (boost off) ÷ 1.9;
  Xeon E5-2690 at 3.0 GHz ÷ 1.5; Apple M1 ÷ 2.5.
- Today (raw): read-back 0.35 ns/px and write 1.5 ns/px on the dev PC [measured by the owner].
  On the i3 that is about 0.9 + 3.75 ns/px, so about **0.4 % + 1.6 % of total CPU** [E].
- Break-even: compression is worth it if it costs less than the write it saves,
  that is less than (1 − 1/ratio) × 1.5 ns/px on the dev PC. For a 10:1 ratio that is about 1.35 ns/px,
  which is about 3 GB/s of BGRA input. Only LZ-type coders on redundant data, or skipping
  unchanged pixels, run this fast.

## 2. Can the frames be stored as NV12 or in another form "exactly what the encoder gets"?

The converter is `Nv12Canvas::draw` in `clip_encoder.rs`. It reads only bytes 0..3 (B, G, R) of each pixel.
Chroma is computed from the RGB **average over each 2 × 2 block of the duplicated (output) picture**.

- **Alpha is never read.** Dropping it (BGRA 32 bpp to BGR 24 bpp) is exactly lossless relative to the encoder.
  The saving is 25 %, and no other format change is needed.
- **σ = 1** (no duplication, bound held, canvas = source, even size): convert-then-store gives the same result.
- **Even σ (2, 4, …):** each output 2 × 2 block lies inside one source pixel. Its chroma is therefore that pixel's own colour,
  so the encoder gets full-resolution source chroma. NV12 of the source averages four *different* source pixels.
  Duplicating that afterwards loses chroma detail. **The two orders are not equal.**
- **Odd σ ≥ 3:** output blocks cross source-pixel edges in a repeating pattern. They mix 1, 2 or 4 source pixels.
  This again differs from duplicating source NV12. **Not equal.**
- Conclusion: source-resolution NV12 is lossless only for σ = 1. It also couples the live side to the worker's canvas
  and to the above-bound nearest-pixel map. Storing NV12 at the output resolution costs σ² × 1.5 B per source pixel,
  which is larger than BGRA for σ ≥ 2. **Keep BGR(A) in `waiting/`, and compress it losslessly.**

## 3. Candidates

Ratio = raw ÷ compressed. "Silesia" = general text/binary corpus (a pessimistic proxy for screen pixels).
CPU = % of total processor time on the i3, at 1080p × 8 frames/s, for compress (live) + decompress (worker).

| Candidate | Ratio | Compress | Decompress | CPU on i3 (comp + dec) | Source |
|---|---|---|---|---|---|
| Raw (today) | 1 | write 1.5 ns/px dev | – | write ≈ 1.6 % [E] | owner measurement |
| LZ4 1.11 (C) | 2.11 Silesia [S] | 780 MB/s [S] | 5060 MB/s [S] | 5.3 + 0.8 % on Silesia [E]; screen content likely 3–10× faster [E] | https://github.com/lz4/lz4 (i7-9700K) |
| lz4_flex (Rust), unsafe / safe | 4.4 on 66 KB JSON [S] | 1615 / 1272 MiB/s JSON; 347 / 259 Dickens [S] | 5512 / 4540 MiB/s JSON [S] | ≈ 1.9 + 0.6 % (JSON proxy) [E] | https://github.com/PSeitz/lz4_flex (Ryzen 5900HX, boost off) |
| Snappy (snap crate) | 2.09 Silesia [S] | 529 MB/s [S] | 1370 MB/s [S] | ≈ 7.8 + 3.0 % [E]; slower than LZ4, same ratio | https://github.com/lz4/lz4 table |
| zstd 1.5.7 −1 | 2.90 Silesia [S] | 510 MB/s [S] | 1550 MB/s [S] | ≈ 8.1 + 2.7 % [E] | https://github.com/facebook/zstd (i7-9700K) |
| zstd 1.5.7 --fast=4 | 2.15 Silesia [S] | 665 MB/s [S] | 2050 MB/s [S] | ≈ 6.2 + 2.0 % [E] | same |
| zlib −1 / zlib-ng −1 | 2.74 / 2.25 Silesia [S] | 108 / 249 MB/s [S] | 385 / 577 MB/s [S] | ≈ 38 % / 17 % compress alone [E] | https://github.com/lz4/lz4 table |
| zlib-rs L1 | Silesia-small | ≈ 132 MB/s (114 ms / 15 MB) [S, derived] | ≈ 369 MB/s [S, derived] | ≈ 31 % compress alone [E] | https://trifectatech.org/blog/current-zlib-rs-performance/ |
| libdeflate L1 / flate2 (miniz_oxide) L1 | 1.09 photo bitmap [S] | 185 / 291 MiB/s photo [S] | – | ≈ 20–35 % [E] | https://docs.rs/crate/zenflate/0.3.6 |
| Deflate "turbo" on **screenshots** (zenflate) | 16 (6.2 % of raw) [S] | 2360 MiB/s [S] | – | ≈ 1.5 % [E]; AGPL-3.0, AI-written, "not all code manually reviewed" → **not usable** | https://docs.rs/crate/zenflate/0.3.6 |
| QOI (qoi.h), web screenshots | 12 (8.3 % of RGB) [S]; game shots 4.1 [S] | 310.6 MP/s [S] | 425 MP/s [S] | ≈ 2.7 + 2.0 % [E] | https://qoiformat.org/benchmark/ (i7-6700K) |
| qoi crate 0.4.1 (Rust) | same as QOI | 290 MP/s vs 225 for qoi.h (1.29×) [S] | 427 vs 283 MP/s [S] | ≈ 2.1 + 1.3 % [E]; last release 2022 | https://github.com/aldanor/qoi-rust (M1) |
| fpng 1-pass (C++) | ≈ QOI −3 % size [S] | 110 MP/s [S] | 162 MP/s [S] | ≈ 5.6 + 3.8 % [E]; no Rust crate | https://github.com/richgel999/fpng (Xeon E5-2690) |
| fpnge (C, AVX2) | not published | 180–800 MP/s [S] | (any PNG decoder) | ≈ 1–4 % [E]; needs AVX2 + C build | https://github.com/veluca93/fpnge (Threadripper 3970X) |
| png 0.18 + fdeflate "ultra-fast" (already a dependency) | not published | "competitive with QOI" (claim only) [S] | – | unknown; measure | https://github.com/image-rs/fdeflate |
| UtVideo / MagicYUV (intra-only lossless) | no screen numbers found | UtVideo 13–14 fps vs FFV1 ≈ 3 fps on one emulator dump [S]; MagicYUV "> 2× UtVideo" (anecdote) [S] | similar | needs FFmpeg/C or a paid SDK (MagicYUV $14+); no temporal gain | https://tasvideos.org/Forum/Posts/302184 , https://obsproject.com/forum/threads/magicyuv-vfw-and-lossless-recording-performance.62524 , https://www.magicyuv.com/ |
| OBS "Lossless" mode | = UtVideo in AVI (user reports) [S] | – | – | as UtVideo | https://obsproject.com/forum/threads/lossless-quality-tremendously-large-file-size-problems-recording.148308/ |
| FFV1 / x264 qp 0 ultrafast | better ratio than UtVideo | ≈ 3 fps / 11–13 fps in the same dump test [S] | slow | far above budget on 2 cores [E]; x264 is GPL | https://tasvideos.org/Forum/Posts/302184 |
| QOV ("Quite OK Video") | – | – | – | niche spec (v3, Feb 2026), no Rust crate, no benchmarks | https://qov-sp.onrender.com/qov-specification.md |
| HW lossless H.264/HEVC (Quick Sync via MF) | – | – | – | **not available.** The MF H.264 encoder offers Baseline/Main/High only, and its QP range is 16–51. Intel staff: "Completely lossless encoding is not supported". | https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder , https://community.intel.com/t5/Media-Intel-oneAPI-Video/Lossless-H264/td-p/960063 (now 404, quoted in search) |
| RDP planar codec | – | no numbers | – | idea only: reversible colour planes + RLE; alpha plane dropped when opaque | https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpegdi/9b422f69-8e05-4c6d-b6fb-fa02ef75a8f2 |
| **Changed-tile delta + lz4_flex** | typical desktop 20–100× [E]; full change (scroll) ≈ 8–15× [E]; video in the window ≈ 1–1.3× [E] | compare ≈ 0.7 ns/px on i3 + LZ4 only on changed tiles [E] | LZ4 on changed tiles + tile copy [E] | typical ≈ 0.5 %; worst (all tiles change) ≈ 2.6 % [E] | estimate built from the rows above |

Notes on the table:

- Screen pixels compress much faster than Silesia with any LZ77 coder. The same deflate coder runs 2360 MiB/s on screenshots
  against 225 MiB/s on photos, which is 10× faster [S, zenflate].
- WGC dirty regions (`GraphicsCaptureSession.DirtyRegionMode`) exist only from Windows 11 24H2 (winrt-26100) [S]
  (https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.dirtyregionmode).
  Windows 10 1903 does not have them, so a CPU compare is needed. Use dirty regions only as an optional hint.

## 4. Recommendation: changed-tile delta + LZ4 (`lz4_flex` 0.14.2)

**Format of one waiting file.** Header (frame index, size, keyframe flag, tile size) + a bit per tile
(changed / same as the previous frame) + one LZ4 block holding the changed tiles' BGRA rows.

**Keyframes.** All tiles are present in the first frame, after any resize, after any write error,
and every N frames (for example N = 16, about 2 s at 8 frames/s).

**Crash safety.** Write `*.tmp`, then rename. Use the `lz4_flex` frame format with content checksum (xxhash32),
or a CRC in the header. If the worker finds a missing or bad file, it drops frames until the next keyframe.
A crash loses at most N frames and never corrupts a later frame.

Reasons:

1. Screen recordings mostly change little between frames. Skipping unchanged tiles removes most bytes
   *and* most compression work. No intra-only coder (QOI, PNG, UtVideo, MagicYUV, FFV1) can do this.
2. LZ4 is the fastest general coder with published numbers [S]. Its decoder is 4–6 GB/s [S], so the worker cost is negligible.
   `lz4_flex` is pure Rust, safe by default, actively maintained (0.14.2, updated 2026-11-15), and matches or beats C LZ4 [S].
3. It is lossless on the bytes the encoder reads, for every σ. The worker keeps one canvas, rebuilds the frame,
   and the existing σ duplication + `Nv12Canvas::draw` run unchanged.
4. Cheaper alternatives in the table fail on cost (deflate, zstd, Snappy: 6–38 % of the i3 on Silesia),
   on licence or build (zenflate AGPL; fpng/fpnge C++/AVX2; MagicYUV paid; x264 GPL),
   or on availability (no hardware lossless through MF).

Expected effect [E, must be measured]:

- **Disk:** typical desktop use goes from 8 MB to roughly 80–400 KB per 1080p frame (20–100×).
  A full-screen scroll is about 8–15×. Photographic or video content inside the window barely compresses, about 1–1.3×.
- **Processor on the i3, 1080p × 8 frames/s:**
  - Tile compare (fold it into the read-back copy if possible): ≈ 0.3 %.
  - LZ4 on changed tiles: ≈ 0.2 % typical, ≈ 1.9 % when every tile changes.
  - Compressed write: ≈ 0.05 % typical.
  - Worker decode: ≈ 0.06–0.6 %.
  - Total for compression: **≈ 0.5 % typical, ≈ 2.6 % worst**, against today's raw write ≈ 1.6 %.
    That leaves most of the 10 % for NV12 and Media Foundation.
  - At 20 frames/s, scale by 2.5. The worst case is then ≈ 6.5 %, so the 20 frames/s worst case must be measured.

Fallback if measurement shows the ratio matters more than CPU: the same tile delta with zstd level 1.
`zstd-sys` 2.0.16 (zstd 1.5.7) is already in `Cargo.lock` through `librocksdb-sys`. Expect about 1.4× smaller files
for about 2.5× more compress CPU [S, Silesia ratio and speed].

## 5. What to measure on this PC before building

1. **Corpus.** Save 30 s of real raw `waiting/` frames for each of: typing in an editor, scrolling a web page,
   a video playing in the window, and a small window with σ = 2 and σ = 3.
2. **Single-thread release bench.** Use `scripts/clip-equation/bench.rs` style, measuring ns/px for compress and decompress
   and bytes per frame. Candidates: raw; `lz4_flex` whole frame; tile delta + `lz4_flex` (tile 32/64/128);
   tile delta + zstd −1 / --fast=4; `qoi` 0.4.1; `png` 0.18 `Compression::Fast`; BGR repack (drop alpha) before LZ4.
3. **Write cost.** Measure write cost with the compressed sizes, including create + rename per file.
4. **Bit-exact proof.** For every frame, the decoded B, G and R must equal the source. A hash of `Nv12Canvas` output from the decoded
   frames must equal the hash from the raw frames, for σ = 1, 2 and 3.
5. **Crash test.** Kill the live process during a write. The worker must skip to the next keyframe,
   and no damaged frame may reach the output.
6. **Weak-machine check.** Run the bench on an i3-class machine, or with the dev PC at a 2.2 GHz cap, to confirm the ÷2.5 scaling.
   Measure the share of changed tiles in normal use, because that share decides the real ratio and CPU.
