# Clip recorder: requirements and equation

Status: draft for the owner's approval, 2026-11-21 18:05 IST. Branch `feature/clip-capture`.

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
| R1 | The recording shows flashes from 0.050 s | d_min | 0.050 s |
| R2 | Length 0 to 1 minute by default; longer on demand (a bug may appear once in half an hour) | t_def | 60 s, extendable |
| R3 | Good quality, at least 480p | h_min | 480 lines |
| R4 | The recorded area follows the size of the window | w_win × h_win | the window |
| R5 | Works in the background while the user works in another window | | no focus, low load |
| R6 | Size and quality follow the window's resolution | | per window |
| R7 | Pixel fill / duplication as in game supersampling, for quality and performance | σ | scale factor |
| R8 | Any encoder; a different encoder per resolution if needed | E(w, h) | per size |
| R9 | Files as small as possible, never at the cost of quality | | minimise bytes |

## 2. The equation

The result is "an agent can see the bug in the clip". It is 1 only when every condition is 1:

    V = C · G · Q · B · T = 1

Among all settings with V = 1, take the one with the fewest bytes (R9).

### C: the flash is in the clip (R1)

A flash that lasts d_min is on screen for a whole number of screen refreshes. T_r is the refresh interval, f the recording rate, J the timing error of capture and recorder together.

    W = ⌊d_min / T_r⌋ · T_r        time the flash is really on screen
    C = 1  ⇔  1/f + J ≤ W   ⇒   f ≥ 1 / (W − J)

Second condition: every frame Windows delivers is encoded. Taking only the newest frame per tick can skip the one frame that shows the flash.

This PC: the display runs at 144 Hz, so T_r = 6.94 ms and W = 7 × 6.94 = 48.6 ms.

| Timing error J | Lowest rate that guarantees a 50 ms flash |
|---|---|
| 0 ms | 21 fps |
| 5 ms | 23 fps |
| 10 ms | 26 fps |
| 15 ms | 30 fps |

At 30 fps the allowed error is 15.3 ms. J is not measured yet. Published measurements of Windows.Graphics.Capture (research document, section 5.2) show its interval is not exact: asked for 16.7 ms, it delivered about every 22 ms.

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

### B: it runs in the background (R5)

    B = 1  ⇔  capture works for a covered window  ∧  no focus or restore call  ∧  U ≤ U_max

U is the recorder's processor share. Earlier measurements with other encoders: 7 to 57 % of one core (the PC has 16 threads). U for the built recorder is not measured. U_max is not set by the owner yet.

### T: length (R2)

    t ≤ t_req,   t_req = 60 s by default, longer on demand
    bytes(t) = t · r,    t_max = D / r        D = disk budget, r = bytes per second

A recording longer than a tool call can wait has to start, run on its own and be stopped or extended by later calls; it is written in segments so the viewer can open a part.

With the measured x264 figures (2.2 MB per minute static, up to 10 MB per minute changing text), 30 minutes is 66 to 300 MB. Free disk: 1,257 GB.

### Bytes: what is minimised (R8, R9)

    r = K/τ + n_new · P + (f − n_new) · ρ

K bytes per keyframe, τ keyframe interval, n_new new frames per second, P bytes per new frame, ρ bytes per repeated frame. K, P and ρ depend on the encoder E, the size and q.

- Free parameters after V = 1: τ, E, and q above q_min.
- τ is bounded by seek time and crash loss: τ ≤ min(t_seek · v_dec / f, t_loss), with v_dec the decode speed in frames per second.
- E(w_out, h_out) = the encoder with the fewest bytes among those on this PC that accept the size and rate and reach q_min. Different sizes may get different encoders.

### Viewer

    tokens per call = N · w_v · h_v / 770 + 1500        (N images of w_v × h_v)
    w_v = σ_v · w_out,   σ_v ≥ h_leg / h_text

## 3. Known now, and what must be measured

