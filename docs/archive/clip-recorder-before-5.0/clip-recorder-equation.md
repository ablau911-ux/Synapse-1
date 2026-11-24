# Clip recorder: requirements and equation

Status: approved by the owner 2026-11-21; section 6 (the run for the weakest machine) added 19:20 IST. Branch `feature/clip-capture`.

This document covers steps 1 and 2 of the owner's work order. Steps 3 and 4 follow after approval.

1. Decide what we want.
2. Build the equation that describes the process and the result.
3. Turn the equation into an order of operations and a desired real-world result.
4. Write code in which every part is a parameter of the equation.

The rule behind it: fix the result first, then choose, among all the ways that give that result, the cheapest one. If the built system gives a different result, a term is missing from the equation.

It supersedes these points of `docs/clip-recorder-plan.md`: 2-minute maximum, size fixed at clip start with letterboxing, one encoder, keyframe every second, 30 fps maximum. The plan is updated in step 3.

## 1. What we want (owner, 2026-11-21)

| # | Requirement | Symbol | Value |
|---|---|---|---|
| R1 | The recording shows flashes from 0.050 s | d_min | 0.050 s on screen |
| R2 | Length 0 to 1 minute by default; longer on demand (a bug may appear once in half an hour) | t_def | 60 s, extendable |
| R3 | Good quality, at least 480p | h_min | 480 lines |
| R4 | The recorded area follows the size of the window | w_win × h_win | the window |
| R5 | Works in the background while the user works in another window | | no focus |
| R6 | Size and quality follow the window's resolution | | per window |
| R7 | Pixel fill / duplication as in game supersampling, for quality and performance | σ | scale factor |
| R8 | Any encoder; a different encoder per resolution if needed | E(w, h) | per size |
| R9 | Files as small as possible, never at the cost of quality | | minimise bytes |
| R10 | Not only this PC: every computer, from a Core i3 with standard Intel graphics upward | | no machine constants |

### Base rules (owner, 2026-11-21)

| # | Rule | In the equation |
|---|---|---|
| BR1 | The recorded rate never exceeds 20 fps, on any machine | f = 20 |
| BR2 | The recorder never uses more than 5 % of the processor time, on any machine | U ≤ 0.05 |
| BR3 | 60 Hz is the reference screen; nothing above it is needed | T_r = 1/60 s |
| BR4 | A weaker machine takes longer. It does not get more load, a weaker guarantee or lower quality | time is the free variable |

## 2. The equation

The result is "an agent can see the bug in the clip". It is 1 only when every condition is 1:

    V = C · G · Q · B · T = 1

Among all settings with V = 1, take the one with the fewest bytes (R9).

Because of R10 nothing in the equation is a constant of one PC. Each machine supplies its own inputs at the start of a recording: the number of processor threads n, the encoders it has, and its free disk. The size, the encoder, the length limit and the time until the clip is ready are computed from them.

### C: the flash is in the clip (R1, BR1, BR3)

    T_r = 1/60 s,  f = 20   ⇒   one recorded frame per 3 screen refreshes (50 ms)
    s_k = s_0 + k / f           the recording instants, exactly 50 ms apart

    C = 1  ⇔  (1) the screen is observed at every refresh, up to 60 per second
              (2) the frame recorded for s_k is the last observed frame captured at or before s_k
                  (chosen by its capture time, not by when the recorder got to it)
              (3) no observed frame is lost before that choice

Why this gives the result: something on screen from a to b, with b − a ≥ 50 ms, contains one instant s_k. The frame chosen for s_k is the last one captured before it, and that frame shows it. A late recorder no longer matters, because the choice uses capture times.

Observing is not recording. An observed frame that is not chosen is never read back, converted or encoded, so the recorded rate stays 20 fps.

Limits:

- On a display faster than 60 Hz the screen is still observed 60 times per second (BR3). There, something visible for 66.7 ms or more is guaranteed. Something visible for 50 to 66.7 ms is guaranteed only when nothing else changed in the 16.7 ms before it.
- Whether Windows delivers every refresh must be checked. Published measurements of Windows.Graphics.Capture (research document, section 5.2): asked for 16.7 ms it delivered about every 22 ms; asked for 11.1 ms it delivered every refresh of a 60 Hz display.

