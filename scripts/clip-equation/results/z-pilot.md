# Clip experiments, way Z: pilot and run time T

Pilot on `scroll-1920x1080` (N_new = 400, px = 2,073,600, 6,220,800 B per frame): ZW1 at all 33 levels, the baseline 3 times, and one run of every other runnable way at L = -3. Runner: `node scripts/clip-equation/runs/run-z.mjs --pilot`; raw rows: `results/z-pilot.csv`. Slow levels ran on the first frames only (`frames_cap`: 100 frames for L 7 to 12, 20 for L 13 to 18, 5 for L 19 to 22, 40 for the dictionary ways), because v is per byte.

Columns: bytes per frame (Σ file sizes on disk / frames); compress CPU per frame (process cycles, user plus kernel, every thread, at the measured cycle rate; corpus decode and file writes excluded); RAM = peak private bytes over M_0; M_cctx = zstd's own estimate; decode CPU per frame; decoder RAM = decode-phase peak private bytes over M_0; N_dec = frames decoded to show the worst frame; Σ|diff| = round trip over every pixel; t_ready = last frame compressed to clip.json closed; wall = the whole subprocess.

| way | layout/mode | options | L | rep | frames | B/frame | comp ms/frame | RAM MB | M_cctx MB | dec ms/frame | dec RAM MB | N_dec | Σ\|diff\| | t_ready ms | wall s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| ZW1 | bgr/whole |  | -1 | 1 | 400 | 118549 | 1.72 | 6.7 | 0.5 | 1.77 | 6.6 | 1 | 0 | 0.60 | 4.1 |
| ZW1 | bgr/whole |  | -2 | 1 | 400 | 123051 | 1.82 | 6.7 | 0.5 | 1.88 | 6.6 | 1 | 0 | 0.63 | 4.3 |
| ZW1 | bgr/whole |  | -3 | 1 | 400 | 124125 | 2.00 | 6.7 | 0.5 | 1.95 | 6.6 | 1 | 0 | 0.66 | 4.5 |
| ZW1 | bgr/whole |  | -4 | 1 | 400 | 127825 | 1.87 | 6.7 | 0.5 | 1.91 | 6.6 | 1 | 0 | 0.91 | 4.4 |
| ZW1 | bgr/whole |  | -5 | 1 | 400 | 147562 | 1.84 | 6.7 | 0.5 | 1.94 | 6.6 | 1 | 0 | 0.78 | 4.3 |
| ZW1 | bgr/whole |  | -6 | 1 | 400 | 148043 | 1.81 | 6.7 | 0.5 | 1.88 | 6.6 | 1 | 0 | 0.59 | 4.3 |
| ZW1 | bgr/whole |  | -7 | 1 | 400 | 156451 | 1.85 | 6.7 | 0.5 | 1.95 | 6.6 | 1 | 0 | 0.89 | 4.4 |
| ZW1 | bgr/whole |  | -10 | 1 | 400 | 217040 | 1.78 | 6.7 | 0.5 | 1.87 | 6.6 | 1 | 0 | 0.91 | 4.4 |
| ZW1 | bgr/whole |  | -20 | 1 | 400 | 286307 | 1.57 | 6.7 | 0.5 | 2.07 | 6.6 | 1 | 0 | 1.05 | 4.3 |
| ZW1 | bgr/whole |  | -50 | 1 | 400 | 442815 | 1.48 | 6.7 | 0.5 | 2.12 | 7.0 | 1 | 0 | 1.02 | 4.3 |
| ZW1 | bgr/whole |  | -100 | 1 | 400 | 634914 | 1.28 | 6.7 | 0.5 | 1.95 | 7.2 | 1 | 0 | 1.27 | 4.2 |
| ZW1 | bgr/whole |  | 1 | 1 | 400 | 107238 | 1.80 | 6.7 | 0.6 | 1.78 | 6.6 | 1 | 0 | 0.80 | 4.3 |
| ZW1 | bgr/whole |  | 2 | 1 | 400 | 115690 | 1.88 | 6.9 | 0.7 | 1.74 | 6.8 | 1 | 0 | 0.84 | 4.2 |
| ZW1 | bgr/whole |  | 3 | 1 | 400 | 101744 | 1.82 | 7.4 | 1.2 | 1.62 | 6.3 | 1 | 0 | 0.61 | 4.0 |
| ZW1 | bgr/whole |  | 4 | 1 | 400 | 101756 | 1.90 | 8.6 | 2.5 | 1.66 | 6.3 | 1 | 0 | 0.58 | 4.1 |
| ZW1 | bgr/whole |  | 5 | 1 | 400 | 79087 | 4.43 | 9.1 | 3.0 | 1.91 | 6.2 | 1 | 0 | 0.79 | 5.2 |
| ZW1 | bgr/whole |  | 6 | 1 | 400 | 78676 | 5.76 | 9.1 | 3.0 | 2.22 | 6.2 | 1 | 0 | 1.17 | 6.0 |
| ZW1 | bgr/whole |  | 7 | 1 | 100 | 75936 | 6.76 | 11.6 | 5.5 | 2.15 | 6.2 | 1 | 0 | 1.19 | 1.6 |
| ZW1 | bgr/whole |  | 8 | 1 | 100 | 70619 | 8.65 | 11.6 | 5.5 | 2.07 | 6.2 | 1 | 0 | 1.14 | 1.8 |
| ZW1 | bgr/whole |  | 9 | 1 | 100 | 68820 | 8.63 | 16.6 | 10.5 | 2.02 | 6.2 | 1 | 0 | 1.15 | 1.8 |
| ZW1 | bgr/whole |  | 10 | 1 | 100 | 64720 | 11.08 | 26.6 | 20.5 | 1.85 | 6.2 | 1 | 0 | 1.11 | 2.1 |
| ZW1 | bgr/whole |  | 11 | 1 | 100 | 61747 | 16.80 | 26.6 | 20.5 | 1.75 | 6.2 | 1 | 0 | 1.18 | 2.6 |
| ZW1 | bgr/whole |  | 12 | 1 | 100 | 61748 | 16.81 | 46.7 | 40.5 | 1.75 | 6.2 | 1 | 0 | 1.32 | 2.6 |
| ZW1 | bgr/whole |  | 13 | 1 | 20 | 59801 | 26.58 | 38.7 | 32.5 | 1.78 | 6.2 | 1 | 0 | 1.40 | 0.8 |
| ZW1 | bgr/whole |  | 14 | 1 | 20 | 54990 | 37.73 | 54.7 | 48.5 | 1.72 | 6.1 | 1 | 0 | 1.00 | 1.0 |
| ZW1 | bgr/whole |  | 15 | 1 | 20 | 52023 | 53.32 | 70.8 | 64.5 | 1.69 | 6.1 | 1 | 0 | 0.81 | 1.3 |
| ZW1 | bgr/whole |  | 16 | 1 | 20 | 51299 | 89.16 | 38.8 | 32.6 | 0.73 | 6.2 | 1 | 0 | 1.08 | 2.0 |
| ZW1 | bgr/whole |  | 17 | 1 | 20 | 48753 | 93.14 | 54.8 | 48.6 | 0.68 | 6.2 | 1 | 0 | 1.20 | 2.1 |
| ZW1 | bgr/whole |  | 18 | 1 | 20 | 48985 | 114.88 | 55.4 | 49.3 | 0.89 | 6.1 | 1 | 0 | 1.16 | 2.6 |
| ZW1 | bgr/whole |  | 19 | 1 | 5 | 44470 | 236.48 | 87.5 | 81.3 | 1.01 | 6.1 | 1 | 0 | 1.38 | 1.3 |
| ZW1 | bgr/whole |  | 20 | 1 | 5 | 44470 | 236.19 | 103.6 | 97.3 | 1.03 | 6.1 | 1 | 0 | 1.55 | 1.3 |
| ZW1 | bgr/whole |  | 21 | 1 | 5 | 43762 | 318.05 | 135.6 | 129.3 | 0.98 | 6.1 | 1 | 0 | 1.25 | 1.7 |
| ZW1 | bgr/whole |  | 22 | 1 | 5 | 43335 | 508.88 | 135.6 | 129.3 | 1.02 | 6.1 | 1 | 0 | 1.25 | 2.6 |
| ZW1 | bgr/whole |  | -3 | 2 | 400 | 124125 | 1.72 | 6.7 | 0.5 | 1.81 | 6.6 | 1 | 0 | 0.65 | 4.0 |
| ZW1 | bgr/whole |  | -3 | 3 | 400 | 124125 | 1.73 | 6.7 | 0.5 | 1.82 | 6.6 | 1 | 0 | 0.76 | 4.1 |
| ZW7 | bgr/xor | K_z 20 | -3 | 1 | 400 | 170559 | 2.86 | 12.6 | 0.5 | 1.99 | 12.5 | 20 | 0 | 0.62 | 4.7 |
| ZW8 | bgr/tiles | tile 32, K_z 20 | -3 | 1 | 400 | 128147 | 2.08 | 18.6 | 0.5 | 1.43 | 12.5 | 20 | 0 | 0.80 | 4.1 |
| ZW8 | bgr/tiles | tile 64, K_z 20 | -3 | 1 | 400 | 125360 | 2.24 | 18.5 | 0.5 | 1.58 | 12.5 | 20 | 0 | 0.68 | 4.3 |
| ZW8 | bgr/tiles | tile 128, K_z 20 | -3 | 1 | 400 | 123500 | 2.36 | 18.6 | 0.5 | 1.57 | 12.5 | 20 | 0 | 1.02 | 4.4 |
| ZW9 | bgr/rect | K_z 20 | -3 | 1 | 400 | 116909 | 2.92 | 18.6 | 0.5 | 1.25 | 12.5 | 20 | 0 | 0.91 | 4.3 |
| ZW0 | bgr/raw |  | -3 | 1 | 400 | 6220800 | 0.40 | 6.1 | 0.0 | 0.36 | 12.1 | 1 | 0 | 2.57 | 4.4 |
| ZW1 | bgr/whole | drain 16384 | -3 | 1 | 400 | 124194 | 1.96 | 7.4 | 0.5 | 1.81 | 6.3 | 1 | 0 | 0.64 | 4.1 |
| ZW2 | bgra/whole |  | -3 | 1 | 400 | 135063 | 2.89 | 16.6 | 0.5 | 2.41 | 14.5 | 1 | 0 | 0.77 | 4.9 |
| ZW3 | planar/whole |  | -3 | 1 | 400 | 223692 | 2.46 | 12.6 | 0.5 | 1.73 | 12.5 | 1 | 0 | 0.65 | 4.3 |
| ZW4 | sg/whole |  | -3 | 1 | 400 | 123834 | 2.69 | 12.6 | 0.5 | 2.78 | 12.5 | 1 | 0 | 0.60 | 5.0 |
| ZW4 | sgp/whole |  | -3 | 1 | 400 | 223659 | 2.46 | 12.6 | 0.5 | 1.80 | 12.5 | 1 | 0 | 0.64 | 4.4 |
| ZW5 | ycocg/whole |  | -3 | 1 | 400 | 226800 | 2.72 | 12.6 | 0.5 | 1.92 | 12.5 | 1 | 0 | 0.76 | 4.8 |
| ZW6 | predl/whole |  | -3 | 1 | 400 | 133254 | 2.34 | 12.6 | 0.5 | 4.80 | 12.5 | 1 | 0 | 0.70 | 5.8 |
| ZW6 | predu/whole |  | -3 | 1 | 400 | 132481 | 2.12 | 12.6 | 0.5 | 1.97 | 12.5 | 1 | 0 | 1.13 | 4.4 |
| ZW7 | bgr/xor | K_z 1 | -3 | 1 | 400 | 124125 | 2.04 | 12.6 | 0.5 | 1.92 | 6.6 | 1 | 0 | 0.73 | 4.3 |
| ZW7 | bgr/xor | K_z 100 | -3 | 1 | 400 | 172502 | 3.14 | 12.6 | 0.5 | 2.09 | 12.5 | 100 | 0 | 1.29 | 5.0 |
| ZW7 | bgr/xor | K_z all | -3 | 1 | 400 | 172836 | 2.90 | 12.6 | 0.5 | 2.04 | 12.5 | 400 | 0 | 1.35 | 4.8 |
| ZW7 | bgr/sub | K_z 20 | -3 | 1 | 400 | 240646 | 3.61 | 12.6 | 0.5 | 2.51 | 12.5 | 20 | 0 | 0.90 | 5.3 |
| ZW12 | bgr/prefix | W 24, K_z 1 | -3 | 1 | 400 | 116792 | 1.98 | 12.6 | 0.5 | 1.75 | 6.6 | 1 | 0 | 0.78 | 4.2 |
| ZW12 | bgr/prefix | W 24, K_z 20 | -3 | 1 | 400 | 116591 | 2.13 | 12.6 | 0.5 | 1.82 | 12.5 | 20 | 0 | 0.77 | 4.3 |
| ZW12 | bgr/prefix | W 24, K_z 100 | -3 | 1 | 400 | 116588 | 2.16 | 12.6 | 0.5 | 1.86 | 12.5 | 100 | 0 | 0.60 | 4.3 |
| ZW12 | bgr/prefix | W 24, K_z all | -3 | 1 | 400 | 116587 | 2.11 | 12.6 | 0.5 | 1.83 | 12.5 | 400 | 0 | 0.70 | 4.3 |
| ZW13 | bgr/stream | W 23, K_z 20 | -3 | 1 | 400 | 42609 | 1.53 | 14.9 | 8.8 | 1.29 | 14.7 | 20 | 0 | 0.60 | 3.9 |
| ZW13 | bgr/stream | W 24, K_z 20 | -3 | 1 | 400 | 31042 | 1.42 | 22.9 | 16.8 | 1.22 | 22.7 | 20 | 0 | 0.58 | 3.7 |
| ZW13 | bgr/stream | W 25, K_z 20 | -3 | 1 | 400 | 20694 | 1.26 | 38.9 | 32.8 | 1.03 | 38.7 | 20 | 0 | 0.57 | 3.6 |
| ZW13 | bgr/stream | W 27, K_z 20 | -3 | 1 | 400 | 10238 | 1.22 | 135.1 | 128.8 | 1.06 | 134.9 | 20 | 0 | 0.59 | 3.6 |
| ZW13 | bgr/stream | W 24, K_z 1 | -3 | 1 | 400 | 116792 | 1.82 | 12.8 | 16.8 | 1.80 | 6.3 | 1 | 0 | 0.68 | 4.2 |
| ZW13 | bgr/stream | W 24, K_z 100 | -3 | 1 | 400 | 30007 | 1.34 | 22.9 | 16.8 | 1.22 | 22.7 | 100 | 0 | 0.62 | 3.6 |
| ZW13 | bgr/stream | W 24, K_z all | -3 | 1 | 400 | 29707 | 1.41 | 22.9 | 16.8 | 1.20 | 22.7 | 400 | 0 | 0.75 | 3.8 |
| ZW14 | bgr/stream | W 24, ldm, K_z 20 | -3 | 1 | 400 | 25410 | 4.68 | 23.9 | 17.8 | 1.07 | 22.7 | 20 | 0 | 0.67 | 5.1 |
| ZW15 | bgr/prefix | W 24, ldm, K_z 20 | -3 | 1 | 400 | 8861 | 6.28 | 13.6 | 1.6 | 0.48 | 12.2 | 20 | 0 | 0.91 | 5.4 |
| ZW16 | bgr/whole | dict 110 KB | -3 | 1 | 40 | 127695 | 1.76 | 6.9 | 0.6 | 1.92 | 6.8 | 1 | 0 | 0.62 | 1.4 |
| ZW16 | bgr/whole | dict 1024 KB | -3 | 1 | 40 | 127689 | 1.91 | 6.8 | 0.6 | 1.97 | 6.9 | 1 | 0 | 1.15 | 1.0 |
| ZW17 | bgr/whole | workers 1 | -3 | 1 | 400 | 124423 | 2.41 | 16.8 | 0.5 | 1.89 | 6.7 | 1 | 0 | 0.76 | 4.3 |
| ZW17 | bgr/whole | workers 2 | -3 | 1 | 400 | 124423 | 2.34 | 21.4 | 0.5 | 1.87 | 6.7 | 1 | 0 | 0.64 | 4.0 |
| ZW18 | bgr/switch | tile 32, K_z 20, θ 10 % | -3 | 1 | 400 | 124126 | 2.24 | 18.6 | 0.5 | 1.87 | 12.5 | 1 | 0 | 0.65 | 4.4 |
| ZW18 | bgr/switch | tile 32, K_z 20, θ 25 % | -3 | 1 | 400 | 124126 | 2.30 | 18.6 | 0.5 | 1.84 | 12.5 | 1 | 0 | 0.67 | 4.4 |
| ZW18 | bgr/switch | tile 32, K_z 20, θ 50 % | -3 | 1 | 400 | 128148 | 2.16 | 18.6 | 0.5 | 1.45 | 12.5 | 20 | 0 | 0.67 | 4.1 |
| ZW19 | ycocg/xor | K_z 20 | -3 | 1 | 400 | 284911 | 3.87 | 18.5 | 0.5 | 2.55 | 18.5 | 20 | 0 | 1.23 | 5.5 |
| ZW19 | ycocg/tiles | tile 32, K_z 20 | -3 | 1 | 400 | 246260 | 3.44 | 24.5 | 0.5 | 1.69 | 18.5 | 20 | 0 | 0.96 | 4.9 |

