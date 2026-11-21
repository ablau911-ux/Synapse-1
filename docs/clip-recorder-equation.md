# Clip recorder: requirements and equation

Status: draft for the owner's approval, 2026-11-21 18:25 IST (second version: adds R10, the 5 % load limit, and a corrected capture condition). Branch `feature/clip-capture`.

This document covers steps 1 and 2 of the owner's work order. Steps 3 and 4 follow after approval.

1. Decide what we want.
2. Build the equation that describes the process and the result.
3. Turn the equation into an order of operations and a desired real-world result.
4. Write code in which every part is a parameter of the equation.

The rule behind it: fix the result first, then choose, among all the ways that give that result, the cheapest one. If the built system gives a different result, a term is missing from the equation.

It supersedes these points of `docs/clip-recorder-plan.md`: 20 fps default, 2-minute maximum, size fixed at clip start with letterboxing, one encoder, keyframe every second. The plan is updated in step 3.

## 1. What we want (owner, 2026-11-21)

| # | Requirement | Symbol | Value |
|---|---|---|---|
| R1 | The recording shows flashes from 0.050 s | d_min | 0.050 s on screen |
| R2 | Length 0 to 1 minute by default; longer on demand (a bug may appear once in half an hour) | t_def | 60 s, extendable |
| R3 | Good quality, at least 480p | h_min | 480 lines |
| R4 | The recorded area follows the size of the window | w_win × h_win | the window |
| R5 | Works in the background while the user works in another window | U_max | 5 % of the processor |
| R6 | Size and quality follow the window's resolution | | per window |
| R7 | Pixel fill / duplication as in game supersampling, for quality and performance | σ | scale factor |
| R8 | Any encoder; a different encoder per resolution if needed | E(w, h) | per size |
| R9 | Files as small as possible, never at the cost of quality | | minimise bytes |
| R10 | Not only this PC: every computer, from a Core i3 with standard Intel graphics upward | | no machine constants |

## 2. The equation

The result is "an agent can see the bug in the clip". It is 1 only when every condition is 1:

    V = C · G · Q · B · T = 1

Among all settings with V = 1, take the one with the fewest bytes (R9).

Because of R10 nothing in the equation is a constant of one PC. Each machine supplies its own inputs at the start of a recording: the display's refresh interval T_r, the number of processor threads n, the encoders it has, and its free disk. The rate, the size, the encoder and the length limit are computed from them.

### C: the flash is in the clip (R1)

The screen shows whole refresh frames. Counted in refresh frames:

    n_min = ⌈d_min / T_r⌉          frames a flash of d_min is on screen
    m     = ⌈(1/f) / T_r⌉          Windows delivers one frame every m refreshes while the screen keeps changing
    j                              refreshes by which a delivery can be late on this machine (measured)

    C = 1  ⇔  m + j ≤ n_min   ⇒   1/f ≤ (n_min − j) · T_r

Second condition: every frame Windows delivers is encoded. Taking only the newest frame per tick can skip the one frame that shows the flash.

| Display | n_min | Lowest rate, no lateness (j = 0) | Lowest rate, one refresh late (j = 1) |
|---|---|---|---|
| 60 Hz | 3 | 20 fps | 30 fps |
| 75 Hz | 4 | 19 fps | 25 fps |
| 120 Hz | 6 | 20 fps | 24 fps |
| 144 Hz (this PC) | 8 | 18 fps | 21 fps |

20 fps is enough on every display only if deliveries are never late. j is not measured yet. Published measurements of Windows.Graphics.Capture (research document, section 5.2) show its interval is not exact: asked for 16.7 ms, it delivered about every 22 ms. Until j is measured on a machine, the safe value is j = 1.

The recorder checks this condition in every clip: the largest gap between two delivered frames while the screen was changing must not exceed n_min · T_r.

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

### Q: the picture is good enough to read (R3, R6, R7, R9)

h_text is the smallest text that must stay readable, h_leg the smallest text an agent reads reliably.

    scale:     h_text · σ_total ≥ h_leg      ⇒  σ_total ≥ h_leg / h_text
    encoding:  q ≥ q_min(w_out, h_out)       q_min = lowest quality level at which the decoded text is readable
    fallback bitrate follows the size:  bits/s = β · w_out · h_out · f

- With h_text = 12 px and h_leg = 9 px (both assumed, to be checked with a real agent): σ_total ≥ 0.75. A picture may be shrunk to 75 % at most, which saves 44 % of its pixels. Below that the text is gone and no upscaling brings it back.
- Supersampling therefore has three uses. Recording: whole-number duplication for small windows (G). Viewer overview: shrink by averaging all source pixels (supersampled), down to 75 %. Viewer close-up: whole-number duplication ×2 of a crop.
- β measured on this PC with x264 on a changing-text window (1252 × 890, 20 fps): 0.043 bit per pixel per frame at quality 28, 0.060 at quality 23. q_min itself is not measured yet.

### B: it runs in the background, on every machine (R5, R10)

    B = 1  ⇔  capture works for a covered window  ∧  no focus or restore call  ∧  U ≤ U_max = 0.05

    U = processor time of the recorder / (clip time · n)
    U · n = n_new · w_out · h_out · (c_read + c_conv + c_enc) + f · c_tick

n_new is the number of new frames per second; c_read, c_conv and c_enc are the processor time per pixel to read a frame back, convert its colours and encode it; c_tick is the fixed cost of one tick.

