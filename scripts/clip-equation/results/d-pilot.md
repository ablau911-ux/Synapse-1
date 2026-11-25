# Clip experiments step 2b, way D: pilot and run time T_D

Status 2026-11-22 21:45 IST: pilot done, full run not started (it waits for the owner). Spec: `docs/clip-recorder-experiments-plan.md` (equation 7, factors D1 to D5, procedure 2 and 3, acceptance rows marked D), ways DW1 to DW4 of `docs/clip-recorder-ways.md`. Probe `clip-probe exp-d` at commit `565ee2d1`; runner `scripts/clip-equation/runs/run-d.mjs grid=pilot`; rows in `results/d-pilot.csv`.

Pilot: `typing-1920x1080` and `scroll-1920x1080`, every H.264 encoder of `results/encoders.json` at q 30, 70 and 100, τ_f 20, hint on, quality mode, one repeat. CPU is the process cycle counter (user plus kernel, every thread; `exp_cpu.rs`, the same reading as way Z), converted at the rate of `clip-probe exp-rate`; the `GetProcessTimes` sum is shown as "gpt" beside it. RAM is the peak private bytes above the part's baseline, sampled every 50 ms.

## Encoders (D1): every one is a result

| Way | Encoder | Activates (HRESULT) | Clip | Result |
|---|---|---|---|---|
| DW1 | Intel Quick Sync Video H.264 Encoder MFT (VEN_8086) | yes (0x00000000) | ok | encoded through the product's Sink Writer path with a DXGI device manager on the Intel adapter |
| DW2 | H264 Encoder MFT, Microsoft software | yes (0x00000000) | ok | encoded with hardware transforms off; the software MFT reports no friendly name, so the run checks "hardware off and a software encoder read back" |
| DW3 | NVIDIA H.264 Encoder MFT (VEN_10DE) | no (0x8000ffff) | not_activated | `IMFActivate::ActivateObject` fails with 0x8000ffff (E_UNEXPECTED, "Catastrophic failure") on every run; no clip |
| DW4 | Microsoft AVC DX12 Encoder (VEN_1414) | yes (0x00000000) | no_product_path | no: SetOutputType 0x80041000. The "to confirm" of the ways register is answered: without a device manager it does not reach a system-memory NV12 frame, because `SetOutputType` already fails. The Sink Writer cannot select it, so the product's path has no clip for DW4 |

Encoders read back from the Sink Writer: DW1: hardware H.264 encoder (VEN_8086); DW2: unnamed software H.264 encoder.

Findings of the pilot, not yet explained: R stays below 1 at every q, also at q 100 (typing 0.959 to 0.994, scroll 0.995 to 0.997), so q_min is not reached on these two folders with whole-frame OCR; the software encoder's worker peaks near 700 MB against about 110 to 125 MB for Quick Sync; and 420 decoded frames carry a time more than half an instant away from their frame's instant (the frames are matched by decode order, which holds with no B-frames).

## Pilot table (per minute = × 3, the corpus clip is 20 s)

