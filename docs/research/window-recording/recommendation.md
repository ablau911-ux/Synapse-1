# Recommendation: how to record an application window for an AI

Written by Claude on 2026-11-23, after five measurement rounds on this PC. Each point names its support: a quote in `proof.md` (P-numbers) or a table in `measurements.md`. The owner decides.

**Task:** record a clip of an application window on Windows so that an AI model can read back what happened. Goals: maximum quality, small files, low CPU, low RAM.

## The method

**Video while the screen changes, plus an exact still when it settles.**

1. **Capture** the window with Windows.Graphics.Capture.
2. **Encode one video stream of the whole window** at full resolution, one key picture every 5 seconds.
3. **Write one lossless still of the window each time the picture stops changing** for half a second.
4. **Later addition:** a text and event track from UI Automation and input, beside the pictures.

There is no "moving rectangle" logic and no lossless storage of changing pixels.

## Which encoder: two measured choices

| | A. Software AV1 (libaom, realtime, screen mode) | B. HEVC on the integrated graphics chip |
|---|---|---|
| Works on any PC | yes, it is software | only where the chip has an HEVC encoder |
| Royalty | none: "no-charge, royalty-free" patent licence (P67) | open legal question (P55, P61, P66) |
| Size, interface work | 0.5–0.7 MB per minute | 0.5–1.0 |
| Size, scrolling | 2.5 MB per minute | 2.7 |
| Size, real YouTube page | 6.8 MB per minute at closeness 0.9796 | 4.0–4.6 at closeness 0.98 |
| CPU | 0.28–0.48 of one core | 0.03–0.15 of one core |
| RAM (whole ffmpeg program) | 382–412 MB | 327–568 MB |
| Start-up | about 70 ms | 130 ms (Direct3D 12) to 400 ms (Intel's library) |
| Reading it back | AV1 decoder (Microsoft's AV1 extension is installed here) | HEVC decoder (Microsoft's HEVC extension is installed here) |

**Claude's choice: A, software AV1, as the standard encoder.** It needs no particular hardware, carries no royalty question, starts at once, and matched the hardware HEVC encoder's size on interface work and scrolling. Its cost is processor time: about a third to a half of one core on this 16-thread PC.

**B is the upgrade where it applies:** on a PC with an HEVC encoder in the integrated chip, and once the royalty question has a legal answer, it cuts processor time to a tenth and the file by about a third on video-heavy pages. Reach it through Direct3D 12 or the chip maker's library, never through the Media Foundation sink writer, and never on the second (NVIDIA) chip.

## What was measured for this method

All on this PC, one run each.

| goal | result | where |
|---|---|---|
| Small files | Real YouTube page: 4.0–6.8 MB per minute of video plus 4.3 for the exact stills. Today's recorder: 187. | measurements, second part |
| Small files, interface work | 0.5–0.7 MB per minute, plus 0.06 for stills | first and second parts |
| Low CPU | A: 0.28–0.48 of one core. B: 0.034 in a live recording. | first and third parts |
| Low RAM | 327–412 MB peak for the whole ffmpeg program | first and third parts |
| Starts at once | A: about 70 ms. B: 113–141 ms through Direct3D 12. | third part and this file's source table |
| Quality when still | Exact pixels (the lossless still) | by construction |
| Quality when changing | Closeness 0.98 on the real page, 0.996–0.997 on interface work | first and second parts |
| A model can read it | Claude read the same text from the source and from the encodes: typing, mid-scroll, and 12–13 pixel grey text beside the playing video | first and third parts |
| Reading one moment back | 171 ms worst case with a key picture every 5 s (measured on HEVC, software decoder) | third part |

Against today's recorder, the method needs about 5–9 % of the file size on the real page.

## Why each choice

- **Video plus exact still:** it is what the leading screen products do. Citrix: "Build to lossless compresses the screen using H.264 (or H.265) during screen activity and sharpens to pixel perfect (lossless) when activity stops." (P34) PCoIP: "Unchanged regions of the image progressively build to a lossless (perfect) quality…" (P51)
- **AV1 for screens:** Microsoft Teams measured "lowered bandwidth for Teams screensharing by 63% compared to the H.264 video codec" after moving to AV1 (P4); Chrome's libaom "includes improvements targeting screen sharing" (P24). On this PC it wrote the smallest files of all software encoders on interface work and scrolling.
- **Key picture every 5 s:** costs 47 % more than every 30 s and cuts the worst-case fetch from 460 ms to 171 ms.
- **Not the Media Foundation route:** the same Intel encoder started in 113–141 ms through Direct3D 12 and in 0.9–2.7 s through Media Foundation.
- **Not the NVIDIA chip:** its encoders wrote larger files than Intel's, and their first start after idle took about 1.7 s while the chip woke from power state D3.
- **Text and event track later:** screenpipe pairs "each screenshot with accessibility tree data" (P13); UI Automation events remove "the need to continually poll the UI elements" (P41).

## Why not the alternatives (measured)

| alternative | result |
|---|---|
| Lossless stills of every changed picture | 790 MB per minute on the real page; encoding took 361 s for 40 s |
| Lossless video | 11–28 MB per minute on text and interface scenes; thousands on video |
| Today's design (lossless layer + moving rectangle) | 187 MB per minute; frames lost when the rectangle changed (#290) |
| Settling by region instead of whole window | Regions beside the playing video are still only 5–7 % of the time |
| x264 | Small (4.3 MB per minute in 4:4:4) but "released under the terms of the GNU GPL" (P56) |
| Windows' own software H.264 | About 18 MB per minute at closeness 0.98; usable as a last fallback |
| AV1 on the graphics chip | Not usable on this PC (Intel: "Current codec type is unsupported"; NVIDIA AV1 below closeness 0.98 in the range tested) |
| HEVC screen profile | Valid file, not smaller, and ffmpeg's software decoder cannot read it |

## What a build must still measure

These cannot be answered by reading or by measuring ffmpeg; they belong to the first build step.

- **Memory of a recorder written for the product.** ffmpeg stood in for it (327–412 MB for the whole program, including its own reader and buffers).
- **Speed of software AV1 on a slower or older PC.** Here it ran 4.9–10 times faster than real time at 20 pictures per second.

## Suggested first step

Build the smallest version as a separate probe (not in the product): Windows.Graphics.Capture → libaom AV1 in realtime screen mode → file, plus the lossless still when the window settles. Record the same real page and compare with the tables in `measurements.md`.
