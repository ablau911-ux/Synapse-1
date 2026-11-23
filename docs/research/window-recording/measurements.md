# Measurements on this PC

Measured by Claude on 2026-11-23, one run per scene and encoder. These are measurements, not quotes; the scripts are in `.runs/measure/` of the `Synapse-clip` worktree (`make_src.mjs`, `measure.py`, `startup.py`).

## What was measured, and how

- **PC:** Intel Core i7-13620H with Intel UHD graphics, plus NVIDIA GeForce RTX 4050 Laptop GPU; Windows 11.
- **Tool:** ffmpeg 9.0.1 (gyan.dev full build), run at below-normal priority.
- **Input:** five real captures of a Chrome window at 1920 × 1080, 20 pictures per second, taken earlier with Windows.Graphics.Capture and stored without loss:

| scene | length | what is on screen | new pictures |
|---|---|---|---|
| typing | 20 s | text being typed on a page | 143 of 400 |
| ui | 20 s | a user-interface page with changing controls | 400 of 400 |
| scroll | 20 s | a page scrolling | 400 of 400 |
| video | 20 s | a test video filling the window | 400 of 400 |
| mix | 60 s | a mix of the above | 686 of 1,200 |

- **Each encoder** read the lossless source and wrote a file with one key picture per 30 s.
- **Size** = file size scaled to one minute.
- **Closeness** = SSIM between the decoded file and the source, both compared in RGB (1.0000 = identical).
- **CPU** = processor seconds used per second of recording, after subtracting the cost of reading the source (1.00 = one core fully busy; the PC has 16 logical processors).
- **RAM** = peak memory of the whole ffmpeg program, which also holds the source reader.

## Limits of this measurement (read before using the numbers)

- **The quality settings are not matched between encoders.** Each ran at one common setting. Size and closeness must be read together.
- **The "video" scene is a poor guide.** It barely compresses (the lossless source is 1,192 MB for 20 s) and every lossy encoder scored low on it (0.69–0.82). It behaves like noise, not like a film.
- **CPU includes ffmpeg's own colour conversion and file writing**, so a recorder that keeps frames on the graphics chip would use less.
- **RAM is ffmpeg's, not a recorder's.** It shows order of size between encoders only.
- **One run each.** No spread is known.
- **The scenes are test pages**, not a real application under real use.

## Results

### File size, MB per minute

| encoder | typing | ui | scroll | video | mix |
|---|---|---|---|---|---|
| x264, 4:2:0, CRF 23 | 0.1 | 1.1 | 8.9 | 172.3 | 1.2 |
| x264, 4:4:4, CRF 23 | 0.1 | 1.3 | 9.3 | 154.8 | 1.3 |
| x265, 4:2:0, CRF 26 | 0.3 | 1.7 | 57.8 | 13.7 | 5.7 |
| x265, 4:4:4, CRF 26 | 0.4 | 2.0 | 68.3 | 13.4 | 6.8 |
| SVT-AV1, preset 10, CRF 35, screen mode | 0.1 | 1.4 | 8.2 | 5.2 | 1.2 |
| VP9, realtime, screen mode | 0.1 | 1.4 | 6.2 | 33.6 | 1.1 |
| Intel hardware H.264, quality 23 | 0.1 | 1.2 | 5.4 | 173.6 | 1.1 |
| Intel hardware HEVC, quality 26 | 0.1 | 1.0 | 2.7 | 15.3 | 0.6 |
| x264 RGB, lossless | 11.3 | 19.3 | 28.1 | 4,631.1 | 15.7 |
| WebP animated, lossless | 0.1 | 2.2 | 81.0 | 2,609.7 | 7.5 |
| APNG, lossless | 0.1 | 7.5 | 156.0 | 4,063.6 | 15.1 |

### Closeness to the source (SSIM, RGB)

| encoder | typing | ui | scroll | video | mix |
|---|---|---|---|---|---|
| x264, 4:2:0, CRF 23 | 0.9993 | 0.9966 | 0.9813 | 0.8168 | 0.9972 |
| x264, 4:4:4, CRF 23 | 0.9996 | 0.9983 | 0.9935 | 0.8019 | 0.9988 |
| x265, 4:2:0, CRF 26 | 0.9992 | 0.9964 | 0.9773 | 0.6970 | 0.9967 |
| x265, 4:4:4, CRF 26 | 0.9995 | 0.9980 | 0.9807 | 0.6958 | 0.9976 |
| SVT-AV1, screen mode | 0.9992 | 0.9961 | 0.9828 | 0.6907 | 0.9970 |
| VP9, screen mode | 0.9991 | 0.9950 | 0.9792 | 0.7100 | 0.9962 |
| Intel hardware H.264 | 0.9992 | 0.9956 | 0.9827 | 0.7656 | 0.9971 |
| Intel hardware HEVC | 0.9994 | 0.9973 | 0.9865 | 0.7172 | 0.9976 |
| the three lossless ones | 1.0000 | 1.0000 | 1.0000 | 1.0000 | 1.0000 |