| folder | encoder | q | bytes per min (MB) | SSIM-Y mean / min | PSNR-Y mean / min (dB) | R | CPU live per min (ms) | CPU worker per min (ms) | RAM live / worker / decode (MB) | frames / keyframes (known) / B | hint / rate read back |
|---|---|---|---|---|---|---|---|---|---|---|---|
| typing-1920x1080 | DW1 | 30 | 0.15 | 0.9986 / 0.9983 | 49.12 / 47.47 | 0.933 | 869 (gpt 891) | 4569 (gpt 4219) | 19.8 / 107.9 / 216.4 | 143 / 9 (8) / 0 | 1 / quality 30 |
| typing-1920x1080 | DW1 | 70 | 0.29 | 0.9997 / 0.9997 | 58.59 / 57.27 | 0.977 | 872 (gpt 984) | 4551 (gpt 4313) | 19.3 / 121.8 / 215.9 | 143 / 9 (8) / 0 | 1 / quality 70 |
| typing-1920x1080 | DW1 | 100 | 0.36 | 1.0000 / 1.0000 | 66.01 / 62.66 | 0.971 | 864 (gpt 1031) | 4719 (gpt 4641) | 18.7 / 111.3 / 222.4 | 143 / 9 (8) / 0 | 1 / quality 100 |
| typing-1920x1080 | DW2 | 30 | 0.26 | 0.9992 / 0.9991 | 50.04 / 48.23 | 0.926 | 909 (gpt 891) | 6893 (gpt 6563) | 19.4 / 694.4 / 250.3 | 143 / 9 (8) / 0 | 1 / quality 30 |
| typing-1920x1080 | DW2 | 70 | 0.35 | 0.9999 / 0.9999 | 61.37 / 59.11 | 0.994 | 878 (gpt 891) | 6466 (gpt 6844) | 19.7 / 729.5 / 244.4 | 143 / 9 (8) / 0 | 1 / quality 70 |
| typing-1920x1080 | DW2 | 100 | 0.44 | 1.0000 / 1.0000 | 66.33 / 62.89 | 0.959 | 878 (gpt 891) | 6532 (gpt 6422) | 19.3 / 717.7 / 244.8 | 143 / 9 (8) / 0 | 1 / quality 100 |
| scroll-1920x1080 | DW1 | 30 | 11.46 | 0.9891 / 0.9795 | 35.37 / 34.58 | 0.995 | 4039 (gpt 4219) | 13504 (gpt 13688) | 18.7 / 113.1 / 236.0 | 400 / 21 (20) / 0 | 1 / quality 30 |
| scroll-1920x1080 | DW1 | 70 | 28.26 | 0.9983 / 0.9964 | 45.64 / 43.82 | 0.997 | 4019 (gpt 4172) | 13993 (gpt 13875) | 19.6 / 125.1 / 238.6 | 400 / 21 (20) / 0 | 1 / quality 70 |
| scroll-1920x1080 | DW1 | 100 | 75.32 | 0.9997 / 0.9994 | 52.22 / 45.98 | 0.997 | 3991 (gpt 4219) | 13145 (gpt 13078) | 19.6 / 125.0 / 237.7 | 400 / 21 (20) / 0 | 1 / quality 100 |
| scroll-1920x1080 | DW2 | 30 | 10.40 | 0.9904 / 0.9871 | 35.49 / 35.02 | 0.996 | 4027 (gpt 3938) | 19934 (gpt 19922) | 19.3 / 696.5 / 245.8 | 400 / 21 (20) / 0 | 1 / quality 30 |
| scroll-1920x1080 | DW2 | 70 | 27.74 | 0.9987 / 0.9978 | 46.30 / 44.05 | 0.996 | 3992 (gpt 3703) | 20413 (gpt 20109) | 19.0 / 697.4 / 246.8 | 400 / 21 (20) / 0 | 1 / quality 70 |
| scroll-1920x1080 | DW2 | 100 | 79.96 | 0.9997 / 0.9994 | 51.68 / 45.89 | 0.996 | 3984 (gpt 4125) | 21571 (gpt 21000) | 18.8 / 698.6 / 247.6 | 400 / 21 (20) / 0 | 1 / quality 100 |

Frames whose decoded time is not their instant: 420. OCR: 10 frames; 825 source chars / 10 frames; 35605 source chars. SSIM-Y: 8 × 8 uniform windows at a stride of 4. R: Windows.Media.Ocr over the whole frame, 10 evenly spaced new frames, decoded against the source scaled by σ.