Every pilot run finished.

Ways not run (docs/clip-recorder-ways.md section 4):
- ZW10: Windows dirty rectangles: the headless corpus has no dirty regions; needs a live capture.
- ZW11: GPU change mask: needs a new compute shader in the probe.
- ZW20: nvCOMP on the NVIDIA GPU: CUDA only, the capture adapter is the Intel GPU, no crate.

## Check output (`node scripts/clip-equation/spec/step2z.mjs results/z-pilot.csv`)

```
step2z: 75 rows from C:\Users\Adam\Synapse-clip-viewer\scripts\clip-equation\results\z-pilot.csv
PASS  frames compressed: N_comp = N_new of the folder  (75 rows pass, 0 miss)
PASS  bytes: bytes = Σ output sizes = Σ clip file sizes on disk  (75 rows pass, 0 miss)
FAIL  baseline equals the corpus: ZW1, L = -3, interleaved, independent: bytes = D_wait  (0 rows pass, 3 miss)
        miss: scroll-1920x1080 ZW1 L=-3 whole/bgr rep 1: known 49677690, measured 49650183
        miss: scroll-1920x1080 ZW1 L=-3 whole/bgr rep 2: known 49677690, measured 49650183
        miss: scroll-1920x1080 ZW1 L=-3 whole/bgr rep 3: known 49677690, measured 49650183
        evidence, drain=16384 (corpus writer's call) scroll-1920x1080 rep 1: D_wait 49677690, bytes 49677690, equal
PASS  determinism: bytes(run 1) = bytes(run 2) = bytes(run 3)  (1 rows pass, 0 miss)
FAIL  RAM while compressing: M_comp = M_0 + M_cctx + B_c · px · 3 within ±10 % of peak private bytes  (72 rows pass, 3 miss)
        miss: scroll-1920x1080 ZW13 L=-3 stream/bgr rep 1: known 31851033, measured 21463040
        miss: scroll-1920x1080 ZW17 L=-3 whole/bgr rep 1: known 14802968, measured 25694208
        miss: scroll-1920x1080 ZW17 L=-3 whole/bgr rep 1: known 14811160, measured 30498816
PENDING  CPU: C_comp = N_new · (c_0 + px · κ_c), held-out rows within ±10 %  (0 rows pass, 0 miss)
        needs κ_c fitted on the full run (step 4); the pilot reports C_comp per frame only
PENDING  run time: full-run wall time within ±20 % of T (equation 6)  (0 rows pass, 0 miss)
        needs the full run; z-pilot.md gives T
PASS  round trip: Σ |decoded − corpus| = 0  (75 rows pass, 0 miss)
PASS  seek cost: N_dec(worst) = 1 independent; <= K_z otherwise  (75 rows pass, 0 miss)
FAIL  decoder RAM: M_dec = M_0 + M_dctx + B_d · px · 3 + b_max within ±10 % of decode peak private bytes  (74 rows pass, 1 miss)
        miss: scroll-1920x1080 ZW13 L=-3 stream/bgr rep 1: known 31654686, measured 14618624
PASS  time until ready: t_ready = t_close <= 50 ms  (75 rows pass, 0 miss)
step2z: 6 pass, 3 fail, 2 pending
```