Checked in every clip: the largest gap between observed frames while the screen was changing is at most T_r, and no observed frame was lost.

### G: size follows the window, never below 480 lines (R3, R4, R6, R7)

    σ = 1                   if h_win ≥ h_min
    σ = ⌈h_min / h_win⌉      otherwise (whole-number pixel duplication: exact, no blur)
    w_out = even(σ · w_win),  h_out = even(σ · h_win)

| Window | σ | Recorded size |
|---|---|---|
| 1252 × 890 | 1 | 1252 × 890 |
| 640 × 360 | 2 | 1280 × 720 |
| 300 × 200 | 3 | 900 × 600 |

When the window is resized, σ and the size are computed again and a new segment starts at the new size.

### Q: the picture is good enough to read (R3, R6, R7, R9, BR4)

h_text is the smallest text that must stay readable, h_leg the smallest text an agent reads reliably.

    scale:     h_text · σ_total ≥ h_leg      ⇒  σ_total ≥ h_leg / h_text
    encoding:  q ≥ q_min(w_out, h_out)       q_min = lowest quality level at which the decoded text is readable
    fallback bitrate follows the size:  bits/s = β · w_out · h_out · f

- q is the same on every machine (BR4).
- With h_text = 12 px and h_leg = 9 px (both assumed, to be checked with a real agent): σ_total ≥ 0.75. A picture may be shrunk to 75 % at most, which saves 44 % of its pixels. Below that the text is gone and no upscaling brings it back.
- Supersampling therefore has three uses. Recording: whole-number duplication for small windows (G). Viewer overview: shrink by averaging all source pixels (supersampled), down to 75 %. Viewer close-up: whole-number duplication ×2 of a crop.
- β measured on this PC with x264 on a changing-text window (1252 × 890, 20 fps): 0.043 bit per pixel per frame at quality 28, 0.060 at quality 23. q_min itself is not measured yet.

### B: it runs in the background, on every machine (R5, R10, BR2, BR4)

    B = 1  ⇔  capture works for a covered window  ∧  no focus or restore call  ∧  U ≤ 0.05

    U = processor time of the recorder / (wall time · n)

The work is split by whether it can wait:

    cannot wait (while capturing):   a_rt  = f_o · c_obs + n_new · w_out · h_out · c_read        per second
    can wait:                        A_def = N_new · w_out · h_out · (c_conv + c_enc)            per clip

    a_rt ≤ 0.05 · n
    t_ready = max( t_clip ,  A_def / (0.05 · n − a_rt) )

f_o is the observation rate (up to 60 per second), c_obs the cost of receiving one frame without touching its pixels, n_new the new frames recorded per second (at most 20), N_new their number in the clip, and c_read, c_conv, c_enc the processor time per pixel to read a frame back, convert its colours and encode it.

- This is BR4 as a formula: a weaker machine has larger costs, so t_ready is later. The load, the rate and the quality do not change.
- With a hardware encoder and colour conversion on the graphics chip, c_conv and c_enc are close to zero on the processor and t_ready = t_clip. Standard Intel graphics has a hardware H.264 encoder (Quick Sync) when the Intel driver is installed.
- Frames waiting for the work that can wait are kept on disk: at most N_waiting · w_out · h_out · 4 bytes.
- On a two-core, four-thread Core i3, 5 % of the processor is 0.2 of one thread.
- A repeated frame costs nothing: no read-back, no conversion, no encoding.

Measured so far, with other encoders on this PC (Core i7-13620H, 16 threads): 7 to 57 % of one core. Not measured: the built recorder, and anything on a Core i3.

### T: length (R2)

    t ≤ t_req,   t_req = 60 s by default, longer on demand
    bytes(t) = t · r,    t_max = D / r        D = disk budget, r = bytes per second

A recording longer than a tool call can wait has to start, run on its own and be stopped or extended by later calls; it is written in segments so the viewer can open a part.

With the measured x264 figures (2.2 MB per minute static, up to 10 MB per minute changing text), 30 minutes is 66 to 300 MB.