Keyframes: every encoded clip (Intel Quick Sync and the Microsoft software encoder alike) holds one clean point more than N_key = ⌈N_enc / τ_f⌉ (typing-1920x1080 DW1 q30: 9 against 8; typing-1920x1080 DW1 q70: 9 against 8; typing-1920x1080 DW1 q100: 9 against 8; typing-1920x1080 DW2 q30: 9 against 8; typing-1920x1080 DW2 q70: 9 against 8; typing-1920x1080 DW2 q100: 9 against 8; scroll-1920x1080 DW1 q30: 21 against 20; scroll-1920x1080 DW1 q70: 21 against 20; scroll-1920x1080 DW1 q100: 21 against 20; scroll-1920x1080 DW2 q30: 21 against 20; scroll-1920x1080 DW2 q70: 21 against 20; scroll-1920x1080 DW2 q100: 21 against 20); the longest gap between clean points is still τ_f (column max_gop). The cause is not yet known; this stays a failed known result.

## step2d checks on the pilot

```
step2d: corpus C:\Users\Adam\clip-corpus; runs results/d-pilot.csv: 24 rows
action | equation | known | equation value | measured | result
D: encoders | count(activated) | 4 | 4 | 3 (DW1 DW2 DW4); misses: DW3 0x8000ffff | FAIL
D: frames encoded | N_enc = N_new | exact, every run | N_new per folder | 12/12 runs equal | PASS
D: keyframes | N_key = ceil(N_enc / tau_f) | exact for tau_f in {20, 40, 100, 200} | ceil(N_enc / tau_f) | 0/12 runs equal; misses: typing-1920x1080 DW1 q30 tau20: 9 != 8; typing-1920x1080 DW1 q70 tau20: 9 != 8; typing-1920x1080 DW1 q100 tau20: 9 != 8; typing-1920x1080 DW2 q30 tau20: 9 != 8; typing-1920x1080 DW2 q70 tau20: 9 != 8 (+7) | FAIL
D: B-frames | count(B) | 0 | 0 | 12/12 runs with 0 | PASS
D: meter self-check | SSIM-Y(x, x); PSNR-Y(x, x); R(x, x) | 1; inf; 1 | 1; inf; 1 | 1; inf; 1 (OCR 88 chars) | PASS
D: known distortion | MSE = d^2, PSNR = 20 * log10(255 / d), d = 4 | 36.0896 dB within 0.01 dB | 36.0896 dB | 36.0896 dB (clipped pixels 0) | PASS
D: bytes against the prior | prior B_min = 10.3 MB * px_out / (1252 * 890) (Quick Sync, q 70, docs/clip-recorder-equation.md section 6) | reported, no pass or fail before the fit | prior | 12 runs; typing-1920x1080 DW1 q30: 0.15 vs 19.17 MB/min (-99.2 %); typing-1920x1080 DW1 q70: 0.29 vs 19.17 MB/min (-98.5 %); typing-1920x1080 DW1 q100: 0.36 vs 19.17 MB/min (-98.1 %); typing-1920x1080 DW2 q30: 0.26 vs 19.17 MB/min (-98.6 %); typing-1920x1080 DW2 q70: 0.35 vs 19.17 MB/min (-98.2 %); typing-1920x1080 DW2 q100: 0.44 vs 19.17 MB/min (-97.7 %); scroll-1920x1080 DW1 q30: 11.46 vs 19.17 MB/min (-40.2 %); scroll-1920x1080 DW1 q70: 28.26 vs 19.17 MB/min (47.4 %) | PENDING
D: CPU against the prior | prior a_w = N_new * px_out * (c_conv + c_enc(E)), c_conv 1.65 ns, c_enc 7.0 ns (software) or 2.0 ns (hardware) per pixel | reported, no pass or fail before the fit | prior | typing-1920x1080 DW1 q30: a_w 1523 vs 1082 ms (40.7 %); typing-1920x1080 DW1 q70: a_w 1517 vs 1082 ms (40.2 %); typing-1920x1080 DW1 q100: a_w 1573 vs 1082 ms (45.3 %); typing-1920x1080 DW2 q30: a_w 2298 vs 2565 ms (-10.4 %); typing-1920x1080 DW2 q70: a_w 2155 vs 2565 ms (-16.0 %); typing-1920x1080 DW2 q100: a_w 2177 vs 2565 ms (-15.1 %); scroll-1920x1080 DW1 q30: a_w 4501 vs 3027 ms (48.7 %); scroll-1920x1080 DW1 q70: a_w 4664 vs 3027 ms (54.1 %) | PENDING
D: RAM against the prior | prior M_worker = px_out * (3 + 1.5), one frame held (d = 1), M0_w and M_enc not yet known | reported, no pass or fail before the fit | prior | typing-1920x1080 DW1 q30: M_worker 107.9 vs 9.3 MB (1056.4 %); typing-1920x1080 DW1 q70: M_worker 121.8 vs 9.3 MB (1205.0 %); typing-1920x1080 DW1 q100: M_worker 111.3 vs 9.3 MB (1092.3 %); typing-1920x1080 DW2 q30: M_worker 694.4 vs 9.3 MB (7341.2 %); typing-1920x1080 DW2 q70: M_worker 729.5 vs 9.3 MB (7717.6 %); typing-1920x1080 DW2 q100: M_worker 717.7 vs 9.3 MB (7591.6 %); scroll-1920x1080 DW1 q30: M_worker 113.1 vs 9.3 MB (1112.3 %); scroll-1920x1080 DW1 q70: M_worker 125.1 vs 9.3 MB (1240.1 %) | PENDING
step2d: 2 failed
```