Exit code 1.

### Rows that miss, and their causes (pilot of 2026-11-22)

- **Baseline equals the corpus.** Same libzstd on both sides (zstd-sys 2.1.3 = zstd 1.5.7; Node `process.versions.zstd` = 1.5.7) and the same frame header (frame header descriptor 0x80: 4-byte content size, no single-segment flag, no checksum, no dictionary; the first 14 bytes are identical). The bodies differ: the corpus writer (`corpus/render.mjs`, `zlib.zstdCompressSync`) drains the output 16 KB at a time, which puts zstd in buffered mode, and its block splitting differs from the one-shot `ZSTD_compress2` of the product (`zstd::bulk::compress`). Proof: Node itself gives 1,782 B for typing-1280x720 frame 0 at chunkSize 16 KB and 64 KB, and 1,770 B at 1 MB and 8 MB, the same as exp-z one-shot; exp-z with `drain=16384` gives D_wait to the byte (49,677,690 B). One-shot gives 49,650,183 B, 27,507 B (0.055 %) fewer. The row stays a miss: the known result assumed the same call, not only the same coder.
- **RAM, ZW17 nbWorkers 1 and 2.** zstd's estimate functions cover single-threaded compression only (zstd.h: "Estimate CCtx size is supported for single-threaded compression only"), so M_cctx is the single-thread estimate (0.52 MB). `ZSTD_sizeof_CCtx` after the run is 10.5 MB (1 worker) and 15.1 MB (2 workers), and M_0 + that + B_c · px · 3 matches the measured peak within 1 %. No zstd estimate exists for this row.
- **RAM and decoder RAM, ZW13 with K_z = 1.** `ZSTD_estimateCStreamSize_usingCCtxParams` and `ZSTD_estimateDStreamSize` take no source size, so they count a 2^W window (16 MB at W = 24). With K_z = 1 every zstd frame ends in its first call, so zstd knows the 6.2 MB frame size and sizes the window to it (`ZSTD_sizeof_CCtx` 6.7 MB). Every other stream row matches its estimate exactly (`ZSTD_sizeof_CCtx` = the estimate to the byte for W 23, 24, 25 and 27).
- **Corrections made in this step, before the rerun** (first pilot, kept out of the CSV): long-distance matching crashed inside zstd's estimate with a division by zero (`STATUS_INTEGER_DIVIDE_BY_ZERO`) because unset ldm fields stay 0 there; the ldm parameters are now set explicitly on the context and in the estimate. B_c and B_d now count every buffer the way allocates up front (a B,G,R,A buffer is 4/3 of px · 3; the gathered tiles and the delta target are allocated at the start, so a reallocation can no longer double them for a moment). The compress peak is the operating system's exact `PeakPagefileUsage` when compressing sets a new peak (the 50 ms sampler missed short peaks). Equation 4 for the decoder gains b_max, the largest clip file, which the viewer holds while it decodes (ZW0 raw: 6.2 MB).
- **CPU.** `GetProcessTimes` advances in clock ticks of 15.6 ms; around one frame (about 2 ms at L = -3) it read 0 for whole runs and 0.5 to 0.8 of the true time over 400 frames. Compress and decode CPU are therefore the process cycle counter (`QueryProcessCycleTime`, user plus kernel, every thread) at the measured cycle rate; the `GetProcessTimes` sums are kept in the `*_gpt_ns` columns.