### Bytes: what is minimised (R8, R9)

    r = K/τ + n_new · P + (f − n_new) · ρ

K bytes per keyframe, τ keyframe interval, P bytes per new frame, ρ bytes per repeated frame. K, P and ρ depend on the encoder E, the size and q.

- Free parameters after V = 1: τ, E, and q above q_min.
- τ is bounded by seek time and crash loss: τ ≤ min(t_seek · v_dec / f, t_loss), with v_dec the decode speed in frames per second.
- E(w_out, h_out) = the encoder with the fewest bytes among those the machine has that accept the size and rate and reach q_min. Different sizes may get different encoders.
- The encoder that exists on every Windows PC is H.264 through Media Foundation: hardware when the graphics driver provides one, the Windows software encoder otherwise. Another codec is used only where the machine has both its encoder and its decoder.

### Viewer

    tokens per call = N · w_v · h_v / 770 + 1500        (N images of w_v × h_v)
    w_v = σ_v · w_out,   σ_v ≥ h_leg / h_text

### Every clip reports its own conditions

The recorder writes, for each condition, the measured value and whether it held: the largest observation gap and the lost frames (C), the size and σ used (G), the quality level and encoder used (Q), the measured processor share against 5 % and the time until ready (B), the length and bytes (T). A clip in which a condition failed says which one and when.

## 3. Known now, and what must be measured

Known: d_min, f, T_r, h_min, the σ rule, t_def, U_max, and for this PC 16 threads and 1,257 GB free.

To measure with the built recorder, in one session of two short clips (one still window, one changing):

| Unknown | How |
|---|---|
| observation gaps | gaps between delivered frames while the screen changes, at a 60-per-second request |
| K, P, ρ | two clips give two equations: bytes = t · (K/τ + n_new · P + (f − n_new) · ρ) |
| q_min | text crop of the decoded frame against the original, at several quality levels |
| c_obs, c_read, c_conv, c_enc | processor time of the recorder in the two clips |
| v_dec | time the viewer needs to decode a known number of frames |
| E and its limits | which encoder Windows grants at each size, hardware or software |
| h_leg | smallest text a real agent reads correctly from a returned image |

The processor costs on the minimum machine cannot be measured on this PC. They need one run on a real Core i3 with Intel graphics; until then only an estimate scaled from this PC is possible.

## 4. Parameter map: equation to code (commit be2447f8)

| Symbol | Code constant | File | Today | The equation requires |
|---|---|---|---|---|
| f | `DEFAULT_FPS` | `server/capture_clip.rs:52` | 20 | 20 |
| f maximum | `MAX_FPS` | `server/capture_clip.rs:53` | 30 | 20 |
| observation rate f_o | `min_update_interval_ms` = 1000/fps | `server/capture_clip.rs` | 20 per second | 60 per second |
| choice of the frame for s_k | `drain_newest` | `server/capture_clip.rs:707` | newest frame on arrival; the rest counted as skipped | last frame captured at or before s_k |
| n | (none) | | | read from the machine at start |
| t_def | (none; `duration_ms` is required) | | | 60 s |
| t maximum | `MAX_DURATION_MS` | `server/capture_clip.rs:51` | 120 s, call waits to the end | D / r, with start, stop and extend |
| h_min, σ | (none) | | size of the first frame | σ rule, 480 lines minimum |
| resize | letterbox into the first size | `server/capture_clip.rs` | fixed size | new segment at the new size |
| q | `H264_QUALITY_LEVEL` | `server/clip_encoder.rs:34` | 70 | q_min, measured, same on every machine |
| fallback bitrate | `FALLBACK_BITRATE_BPS` | `server/clip_encoder.rs:36` | 6,000,000 fixed | β · w_out · h_out · f |
| τ | `KEYFRAME_INTERVAL_SECONDS` | `server/clip_encoder.rs:37` | 1 s | min(t_seek · v_dec / f, t_loss) |
| E | one Media Foundation H.264 path | `server/clip_encoder.rs` | one encoder | E(w_out, h_out) per machine |
| U_max | (none) | | not measured, not limited | 0.05, enforced and measured in every clip |
| t_ready | (none) | | all work during capture | work that can wait is deferred within the 5 % |
| conditions report | `clip.json` health block | `server/capture_clip.rs` | frame counts only | value and pass/fail per condition |
| N | `MAX_IMAGES_PER_CALL` | `server/clip_viewer.rs:45` | 8 | from the token budget |
| σ_v | `DEFAULT_LONG_EDGE` | `server/clip_viewer.rs:48` | 1280 | ≥ (h_leg / h_text) · w_out, supersampled |
| change test | `BLOCK`, `PIXEL_DELTA`, `MIN_CHANGED_PIXELS` | `server/clip_viewer.rs:55-61` | 8, 32, 3 | measured against encoder noise |
| short-lived change | `SHORT_LIVED_MS`, `SETTLE_MS` | `server/clip_viewer.rs:63-65` | 500, 200 | ≥ d_min is covered |

