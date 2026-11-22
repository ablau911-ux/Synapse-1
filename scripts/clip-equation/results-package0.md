# Clip recorder, package 0: measurement results

Measured 2026-11-22 03:12 to 03:25 IST on the development PC, with the probes in `scripts/clip-equation/probe/` (built with `cargo build --release`). Plan: `docs/clip-recorder-plan.md` version 4, section 0 (M1 to M9). No product code was changed.

Every development-PC figure below comes from a run listed here. Weakest-machine figures are **estimates**: processor costs are multiplied by the PassMark single-thread ratio 3546 / 964 = 3.68 (equation section 6); figures that are not processor costs (delivery, disk speed) cannot be scaled that way and are marked "not estimable". They stay estimates until one run on a real Core i3.

## The PC

| Item | Value |
|---|---|
| Processor | Intel Core i7-13620H, 10 cores, 16 logical processors |
| Memory | 32 GB |
| Graphics | Intel UHD Graphics (driver 32.0.101.6790), which drives the panel and is the adapter windows-capture's own device lands on; NVIDIA GeForce RTX 4050 Laptop GPU (driver 32.0.15.9200) |
| Display | 1920 x 1080 at 144 Hz (DWM refresh 10,000,000 / 69,434 = 144.02 Hz) |
| Windows | Windows 11 Pro 26H2, build 26300.9974 |
| Disk | WD PC SN740 2 TB NVMe, about 1,437 GB free |
| Graphics capture | supported; cursor, border, secondary windows, minimum update interval and dirty regions all reported supported by windows-capture 2.0.0 |

Processor time is read from the thread and process cycle counters (`QueryThreadCycleTime`, `QueryProcessCycleTime`). `clip-probe info` measured their rate at 2.84 to 2.91 GHz idle, with every core busy, and in 1 ms bursts between sleeps; a 2 s spin read 1.979 to 1.997 s against 2.000 s of wall time. Processor figures therefore carry about ±2 %.

## How to run

    cd scripts/clip-equation/probe
    cargo build --release
    target/release/clip-probe.exe <info | readback | observe | delivery | newframes [nocursor] | disk | threading | lateness | encoder>
    python -I ../run_equation.py

Every probe uses only its own window (topmost, never activated). The mouse moved only inside that window, at most 4.9 s per run, and was put back. Typing went as `WM_CHAR` to the probe's own edit control. Disk writes went to `%LOCALAPPDATA%\Temp\clip-probe-*`: 1.85 GB in total over all runs (M5 1.33 GB, M7 0.52 GB), free space stayed above 1,436 GB, and every file was deleted.

## Summary

| # | Command | Development PC | Weakest machine (estimate) | Fills |
|---|---|---|---|---|
| M1 | `clip-probe readback` | c_read 0.456 / 0.330 / 0.349 ns per pixel at 854 x 480 / 1280 x 720 / 1920 x 1080 (whole process, blocking Map, Intel adapter); 0.72 ms per 1920 x 1080 frame | 1.68 / 1.21 / 1.28 ns per pixel; 2.7 ms per 1920 x 1080 frame | c_read |
| M2 | `clip-probe observe` | c_obs 0.373 ms per delivered frame (whole process); 0.245 ms on the capture thread alone; handler body 5 µs | 1.37 ms (0.90 ms capture thread) | c_obs |
| M3 | `clip-probe delivery` | Default: 48.1 per second, every gap 20.83 ms (above T_r); 16.7 ms: 48.0 per second, every gap 20.83 ms; 11 ms: 72.0 per second, every gap 13.89 ms (none above T_r) | not estimable (Windows 10 has only Default; 60 Hz not available here) | `REQUESTED_UPDATE_INTERVAL_MS`, f_o |
| M4 | `clip-probe newframes`, `clip-probe newframes nocursor` | n_new: idle 0; caret 1.67; typing at 8 characters per second 8.0; mouse moving with the cursor in the frames 20.0 (every instant); mouse moving with the cursor left out 0 | same activity figures (they do not depend on the processor) | n_new, E8 |
| M5 | `clip-probe disk` | c_disk 1.507 ns per pixel through the file cache at 20 frames per second (3.1 ms per 1920 x 1080 frame), 0.756 without the cache; disk took 3,255 MB/s buffered, 3,004 MB/s write-through, 166 MB/s paced with 0 late frames | c_disk 5.54 (2.78 without the cache) ns per pixel; disk speed not estimable | c_disk, `SEGMENT_RAW_BUDGET_BYTES`, E8 |
| M6 | `clip-probe threading` | multithread protection on; longest handler time while the reader maps: 2.00 ms with a blocking Map, 1.71 ms with "do not wait" + yield, 0.111 ms with "do not wait" + 1 ms sleep; reader processor time per 1280 x 720 read 0.40 / 2.22 / 0.50 ms | handler time 0.41 ms with "do not wait" + sleep (processor part only) | C3 mechanism |
| M7 | `clip-probe lateness` | 0 instants late, idle and with all 16 logical processors busy; read-back plus raw write at most 18.7 ms; 0 lost frames; at most 3 slots in use | up to 1 instant late (18.7 ms x 3.68 = 69 ms > 50 ms) | `MAX_SELECTION_LATENESS_INSTANTS`, `OBSERVE_SLOTS` |
| M8 | `clip-probe encoder` + ffprobe | software MFT and Intel Quick Sync (granted with hardware transforms on): GOP 20, B-pictures 0, quality mode 70 and the screen-content hint all accepted and read back; keyframes at frames 0, 20, 40, 60, 80; no B-frames | same Windows software encoder expected; Quick Sync on 2011 Intel graphics unverified | `GOP_FRAMES`, O4 |
| M9 | `python -I run_equation.py` | live share 0.01 % to 0.65 % of the PC; worker cap 4.35 % to 4.99 % | live share 0.18 % to 9.14 %; above 5 % at 1280 x 720 and 1920 x 1080 when the window changes every recorded frame (including mouse movement with the cursor in the frames) | worker cap formula |

## Recommended parameter-map values

| Constant | Value | From |
|---|---|---|
| `REQUESTED_UPDATE_INTERVAL_MS` | 11 (confirmed) | M3: 72 per second, longest gap 13.89 ms ≤ T_r on this 144 Hz panel; 16.7 ms gives 48 per second with 20.83 ms gaps |
| `MAX_SELECTION_LATENESS_INSTANTS` | 2 | M7: 0 measured here under full load; weakest-machine estimate 1; one instant of margin |
| `OBSERVE_SLOTS` | 5 with the probe's slot rule (lateness + 3); 4 by the plan's formula (lateness + 2) only if a slot is freed as soon as it has been read and a newer frame is pinned | M6, M7: 3 slots in use at lateness 0 (see finding 6) |
| `GOP_FRAMES` | 20 | M8: accepted, read back and found in the output on both encoders |
| B-picture count (O4) | 0 | M8: accepted, read back; ffprobe finds only I and P frames |
| screen-content hint (B6) | `CODECAPI_AVScenarioInfo` = `eAVScenarioInfo_DisplayRemoting` | M8: accepted and read back on both encoders; byte effect on the synthetic test content: none (software) and -0.04 % (Quick Sync); the real effect is proof case 13 |
| `SEGMENT_RAW_BUDGET_BYTES` | 1,073,741,824 (1 GiB), proposed | derived, not measured: at 1920 x 1080 it holds 129 frames, 6.5 s at 20 new frames per second (M4 mouse), 16 s at 8 per second (M4 typing); the caret rate (1.67 per second) reaches the 60 s `SEGMENT_MS` first. This disk writes 1 GiB in 0.36 s (M5) |
| c_read | 0.456 / 0.330 / 0.349 ns per pixel | M1 |
| c_obs | 0.373 ms per delivered frame | M2 |
| c_disk | 1.507 ns per pixel (file cache), 0.756 (no cache) | M5 |
| f_o | 72 per second here at 11 ms; at most 60 on a 60 Hz panel | M3 |
| n_new | 1.67 caret, 8.0 typing, 20.0 mouse with cursor in the frames, 0 mouse without | M4 |
| Map mechanism (C3) | `Map` with `D3D11_MAP_FLAG_DO_NOT_WAIT`, on "still drawing" sleep 1 ms and retry; multithread protection on | M6 |

## M1: read-back cost (c_read)

Command `clip-probe readback` (03:16 IST; an earlier run at 03:13 IST without the sleep-retry way gave the higher blocking figures used for c_read). A GPU texture shaped like a capture frame (BGRA, render target) is copied into a staging texture, mapped, and its rows copied tightly into processor memory; 10 warm-up rounds, then 120 measured rounds per line. "Thread" is the calling thread, "process" adds the driver's own threads.

Intel UHD Graphics (the capture adapter):