Known: d_min, T_r, W, h_min, the σ rule, t_def, the display (1920 × 1080 at 144 Hz), free disk.

To measure with the built recorder, in one session of two short clips (one still window, one changing):

| Unknown | How |
|---|---|
| J | recorded frame times against the ideal times; flashes of known length |
| K, P, ρ | two clips give two equations: bytes = t · (K/τ + n_new · P + (f − n_new) · ρ) |
| q_min | text crop of the decoded frame against the original, at several quality levels |
| U | processor time of the recorder during the clip |
| v_dec | time the viewer needs to decode a known number of frames |
| E and its limits | which encoder Windows grants at each size, hardware or software |
| h_leg | smallest text a real agent reads correctly from a returned image |

## 4. Parameter map: equation to code (commit be2447f8)

| Symbol | Code constant | File | Today | The equation requires |
|---|---|---|---|---|
| f | `DEFAULT_FPS` | `server/capture_clip.rs:52` | 20 | ≥ 1/(W − J): 21 at J = 0, 30 at J = 15 ms |
| f maximum | `MAX_FPS` | `server/capture_clip.rs:53` | 30 | at least the required f |
| capture interval | `min_update_interval_ms` = 1000/fps | `server/capture_clip.rs` | 1000/fps | 1000/f |
| every delivered frame encoded | `drain_newest` | `server/capture_clip.rs` | newest frame per tick | every frame, in order |
| t_def | (none; `duration_ms` is required) | | | 60 s |
| t maximum | `MAX_DURATION_MS` | `server/capture_clip.rs:51` | 120 s, call waits to the end | D / r, with start, stop and extend |
| h_min, σ | (none) | | size of the first frame | σ rule, 480 lines minimum |
| resize | letterbox into the first size | `server/capture_clip.rs` | fixed size | new segment at the new size |
| q | `H264_QUALITY_LEVEL` | `server/clip_encoder.rs:34` | 70 | q_min, measured |
| fallback bitrate | `FALLBACK_BITRATE_BPS` | `server/clip_encoder.rs:36` | 6,000,000 fixed | β · w_out · h_out · f |
| τ | `KEYFRAME_INTERVAL_SECONDS` | `server/clip_encoder.rs:37` | 1 s | min(t_seek · v_dec / f, t_loss) |
| E | one Media Foundation H.264 path | `server/clip_encoder.rs` | one encoder | E(w_out, h_out) |
| N | `MAX_IMAGES_PER_CALL` | `server/clip_viewer.rs:45` | 8 | from the token budget |
| σ_v | `DEFAULT_LONG_EDGE` | `server/clip_viewer.rs:48` | 1280 | ≥ (h_leg / h_text) · w_out, supersampled |
| change test | `BLOCK`, `PIXEL_DELTA`, `MIN_CHANGED_PIXELS` | `server/clip_viewer.rs:55-61` | 8, 32, 3 | measured against encoder noise |
| short-lived change | `SHORT_LIVED_MS`, `SETTLE_MS` | `server/clip_viewer.rs:63-65` | 500, 200 | ≥ d_min is covered |

## 5. Where the built code does not meet the equation

1. Rate: 20 fps cannot guarantee a 50 ms flash on this 144 Hz display, even with no timing error.
2. The recorder takes only the newest frame per tick, so a delivered flash frame can be skipped.
3. Length: 2-minute maximum and a call that waits until the end; no default of 60 s, no extension.
4. No 480-line minimum and no pixel duplication for small windows.
5. A resized window is letterboxed into the first size; it does not get its own size.
6. The fallback bitrate is fixed and does not follow the size.
7. One encoder for every size.
8. Keyframe interval is fixed at 1 s without a calculation.

## Withdrawn

The simulation figures reported on 2026-11-21 16:37 (20 fps "knee", token counts, keyframe table) are withdrawn. That model had an error in how frames are delivered and an over-optimistic scenario. This derivation replaces it.