The red run before exp-d existed is in `results/step2d-red.txt` (9 failed).

## Run time of the full D grids (equation 6 for way D)

    T_D = 3 · Σ_runs [ N_new · ( px · τ_live + px_out · (τ_w(E) + τ_dec + τ_q) ) + 10 · px_out · τ_ocr · [text scenario] + t_0 ]

px = w · h of the window, px_out = σ · w × σ · h, N_new from each folder's frames.jsonl. Coefficients fitted on the pilot rows (ns per pixel and frame): τ_live 3.10, τ_dec 6.06, τ_q 8.58, τ_ocr 59.37 per OCR frame, τ_w DW1 5.41, DW2 4.88; t_0 = 1.33 s per run (process start, activation, read back); a run without a clip (DW3, DW4) costs 0.36 s.

Runs: 2808 (grid A-D: 4 encoders × 8 q × 24 folders; grid B: τ_f 40, 100, 200, hint off and β 0.05, 0.10, 0.20 around DW1 at q 70, for every folder; 3 repeats each).

**T_D = 4.86 h.** Parts: quality 1.55 h, decode 1.10 h, worker 0.94 h, overhead 0.61 h, live 0.35 h, ocr 0.19 h, failed 0.12 h. By encoder: DW1 3.11 h, DW2 1.63 h.

T_D is under 10 h, so no part has to be cut. The costliest parts are listed above in order; the quality meter (SSIM and PSNR over every decoded frame) and decoding cost more than encoding.

Assumption: the pilot measured 1920 × 1080 only; the other sizes are predicted linearly in pixels. The full run's wall time checks this (acceptance: within ±20 %).

## Differences from the product's encoder path

- The run chooses the encoder: DW1 with hardware transforms on and a DXGI device manager on the vendor's adapter; DW2 with hardware transforms off. The product lets the Sink Writer choose, with no device manager, and retries once in software (`encode_segment`); the retry is not used, so a failure is a result.
- DW4 is not reachable through the Sink Writer: it is activated from its enumeration object and driven directly with one frame to answer the "to confirm" question.
- One segment per run: the whole folder, with no segment split.
- Quality goes through the product decoder's RGB32 output, then Y with the product's matrix; the decoder's colour conversion is part of the measured loss.
- Everything else is the verbatim copy at commit e95e1717: σ scaling, `Nv12Canvas::draw`, the encoder settings and their read-back, the variable time stamps at the frames' instants, and the zstd −3 waiting-frame writer and reader.