## Run time of the full run (equation 6)

    T = Σ_runs N_new · px · 3 / v(L, m) · repeats
    v(L, m) = v_ZW1(L) · v_m(-3) / v_ZW1(-3)        v = raw bytes per second of the whole run subprocess

Runs: 16776 (Grid A-Z 4,752 + Grid B 840 configurations, × 3 repeats), over every folder's own N_new and px.

**T = 34.8 h** (125160 s).

Assumptions: v per raw byte measured on scroll at 1920 × 1080 holds for every content and size; a way's cost ratio to ZW1 at L = -3 holds at every level (for tiles and rectangles zstd sees only the changed part, so at slow levels this overestimates their time). Video frames compress worst, so their v at slow levels is the largest unknown.


| L | v_ZW1(L), MB/s of raw frames |
|---|---|
| -100 | 591.0 |
| -50 | 577.7 |
| -20 | 584.2 |
| -10 | 570.2 |
| -7 | 564.4 |
| -6 | 584.4 |
| -5 | 577.9 |
| -4 | 570.6 |
| -3 | 621.8 |
| -2 | 584.7 |
| -1 | 612.0 |
| 1 | 576.8 |
| 2 | 598.7 |
| 3 | 629.6 |
| 4 | 606.2 |
| 5 | 477.1 |
| 6 | 417.1 |
| 7 | 378.6 |
| 8 | 337.4 |
| 9 | 337.9 |
| 10 | 297.1 |
| 11 | 234.7 |
| 12 | 235.2 |
| 13 | 159.3 |
| 14 | 126.1 |
| 15 | 96.8 |
| 16 | 62.4 |
| 17 | 59.3 |
| 18 | 48.8 |
| 19 | 24.2 |
| 20 | 24.1 |
| 21 | 18.4 |
| 22 | 11.7 |

### Slowest level and size cells (all modes, contents and repeats)

| cell | hours | share of T |
|---|---|---|
| L=22 1920x1080 | 5.01 | 14.4 % |
| L=21 1920x1080 | 3.20 | 9.2 % |
| L=20 1920x1080 | 2.44 | 7.0 % |
| L=19 1920x1080 | 2.43 | 7.0 % |
| L=22 1280x720 | 2.23 | 6.4 % |
| L=21 1280x720 | 1.42 | 4.1 % |
| L=18 1920x1080 | 1.21 | 3.5 % |
| L=20 1280x720 | 1.09 | 3.1 % |
| L=19 1280x720 | 1.08 | 3.1 % |
| L=17 1920x1080 | 0.99 | 2.9 % |
| L=16 1920x1080 | 0.94 | 2.7 % |
| L=-3 1920x1080 | 0.72 | 2.1 % |
| L=15 1920x1080 | 0.61 | 1.7 % |
| L=22 640x360 | 0.56 | 1.6 % |
| L=18 1280x720 | 0.54 | 1.5 % |