### CPU, processor seconds per second of recording

| encoder | typing | ui | scroll | video | mix |
|---|---|---|---|---|---|
| Intel hardware H.264 | 0.15 | 0.16 | 0.14 | 0.16 | 0.14 |
| Intel hardware HEVC | 0.24 | 0.15 | 0.12 | 0.11 | 0.14 |
| x264 RGB, lossless | 0.20 | 0.23 | 0.24 | 1.00 | 0.21 |
| VP9, screen mode | 0.27 | 0.29 | 0.50 | 1.37 | 0.31 |
| x264, 4:2:0 | 0.29 | 0.36 | 0.43 | 1.75 | 0.34 |
| SVT-AV1, screen mode | 0.36 | 0.38 | 0.51 | 0.79 | 0.42 |
| x264, 4:4:4 | 0.37 | 0.45 | 0.60 | 1.92 | 0.43 |
| APNG, lossless | 0.29 | 0.31 | 0.65 | 3.40 | 0.33 |
| WebP animated, lossless | 0.36 | 0.46 | 1.77 | 15.60 | 0.48 |
| x265, 4:2:0 | 0.93 | 0.97 | 1.66 | 3.59 | 0.99 |
| x265, 4:4:4 | 1.32 | 1.36 | 2.33 | 4.54 | 1.50 |

### Peak RAM of the ffmpeg program, and slowest speed against real time

| encoder | peak RAM, MB | slowest speed |
|---|---|---|
| APNG, lossless | 317 | 0.3 × real time |
| Intel hardware H.264 | 484 | 10.0 × |
| Intel hardware HEVC | 568 | 5.7 × |
| VP9, screen mode | 710 | 6.5 × |
| x264 RGB, lossless | 910 | 10.0 × |
| x265, 4:2:0 | 942 | 3.4 × |
| x264, 4:2:0 | 1,030 | 6.5 × |
| x265, 4:4:4 | 1,160 | 2.6 × |
| SVT-AV1, screen mode | 1,426 | 10.7 × |
| x264, 4:4:4 | 1,635 | 5.7 × |
| WebP animated, lossless | 2,695 | 0.1 × real time |

## What did not run, with the program's own error text

| encoder | result | error text |
|---|---|---|
| Intel hardware AV1 (`av1_qsv`) | does not exist on this PC | "Current codec type is unsupported" |
| NVIDIA hardware HEVC and AV1 (`hevc_nvenc`, `av1_nvenc`) | could not be tested | "Driver does not support the required nvenc API version. Required: 13.1 Found: 13.0"; "The minimum required Nvidia driver for nvenc is 610.00 or newer" |
| Intel hardware HEVC with the screen profile (`-profile:v scc`) | wrote files, but the result is unusable | Decoded closeness was 0.21–0.27 on every scene. Whether the encoder or the decoder is at fault was not determined. |
| libaom AV1, realtime | first attempt failed on Claude's own wrong setting | "Error setting option cpu-used to value 9." Re-run with a valid setting: see the section below once it is filled in. |

## Plain reading of the tables (Claude's reading, not a proven conclusion)

- On the text, interface and mixed scenes, every lossy video encoder produced 0.1–2 MB per minute at a closeness of 0.995 or higher. The differences between codecs there are small in absolute size.
- Scrolling is where they differ: 2.7 MB per minute (Intel hardware HEVC) to 9.3 (x264), and 58–68 for x265 at this setting.
- Lossless is cheap only while little changes: 0.1 MB per minute for typing in WebP or APNG, against 81–156 for scrolling and thousands for video.
- The Intel hardware encoders used the least CPU (0.11–0.24 of one core) on every scene.
- Two lossless image-sequence formats were far slower than real time at these settings (0.1 × and 0.3 ×).
- 4:4:4 against 4:2:0 in x264: closer to the source on every scene except video, at 5–20 % more size and more RAM.

Not measured here: start-up time (section to follow), how well a model reads the text in each output (section to follow), stills taken only on change with changed rectangles, a real application under real use, and a recorder that keeps frames on the graphics chip.