## 5. Where the built code does not meet the equation

1. `MAX_FPS` allows 30 fps; the base rule is never above 20.
2. The screen is observed only 20 times per second and the newest frame is taken when the recorder gets to it. A 50 ms flash can be missed when a delivery or a tick is late. The equation needs observation at 60 per second and the choice by capture time.
3. Length: 2-minute maximum and a call that waits until the end; no default of 60 s, no extension.
4. No 480-line minimum and no pixel duplication for small windows.
5. A resized window is letterboxed into the first size; it does not get its own size.
6. The fallback bitrate is fixed and does not follow the size.
7. One encoder for every size, chosen without looking at the machine.
8. Keyframe interval is fixed at 1 s without a calculation.
9. Processor load is neither measured nor limited, and no work is deferred. A repeated frame is not converted again, but it is still handed to the encoder on every tick.
10. A clip does not report whether its conditions held.

## 6. The equation run for the weakest machine (2026-11-21 19:20 IST)

Weakest machine (owner): Core i3 2.2 GHz, two cores (four threads), 4 GB memory, Intel graphics. Taken as the Core i3-2330M class.

### Inputs

Measured on the development PC (Core i7-13620H), single thread, then multiplied by the speed ratio 3.68 (PassMark single-thread rating 3546 against 964). Nothing was measured on a real Core i3.

| Cost | Development PC | Weakest machine | Per 1920 × 1080 frame there |
|---|---|---|---|
| c_read, read a frame back | 0.13 ns per pixel | 0.5 | 1 ms |
| c_conv, convert colours (the code as built) | 1.65 | 6.1 | 13 ms |
| compare 8 × 8 blocks (viewer) | 1.75 | 6.4 | 13 ms |
| c_enc, Windows software H.264 encoder | 7.0 (measured 4.8 to 7.4) | 25.7 | 53 ms |
| c_enc, Intel Quick Sync, processor part | 2.0 (measured 1.2 to 2.3) | 7.4 | 15 ms |

- Encoder costs: the Media Foundation encoders run through ffmpeg (`h264_mf`, quality 70) on two recorded screen clips, processor time minus the decode-only run.
- The software encoder spends as much on a repeated frame as on a new one (8.2 ms against 8.0 ms per frame at 1252 × 890).
- Budget on the weakest machine: 0.05 × 4 threads = 0.2 thread-seconds per second.
- Activity: "typical" is the measured changing-text window, 3.8 new frames per second; "worst" is a window that changes on every recorded frame, 20 per second.
- Not included: c_obs, the cost of observing a frame without reading it (not measured).

### Result: processor share and time until a 60-second clip is ready

`t_ready = max(t_clip, A_def / (0.05 · n − a_rt))`. "Share if done live" is what the work would take without deferring; above 5 % the work is deferred and the clip is ready later.