- On a two-core, four-thread Core i3, 5 % of the processor is 0.2 of one thread.
- So the load has to follow the changes on screen, not the rate: a repeated frame may cost only c_tick, with no conversion and no encoding work.
- With a hardware encoder and colour conversion on the graphics chip, c_conv and c_enc are close to zero on the processor. Standard Intel graphics has a hardware H.264 encoder (Quick Sync) when the Intel driver is installed.
- When the three costs are known for a machine, the largest window and change rate it can record within 5 % follow from the formula.

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

The recorder writes, for each condition, the measured value and whether it held: the largest delivery gap against n_min · T_r (C), the size and σ used (G), the quality level and encoder used (Q), the measured processor share against 5 % (B), the length and bytes (T). A clip in which a condition failed says which one and when.

Open, for the owner: when a machine cannot meet every condition for a window, which condition gives way first.

## 3. Known now, and what must be measured

Known: d_min, h_min, the σ rule, t_def, U_max, and for this PC the display (1920 × 1080 at 144 Hz), 16 threads, 1,257 GB free.

To measure with the built recorder, in one session of two short clips (one still window, one changing):

| Unknown | How |
|---|---|
| j | gaps between delivered frames while the screen changes; flashes of known length |
| K, P, ρ | two clips give two equations: bytes = t · (K/τ + n_new · P + (f − n_new) · ρ) |
| q_min | text crop of the decoded frame against the original, at several quality levels |
| c_read, c_conv, c_enc, c_tick | processor time of the recorder in the two clips |
| v_dec | time the viewer needs to decode a known number of frames |
| E and its limits | which encoder Windows grants at each size, hardware or software |
| h_leg | smallest text a real agent reads correctly from a returned image |

The processor costs and j on the minimum machine cannot be measured on this PC. They need one run on a real Core i3 with Intel graphics; until then only an estimate scaled from this PC is possible.

## 4. Parameter map: equation to code (commit be2447f8)

| Symbol | Code constant | File | Today | The equation requires |
|---|---|---|---|---|
| f | `DEFAULT_FPS` | `server/capture_clip.rs:52` | 20 | computed per machine: 1/f ≤ (n_min − j) · T_r |
| f maximum | `MAX_FPS` | `server/capture_clip.rs:53` | 30 | at least the computed f |
| T_r, n | (none) | | | read from the machine at start |
| capture interval | `min_update_interval_ms` = 1000/fps | `server/capture_clip.rs` | 1000/fps | 1000/f |
| every delivered frame encoded | `drain_newest` | `server/capture_clip.rs:707` | newest frame per tick; the rest counted as skipped | every frame, in order |
| t_def | (none; `duration_ms` is required) | | | 60 s |
| t maximum | `MAX_DURATION_MS` | `server/capture_clip.rs:51` | 120 s, call waits to the end | D / r, with start, stop and extend |
| h_min, σ | (none) | | size of the first frame | σ rule, 480 lines minimum |
| resize | letterbox into the first size | `server/capture_clip.rs` | fixed size | new segment at the new size |
| q | `H264_QUALITY_LEVEL` | `server/clip_encoder.rs:34` | 70 | q_min, measured |
| fallback bitrate | `FALLBACK_BITRATE_BPS` | `server/clip_encoder.rs:36` | 6,000,000 fixed | β · w_out · h_out · f |
| τ | `KEYFRAME_INTERVAL_SECONDS` | `server/clip_encoder.rs:37` | 1 s | min(t_seek · v_dec / f, t_loss) |
| E | one Media Foundation H.264 path | `server/clip_encoder.rs` | one encoder | E(w_out, h_out) per machine |
| U_max | (none) | | not measured, not limited | 0.05, measured in every clip |
| conditions report | `clip.json` health block | `server/capture_clip.rs` | frame counts only | value and pass/fail per condition |
| N | `MAX_IMAGES_PER_CALL` | `server/clip_viewer.rs:45` | 8 | from the token budget |
| σ_v | `DEFAULT_LONG_EDGE` | `server/clip_viewer.rs:48` | 1280 | ≥ (h_leg / h_text) · w_out, supersampled |
| change test | `BLOCK`, `PIXEL_DELTA`, `MIN_CHANGED_PIXELS` | `server/clip_viewer.rs:55-61` | 8, 32, 3 | measured against encoder noise |
| short-lived change | `SHORT_LIVED_MS`, `SETTLE_MS` | `server/clip_viewer.rs:63-65` | 500, 200 | ≥ d_min is covered |

## 5. Where the built code does not meet the equation

1. Rate: fixed at 20 fps. That guarantees a 50 ms flash only if no delivery is ever late; with one refresh of lateness a 60 Hz display needs 30 fps and this 144 Hz display 21 fps. The rate is not computed from the display.
2. The recorder takes only the newest frame per tick, so a delivered flash frame can be skipped.
3. Length: 2-minute maximum and a call that waits until the end; no default of 60 s, no extension.
4. No 480-line minimum and no pixel duplication for small windows.
5. A resized window is letterboxed into the first size; it does not get its own size.
6. The fallback bitrate is fixed and does not follow the size.
7. One encoder for every size, chosen without looking at the machine.
8. Keyframe interval is fixed at 1 s without a calculation.
9. Processor load is neither measured nor limited. A repeated frame is not converted again, but it is still handed to the encoder on every tick.
10. A clip does not report whether its conditions held.

## Corrections

- The first version of this document (18:05) said 20 fps cannot guarantee a 50 ms flash on a 144 Hz display even without timing error. That counted a flash by the application's clock. Counted by what is on screen, which is the requirement, 20 fps is enough when no delivery is late; the table in section 2 replaces that statement.
- The simulation figures reported on 2026-11-21 16:37 (20 fps "knee", token counts, keyframe table) are withdrawn. That model had an error in how frames are delivered and an over-optimistic scenario. This derivation replaces it.