| Size | Way | Wall ms mean / p99 | Thread ms per frame | Thread ns per pixel | Process ns per pixel |
|---|---|---|---|---|---|
| 854 x 480 | reused staging, blocking Map | 1.138 / 1.261 | 0.127 | 0.310 | 0.390 (first run 0.456) |
| 854 x 480 | reused staging, do not wait + yield | 1.035 / 1.187 | 1.032 | 2.517 | 4.872 |
| 854 x 480 | reused staging, do not wait + 1 ms sleep | 1.614 / 2.028 | 0.164 | 0.400 | 0.537 |
| 854 x 480 | new staging per frame (today's path, F1) | 1.537 / 1.720 | 0.467 | 1.139 | 1.275 |
| 1280 x 720 | reused staging, blocking Map | 2.105 / 2.417 | 0.265 | 0.288 | 0.330 (first run 0.329) |
| 1280 x 720 | reused staging, do not wait + yield | 1.964 / 2.099 | 1.927 | 2.091 | 3.987 |
| 1280 x 720 | reused staging, do not wait + 1 ms sleep | 1.774 / 3.316 | 0.249 | 0.270 | 0.329 |
| 1280 x 720 | new staging per frame | 1.542 / 1.788 | 0.897 | 0.973 | 1.041 |
| 1920 x 1080 | reused staging, blocking Map | 1.496 / 1.917 (first run 4.467) | 0.596 | 0.287 | 0.307 (first run 0.349) |
| 1920 x 1080 | reused staging, do not wait + yield | 1.342 / 1.515 | 1.317 | 0.635 | 1.048 |
| 1920 x 1080 | reused staging, do not wait + 1 ms sleep | 1.836 / 2.691 | 0.571 | 0.276 | 0.306 |
| 1920 x 1080 | new staging per frame | 3.213 / 3.718 | 1.903 | 0.918 | 0.953 |

NVIDIA RTX 4050 (not the capture adapter, shown for comparison): blocking Map 0.50 / 0.33 / 0.31 ns per pixel on the thread but 2.29 / 0.78 / 0.69 for the whole process; "do not wait" + 1 ms sleep took 6.0 to 6.6 ms of wall time (4 retries).

Result: the real read-back costs 0.31 to 0.46 ns per pixel of whole-process time, 2.4 to 3.5 times the memory copy the equation used (0.13, F16), and still small next to the raw write (M5). A new staging texture per frame costs about 3 times as much as a reused one. Weakest machine (estimate): 1.68 / 1.21 / 1.28 ns per pixel; the GPU of a 2011 Intel chip is not in the ratio.

## M2: observer cost per delivered frame (c_obs)

Command `clip-probe observe` (03:13 IST). The probe window (1280 x 720) repaints its counter once per composition (144 per second). The handler does one GPU copy into a slot texture and reads the composition time; no read-back. Interval Default, 10 s measured after 1 s of warm-up; the same process without capture was measured for 5 s first.

| Figure | Value |
|---|---|
| Delivered | 480 frames, 48.0 per second |
| Capture thread processor time per frame (everything on that thread between frames, windows-capture's own work included) | 245 µs |
| Whole process minus painter thread minus the idle baseline, per frame | 373 µs |
| Handler body per frame | 5.2 µs |
| Handler entry to return | mean 0.006 ms, max 0.035 ms |

c_obs = 0.373 ms per delivered frame (0.245 ms on the capture thread). Almost all of it is outside the handler: the WinRT event, `TryGetNextFrame`, the surface and texture queries that windows-capture does for every frame. Weakest machine (estimate): 1.37 ms per frame; at 60 delivered frames per second that is 2.06 % of a 4-thread machine before a single frame is read back.

## M3: delivery (f_o, REQUESTED_UPDATE_INTERVAL_MS)

Command `clip-probe delivery` (03:14 IST). Counter window as in M2; each captured frame's paint counter is read from 12 black-or-white blocks; 8 s per interval.

| Interval | Delivered per second | Gap between composition times mean / max | Gaps above T_r = 16.7 ms | Paints | Paints not in any delivered frame | Longest run not delivered |
|---|---|---|---|---|---|---|
| Default | 48.12 | 20.831 / 20.832 ms | 384 of 384 | 1,153 | 768 | 2 |
| 16.7 ms | 48.00 | 20.831 / 20.832 ms | 383 of 383 | 1,152 | 766 | 2 |
| 11 ms | 72.00 | 13.887 / 13.889 ms | 0 of 575 | 1,152 | 575 | 2 |

Every gap is a whole number of refreshes (6.944 ms): Default and 16.7 ms deliver every third refresh, 11 ms every second one. No frame was missing beyond that pattern. The requested 11 ms satisfies condition C (1) on this display (longest gap 13.9 ms ≤ 16.7 ms); Default and 16.7 ms do not.

Frames arrive in the handler *before* the time `Frame::timestamp()` reports: on average 5.46 to 5.58 ms before it, and never less than 3.2 ms before it, in every run with that figure (M2, M3, M6; in M7 even the handler's return came at least 1.98 ms before it). See finding 1.

Weakest machine: not estimable. Windows 10 has no interval setting (Default only), and the Default behaviour on a 60 Hz display could not be measured here (the owner's display setting is not changed).

## M4: new-frame rate (n_new)

Command `clip-probe newframes` (03:18 IST) and `clip-probe newframes nocursor` (03:19 IST). Probe text window 854 x 480 with an edit control; capture interval Default; every delivered frame is read back and hashed. Counts are on the 50 ms instant grid: "identity" is a new composition time (the product's rule, A7), "content" is different pixels.

| Phase | Seconds | Delivered per second | n_new by identity | n_new by content |
|---|---|---|---|---|
| idle | 5.0 | 0 | 0 | 0 |
| caret (edit control has the focus of the probe's thread) | 6.0 | 1.67 | 1.67 | 1.67 |
| typing, 8 characters per second | 5.0 | 8.00 | 8.00 | 8.00 |
| mouse moving inside the window, cursor in the frames | 4.9 | 47.95 | 20.00 | 20.00 |
| mouse moving inside the window, cursor left out (`nocursor` run) | 4.9 | 0 | 0 | 0 |

Every new frame by identity was also new by content. Each typed character gave exactly one new frame. Mouse movement over the window gives a new frame at every instant, but only because Windows draws the cursor into the frames: with the cursor left out, the same movement gives no frame at all. Weakest machine: the same figures; they follow the activity, not the processor.

## M5: raw write volume and its processor cost (c_disk)

Command `clip-probe disk` (03:19 IST). 1920 x 1080 BGRA frames (8,294,400 bytes of pseudo-random data) written one file per frame into `%LOCALAPPDATA%\Temp\clip-probe-disk-*`, deleted after each phase.

| Phase | Frames | MB/s | Write ms mean / p99 / max | Late frames | Thread processor ms per frame | Process processor ns per pixel |
|---|---|---|---|---|---|---|
| through the file cache, as fast as possible | 60 | 3,254.9 | 2.542 / 2.934 / 3.121 | – | 2.52 | 1.254 |
| no cache, write-through (`FILE_FLAG_NO_BUFFERING` and `FILE_FLAG_WRITE_THROUGH`) | 60 | 3,004.2 | 2.755 / 7.046 / 7.380 | – | 0.72 | 0.756 |
| through the file cache at 20 frames per second (166 MB/s) | 40 | 169.8 | 3.221 / 4.629 / 4.629 | 0 | 3.14 | 1.507 |

c_disk = 1.507 ns per pixel for the product's way (one file per frame through the file cache, at the worst live rate); 0.756 without the cache. The System process, which does the file cache's lazy writes, refused the processor-time query (`OpenProcess` on PID 4), so that kernel work is **not** in the buffered figure. This disk sustained 3,004 MB/s with the cache bypassed and took the worst live rate (166 MB/s) for 2 s with no late frame: on this PC the disk is not the limit, its processor cost is. Weakest machine (estimate): 5.54 ns per pixel (2.78 without the cache), 11.5 ms per 1920 x 1080 frame; its disk speed is not estimable from this PC.

## M6: threading rule (C3)

Command `clip-probe threading` (03:16 IST). Counter window 1280 x 720, interval Default (47.9 delivered per second). The handler copies each frame on the GPU into the slot of its instant and never maps (`slots.rs`); multithread protection is switched on for the capture device (`ID3D11Multithread::SetMultithreadProtected(true)`); a reader thread picks the frame for each 50 ms instant 10 ms after it and reads it back. 8 s per way; every instant had a new frame (160 reads).

| Reader's Map | Handler entry to return: max / p99 | Read-back wall ms mean / p99 | Reader processor ms per read | Map retries per read |
|---|---|---|---|---|
| blocking | 1.999 / 1.917 ms | 2.353 / 3.008 | 0.398 | 0 |
| do not wait + yield | 1.713 / 0.064 ms | 2.353 / 3.288 | 2.221 | 3,050 |
| do not wait + 1 ms sleep | 0.111 / 0.061 ms | 3.415 / 4.082 | 0.502 | 1.9 |

In all three: 0 lost frames, 0 instants late, at most 3 slots in use, 0 frames delivered later than the 10 ms margin. A blocking Map holds the device lock while it waits, and the handler's next GPU copy waits behind it (up to 2.0 ms). "Do not wait" with a yield loop releases the lock but spins the processor. "Do not wait" with a 1 ms sleep keeps the handler under 0.12 ms for 0.1 ms more reader processor time than blocking. The run without multithread protection was not made: sharing the immediate context between two threads without it is undefined behaviour.

## M7: selection lateness (MAX_SELECTION_LATENESS_INSTANTS)

Command `clip-probe lateness` (03:17 IST). Counter window 854 x 480; every new frame is read back and written raw to `%LOCALAPPDATA%\Temp` (as the live part will); 4 s idle, then 4 s with one spinning thread per logical processor (16, normal priority).

| Reader's Map | Phase | Read-back + write ms mean / p99 / max | Instants late (max) | Lost frames | Slots in use (max) |
|---|---|---|---|---|---|
| blocking | idle | 2.347 / 3.104 / 3.172 | 0 | 0 | 3 |
| blocking | full load | 5.247 / 12.647 / 18.742 | 0 | 0 | 3 |
| do not wait + 1 ms sleep | idle | 2.796 / 4.573 / 5.048 | 0 | 0 | 3 |
| do not wait + 1 ms sleep | full load | 5.443 / 14.720 / 17.489 | 0 | 0 | 3 |

The reader never fell behind on this PC, even with every processor busy. Weakest machine (estimate): the slowest read plus write, 18.7 ms x 3.68 = 69 ms, passes one instant (50 ms), so the reader can be 1 instant late; its mean (20 ms) lets it catch up. Hence `MAX_SELECTION_LATENESS_INSTANTS` = 2 (1 plus one of margin).

## M8: Media Foundation H.264 encoder settings (GOP_FRAMES, O4)

Command `clip-probe encoder` (03:21 IST), then `ffprobe -v error -select_streams v:0 -show_entries frame=key_frame,pict_type -of csv=p=0 <file>` on each output (files deleted afterwards). 100 frames of 1280 x 720 NV12 with moving text-like stripes, sample durations 50, 50, 150, 50, 100 ms repeating (as when repeated frames are not encoded); fragmented MP4 as in the product. The settings were handed over at `SetInputMediaType` (accepted at open in every run), then set again through `ICodecAPI` and read back.

H.264 encoders present: hardware: NVIDIA H.264 Encoder MFT (VEN_10DE), Intel Quick Sync Video H.264 Encoder MFT (VEN_8086, listed twice); software: H264 Encoder MFT.

| Run | Encoder granted | GOP 20 | B-pictures 0 | Quality mode, level 70 | Screen hint (display remoting) | Keyframes found | Frame types | Bytes | Processor ms per frame |
|---|---|---|---|---|---|---|---|---|---|
| software, no hint | software MFT (hardware transforms off) | read back 20 | read back 0 | read back 3 / 70 | not set | 0, 20, 40, 60, 80 | 5 I, 95 P | 1,048,732 | 4.30 |
| software, hint | software MFT | 20 | 0 | 3 / 70 | read back 1 | 0, 20, 40, 60, 80 | 5 I, 95 P | 1,048,732 | 4.49 |
| hardware, no hint | Intel Quick Sync (VEN_8086) | 20 | 0 | 3 / 70 | not set | 0, 20, 40, 60, 80 | 5 I, 95 P | 724,299 | 1.34 |
| hardware, hint | Intel Quick Sync (VEN_8086) | 20 | 0 | 3 / 70 | read back 1 | 0, 20, 40, 60, 80 | 5 I, 95 P | 723,975 | 1.24 |

Every setting was reported supported, accepted through `ICodecAPI`, and read back with the value asked for. Keyframes fall every 20 encoded frames regardless of the uneven durations, so `GOP_FRAMES` counts frames as the plan requires (S5). The processor time per frame is the encoder path only (frames were made before the clock started): 4.66 to 4.87 ns per pixel for the software encoder and 1.35 to 1.45 for Quick Sync, on synthetic content; the equation keeps its real-screen figures (7.0 and 2.0). Weakest machine: the same Windows software encoder is expected to accept the same settings (not verified); whether Quick Sync exists on a 2011 Intel chip under a current driver stays unverified.

## M9: live share and the worker's remainder

Command `python -I run_equation.py` (03:23 IST), with the measured c_obs, c_read, c_disk, f_o and n_new. Live part: a_rt = f_o · c_obs + n_new · w · h · (c_read + c_disk); worker cap = 5 % − live share (E3). Where a window changes on every refresh the machine's f_o is used (72 here, 60 on the weakest machine).

Highlights (full tables below):

- Development PC: live share 0.01 % (caret, 854 x 480) to 0.65 % (1920 x 1080, changes every recorded frame); worker cap at least 4.35 %.
- Weakest machine (estimate), through the file cache: 0.18 % to 3.11 % for caret, the 3.8-per-second text window and typing at every size; 3.54 % at 854 x 480 with mouse movement; **5.17 % at 1280 x 720 and 9.14 % at 1920 x 1080 with mouse movement or a window that changes every recorded frame**: the live part alone passes 5 %, the worker gets nothing, and E1 load shedding applies.
- Without the file cache (c_disk 0.756), the weakest machine's worst case at 1280 x 720 drops to 3.90 % (worker cap 1.10 %), at 1920 x 1080 to 6.27 % (still above 5 %).

### Output of run_equation.py

speed ratio 3.68

| Measured cost (development PC) | Value | Weakest machine (estimate) |
|---|---|---|
| c_obs, whole process | 0.373 ms per delivered frame | 1.372 ms per delivered frame |
| c_obs, capture thread only | 0.245 ms per delivered frame | 0.901 ms per delivered frame |
| c_read 854 x 480 | 0.456 ns per pixel | 1.677 ns per pixel |
| c_read 1280 x 720 | 0.330 ns per pixel | 1.214 ns per pixel |
| c_read 1920 x 1080 | 0.349 ns per pixel | 1.284 ns per pixel |
| c_disk, through the file cache | 1.507 ns per pixel | 5.543 ns per pixel |
| c_disk, no cache, write-through | 0.756 ns per pixel | 2.781 ns per pixel |
| c_enc, Media Foundation software (synthetic) | 4.870 ns per pixel | 17.914 ns per pixel |
| c_enc, Media Foundation Quick Sync (synthetic) | 1.450 ns per pixel | 5.334 ns per pixel |

**M9: live share and the worker's remainder (5 % minus the live share)**

| Machine | Window | Activity | Live share | Worker cap |
|---|---|---|---|---|
| development PC | 854 x 480 | caret blinking (M4) | 0.01 % | 4.99 % |
| development PC | 854 x 480 | changing text window (equation, 3.8/s) | 0.03 % | 4.97 % |
| development PC | 854 x 480 | typing 8 chars/s (M4) | 0.06 % | 4.94 % |
| development PC | 854 x 480 | mouse over window, cursor in frames (M4) | 0.27 % | 4.73 % |
| development PC | 854 x 480 | changes every refresh (M3 counter window) | 0.27 % | 4.73 % |
| development PC | 1280 x 720 | caret blinking (M4) | 0.02 % | 4.98 % |
| development PC | 1280 x 720 | changing text window (equation, 3.8/s) | 0.05 % | 4.95 % |
| development PC | 1280 x 720 | typing 8 chars/s (M4) | 0.10 % | 4.90 % |
| development PC | 1280 x 720 | mouse over window, cursor in frames (M4) | 0.38 % | 4.62 % |
| development PC | 1280 x 720 | changes every refresh (M3 counter window) | 0.38 % | 4.62 % |
| development PC | 1920 x 1080 | caret blinking (M4) | 0.04 % | 4.96 % |
| development PC | 1920 x 1080 | changing text window (equation, 3.8/s) | 0.10 % | 4.90 % |
| development PC | 1920 x 1080 | typing 8 chars/s (M4) | 0.21 % | 4.79 % |
| development PC | 1920 x 1080 | mouse over window, cursor in frames (M4) | 0.65 % | 4.35 % |
| development PC | 1920 x 1080 | changes every refresh (M3 counter window) | 0.65 % | 4.35 % |
| weakest machine (estimate) | 854 x 480 | caret blinking (M4) | 0.18 % | 4.82 % |
| weakest machine (estimate) | 854 x 480 | changing text window (equation, 3.8/s) | 0.41 % | 4.59 % |
| weakest machine (estimate) | 854 x 480 | typing 8 chars/s (M4) | 0.87 % | 4.13 % |
| weakest machine (estimate) | 854 x 480 | mouse over window, cursor in frames (M4) | 3.54 % | 1.46 % |
| weakest machine (estimate) | 854 x 480 | changes every refresh (M3 counter window) | 3.54 % | 1.46 % |
| weakest machine (estimate) | 1280 x 720 | caret blinking (M4) | 0.32 % | 4.68 % |
| weakest machine (estimate) | 1280 x 720 | changing text window (equation, 3.8/s) | 0.73 % | 4.27 % |
| weakest machine (estimate) | 1280 x 720 | typing 8 chars/s (M4) | 1.52 % | 3.48 % |
| weakest machine (estimate) | 1280 x 720 | mouse over window, cursor in frames (M4) | 5.17 % | none: live part alone above 5 % (E1 load shedding) |
| weakest machine (estimate) | 1280 x 720 | changes every refresh (M3 counter window) | 5.17 % | none: live part alone above 5 % (E1 load shedding) |
| weakest machine (estimate) | 1920 x 1080 | caret blinking (M4) | 0.65 % | 4.35 % |
| weakest machine (estimate) | 1920 x 1080 | changing text window (equation, 3.8/s) | 1.49 % | 3.51 % |
| weakest machine (estimate) | 1920 x 1080 | typing 8 chars/s (M4) | 3.11 % | 1.89 % |
| weakest machine (estimate) | 1920 x 1080 | mouse over window, cursor in frames (M4) | 9.14 % | none: live part alone above 5 % (E1 load shedding) |
| weakest machine (estimate) | 1920 x 1080 | changes every refresh (M3 counter window) | 9.14 % | none: live part alone above 5 % (E1 load shedding) |

**Live share with unbuffered raw writes (c_disk without the file cache)**

| Machine | Window | Activity | Live share | Worker cap |
|---|---|---|---|---|
| development PC | 1280 x 720 | typing 8 chars/s (M4) | 0.07 % | 4.93 % |
| development PC | 1280 x 720 | mouse over window, cursor in frames (M4) | 0.29 % | 4.71 % |
| development PC | 1280 x 720 | changes every refresh (M3 counter window) | 0.29 % | 4.71 % |
| development PC | 1920 x 1080 | typing 8 chars/s (M4) | 0.13 % | 4.87 % |
| development PC | 1920 x 1080 | mouse over window, cursor in frames (M4) | 0.45 % | 4.55 % |
| development PC | 1920 x 1080 | changes every refresh (M3 counter window) | 0.45 % | 4.55 % |
| weakest machine (estimate) | 1280 x 720 | typing 8 chars/s (M4) | 1.01 % | 3.99 % |
| weakest machine (estimate) | 1280 x 720 | mouse over window, cursor in frames (M4) | 3.90 % | 1.10 % |
| weakest machine (estimate) | 1280 x 720 | changes every refresh (M3 counter window) | 3.90 % | 1.10 % |
| weakest machine (estimate) | 1920 x 1080 | typing 8 chars/s (M4) | 1.96 % | 3.04 % |
| weakest machine (estimate) | 1920 x 1080 | mouse over window, cursor in frames (M4) | 6.27 % | none (E1) |
| weakest machine (estimate) | 1920 x 1080 | changes every refresh (M3 counter window) | 6.27 % | none (E1) |

**Raw write volume (E8): n_new * width * height * 4 bytes**

| Window | Activity | MB per second | GB per hour |
|---|---|---|---|
| 854 x 480 | caret blinking (M4) | 2.7 | 10 |
| 854 x 480 | changing text window (equation, 3.8/s) | 6.3 | 23 |
| 854 x 480 | typing 8 chars/s (M4) | 13.1 | 47 |
| 854 x 480 | mouse over window, cursor in frames (M4) | 32.8 | 118 |
| 854 x 480 | changes every refresh (M3 counter window) | 32.8 | 118 |
| 1280 x 720 | caret blinking (M4) | 6.2 | 22 |
| 1280 x 720 | changing text window (equation, 3.8/s) | 14.1 | 51 |
| 1280 x 720 | typing 8 chars/s (M4) | 29.5 | 106 |
| 1280 x 720 | mouse over window, cursor in frames (M4) | 73.7 | 265 |
| 1280 x 720 | changes every refresh (M3 counter window) | 73.7 | 265 |
| 1920 x 1080 | caret blinking (M4) | 13.9 | 50 |
| 1920 x 1080 | changing text window (equation, 3.8/s) | 31.8 | 114 |
| 1920 x 1080 | typing 8 chars/s (M4) | 66.4 | 239 |
| 1920 x 1080 | mouse over window, cursor in frames (M4) | 165.9 | 597 |
| 1920 x 1080 | changes every refresh (M3 counter window) | 165.9 | 597 |

**Weakest machine (estimate), software encoder: time until a 60 s clip is ready**

| Window | Activity | Live share | Deferred work if done live | Ready after |
|---|---|---|---|---|
| 854 x 480 | caret blinking (M4) | 0.18 % | 0.5 % | 60 s |
| 854 x 480 | changing text window (equation, 3.8/s) | 0.41 % | 1.2 % | 60 s |
| 854 x 480 | typing 8 chars/s (M4) | 0.87 % | 2.6 % | 60 s |
| 854 x 480 | mouse over window, cursor in frames (M4) | 3.54 % | 6.5 % | 268 s |
| 854 x 480 | changes every refresh (M3 counter window) | 3.54 % | 6.5 % | 268 s |
| 1280 x 720 | caret blinking (M4) | 0.32 % | 1.2 % | 60 s |
| 1280 x 720 | changing text window (equation, 3.8/s) | 0.73 % | 2.8 % | 60 s |
| 1280 x 720 | typing 8 chars/s (M4) | 1.52 % | 5.9 % | 101 s |
| 1280 x 720 | mouse over window, cursor in frames (M4) | 5.17 % | 14.7 % | never within 5 %: live part alone uses the budget |
| 1280 x 720 | changes every refresh (M3 counter window) | 5.17 % | 14.7 % | never within 5 %: live part alone uses the budget |
| 1920 x 1080 | caret blinking (M4) | 0.65 % | 2.8 % | 60 s |
| 1920 x 1080 | changing text window (equation, 3.8/s) | 1.49 % | 6.3 % | 108 s |
| 1920 x 1080 | typing 8 chars/s (M4) | 3.11 % | 13.2 % | 418 s |
| 1920 x 1080 | mouse over window, cursor in frames (M4) | 9.14 % | 33.0 % | never within 5 %: live part alone uses the budget |
| 1920 x 1080 | changes every refresh (M3 counter window) | 9.14 % | 33.0 % | never within 5 %: live part alone uses the budget |

**Weakest machine (estimate), Quick Sync, conversion on the processor: time until a 60 s clip is ready**

| Window | Activity | Live share | Deferred work if done live | Ready after |
|---|---|---|---|---|
| 854 x 480 | caret blinking (M4) | 0.18 % | 0.2 % | 60 s |
| 854 x 480 | changing text window (equation, 3.8/s) | 0.41 % | 0.5 % | 60 s |
| 854 x 480 | typing 8 chars/s (M4) | 0.87 % | 1.1 % | 60 s |
| 854 x 480 | mouse over window, cursor in frames (M4) | 3.54 % | 2.8 % | 113 s |
| 854 x 480 | changes every refresh (M3 counter window) | 3.54 % | 2.8 % | 113 s |
| 1280 x 720 | caret blinking (M4) | 0.32 % | 0.5 % | 60 s |
| 1280 x 720 | changing text window (equation, 3.8/s) | 0.73 % | 1.2 % | 60 s |
| 1280 x 720 | typing 8 chars/s (M4) | 1.52 % | 2.5 % | 60 s |
| 1280 x 720 | mouse over window, cursor in frames (M4) | 5.17 % | 6.2 % | never within 5 %: live part alone uses the budget |
| 1280 x 720 | changes every refresh (M3 counter window) | 5.17 % | 6.2 % | never within 5 %: live part alone uses the budget |
| 1920 x 1080 | caret blinking (M4) | 0.65 % | 1.2 % | 60 s |
| 1920 x 1080 | changing text window (equation, 3.8/s) | 1.49 % | 2.7 % | 60 s |
| 1920 x 1080 | typing 8 chars/s (M4) | 3.11 % | 5.6 % | 176 s |
| 1920 x 1080 | mouse over window, cursor in frames (M4) | 9.14 % | 13.9 % | never within 5 %: live part alone uses the budget |
| 1920 x 1080 | changes every refresh (M3 counter window) | 9.14 % | 13.9 % | never within 5 %: live part alone uses the budget |

**Weakest machine (estimate), Quick Sync, conversion on the graphics chip: time until a 60 s clip is ready**

| Window | Activity | Live share | Deferred work if done live | Ready after |
|---|---|---|---|---|
| 854 x 480 | caret blinking (M4) | 0.18 % | 0.1 % | 60 s |
| 854 x 480 | changing text window (equation, 3.8/s) | 0.41 % | 0.3 % | 60 s |
| 854 x 480 | typing 8 chars/s (M4) | 0.87 % | 0.6 % | 60 s |
| 854 x 480 | mouse over window, cursor in frames (M4) | 3.54 % | 1.5 % | 62 s |
| 854 x 480 | changes every refresh (M3 counter window) | 3.54 % | 1.5 % | 62 s |
| 1280 x 720 | caret blinking (M4) | 0.32 % | 0.3 % | 60 s |
| 1280 x 720 | changing text window (equation, 3.8/s) | 0.73 % | 0.6 % | 60 s |
| 1280 x 720 | typing 8 chars/s (M4) | 1.52 % | 1.4 % | 60 s |
| 1280 x 720 | mouse over window, cursor in frames (M4) | 5.17 % | 3.4 % | never within 5 %: live part alone uses the budget |
| 1280 x 720 | changes every refresh (M3 counter window) | 5.17 % | 3.4 % | never within 5 %: live part alone uses the budget |
| 1920 x 1080 | caret blinking (M4) | 0.65 % | 0.6 % | 60 s |
| 1920 x 1080 | changing text window (equation, 3.8/s) | 1.49 % | 1.5 % | 60 s |
| 1920 x 1080 | typing 8 chars/s (M4) | 3.11 % | 3.1 % | 97 s |
| 1920 x 1080 | mouse over window, cursor in frames (M4) | 9.14 % | 7.6 % | never within 5 %: live part alone uses the budget |
| 1920 x 1080 | changes every refresh (M3 counter window) | 9.14 % | 7.6 % | never within 5 %: live part alone uses the budget |

**Bytes, Windows software encoder at quality 70 (2026-11-21)**

| Window | Content | Keyframe every 1 s | Keyframe every 5 s |
|---|---|---|---|
| 854 x 480 | small changing text | 5.5 MB/min | 3.3 MB/min |
| 854 x 480 | still window | 0.7 MB/min | 0.1 MB/min |
| 1280 x 720 | small changing text | 12.4 MB/min | 7.4 MB/min |
| 1280 x 720 | still window | 1.5 MB/min | 0.3 MB/min |
| 1920 x 1080 | small changing text | 27.9 MB/min | 16.7 MB/min |
| 1920 x 1080 | still window | 3.3 MB/min | 0.7 MB/min |
| 1252 x 890 (measured) | small changing text | 15.0 MB/min | 9.0 MB/min |
| 1252 x 890 (measured) | still window | 1.8 MB/min | 0.4 MB/min |

## Findings that contradict or change the plan (the plan was not edited)

1. **`Frame::timestamp()` is not the composition moment.** `SystemRelativeTime` lies after the moment the frame reaches the handler in every one of the 3,742 frames of M2, M3, M6 and M7 (on average 5.5 ms after it, at least 3.2 ms after it where arrival was recorded, at least 1.98 ms after the handler returned in M7), and consecutive values are exact multiples of the refresh period. It behaves as the frame's target display (vertical blank) time. Consequences: (a) the O3 "hold time, composition time to frame release" comes out negative; hold time has to be measured from arrival (handler entry) to release; (b) choosing by this time picks the frame that is on screen at s_k, which serves condition C (2); (c) a frame for s_k arrives before s_k, so the reader needs no waiting margin beyond that (0 frames arrived later than 10 ms after their instant). F2/F5 and O2 should say "display time" instead of "composition time".
2. **Default interval is not "every composition".** On this Windows 11 26H2 PC, Default delivered 48 per second on a 144 Hz panel (every third refresh, 20.8 ms gaps), exactly like a 16.7 ms request; it did not deliver per composition up to the display rate as O1 and the "Requested interval" paragraph assume for the Default path. Where the interval setting is missing (Windows 10, Windows 11 before 24H2) condition C (1) is therefore not established; on a 144 Hz panel it fails (20.8 ms > 16.7 ms). The Windows 10 behaviour must be measured in proof case 4.
3. **Mouse movement is the worst case, and only because of the cursor.** With the cursor drawn into the frames (O1 asks for it where supported), moving the mouse over the window gives a new frame at every instant (20 per second): 166 MB/s of raw writes at 1920 x 1080 (597 GB per hour), against E8's accepted 31 MB/s. With the cursor left out, the same movement gives no frame. A8 already logs the cursor position per instant and D5 masks the cursor, so recording without the cursor and drawing it from the log would remove this load. This is an owner decision under E8.
4. **The live part can pass 5 % on the weakest machine** (estimate): 5.17 % at 1280 x 720 and 9.14 % at 1920 x 1080 when the window changes every recorded frame, mouse movement included. The review's estimate that the live part stays under 5 % at 1920 x 1080 holds only for the lighter activities. c_obs alone at 60 frames per second is 2.06 % of that machine.
5. **The raw write costs more processor time than the read-back.** c_disk through the file cache is 1.507 ns per pixel against c_read 0.35: the disk write, not the read-back, dominates a_rt. Writing without the file cache halves c_disk (0.756) on this NVMe disk; on a hard disk it would also make the worker's reads slower. The kernel's lazy-writer time for cached writes could not be read and is not in 1.507.
6. **Slot count.** With the probe's rule (the reader frees older slots when it picks the next instant) 3 slots were in use at lateness 0: the slot being read, the frame pinned for the next instant (frames arrive before their time, finding 1) and the candidate. The plan's `OBSERVE_SLOTS` = lateness + 2 is one short for that rule; it suffices only if package 1 frees a slot as soon as it has been read and a newer frame is pinned.
7. **"Do not wait" + retry needs a sleep, not a spin.** C3 proposes `Map` with "do not wait" and retry. With a yield loop the reader spends 2.2 ms of processor time per 1280 x 720 read (5.5 times blocking); with a 1 ms sleep, 0.5 ms, and the handler's longest time drops from 2.0 ms (blocking) to 0.11 ms.
8. **The encoder does not name itself.** On this PC neither the software MFT nor Intel Quick Sync carries `MFT_FRIENDLY_NAME_Attribute` on the transform the sink writer inserted, so the product's `encoder_identity` (`clip_encoder.rs`) reports "unnamed"; F8's "reports which one it got" does not hold here. The hardware vendor id (`MFT_ENUM_HARDWARE_VENDOR_ID_Attribute`, here VEN_8086) is present. Media Foundation granted Intel Quick Sync, although NVIDIA is listed first.
9. **E8 typical figure.** "Typical 3.8 new frames per second" is below what ordinary typing gives (8 per second at 8 characters per second, one frame per character): 66 MB/s at 1920 x 1080.

## Not measured, and why

- Delivery at 60 Hz and the Windows 10 Default path: the only display here runs at 144 Hz and the owner's display settings are not changed; there is no Windows 10 machine. Proof case 4 covers Windows 10.
- The file cache's lazy-writer processor time (System process): Windows refused the query.
- Anything on the weakest machine: every weakest-machine figure is an estimate (processor costs x 3.68); disk speed, GPU read-back on a 2011 Intel chip and Quick Sync availability there cannot be estimated from this PC.
- GPU time: only processor time was measured.
- The NVIDIA hardware encoder: the sink writer chose Intel Quick Sync and offers no way to pick the MFT.
- The run without multithread protection (M6): undefined behaviour, not run.
- Byte effect of the screen-content hint on real screen content, q_min, K, P: proof cases 13 to 15. `scripts/clip-equation/flash-page.html` (plan proof, "written in package 0") was not written: this package used the probe's own counter window instead of a browser page.

## Raw probe output

M1 (`clip-probe readback`):

```
cycle_counter_hz=2894548205
default_adapter="Intel(R) UHD Graphics"
M1 adapter="Intel(R) UHD Graphics" size=854x480 way=reused_staging+blocking_map wall_ms mean=1.138 p50=1.131 p99=1.261 max=1.287 n=120 | thread_cpu_ms mean=0.127 p50=0.123 p99=0.212 max=0.222 n=120 | thread_cpu_ns_per_px=0.3098 process_cpu_ns_per_px=0.3895 map_retries_per_read=0.0
M1 adapter="Intel(R) UHD Graphics" size=854x480 way=reused_staging+do_not_wait_yield_retry wall_ms mean=1.035 p50=1.026 p99=1.187 max=1.306 n=120 | thread_cpu_ms mean=1.032 p50=1.027 p99=1.164 max=1.312 n=120 | thread_cpu_ns_per_px=2.5168 process_cpu_ns_per_px=4.8717 map_retries_per_read=2189.0
M1 adapter="Intel(R) UHD Graphics" size=854x480 way=reused_staging+do_not_wait_sleep1ms_retry wall_ms mean=1.614 p50=1.611 p99=2.028 max=2.833 n=120 | thread_cpu_ms mean=0.164 p50=0.150 p99=0.286 max=0.830 n=120 | thread_cpu_ns_per_px=0.3999 process_cpu_ns_per_px=0.5366 map_retries_per_read=1.0
M1 adapter="Intel(R) UHD Graphics" size=854x480 way=new_staging_per_frame+blocking_map wall_ms mean=1.537 p50=1.525 p99=1.720 max=2.792 n=120 | thread_cpu_ms mean=0.467 p50=0.453 p99=0.590 max=1.687 n=120 | thread_cpu_ns_per_px=1.1386 process_cpu_ns_per_px=1.2748 map_retries_per_read=0.0
M1 adapter="Intel(R) UHD Graphics" size=1280x720 way=reused_staging+blocking_map wall_ms mean=2.105 p50=2.091 p99=2.417 max=2.462 n=120 | thread_cpu_ms mean=0.265 p50=0.244 p99=0.566 max=0.593 n=120 | thread_cpu_ns_per_px=0.2877 process_cpu_ns_per_px=0.3301 map_retries_per_read=0.0
M1 adapter="Intel(R) UHD Graphics" size=1280x720 way=reused_staging+do_not_wait_yield_retry wall_ms mean=1.964 p50=1.948 p99=2.099 max=2.105 n=120 | thread_cpu_ms mean=1.927 p50=1.935 p99=2.079 max=2.098 n=120 | thread_cpu_ns_per_px=2.0905 process_cpu_ns_per_px=3.9867 map_retries_per_read=4016.5
M1 adapter="Intel(R) UHD Graphics" size=1280x720 way=reused_staging+do_not_wait_sleep1ms_retry wall_ms mean=1.774 p50=1.595 p99=3.316 max=3.491 n=120 | thread_cpu_ms mean=0.249 p50=0.227 p99=0.455 max=1.420 n=120 | thread_cpu_ns_per_px=0.2700 process_cpu_ns_per_px=0.3289 map_retries_per_read=1.1
M1 adapter="Intel(R) UHD Graphics" size=1280x720 way=new_staging_per_frame+blocking_map wall_ms mean=1.542 p50=1.537 p99=1.788 max=1.822 n=120 | thread_cpu_ms mean=0.897 p50=0.893 p99=1.088 max=1.182 n=120 | thread_cpu_ns_per_px=0.9731 process_cpu_ns_per_px=1.0413 map_retries_per_read=0.0
M1 adapter="Intel(R) UHD Graphics" size=1920x1080 way=reused_staging+blocking_map wall_ms mean=1.496 p50=1.462 p99=1.917 max=2.680 n=120 | thread_cpu_ms mean=0.596 p50=0.587 p99=0.955 max=1.820 n=120 | thread_cpu_ns_per_px=0.2874 process_cpu_ns_per_px=0.3074 map_retries_per_read=0.0
M1 adapter="Intel(R) UHD Graphics" size=1920x1080 way=reused_staging+do_not_wait_yield_retry wall_ms mean=1.342 p50=1.336 p99=1.515 max=1.520 n=120 | thread_cpu_ms mean=1.317 p50=1.320 p99=1.499 max=1.521 n=120 | thread_cpu_ns_per_px=0.6351 process_cpu_ns_per_px=1.0484 map_retries_per_read=1854.5
M1 adapter="Intel(R) UHD Graphics" size=1920x1080 way=reused_staging+do_not_wait_sleep1ms_retry wall_ms mean=1.836 p50=1.797 p99=2.691 max=2.751 n=120 | thread_cpu_ms mean=0.571 p50=0.548 p99=1.032 max=1.303 n=120 | thread_cpu_ns_per_px=0.2756 process_cpu_ns_per_px=0.3057 map_retries_per_read=1.0
M1 adapter="Intel(R) UHD Graphics" size=1920x1080 way=new_staging_per_frame+blocking_map wall_ms mean=3.213 p50=3.186 p99=3.718 max=3.769 n=120 | thread_cpu_ms mean=1.903 p50=1.890 p99=2.123 max=2.153 n=120 | thread_cpu_ns_per_px=0.9175 process_cpu_ns_per_px=0.9531 map_retries_per_read=0.0
M1 adapter="NVIDIA GeForce RTX 4050 Laptop GPU" size=854x480 way=reused_staging+blocking_map wall_ms mean=0.806 p50=0.681 p99=1.848 max=1.911 n=120 | thread_cpu_ms mean=0.205 p50=0.198 p99=0.287 max=0.328 n=120 | thread_cpu_ns_per_px=0.5012 process_cpu_ns_per_px=2.2929 map_retries_per_read=0.0
M1 adapter="NVIDIA GeForce RTX 4050 Laptop GPU" size=854x480 way=reused_staging+do_not_wait_yield_retry wall_ms mean=0.502 p50=0.603 p99=1.291 max=1.895 n=120 | thread_cpu_ms mean=0.492 p50=0.586 p99=1.299 max=1.883 n=120 | thread_cpu_ns_per_px=1.2003 process_cpu_ns_per_px=1.4352 map_retries_per_read=2300.3
M1 adapter="NVIDIA GeForce RTX 4050 Laptop GPU" size=854x480 way=reused_staging+do_not_wait_sleep1ms_retry wall_ms mean=6.037 p50=6.079 p99=6.777 max=6.896 n=120 | thread_cpu_ms mean=0.236 p50=0.238 p99=0.335 max=0.359 n=120 | thread_cpu_ns_per_px=0.5768 process_cpu_ns_per_px=1.1969 map_retries_per_read=4.0
M1 adapter="NVIDIA GeForce RTX 4050 Laptop GPU" size=854x480 way=new_staging_per_frame+blocking_map wall_ms mean=0.571 p50=0.562 p99=0.692 max=0.905 n=120 | thread_cpu_ms mean=0.485 p50=0.482 p99=0.555 max=0.556 n=120 | thread_cpu_ns_per_px=1.1824 process_cpu_ns_per_px=1.9424 map_retries_per_read=0.0
M1 adapter="NVIDIA GeForce RTX 4050 Laptop GPU" size=1280x720 way=reused_staging+blocking_map wall_ms mean=0.571 p50=0.529 p99=1.158 max=1.179 n=120 | thread_cpu_ms mean=0.300 p50=0.291 p99=0.409 max=0.411 n=120 | thread_cpu_ns_per_px=0.3259 process_cpu_ns_per_px=0.7833 map_retries_per_read=0.0
M1 adapter="NVIDIA GeForce RTX 4050 Laptop GPU" size=1280x720 way=reused_staging+do_not_wait_yield_retry wall_ms mean=0.532 p50=0.505 p99=0.950 max=1.037 n=120 | thread_cpu_ms mean=0.522 p50=0.499 p99=0.920 max=0.950 n=120 | thread_cpu_ns_per_px=0.5663 process_cpu_ns_per_px=0.6729 map_retries_per_read=1867.3
M1 adapter="NVIDIA GeForce RTX 4050 Laptop GPU" size=1280x720 way=reused_staging+do_not_wait_sleep1ms_retry wall_ms mean=6.072 p50=6.075 p99=7.137 max=7.492 n=120 | thread_cpu_ms mean=0.399 p50=0.391 p99=0.589 max=0.604 n=120 | thread_cpu_ns_per_px=0.4333 process_cpu_ns_per_px=0.7486 map_retries_per_read=4.0
M1 adapter="NVIDIA GeForce RTX 4050 Laptop GPU" size=1280x720 way=new_staging_per_frame+blocking_map wall_ms mean=1.242 p50=1.179 p99=1.850 max=2.200 n=120 | thread_cpu_ms mean=0.982 p50=0.936 p99=1.553 max=1.884 n=120 | thread_cpu_ns_per_px=1.0656 process_cpu_ns_per_px=1.6031 map_retries_per_read=0.0
M1 adapter="NVIDIA GeForce RTX 4050 Laptop GPU" size=1920x1080 way=reused_staging+blocking_map wall_ms mean=1.279 p50=1.219 p99=1.803 max=2.051 n=120 | thread_cpu_ms mean=0.641 p50=0.595 p99=1.087 max=1.114 n=120 | thread_cpu_ns_per_px=0.3093 process_cpu_ns_per_px=0.6914 map_retries_per_read=0.0
M1 adapter="NVIDIA GeForce RTX 4050 Laptop GPU" size=1920x1080 way=reused_staging+do_not_wait_yield_retry wall_ms mean=1.334 p50=1.270 p99=2.077 max=2.190 n=120 | thread_cpu_ms mean=1.338 p50=1.278 p99=2.052 max=2.185 n=120 | thread_cpu_ns_per_px=0.6451 process_cpu_ns_per_px=0.7031 map_retries_per_read=4094.8
M1 adapter="NVIDIA GeForce RTX 4050 Laptop GPU" size=1920x1080 way=reused_staging+do_not_wait_sleep1ms_retry wall_ms mean=6.640 p50=6.622 p99=9.622 max=10.039 n=120 | thread_cpu_ms mean=0.907 p50=0.882 p99=1.390 max=1.422 n=120 | thread_cpu_ns_per_px=0.4375 process_cpu_ns_per_px=0.5446 map_retries_per_read=4.0
M1 adapter="NVIDIA GeForce RTX 4050 Laptop GPU" size=1920x1080 way=new_staging_per_frame+blocking_map wall_ms mean=2.549 p50=2.523 p99=3.031 max=3.090 n=120 | thread_cpu_ms mean=1.940 p50=1.920 p99=2.204 max=2.464 n=120 | thread_cpu_ns_per_px=0.9353 process_cpu_ns_per_px=1.3591 map_retries_per_read=0.0
```

M2 (`clip-probe observe`):

```
cycle_counter_hz=2871901674 capture_device_adapter="Intel(R) UHD Graphics"
M2 delivered_frames=480 delivered_per_s=48.00
M2 gap_ms mean=20.831 p50=20.831 p99=20.832 max=20.832 n=479 gaps_over_T_r(16.67ms)=479
M2 hold_ms(composition->release) mean=-5.458 p50=-5.489 p99=-4.781 max=-4.541 n=480
M2 handler_ms(entry->return) mean=0.006 p50=0.006 p99=0.011 max=0.035 n=480
M2 delivery_latency_ms(composition->entry) mean=-5.464 p50=-5.494 p99=-4.787 max=-4.548 n=480
M2 capture_thread_cpu_us_per_frame=245.12 handler_cpu_us_per_frame=5.17
M2 painter paints_baseline=720 paints_measured=1441 painter_cpu_ms_baseline=155.0 painter_cpu_ms_measured=321.7
M2 c_obs_capture_thread_us=245.12 c_obs_process_rest_us=372.73 handler_only_us=5.17
```

M3 (`clip-probe delivery`):

```
M3 display_refresh=10000000/69434
M3 interval=default delivered_frames=385 delivered_per_s=48.12
M3 interval=default gap_ms mean=20.831 p50=20.831 p99=20.832 max=20.832 n=384 gaps_over_T_r(16.67ms)=384
M3 interval=default hold_ms(composition->release) mean=-5.448 p50=-5.476 p99=-4.844 max=-2.659 n=385
M3 interval=default handler_ms(entry->return) mean=0.075 p50=0.052 p99=0.899 max=1.918 n=385
M3 interval=default delivery_latency_ms(composition->entry) mean=-5.523 p50=-5.531 p99=-5.031 max=-4.577 n=385
M3 interval=default capture_thread_cpu_us_per_frame=256.39 handler_cpu_us_per_frame=71.64
M3 interval=default paints=1153 frames_with_counter=385 paints_not_delivered=768 longest_run_not_delivered=2 repeated_paint_frames=0 unread=0
M3 interval=16.7ms delivered_frames=384 delivered_per_s=48.00
M3 interval=16.7ms gap_ms mean=20.831 p50=20.831 p99=20.832 max=20.832 n=383 gaps_over_T_r(16.67ms)=383
M3 interval=16.7ms hold_ms(composition->release) mean=-5.430 p50=-5.460 p99=-4.822 max=-1.369 n=384
M3 interval=16.7ms handler_ms(entry->return) mean=0.080 p50=0.051 p99=0.407 max=4.024 n=384
M3 interval=16.7ms delivery_latency_ms(composition->entry) mean=-5.509 p50=-5.520 p99=-5.090 max=-4.544 n=384
M3 interval=16.7ms capture_thread_cpu_us_per_frame=266.46 handler_cpu_us_per_frame=77.94
M3 interval=16.7ms paints=1152 frames_with_counter=384 paints_not_delivered=766 longest_run_not_delivered=2 repeated_paint_frames=0 unread=0
M3 interval=11ms delivered_frames=576 delivered_per_s=72.00
M3 interval=11ms gap_ms mean=13.887 p50=13.887 p99=13.888 max=13.889 n=575 gaps_over_T_r(16.67ms)=0
M3 interval=11ms hold_ms(composition->release) mean=-5.435 p50=-5.458 p99=-4.947 max=-2.559 n=576
M3 interval=11ms handler_ms(entry->return) mean=0.067 p50=0.049 p99=0.326 max=2.667 n=576
M3 interval=11ms delivery_latency_ms(composition->entry) mean=-5.502 p50=-5.514 p99=-5.075 max=-4.930 n=576
M3 interval=11ms capture_thread_cpu_us_per_frame=246.88 handler_cpu_us_per_frame=65.09
M3 interval=11ms paints=1152 frames_with_counter=576 paints_not_delivered=575 longest_run_not_delivered=2 repeated_paint_frames=0 unread=0
```

M4 (`clip-probe newframes`, then `clip-probe newframes nocursor`):

```
M4 phase=idle seconds=5.00 delivered_frames=0 delivered_per_s=0.00 instants=100 n_new_identity_per_s=0.00 n_new_content_per_s=0.00
M4 phase=caret seconds=6.00 delivered_frames=10 delivered_per_s=1.67 instants=120 n_new_identity_per_s=1.67 n_new_content_per_s=1.67
M4 phase=typing seconds=5.00 delivered_frames=40 delivered_per_s=8.00 instants=100 n_new_identity_per_s=8.00 n_new_content_per_s=8.00
M4 phase=mouse seconds=4.90 delivered_frames=235 delivered_per_s=47.95 instants=98 n_new_identity_per_s=20.00 n_new_content_per_s=20.00
```

```
M4 cursor_in_frames=false
M4 phase=mouse seconds=4.90 delivered_frames=0 delivered_per_s=0.00 instants=98 n_new_identity_per_s=0.00 n_new_content_per_s=0.00
```

M5 (`clip-probe disk`):

```
M5 folder=%LOCALAPPDATA%\Temp\clip-probe-disk-3232 free_bytes_before=1436912406528
M5 phase=buffered frames=60 bytes=497664000 wall_s=0.153 mb_per_s=3254.9 write_ms mean=2.542 p50=2.530 p99=2.934 max=3.121 n=60 late_frames=0
M5 phase=buffered thread_cpu_s=0.151 process_cpu_s=0.156 system_process_cpu_s=unavailable thread_cpu_ns_per_px=1.2172 thread_cpu_ms_per_frame=2.524 share_of_one_thread=0.990
M5 phase=write_through frames=60 bytes=497664000 wall_s=0.166 mb_per_s=3004.2 write_ms mean=2.755 p50=2.439 p99=7.046 max=7.380 n=60 late_frames=0
M5 phase=write_through thread_cpu_s=0.043 process_cpu_s=0.094 system_process_cpu_s=unavailable thread_cpu_ns_per_px=0.3475 thread_cpu_ms_per_frame=0.721 share_of_one_thread=0.261
M5 phase=paced_20fps_buffered frames=40 bytes=331776000 wall_s=1.954 mb_per_s=169.8 write_ms mean=3.221 p50=3.289 p99=4.629 max=4.629 n=40 late_frames=0
M5 phase=paced_20fps_buffered thread_cpu_s=0.125 process_cpu_s=0.125 system_process_cpu_s=unavailable thread_cpu_ns_per_px=1.5122 thread_cpu_ms_per_frame=3.136 share_of_one_thread=0.064
M5 total_bytes_written=1327104000 free_bytes_after=1436912390144
```

M6 (`clip-probe threading`):

```
M6 wait=blocking_map delivered_frames=383 delivered_per_s=47.88
M6 wait=blocking_map gap_ms mean=20.831 p50=20.831 p99=20.832 max=20.833 n=382 gaps_over_T_r(16.67ms)=382
M6 wait=blocking_map hold_ms(composition->release) mean=-5.455 p50=-5.578 p99=-3.401 max=-3.216 n=383
M6 wait=blocking_map handler_ms(entry->return) mean=0.032 p50=0.006 p99=1.917 max=1.999 n=383
M6 wait=blocking_map delivery_latency_ms(composition->entry) mean=-5.487 p50=-5.587 p99=-3.411 max=-3.220 n=383
M6 wait=blocking_map capture_thread_cpu_us_per_frame=221.12 handler_cpu_us_per_frame=5.67
M6 wait=blocking_map instants=160 new_frames=160 read_ms mean=2.353 p50=2.320 p99=3.008 max=3.308 n=160 | read_cpu_ms mean=0.398 p50=0.374 p99=0.701 max=0.841 n=160 | map_retries_per_read=0.0
M6 wait=blocking_map lateness_instants mean=0.000 p50=0.000 p99=0.000 max=0.000 n=160
M6 wait=blocking_map slot_overflows=0 max_slots_in_use=3 deliveries_later_than_margin(10ms)=0
M6 wait=do_not_wait_yield_retry delivered_frames=383 delivered_per_s=47.88
M6 wait=do_not_wait_yield_retry gap_ms mean=20.831 p50=20.831 p99=20.832 max=20.837 n=382 gaps_over_T_r(16.67ms)=382
M6 wait=do_not_wait_yield_retry hold_ms(composition->release) mean=-5.562 p50=-5.583 p99=-4.871 max=-3.782 n=383
M6 wait=do_not_wait_yield_retry handler_ms(entry->return) mean=0.017 p50=0.005 p99=0.064 max=1.713 n=383
M6 wait=do_not_wait_yield_retry delivery_latency_ms(composition->entry) mean=-5.579 p50=-5.590 p99=-5.094 max=-4.565 n=383
M6 wait=do_not_wait_yield_retry capture_thread_cpu_us_per_frame=228.80 handler_cpu_us_per_frame=5.29
M6 wait=do_not_wait_yield_retry instants=160 new_frames=160 read_ms mean=2.353 p50=2.280 p99=3.288 max=4.012 n=160 | read_cpu_ms mean=2.221 p50=2.188 p99=3.165 max=3.982 n=160 | map_retries_per_read=3050.3
M6 wait=do_not_wait_yield_retry lateness_instants mean=0.000 p50=0.000 p99=0.000 max=0.000 n=160
M6 wait=do_not_wait_yield_retry slot_overflows=0 max_slots_in_use=3 deliveries_later_than_margin(10ms)=0
M6 wait=do_not_wait_sleep1ms_retry delivered_frames=383 delivered_per_s=47.88
M6 wait=do_not_wait_sleep1ms_retry gap_ms mean=20.831 p50=20.831 p99=20.832 max=20.833 n=382 gaps_over_T_r(16.67ms)=382
M6 wait=do_not_wait_sleep1ms_retry hold_ms(composition->release) mean=-5.537 p50=-5.557 p99=-5.001 max=-3.398 n=383
M6 wait=do_not_wait_sleep1ms_retry handler_ms(entry->return) mean=0.007 p50=0.006 p99=0.061 max=0.111 n=383
M6 wait=do_not_wait_sleep1ms_retry delivery_latency_ms(composition->entry) mean=-5.544 p50=-5.562 p99=-5.008 max=-3.402 n=383
M6 wait=do_not_wait_sleep1ms_retry capture_thread_cpu_us_per_frame=228.87 handler_cpu_us_per_frame=5.21
M6 wait=do_not_wait_sleep1ms_retry instants=160 new_frames=160 read_ms mean=3.415 p50=3.462 p99=4.082 max=4.423 n=160 | read_cpu_ms mean=0.502 p50=0.476 p99=1.064 max=1.368 n=160 | map_retries_per_read=1.9
M6 wait=do_not_wait_sleep1ms_retry lateness_instants mean=0.000 p50=0.000 p99=0.000 max=0.000 n=160
M6 wait=do_not_wait_sleep1ms_retry slot_overflows=0 max_slots_in_use=3 deliveries_later_than_margin(10ms)=0
```

M7 (`clip-probe lateness`):

```
M7 wait=blocking_map phase=idle instants=80 new_frames=80 read_ms mean=2.347 p50=2.268 p99=3.104 max=3.172 n=80 | read_cpu_ms mean=1.189 p50=1.062 p99=1.798 max=1.921 n=80 | map_retries_per_read=0.0
M7 wait=blocking_map phase=idle lateness_instants mean=0.000 p50=0.000 p99=0.000 max=0.000 n=80
M7 wait=blocking_map phase=idle delivered_frames=192 hold_ms mean=-5.452 p50=-5.728 p99=-2.679 max=-1.980 n=192
M7 wait=blocking_map phase=load instants=80 new_frames=80 read_ms mean=5.247 p50=4.401 p99=12.647 max=18.742 n=80 | read_cpu_ms mean=1.164 p50=1.160 p99=1.448 max=1.522 n=80 | map_retries_per_read=0.0
M7 wait=blocking_map phase=load lateness_instants mean=0.000 p50=0.000 p99=0.000 max=0.000 n=80
M7 wait=blocking_map phase=load delivered_frames=192 hold_ms mean=-4.634 p50=-5.060 p99=-2.272 max=-2.025 n=192
M7 wait=blocking_map slot_overflows=0 max_slots_in_use=3 raw_bytes_written=262348800
M7 wait=do_not_wait_sleep1ms_retry phase=idle instants=80 new_frames=80 read_ms mean=2.796 p50=2.642 p99=4.573 max=5.048 n=80 | read_cpu_ms mean=1.253 p50=1.129 p99=1.917 max=2.047 n=80 | map_retries_per_read=1.1
M7 wait=do_not_wait_sleep1ms_retry phase=idle lateness_instants mean=0.000 p50=0.000 p99=0.000 max=0.000 n=80
M7 wait=do_not_wait_sleep1ms_retry phase=idle delivered_frames=192 hold_ms mean=-5.712 p50=-5.764 p99=-4.591 max=-4.101 n=192
M7 wait=do_not_wait_sleep1ms_retry phase=load instants=80 new_frames=80 read_ms mean=5.443 p50=4.426 p99=14.720 max=17.489 n=80 | read_cpu_ms mean=1.207 p50=1.103 p99=1.347 max=7.961 n=80 | map_retries_per_read=1.0
M7 wait=do_not_wait_sleep1ms_retry phase=load lateness_instants mean=0.000 p50=0.000 p99=0.000 max=0.000 n=80
M7 wait=do_not_wait_sleep1ms_retry phase=load delivered_frames=192 hold_ms mean=-5.879 p50=-5.951 p99=-5.151 max=-4.317 n=192
M7 wait=do_not_wait_sleep1ms_retry slot_overflows=0 max_slots_in_use=3 raw_bytes_written=262348800
```

M8 (`clip-probe encoder`; keyframes from ffprobe):

```
M8 hardware_encoders count=3 names=["NVIDIA H.264 Encoder MFT (VEN_10DE)", "IntelÂ® Quick Sync Video H.264 Encoder MFT (VEN_8086)", "IntelÂ® Quick Sync Video H.264 Encoder MFT (VEN_8086)"]
M8 software_encoders count=1 names=["H264 Encoder MFT"]
M8 output_folder=%LOCALAPPDATA%\Temp\clip-probe-encoder
M8 run=sw-nohint encoder="unnamed" hardware_mft=false parameters_accepted_at_open=true
M8 run=sw-nohint setting=gop_size wanted=20 supported=true set_through_codecapi=ok read_back=20
M8 run=sw-nohint setting=b_picture_count wanted=0 supported=true set_through_codecapi=ok read_back=0
M8 run=sw-nohint setting=rate_control_quality wanted=3 supported=true set_through_codecapi=ok read_back=3
M8 run=sw-nohint setting=quality wanted=70 supported=true set_through_codecapi=ok read_back=70
M8 run=sw-nohint frames=100 bytes=1048732 wall_ms=113.4 process_cpu_ms_per_frame=4.296 file=%LOCALAPPDATA%\Temp\clip-probe-encoder\sw-nohint.mp4
M8 run=sw-hint encoder="unnamed" hardware_mft=false parameters_accepted_at_open=true
M8 run=sw-hint setting=gop_size wanted=20 supported=true set_through_codecapi=ok read_back=20
M8 run=sw-hint setting=b_picture_count wanted=0 supported=true set_through_codecapi=ok read_back=0
M8 run=sw-hint setting=rate_control_quality wanted=3 supported=true set_through_codecapi=ok read_back=3
M8 run=sw-hint setting=quality wanted=70 supported=true set_through_codecapi=ok read_back=70
M8 run=sw-hint setting=scenario_display_remoting wanted=1 supported=true set_through_codecapi=ok read_back=1
M8 run=sw-hint frames=100 bytes=1048732 wall_ms=123.3 process_cpu_ms_per_frame=4.490 file=%LOCALAPPDATA%\Temp\clip-probe-encoder\sw-hint.mp4
M8 run=hw-nohint encoder="unnamed, vendor VEN_8086" hardware_mft=true parameters_accepted_at_open=true
M8 run=hw-nohint setting=gop_size wanted=20 supported=true set_through_codecapi=ok read_back=20
M8 run=hw-nohint setting=b_picture_count wanted=0 supported=true set_through_codecapi=ok read_back=0
M8 run=hw-nohint setting=rate_control_quality wanted=3 supported=true set_through_codecapi=ok read_back=3
M8 run=hw-nohint setting=quality wanted=70 supported=true set_through_codecapi=ok read_back=70
M8 run=hw-nohint frames=100 bytes=724299 wall_ms=106.0 process_cpu_ms_per_frame=1.340 file=%LOCALAPPDATA%\Temp\clip-probe-encoder\hw-nohint.mp4
M8 run=hw-hint encoder="unnamed, vendor VEN_8086" hardware_mft=true parameters_accepted_at_open=true
M8 run=hw-hint setting=gop_size wanted=20 supported=true set_through_codecapi=ok read_back=20
M8 run=hw-hint setting=b_picture_count wanted=0 supported=true set_through_codecapi=ok read_back=0
M8 run=hw-hint setting=rate_control_quality wanted=3 supported=true set_through_codecapi=ok read_back=3
M8 run=hw-hint setting=quality wanted=70 supported=true set_through_codecapi=ok read_back=70
M8 run=hw-hint setting=scenario_display_remoting wanted=1 supported=true set_through_codecapi=ok read_back=1
M8 run=hw-hint frames=100 bytes=723975 wall_ms=92.0 process_cpu_ms_per_frame=1.242 file=%LOCALAPPDATA%\Temp\clip-probe-encoder\hw-hint.mp4
```

```
sw-nohint: frames=100 keys=0,20,40,60,80 types={"I":5,"P":95}
sw-hint: frames=100 keys=0,20,40,60,80 types={"I":5,"P":95}
hw-nohint: frames=100 keys=0,20,40,60,80 types={"I":5,"P":95}
hw-hint: frames=100 keys=0,20,40,60,80 types={"I":5,"P":95}
```

## Package 0b: compression of the waiting frames (M10 to M14)

Measured 2026-11-22 09:50 to 09:57 (system clock, JST) on the same development PC with `clip-probe compress` (release build, `lz4_flex` 0.14.2 with its default safe features, `zstd` 0.14.4 over zstd-sys 2.0.16 / zstd 1.5.7). Plan: `docs/clip-recorder-plan.md` section 0b; research: `docs/clip-recorder-waiting-compression-research.md`. No product code was changed.

How it was measured:

- **M10 frames.** Each scenario is a Chrome `--app` window the probe opens itself (throw-away profile in `%TEMP%`, topmost without activation, closed and its profile deleted afterwards); each page drives itself with JavaScript, the probe sends no input to any window. Capture as in the product plan: windows-capture 2.0.0, requested interval 11 ms, cursor left out (E9). Every delivered frame is read back; per 50 ms instant the last frame at or before it is kept, and it counts as new when its time stamp is new (A7). 21 s per scenario. Frames stayed in memory only and were dropped after each scenario. Pages: `flash-page.html` (unchanged, with and without `?anim=1`), `scroll-page.html` (text scrolling 300 px/s on every refresh), `typing-page.html` (8 characters per second into its own text box), `video-page.html` (canvas animation standing in for a video: moving gradients and soft shapes plus per-pixel grain that changes every frame), `still-page.html`; the two small windows (506 x 333) carry the σ = 2 and σ = 3 checks.
- **M11 methods** (`probe/src/waiting_codec.rs`): (a) raw; (b) `lz4_flex` block on the whole frame; (c) changed tiles + `lz4_flex`; (d) changed tiles + zstd level -1 and 1; tiles 32, 64, 128; alpha kept and dropped (dropped: B, G, R stored, the decoder writes 255). One file = 16-byte header + (tile coders) one bit per tile + one compressed block of the changed tiles' rows. Keyframe (every tile) every `WAITING_KEYFRAME_FRAMES` = 16 frames and on a size change. One thread; processor time from the thread cycle counter; ns per pixel over all source pixels of all frames (keyframes included). The encoder gets the previous source frame from the caller (the live part would swap two read-back buffers) and reuses its output buffer.
- **M12 write** (`write_cost` in `probe/src/compress.rs`): every compressed file of the method, one at a time: create `<n>.tmp`, write through the file cache, rename; thread processor µs per file. At most 500 MB per method (raw: 138 to 184 frames of the 720p scenarios), files deleted after each method; free space stayed above 1.3 TB, at most 0.5 GB on disk at any time.
- **M13 exactness:** every decoded frame compared with its source (alpha kept: all four bytes; dropped: B, G, R). For alpha dropped, the NV12 picture of the decoded frame and of the source were hashed (SipHash) after `Nv12Canvas::draw` at each σ of the scenario. `RecordedSize`, `even_up`, `even_down`, `Nv12Canvas` and `clamp_u8` are copied **verbatim** from `crates/synapse-mcp/src/server/clip_encoder.rs` into `probe/src/nv12_copy.rs`; only a three-field stand-in for `SegmentInput` was added.
- **M14 live share:** a_rt = f_o · c_obs + n_new · (w · h · (c_read + c_comp) + write per file), at 1920 x 1080 and n_new = 20 per second (M4 worst). c_obs 0.373 ms, c_read 0.349 ns/px (package 0); c_comp is the scenario's measured ns/px (720p and 506 x 333 windows; per-pixel cost assumed equal at 1080p, an **estimate**); write per file from the least-squares fit over all 176 M12 points: **308.6 µs + 0.277 µs per KB** (a raw 1080p BGRA file: 1.257 ns/px, against M5's 1.507 at paced 20 per second). Development PC: 16 threads, f_o 72. Weakest machine (**estimate**): costs x 3.68, 4 threads, f_o 60. Limit: `CPU_LIMIT_PERCENT` = 10 (E10).

### M10: the scenarios

| Scenario | Window (frame) | Delivered per s | New frames per s | New frames | σ checked |
|---|---|---|---|---|---|
| flash | 1266 x 713 | 72.0 | 20.00 | 420 | 1 |
| flash-anim | 1266 x 713 | 72.0 | 20.04 | 421 | 1 |
| scroll | 1266 x 713 | 72.0 | 20.04 | 421 | 1 |
| typing | 1266 x 713 | 8.0 | 8.00 | 168 | 1 |
| video (canvas) | 1266 x 713 | 72.0 | 20.00 | 420 | 1 |
| still | 1266 x 713 | 0.1 | 0.10 | 2 | 1 |
| typing-small | 506 x 333 | 8.0 | 8.05 | 169 | 1, 2, 3 |
| scroll-small | 506 x 333 | 72.0 | 20.00 | 420 | 1, 2, 3 |

The flash page's counter changes on every refresh, so it gives a new frame at every instant (20 per second), like the M3 counter window. A covered Chrome window rendered only 3.8 frames per second in a first try; topmost (as the package 0 probe window) it rendered at the display rate.

### M11 and M12: best tile per method, alpha dropped (all tile sizes and alpha kept in the raw output below)

Bytes per frame at the scenario's window size; ratio against raw BGRA; c and d = compress and decompress ns per pixel (development PC, one thread); write = thread µs per file (create, write, rename).

| Scenario | (a) raw | (b) lz4 whole | (c) tiles+lz4 | (d) tiles+zstd -1 | (d) tiles+zstd 1 |
|---|---|---|---|---|---|
| flash | 2,707,990 B, 1.3x, c 0.40, d 0.45, write 1066 | 45,501 B, 79x, c 0.65, d 0.67, write 337 | t32: 7,917 B, 456x, c 0.36, d 0.10, write 304 | t32: 4,724 B, 764x, c 0.35, d 0.10, write 306 | t32: 4,378 B, 825x, c 0.35, d 0.11, write 274 |
| flash-anim | 2,707,990, 1.3x, 0.40, 0.47, 1079 | 45,537, 79x, 0.64, 0.67, 356 | t32: 8,007, 451x, 0.34, 0.11, 322 | t32: 4,741, 762x, 0.36, 0.11, 258 | t32: 4,392, 822x, 0.39, 0.11, 281 |
| scroll | 2,707,990, 1.3x, 0.41, 0.46, 1054 | 139,936, 26x, 1.09, 0.78, 348 | t32: 136,867, 26x, 1.31, 0.78, 360 | t128: 75,436, 48x, 1.68, 1.19, 298 | t128: 68,955, 52x, 1.84, 1.12, 335 |
| typing | 2,707,990, 1.3x, 0.39, 0.45, 1064 | 19,860, 182x, 0.55, 0.65, 300 | t64: 1,932, 1869x, 0.51, 0.14, 311 | t32: 827, 4369x, 0.32, 0.06, 285 | t32: 781, 4623x, 0.31, 0.06, 334 |
| video (canvas) | 2,707,990, 1.3x, 0.40, 0.46, 1033 | 2,442,322, 1.5x, 1.50, 0.71, 973 | t32: 2,467,934, 1.5x, 1.94, 0.71, 982 | t32: 2,416,723, 1.5x, 2.54, 0.80, 952 | t128: 2,034,043, 1.8x, 3.63, 2.77, 896 |
| still (2 frames, keyframes only) | 2,707,990, 1.3x, 0.59, 0.61, 1043 | 39,327, 92x, 1.22, 1.20, 335 | t128: 19,732, 183x, 1.26, 1.04, 305 | t64: 9,371, 385x, 1.32, 1.02, 357 | t64: 8,778, 411x, 1.33, 1.07, 314 |
| typing-small | 505,510, 1.3x, 0.40, 0.48, 447 | 10,981, 61x, 0.64, 0.83, 286 | t32: 1,121, 601x, 0.38, 0.14, 300 | t32: 691, 976x, 0.36, 0.10, 306 | t32: 655, 1029x, 0.40, 0.11, 265 |
| scroll-small | 505,510, 1.3x, 0.43, 0.47, 423 | 36,347, 19x, 1.22, 0.86, 305 | t32: 34,056, 20x, 1.79, 0.96, 300 | t32: 19,660, 34x, 2.18, 1.40, 317 | t32: 18,031, 37x, 2.06, 1.32, 311 |

Tile size: averaged over the scenarios without video, 32 px gives the fewest bytes for every tile coder and both alpha choices (alpha dropped: tiles+lz4 0.0634 / 0.0646 / 0.0674 bytes per pixel at 32 / 64 / 128; zstd -1 0.0364 / 0.0368 / 0.0382; zstd 1 0.0334 / 0.0338 / 0.0352); larger tiles win only on video, by 1 to 5 %. Alpha dropped gives 8 to 25 % fewer bytes than alpha kept in every scenario at about the same cost (the whole-frame coders pay 0.15 to 0.25 ns/px for the repack; on video, lz4 with alpha kept costs 3.30 ns/px against 1.50 dropped).

Writes: a file costs about 0.3 ms of processor time however small it is (create + rename dominate below about 100 KB); a raw 720p file costs 1.0 to 1.3 ms.

### M13: exactness

All 176 method x scenario runs: every decoded frame equal to its source in B, G, R (alpha kept: all four bytes), 0 mismatches. NV12 from the copied `Nv12Canvas::draw`, decoded against source, alpha dropped: equal in every frame for σ = 1 (all scenarios), σ = 2 and σ = 3 (typing-small 169 of 169, scroll-small 420 of 420, at every method and tile size). Alpha kept needs no NV12 check: the decoded bytes are identical to the source. Result: **pass for every method, both alpha choices, σ = 1, 2, 3.**

### M14: live share at 1920 x 1080, 20 new frames per second (tile 32, alpha dropped unless named)

| Content at 20 per second | (a) raw, alpha kept | (a) raw | (b) lz4 whole | (c) tiles+lz4 | (d) zstd -1 | (d) zstd 1 |
|---|---|---|---|---|---|---|
| flash (counter + flash) | 0.65 / 9.10 % | 0.62 / 8.65 % | 0.47 / 6.47 % | 0.39 / 5.35 % | 0.39 / 5.29 % | 0.39 / 5.31 % |
| flash-anim | 0.65 / 9.09 % | 0.62 / 8.64 % | 0.47 / 6.44 % | 0.39 / 5.27 % | 0.39 / 5.35 % | 0.40 / 5.46 % |
| scroll | 0.65 / 9.12 % | 0.62 / 8.68 % | 0.59 / 8.27 % | 0.65 / 9.13 % | 0.74 / **10.47 %** | 0.73 / **10.26 %** |
| typing content at 20 per second | 0.65 / 9.11 % | 0.61 / 8.62 % | 0.44 / 6.08 % | 0.37 / 5.11 % | 0.38 / 5.17 % | 0.38 / 5.14 % |
| video (canvas) | 0.64 / 9.08 % | 0.62 / 8.64 % | 0.88 / **12.53 %** | 1.00 / **14.25 %** | 1.15 / **16.50 %** | 1.45 / **20.99 %** |
| typing-small | 0.64 / 9.05 % | 0.62 / 8.66 % | 0.47 / 6.46 % | 0.40 / 5.40 % | 0.39 / 5.34 % | 0.40 / 5.50 % |
| scroll-small | 0.64 / 9.02 % | 0.62 / 8.77 % | 0.63 / 8.85 % | 0.77 / **10.98 %** | 0.87 / **12.38 %** | 0.84 / **11.92 %** |

Each cell: development PC / weakest machine (estimate); bold = above `CPU_LIMIT_PERCENT` = 10. Typing at its real 8 per second costs less; the table uses 20 per second for every content, as the choice rule asks. The raw rows reproduce package 0's 9.14 % (M9) within the write model's difference.

### The choice by the plan's rule

Rule: the method with the fewest bytes whose compression keeps the live share within `CPU_LIMIT_PERCENT` on the weakest-machine estimate at the M4 worst rate.

- With every measured content at 20 per second, the worst case is the video stand-in: photo-like noise does not compress (1.5x to 1.8x), so every compressor adds 1.1 to 3.6 ns/px of work and saves almost no write. Weakest machine: (b) 12.53 %, (c) 14.25 %, (d) 16.50 % and 20.99 %, all above 10 %. Only raw stays within: 8.64 % with alpha dropped, 9.08 % with alpha kept. **By the rule as written: (a) raw with alpha dropped** (3 bytes per pixel, 25 % fewer than today's raw, lossless by M13, 8.6 % on the weakest machine). No tile size applies.
- If the owner excludes video-like content from the worst case: only (b) lz4 whole frame, alpha dropped, stays within 10 % in every other scenario (worst scroll-small 8.85 %, scroll 8.27 %); (c) fails on scroll-small (10.98 %), (d) on scroll (10.26 to 10.47 %) and scroll-small. Bytes for (b): 19 to 182 times fewer than raw on screen content.
- On text-like content (flash, typing) the tile coders are far ahead: tiles + zstd 1, tile 32, alpha dropped stores 0.8 to 4.4 KB per frame at 720p (raw 3.6 MB) at 5.1 to 5.5 % on the weakest machine against raw's 8.6 %; tiles + zstd -1 is within 10 % of its bytes at slightly lower cost on scroll.

Not decided here (owner, plan version 4.2): whether the worst case includes video-like content. A per-frame fallback (store the frame raw when the previous frame of that window compressed worse than a set ratio) would keep scroll and text at the tile coders' bytes and video at raw cost; it was **not measured**.

### Not measured in package 0b

- A real video player (the stand-in is a canvas animation; real video is often less noisy and may compress somewhat better).
- 1920 x 1080 windows: the largest window was 1266 x 713 (a full-screen window would cover the owner's screen); per-pixel costs were carried over to 1080p (estimate).
- The file cache's lazy-writer time (as M5), the `unsafe` speed features of `lz4_flex`, and anything on the weakest machine itself (every weakest-machine figure is x 3.68).

### Raw output of `clip-probe compress`

```
M10 scenario=flash size=1266x713 seconds=21.0 delivered_per_s=72.0 new_frames=420 new_per_s=20.00 raw_bytes_per_frame=3610632 sigmas=[1]
| method | bytes/frame | ratio | compress ns/px | decompress ns/px | write µs/frame (files) | B,G,R exact | NV12 exact (σ) |
|---|---|---|---|---|---|---|---|
| (a) raw alpha kept | 3610648 | 1.0 | 0.239 | 0.235 | 1282 (138) | 420/420 | all 4 bytes equal |
| (b) lz4 whole alpha kept | 51520 | 70.1 | 0.390 | 0.272 | 339 (420) | 420/420 | all 4 bytes equal |
| (c) tiles+lz4 tile 32 alpha kept | 8652 | 417.3 | 0.293 | 0.080 | 295 (420) | 420/420 | all 4 bytes equal |
| (c) tiles+lz4 tile 64 alpha kept | 9076 | 397.8 | 0.625 | 0.102 | 352 (420) | 420/420 | all 4 bytes equal |
| (c) tiles+lz4 tile 128 alpha kept | 10886 | 331.7 | 0.488 | 0.105 | 307 (420) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 32 alpha kept | 5078 | 711.0 | 0.328 | 0.071 | 303 (420) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 64 alpha kept | 5505 | 655.9 | 0.496 | 0.080 | 307 (420) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 128 alpha kept | 6694 | 539.4 | 0.526 | 0.097 | 292 (420) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 32 alpha kept | 4882 | 739.5 | 0.314 | 0.078 | 263 (420) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 64 alpha kept | 5243 | 688.7 | 0.503 | 0.083 | 300 (420) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 128 alpha kept | 6433 | 561.3 | 0.533 | 0.096 | 292 (420) | 420/420 | all 4 bytes equal |
| (a) raw alpha dropped | 2707990 | 1.3 | 0.397 | 0.449 | 1066 (184) | 420/420 | σ1 420/420 |
| (b) lz4 whole alpha dropped | 45501 | 79.4 | 0.645 | 0.673 | 337 (420) | 420/420 | σ1 420/420 |
| (c) tiles+lz4 tile 32 alpha dropped | 7917 | 456.1 | 0.361 | 0.104 | 304 (420) | 420/420 | σ1 420/420 |
| (c) tiles+lz4 tile 64 alpha dropped | 8236 | 438.4 | 0.468 | 0.113 | 337 (420) | 420/420 | σ1 420/420 |
| (c) tiles+lz4 tile 128 alpha dropped | 9615 | 375.5 | 0.470 | 0.143 | 287 (420) | 420/420 | σ1 420/420 |
| (d) tiles+zstd -1 tile 32 alpha dropped | 4724 | 764.3 | 0.348 | 0.104 | 306 (420) | 420/420 | σ1 420/420 |
| (d) tiles+zstd -1 tile 64 alpha dropped | 5096 | 708.6 | 0.484 | 0.116 | 286 (420) | 420/420 | σ1 420/420 |
| (d) tiles+zstd -1 tile 128 alpha dropped | 6134 | 588.6 | 0.485 | 0.147 | 336 (420) | 420/420 | σ1 420/420 |
| (d) tiles+zstd 1 tile 32 alpha dropped | 4378 | 824.8 | 0.354 | 0.105 | 274 (420) | 420/420 | σ1 420/420 |
| (d) tiles+zstd 1 tile 64 alpha dropped | 4718 | 765.3 | 0.490 | 0.115 | 273 (420) | 420/420 | σ1 420/420 |
| (d) tiles+zstd 1 tile 128 alpha dropped | 5744 | 628.6 | 0.564 | 0.141 | 288 (420) | 420/420 | σ1 420/420 |
M10 scenario=flash-anim size=1266x713 seconds=21.0 delivered_per_s=72.0 new_frames=421 new_per_s=20.04 raw_bytes_per_frame=3610632 sigmas=[1]
| method | bytes/frame | ratio | compress ns/px | decompress ns/px | write µs/frame (files) | B,G,R exact | NV12 exact (σ) |
|---|---|---|---|---|---|---|---|
| (a) raw alpha kept | 3610648 | 1.0 | 0.237 | 0.231 | 1279 (138) | 421/421 | all 4 bytes equal |
| (b) lz4 whole alpha kept | 51594 | 70.0 | 0.403 | 0.273 | 312 (421) | 421/421 | all 4 bytes equal |
| (c) tiles+lz4 tile 32 alpha kept | 8686 | 415.7 | 0.316 | 0.084 | 325 (421) | 421/421 | all 4 bytes equal |
| (c) tiles+lz4 tile 64 alpha kept | 9323 | 387.3 | 0.515 | 0.092 | 327 (421) | 421/421 | all 4 bytes equal |
| (c) tiles+lz4 tile 128 alpha kept | 11213 | 322.0 | 0.489 | 0.110 | 302 (421) | 421/421 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 32 alpha kept | 5103 | 707.5 | 0.326 | 0.077 | 296 (421) | 421/421 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 64 alpha kept | 5566 | 648.7 | 0.498 | 0.089 | 281 (421) | 421/421 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 128 alpha kept | 6731 | 536.4 | 0.507 | 0.105 | 314 (421) | 421/421 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 32 alpha kept | 4905 | 736.1 | 0.324 | 0.080 | 307 (421) | 421/421 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 64 alpha kept | 5310 | 680.0 | 0.498 | 0.090 | 310 (421) | 421/421 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 128 alpha kept | 6431 | 561.5 | 0.522 | 0.114 | 324 (421) | 421/421 | all 4 bytes equal |
| (a) raw alpha dropped | 2707990 | 1.3 | 0.396 | 0.470 | 1079 (184) | 421/421 | σ1 421/421 |
| (b) lz4 whole alpha dropped | 45537 | 79.3 | 0.637 | 0.672 | 356 (421) | 421/421 | σ1 421/421 |
| (c) tiles+lz4 tile 32 alpha dropped | 8007 | 450.9 | 0.342 | 0.109 | 322 (421) | 421/421 | σ1 421/421 |
| (c) tiles+lz4 tile 64 alpha dropped | 8435 | 428.0 | 0.471 | 0.127 | 291 (421) | 421/421 | σ1 421/421 |
| (c) tiles+lz4 tile 128 alpha dropped | 9863 | 366.1 | 0.484 | 0.158 | 295 (421) | 421/421 | σ1 421/421 |
| (d) tiles+zstd -1 tile 32 alpha dropped | 4741 | 761.5 | 0.362 | 0.112 | 258 (421) | 421/421 | σ1 421/421 |
| (d) tiles+zstd -1 tile 64 alpha dropped | 5154 | 700.5 | 0.488 | 0.131 | 271 (421) | 421/421 | σ1 421/421 |
| (d) tiles+zstd -1 tile 128 alpha dropped | 6127 | 589.3 | 0.555 | 0.166 | 262 (421) | 421/421 | σ1 421/421 |
| (d) tiles+zstd 1 tile 32 alpha dropped | 4392 | 822.0 | 0.391 | 0.111 | 281 (421) | 421/421 | σ1 421/421 |
| (d) tiles+zstd 1 tile 64 alpha dropped | 4793 | 753.3 | 0.482 | 0.132 | 291 (421) | 421/421 | σ1 421/421 |
| (d) tiles+zstd 1 tile 128 alpha dropped | 5752 | 627.7 | 0.509 | 0.178 | 300 (421) | 421/421 | σ1 421/421 |
M10 scenario=scroll size=1266x713 seconds=21.0 delivered_per_s=72.0 new_frames=421 new_per_s=20.04 raw_bytes_per_frame=3610632 sigmas=[1]
| method | bytes/frame | ratio | compress ns/px | decompress ns/px | write µs/frame (files) | B,G,R exact | NV12 exact (σ) |
|---|---|---|---|---|---|---|---|
| (a) raw alpha kept | 3610648 | 1.0 | 0.244 | 0.237 | 1299 (138) | 421/421 | all 4 bytes equal |
| (b) lz4 whole alpha kept | 150267 | 24.0 | 0.817 | 0.432 | 365 (421) | 421/421 | all 4 bytes equal |
| (c) tiles+lz4 tile 32 alpha kept | 146097 | 24.7 | 1.018 | 0.702 | 342 (421) | 421/421 | all 4 bytes equal |
| (c) tiles+lz4 tile 64 alpha kept | 146193 | 24.7 | 1.056 | 0.718 | 389 (421) | 421/421 | all 4 bytes equal |
| (c) tiles+lz4 tile 128 alpha kept | 147388 | 24.5 | 1.065 | 0.663 | 365 (421) | 421/421 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 32 alpha kept | 85724 | 42.1 | 1.423 | 0.902 | 352 (421) | 421/421 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 64 alpha kept | 85179 | 42.4 | 1.474 | 0.922 | 291 (421) | 421/421 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 128 alpha kept | 84242 | 42.9 | 1.470 | 0.915 | 350 (421) | 421/421 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 32 alpha kept | 83545 | 43.2 | 1.460 | 0.930 | 339 (421) | 421/421 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 64 alpha kept | 83390 | 43.3 | 1.528 | 0.945 | 305 (421) | 421/421 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 128 alpha kept | 82697 | 43.7 | 1.543 | 0.951 | 315 (421) | 421/421 | all 4 bytes equal |
| (a) raw alpha dropped | 2707990 | 1.3 | 0.407 | 0.459 | 1054 (184) | 421/421 | σ1 421/421 |
| (b) lz4 whole alpha dropped | 139936 | 25.8 | 1.086 | 0.784 | 348 (421) | 421/421 | σ1 421/421 |
| (c) tiles+lz4 tile 32 alpha dropped | 136867 | 26.4 | 1.313 | 0.782 | 360 (421) | 421/421 | σ1 421/421 |
| (c) tiles+lz4 tile 64 alpha dropped | 137050 | 26.3 | 1.288 | 0.816 | 304 (421) | 421/421 | σ1 421/421 |
| (c) tiles+lz4 tile 128 alpha dropped | 137665 | 26.2 | 1.332 | 0.807 | 354 (421) | 421/421 | σ1 421/421 |
| (d) tiles+zstd -1 tile 32 alpha dropped | 77724 | 46.5 | 1.683 | 1.141 | 310 (421) | 421/421 | σ1 421/421 |
| (d) tiles+zstd -1 tile 64 alpha dropped | 76849 | 47.0 | 1.666 | 1.204 | 341 (421) | 421/421 | σ1 421/421 |
| (d) tiles+zstd -1 tile 128 alpha dropped | 75436 | 47.9 | 1.676 | 1.190 | 298 (421) | 421/421 | σ1 421/421 |
| (d) tiles+zstd 1 tile 32 alpha dropped | 71166 | 50.7 | 1.631 | 1.057 | 333 (421) | 421/421 | σ1 421/421 |
| (d) tiles+zstd 1 tile 64 alpha dropped | 70226 | 51.4 | 1.618 | 1.138 | 333 (421) | 421/421 | σ1 421/421 |
| (d) tiles+zstd 1 tile 128 alpha dropped | 68955 | 52.4 | 1.837 | 1.115 | 335 (421) | 421/421 | σ1 421/421 |
M10 scenario=typing size=1266x713 seconds=21.0 delivered_per_s=8.0 new_frames=168 new_per_s=8.00 raw_bytes_per_frame=3610632 sigmas=[1]
| method | bytes/frame | ratio | compress ns/px | decompress ns/px | write µs/frame (files) | B,G,R exact | NV12 exact (σ) |
|---|---|---|---|---|---|---|---|
| (a) raw alpha kept | 3610648 | 1.0 | 0.241 | 0.233 | 1313 (138) | 168/168 | all 4 bytes equal |
| (b) lz4 whole alpha kept | 24206 | 149.2 | 0.269 | 0.282 | 335 (168) | 168/168 | all 4 bytes equal |
| (c) tiles+lz4 tile 32 alpha kept | 2301 | 1569.3 | 0.274 | 0.153 | 330 (168) | 168/168 | all 4 bytes equal |
| (c) tiles+lz4 tile 64 alpha kept | 2305 | 1566.6 | 0.472 | 0.158 | 306 (168) | 168/168 | all 4 bytes equal |
| (c) tiles+lz4 tile 128 alpha kept | 2930 | 1232.5 | 0.461 | 0.158 | 286 (168) | 168/168 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 32 alpha kept | 908 | 3975.8 | 0.297 | 0.048 | 296 (168) | 168/168 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 64 alpha kept | 947 | 3813.3 | 0.481 | 0.043 | 313 (168) | 168/168 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 128 alpha kept | 1312 | 2752.1 | 0.482 | 0.054 | 303 (168) | 168/168 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 32 alpha kept | 863 | 4185.8 | 0.287 | 0.049 | 289 (168) | 168/168 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 64 alpha kept | 892 | 4048.3 | 0.510 | 0.048 | 321 (168) | 168/168 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 128 alpha kept | 1226 | 2944.1 | 0.466 | 0.054 | 355 (168) | 168/168 | all 4 bytes equal |
| (a) raw alpha dropped | 2707990 | 1.3 | 0.389 | 0.453 | 1064 (168) | 168/168 | σ1 168/168 |
| (b) lz4 whole alpha dropped | 19860 | 181.8 | 0.550 | 0.649 | 300 (168) | 168/168 | σ1 168/168 |
| (c) tiles+lz4 tile 32 alpha dropped | 1956 | 1846.0 | 0.300 | 0.140 | 297 (168) | 168/168 | σ1 168/168 |
| (c) tiles+lz4 tile 64 alpha dropped | 1932 | 1868.8 | 0.513 | 0.141 | 311 (168) | 168/168 | σ1 168/168 |
| (c) tiles+lz4 tile 128 alpha dropped | 2535 | 1424.5 | 0.413 | 0.148 | 338 (168) | 168/168 | σ1 168/168 |
| (d) tiles+zstd -1 tile 32 alpha dropped | 827 | 4368.5 | 0.317 | 0.062 | 285 (168) | 168/168 | σ1 168/168 |
| (d) tiles+zstd -1 tile 64 alpha dropped | 859 | 4201.1 | 0.467 | 0.065 | 314 (168) | 168/168 | σ1 168/168 |
| (d) tiles+zstd -1 tile 128 alpha dropped | 1171 | 3082.9 | 0.460 | 0.079 | 337 (168) | 168/168 | σ1 168/168 |
| (d) tiles+zstd 1 tile 32 alpha dropped | 781 | 4623.1 | 0.309 | 0.064 | 334 (168) | 168/168 | σ1 168/168 |
| (d) tiles+zstd 1 tile 64 alpha dropped | 801 | 4510.4 | 0.460 | 0.069 | 283 (168) | 168/168 | σ1 168/168 |
| (d) tiles+zstd 1 tile 128 alpha dropped | 1098 | 3288.6 | 0.430 | 0.079 | 297 (168) | 168/168 | σ1 168/168 |
M10 scenario=video size=1266x713 seconds=21.0 delivered_per_s=72.0 new_frames=420 new_per_s=20.00 raw_bytes_per_frame=3610632 sigmas=[1]
| method | bytes/frame | ratio | compress ns/px | decompress ns/px | write µs/frame (files) | B,G,R exact | NV12 exact (σ) |
|---|---|---|---|---|---|---|---|
| (a) raw alpha kept | 3610648 | 1.0 | 0.234 | 0.235 | 1300 (138) | 420/420 | all 4 bytes equal |
| (b) lz4 whole alpha kept | 3104759 | 1.2 | 3.298 | 0.604 | 1257 (148) | 420/420 | all 4 bytes equal |
| (c) tiles+lz4 tile 32 alpha kept | 2798798 | 1.3 | 6.134 | 1.126 | 1107 (172) | 420/420 | all 4 bytes equal |
| (c) tiles+lz4 tile 64 alpha kept | 2826666 | 1.3 | 6.449 | 1.206 | 1080 (175) | 420/420 | all 4 bytes equal |
| (c) tiles+lz4 tile 128 alpha kept | 2862321 | 1.3 | 6.432 | 1.216 | 1119 (171) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 32 alpha kept | 2897919 | 1.2 | 5.877 | 1.420 | 1136 (162) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 64 alpha kept | 2845791 | 1.3 | 6.544 | 1.536 | 1075 (169) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 128 alpha kept | 2870701 | 1.3 | 6.962 | 1.625 | 1148 (168) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 32 alpha kept | 2350729 | 1.5 | 7.094 | 3.029 | 985 (203) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 64 alpha kept | 2251566 | 1.6 | 7.532 | 3.117 | 946 (216) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 128 alpha kept | 2224074 | 1.6 | 7.923 | 3.113 | 920 (222) | 420/420 | all 4 bytes equal |
| (a) raw alpha dropped | 2707990 | 1.3 | 0.396 | 0.461 | 1033 (184) | 420/420 | σ1 420/420 |
| (b) lz4 whole alpha dropped | 2442322 | 1.5 | 1.496 | 0.713 | 973 (199) | 420/420 | σ1 420/420 |
| (c) tiles+lz4 tile 32 alpha dropped | 2467934 | 1.5 | 1.939 | 0.711 | 982 (199) | 420/420 | σ1 420/420 |
| (c) tiles+lz4 tile 64 alpha dropped | 2472293 | 1.5 | 1.887 | 0.718 | 1006 (199) | 420/420 | σ1 420/420 |
| (c) tiles+lz4 tile 128 alpha dropped | 2487786 | 1.5 | 2.228 | 0.755 | 997 (199) | 420/420 | σ1 420/420 |
| (d) tiles+zstd -1 tile 32 alpha dropped | 2416723 | 1.5 | 2.544 | 0.800 | 952 (202) | 420/420 | σ1 420/420 |
| (d) tiles+zstd -1 tile 64 alpha dropped | 2419188 | 1.5 | 2.528 | 0.816 | 997 (202) | 420/420 | σ1 420/420 |
| (d) tiles+zstd -1 tile 128 alpha dropped | 2434143 | 1.5 | 2.709 | 0.828 | 992 (202) | 420/420 | σ1 420/420 |
| (d) tiles+zstd 1 tile 32 alpha dropped | 2153029 | 1.7 | 3.804 | 2.673 | 913 (224) | 420/420 | σ1 420/420 |
| (d) tiles+zstd 1 tile 64 alpha dropped | 2068894 | 1.7 | 3.630 | 2.756 | 881 (236) | 420/420 | σ1 420/420 |
| (d) tiles+zstd 1 tile 128 alpha dropped | 2034043 | 1.8 | 3.634 | 2.769 | 896 (242) | 420/420 | σ1 420/420 |
M10 scenario=still size=1266x713 seconds=21.0 delivered_per_s=0.1 new_frames=2 new_per_s=0.10 raw_bytes_per_frame=3610632 sigmas=[1]
| method | bytes/frame | ratio | compress ns/px | decompress ns/px | write µs/frame (files) | B,G,R exact | NV12 exact (σ) |
|---|---|---|---|---|---|---|---|
| (a) raw alpha kept | 3610648 | 1.0 | 0.338 | 0.410 | 1217 (2) | 2/2 | all 4 bytes equal |
| (b) lz4 whole alpha kept | 45302 | 79.7 | 0.846 | 0.528 | 337 (2) | 2/2 | all 4 bytes equal |
| (c) tiles+lz4 tile 32 alpha kept | 24922 | 144.9 | 1.388 | 1.433 | 309 (2) | 2/2 | all 4 bytes equal |
| (c) tiles+lz4 tile 64 alpha kept | 23659 | 152.6 | 1.669 | 1.097 | 319 (2) | 2/2 | all 4 bytes equal |
| (c) tiles+lz4 tile 128 alpha kept | 22957 | 157.3 | 1.621 | 1.244 | 328 (2) | 2/2 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 32 alpha kept | 11548 | 312.7 | 1.157 | 0.999 | 307 (2) | 2/2 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 64 alpha kept | 10450 | 345.5 | 1.232 | 1.074 | 302 (2) | 2/2 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 128 alpha kept | 11128 | 324.5 | 1.184 | 0.984 | 307 (2) | 2/2 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 32 alpha kept | 11092 | 325.5 | 1.135 | 1.205 | 336 (2) | 2/2 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 64 alpha kept | 10030 | 360.0 | 1.281 | 1.193 | 324 (2) | 2/2 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 128 alpha kept | 10726 | 336.6 | 1.240 | 1.033 | 368 (2) | 2/2 | all 4 bytes equal |
| (a) raw alpha dropped | 2707990 | 1.3 | 0.587 | 0.610 | 1043 (2) | 2/2 | σ1 2/2 |
| (b) lz4 whole alpha dropped | 39327 | 91.8 | 1.220 | 1.196 | 335 (2) | 2/2 | σ1 2/2 |
| (c) tiles+lz4 tile 32 alpha dropped | 22098 | 163.4 | 1.419 | 1.120 | 315 (2) | 2/2 | σ1 2/2 |
| (c) tiles+lz4 tile 64 alpha dropped | 20902 | 172.7 | 1.362 | 1.143 | 304 (2) | 2/2 | σ1 2/2 |
| (c) tiles+lz4 tile 128 alpha dropped | 19732 | 183.0 | 1.257 | 1.042 | 305 (2) | 2/2 | σ1 2/2 |
| (d) tiles+zstd -1 tile 32 alpha dropped | 10420 | 346.5 | 1.164 | 1.073 | 312 (2) | 2/2 | σ1 2/2 |
| (d) tiles+zstd -1 tile 64 alpha dropped | 9371 | 385.3 | 1.319 | 1.024 | 357 (2) | 2/2 | σ1 2/2 |
| (d) tiles+zstd -1 tile 128 alpha dropped | 9732 | 371.0 | 1.080 | 1.036 | 359 (2) | 2/2 | σ1 2/2 |
| (d) tiles+zstd 1 tile 32 alpha dropped | 9666 | 373.6 | 1.397 | 1.054 | 319 (2) | 2/2 | σ1 2/2 |
| (d) tiles+zstd 1 tile 64 alpha dropped | 8778 | 411.3 | 1.325 | 1.068 | 314 (2) | 2/2 | σ1 2/2 |
| (d) tiles+zstd 1 tile 128 alpha dropped | 9036 | 399.6 | 1.399 | 1.031 | 356 (2) | 2/2 | σ1 2/2 |
M10 scenario=typing-small size=506x333 seconds=21.0 delivered_per_s=8.0 new_frames=169 new_per_s=8.05 raw_bytes_per_frame=673992 sigmas=[1, 2, 3]
| method | bytes/frame | ratio | compress ns/px | decompress ns/px | write µs/frame (files) | B,G,R exact | NV12 exact (σ) |
|---|---|---|---|---|---|---|---|
| (a) raw alpha kept | 674008 | 1.0 | 0.226 | 0.196 | 535 (169) | 169/169 | all 4 bytes equal |
| (b) lz4 whole alpha kept | 12281 | 54.9 | 0.409 | 0.413 | 312 (169) | 169/169 | all 4 bytes equal |
| (c) tiles+lz4 tile 32 alpha kept | 1244 | 541.6 | 0.270 | 0.121 | 265 (169) | 169/169 | all 4 bytes equal |
| (c) tiles+lz4 tile 64 alpha kept | 1569 | 429.6 | 0.410 | 0.158 | 306 (169) | 169/169 | all 4 bytes equal |
| (c) tiles+lz4 tile 128 alpha kept | 2402 | 280.6 | 0.389 | 0.235 | 315 (169) | 169/169 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 32 alpha kept | 781 | 862.5 | 0.337 | 0.061 | 288 (169) | 169/169 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 64 alpha kept | 942 | 715.2 | 0.373 | 0.086 | 345 (169) | 169/169 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 128 alpha kept | 1406 | 479.5 | 0.500 | 0.129 | 322 (169) | 169/169 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 32 alpha kept | 729 | 924.9 | 0.360 | 0.071 | 277 (169) | 169/169 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 64 alpha kept | 880 | 765.6 | 0.395 | 0.087 | 316 (169) | 169/169 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 128 alpha kept | 1327 | 508.0 | 0.471 | 0.140 | 347 (169) | 169/169 | all 4 bytes equal |
| (a) raw alpha dropped | 505510 | 1.3 | 0.401 | 0.477 | 447 (169) | 169/169 | σ1 169/169, σ2 169/169, σ3 169/169 |
| (b) lz4 whole alpha dropped | 10981 | 61.4 | 0.638 | 0.825 | 286 (169) | 169/169 | σ1 169/169, σ2 169/169, σ3 169/169 |
| (c) tiles+lz4 tile 32 alpha dropped | 1121 | 601.0 | 0.377 | 0.144 | 300 (169) | 169/169 | σ1 169/169, σ2 169/169, σ3 169/169 |
| (c) tiles+lz4 tile 64 alpha dropped | 1365 | 493.9 | 0.381 | 0.169 | 338 (169) | 169/169 | σ1 169/169, σ2 169/169, σ3 169/169 |
| (c) tiles+lz4 tile 128 alpha dropped | 2167 | 311.0 | 0.442 | 0.267 | 318 (169) | 169/169 | σ1 169/169, σ2 169/169, σ3 169/169 |
| (d) tiles+zstd -1 tile 32 alpha dropped | 691 | 975.5 | 0.361 | 0.097 | 306 (169) | 169/169 | σ1 169/169, σ2 169/169, σ3 169/169 |
| (d) tiles+zstd -1 tile 64 alpha dropped | 829 | 812.8 | 0.398 | 0.123 | 328 (169) | 169/169 | σ1 169/169, σ2 169/169, σ3 169/169 |
| (d) tiles+zstd -1 tile 128 alpha dropped | 1256 | 536.8 | 0.537 | 0.198 | 326 (169) | 169/169 | σ1 169/169, σ2 169/169, σ3 169/169 |
| (d) tiles+zstd 1 tile 32 alpha dropped | 655 | 1029.4 | 0.403 | 0.106 | 265 (169) | 169/169 | σ1 169/169, σ2 169/169, σ3 169/169 |
| (d) tiles+zstd 1 tile 64 alpha dropped | 766 | 880.1 | 0.484 | 0.140 | 287 (169) | 169/169 | σ1 169/169, σ2 169/169, σ3 169/169 |
| (d) tiles+zstd 1 tile 128 alpha dropped | 1186 | 568.3 | 0.545 | 0.214 | 295 (169) | 169/169 | σ1 169/169, σ2 169/169, σ3 169/169 |
M10 scenario=scroll-small size=506x333 seconds=21.0 delivered_per_s=72.0 new_frames=420 new_per_s=20.00 raw_bytes_per_frame=673992 sigmas=[1, 2, 3]
| method | bytes/frame | ratio | compress ns/px | decompress ns/px | write µs/frame (files) | B,G,R exact | NV12 exact (σ) |
|---|---|---|---|---|---|---|---|
| (a) raw alpha kept | 674008 | 1.0 | 0.218 | 0.191 | 546 (420) | 420/420 | all 4 bytes equal |
| (b) lz4 whole alpha kept | 38645 | 17.4 | 1.038 | 0.452 | 318 (420) | 420/420 | all 4 bytes equal |
| (c) tiles+lz4 tile 32 alpha kept | 36266 | 18.6 | 1.221 | 0.813 | 325 (420) | 420/420 | all 4 bytes equal |
| (c) tiles+lz4 tile 64 alpha kept | 37144 | 18.1 | 1.339 | 0.750 | 296 (420) | 420/420 | all 4 bytes equal |
| (c) tiles+lz4 tile 128 alpha kept | 38703 | 17.4 | 1.273 | 0.702 | 322 (420) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 32 alpha kept | 21515 | 31.3 | 1.784 | 1.100 | 329 (420) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 64 alpha kept | 22093 | 30.5 | 1.807 | 1.103 | 298 (420) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd -1 tile 128 alpha kept | 23048 | 29.2 | 1.914 | 1.090 | 300 (420) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 32 alpha kept | 20914 | 32.2 | 1.805 | 1.157 | 283 (420) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 64 alpha kept | 21585 | 31.2 | 1.883 | 1.163 | 404 (420) | 420/420 | all 4 bytes equal |
| (d) tiles+zstd 1 tile 128 alpha kept | 22607 | 29.8 | 2.124 | 1.173 | 325 (420) | 420/420 | all 4 bytes equal |
| (a) raw alpha dropped | 505510 | 1.3 | 0.429 | 0.472 | 423 (420) | 420/420 | σ1 420/420, σ2 420/420, σ3 420/420 |
| (b) lz4 whole alpha dropped | 36347 | 18.5 | 1.222 | 0.863 | 305 (420) | 420/420 | σ1 420/420, σ2 420/420, σ3 420/420 |
| (c) tiles+lz4 tile 32 alpha dropped | 34056 | 19.8 | 1.785 | 0.964 | 300 (420) | 420/420 | σ1 420/420, σ2 420/420, σ3 420/420 |
| (c) tiles+lz4 tile 64 alpha dropped | 34840 | 19.3 | 1.619 | 0.943 | 340 (420) | 420/420 | σ1 420/420, σ2 420/420, σ3 420/420 |
| (c) tiles+lz4 tile 128 alpha dropped | 36203 | 18.6 | 1.532 | 0.926 | 329 (420) | 420/420 | σ1 420/420, σ2 420/420, σ3 420/420 |
| (d) tiles+zstd -1 tile 32 alpha dropped | 19660 | 34.3 | 2.175 | 1.399 | 317 (420) | 420/420 | σ1 420/420, σ2 420/420, σ3 420/420 |
| (d) tiles+zstd -1 tile 64 alpha dropped | 19983 | 33.7 | 2.092 | 1.408 | 311 (420) | 420/420 | σ1 420/420, σ2 420/420, σ3 420/420 |
| (d) tiles+zstd -1 tile 128 alpha dropped | 20757 | 32.5 | 1.978 | 1.364 | 305 (420) | 420/420 | σ1 420/420, σ2 420/420, σ3 420/420 |
| (d) tiles+zstd 1 tile 32 alpha dropped | 18031 | 37.4 | 2.057 | 1.321 | 311 (420) | 420/420 | σ1 420/420, σ2 420/420, σ3 420/420 |
| (d) tiles+zstd 1 tile 64 alpha dropped | 18404 | 36.6 | 1.995 | 1.355 | 311 (420) | 420/420 | σ1 420/420, σ2 420/420, σ3 420/420 |
| (d) tiles+zstd 1 tile 128 alpha dropped | 19161 | 35.2 | 1.975 | 1.300 | 344 (420) | 420/420 | σ1 420/420, σ2 420/420, σ3 420/420 |
M12 write model: 308.6 µs per file + 0.2772 µs per KB (1.257 ns per raw-1080p pixel for a raw BGRA frame)
M14 at 1920 x 1080, 20 new frames per second, worst scenario per figure (frames ≥ 20)
| method | best tile | bytes/px worst / mean | compress ns/px worst | write µs/frame at 1080p | live share dev PC | live share weakest (estimate) | within 10 % |
|---|---|---|---|---|---|---|---|
| (a) raw alpha kept | - | 4.0001 / 4.0000 | 0.244 | 2608 | 0.65 % | 9.11 % | yes |
| (b) lz4 whole alpha kept | - | 3.4396 / 0.5785 | 3.298 | 2285 | 1.40 % | 20.17 % | no |
| (c) tiles+lz4 tile 32 alpha kept | 32 | 3.1006 / 0.5010 | 6.134 | 2091 | 2.11 % | 30.63 % | no |
| (d) tiles+zstd -1 tile 64 alpha kept | 64 | 3.1527 / 0.4853 | 6.544 | 2120 | 2.22 % | 32.25 % | no |
| (d) tiles+zstd 1 tile 128 alpha kept | 128 | 2.4639 / 0.3876 | 7.923 | 1725 | 2.53 % | 36.78 % | no |
| (a) raw alpha dropped | - | 3.0001 / 3.0000 | 0.429 | 2033 | 0.62 % | 8.76 % | yes |
| (b) lz4 whole alpha dropped | - | 2.7057 / 0.4664 | 1.496 | 1864 | 0.88 % | 12.52 % | no |
| (c) tiles+lz4 tile 32 alpha dropped | 32 | 2.7341 / 0.4449 | 1.939 | 1880 | 1.00 % | 14.24 % | no |
| (d) tiles+zstd -1 tile 32 alpha dropped | 32 | 2.6773 / 0.4137 | 2.544 | 1847 | 1.15 % | 16.49 % | no |
| (d) tiles+zstd 1 tile 128 alpha dropped | 128 | 2.2534 / 0.3521 | 3.634 | 1604 | 1.40 % | 20.20 % | no |
```