| Encoder path | Window | Typical: share if live | Typical: ready after | Worst: share if live | Worst: ready after |
|---|---|---|---|---|---|
| Software encoder | 854 × 480 | 1.3 % | 60 s | 6.6 % | 80 s |
| Software encoder | 1280 × 720 | 2.8 % | 60 s | 14.9 % | 184 s |
| Software encoder | 1920 × 1080 | 6.4 % | 77 s | 33.5 % | 439 s |
| Quick Sync, conversion on the processor | 854 × 480 | 0.5 % | 60 s | 2.8 % | 60 s |
| Quick Sync, conversion on the processor | 1280 × 720 | 1.2 % | 60 s | 6.4 % | 78 s |
| Quick Sync, conversion on the processor | 1920 × 1080 | 2.8 % | 60 s | 14.4 % | 185 s |
| Quick Sync, conversion on the graphics chip | 854 × 480 | 0.3 % | 60 s | 1.6 % | 60 s |
| Quick Sync, conversion on the graphics chip | 1280 × 720 | 0.7 % | 60 s | 3.6 % | 60 s |
| Quick Sync, conversion on the graphics chip | 1920 × 1080 | 1.6 % | 60 s | 8.1 % | 102 s |

### What the numbers decide

1. The 5 % rule holds on the weakest machine in every case; what changes is when the clip is ready. In typical use the clip is ready when the recording ends, except a 1920 × 1080 window with the software encoder (17 s later).
2. Repeated frames must never reach the encoder. Handed over on every tick, as the code does today, the software encoder alone would take 5 % of the processor at 854 × 480, 12 % at 1280 × 720 and 27 % at 1920 × 1080, with nothing changing on screen.
3. Reading frames back is cheap (at most 2 % of one thread); the work is in the conversion and the encoder. The hardware encoder cuts it by about 2.4 times, conversion on the graphics chip by a further 1.8 times.
4. The worst case with the software encoder at 1920 × 1080 leaves 1,036 frames waiting at the end of a 60-second clip. Kept raw that is 8.6 GB, written at 166 MB per second, which an ordinary hard disk cannot do. Frames that wait need a cheaper form than raw, or that case must be reported as not met. Step 3 has to settle this.
5. Memory: the frame buffers at 1920 × 1080 are 36 MB, 0.9 % of 4 GB. The encoder's own memory is not measured.

### Bytes with the Windows software encoder at quality 70

Measured: changing small text at 1252 × 890: 15.0 MB per minute with a keyframe every second, 9.0 with one every 5 seconds. Still window at 1502 × 1038: 2.5 and 0.5 MB per minute. Quick Sync at the same setting: 10.3 and 3.7 MB per minute (keyframe every second).

Scaled by pixel count:

| Window | Small changing text, keyframe 1 s / 5 s | Still window, keyframe 1 s / 5 s |
|---|---|---|
| 854 × 480 | 5.5 / 3.3 MB per minute | 0.7 / 0.1 |
| 1280 × 720 | 12.4 / 7.4 | 1.5 / 0.3 |
| 1920 × 1080 | 27.9 / 16.7 | 3.3 / 0.7 |

A keyframe every 5 seconds saves 40 % on changing text and 79 % on a still window. Quality 70 gives larger files than the x264 runs (7.2 to 10 MB per minute at 1252 × 890); the lowest readable quality q_min is still not measured, so these are upper figures.

### Limits of this run

- The weakest-machine figures are the development PC's figures times one benchmark ratio. Memory speed, the old graphics chip and its driver are not in that ratio.
- Whether Quick Sync is available on a 2011 Intel graphics chip under a current Windows driver is not verified.
- The encoder costs include ffmpeg's own frame handling and a start-up share; the built recorder will differ somewhat.
- Scripts: `scripts/clip-equation/` (`bench.rs`, `measure_encoder.ps1`, `run_equation.py`).

## Corrections

- The first version (18:05) said 20 fps cannot guarantee a 50 ms flash on a 144 Hz display even without timing error. That counted a flash by the application's clock, not by what is on screen.
- The second version (18:25) computed the rate per display, up to 30 fps. The owner's base rules replace that: 20 fps and a 60 Hz reference on every machine. The guarantee now comes from observing every refresh and choosing each recorded frame by its capture time.
- The simulation figures reported on 2026-11-21 16:37 (20 fps "knee", token counts, keyframe table) are withdrawn. That model had an error in how frames are delivered and an over-optimistic scenario. This derivation replaces it.
