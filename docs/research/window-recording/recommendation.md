# Recommendation: how to record an application window for an AI

Written by Claude on 2026-11-23. **This is Claude's recommendation, not a proven fact.** Each point names what supports it: a quote in `proof.md` (P-numbers) or a measurement in `measurements.md`. The choice is the owner's.

**Task:** record a clip of an application window on Windows so that an AI model can read back what happened. Goals: maximum quality, small files, low CPU, low RAM.

## The method

**Video while the screen changes, plus an exact still when it settles.**

1. **One video stream of the whole window**, at full resolution, with a long distance between key pictures.
   - Encoder: the graphics chip's HEVC encoder where the PC has one (Intel on this PC).
   - Fallback on any PC: software x264 in 4:4:4.
2. **One lossless still of the window each time the picture stops changing** (unchanged for 0.5 s).
3. **Later addition:** a text and event track from UI Automation and input, stored beside the pictures.

No "moving rectangle" logic, and no lossless storage of changing pixels.

## Why this one

### It is the method the leading screen products use
- Citrix: "Build to lossless compresses the screen using H.264 (or H.265) during screen activity and sharpens to pixel perfect (lossless) when activity stops." (P34)
- PCoIP: "Unchanged regions of the image progressively build to a lossless (perfect) quality…" (P51)
- Step 3 follows screenpipe, which pairs "each screenshot with accessibility tree data" (P13), and Microsoft's statement that UI Automation events remove "the need to continually poll the UI elements" (P41).

### Size, measured on this PC

Real YouTube page, 40 s (video playing, scroll, typing, pause):

| method | MB per minute | against today |
|---|---|---|
| current Synapse recorder (lossless layer + H.264 moving rectangle) | 187 | 100 % |
| Intel hardware HEVC, at closeness 0.98 | 4.0 | 2 % |
| + exact stills at the 4 settle points | + 4.3 | |
| **the recommended method, together** | **about 8** | **about 4 %** |
| fallback: x264 4:4:4, at closeness 0.98 | 4.3 (+ 4.3 stills) | about 5 % |

Mixed interface work, 60 s:

| method | MB per minute |
|---|---|
| Intel hardware HEVC, at closeness 0.997 | 0.5 |
| + exact stills at the 4 settle points | + 0.06 |
| lossless video (x264 RGB) | 15.7 |
| lossless stills of every changed picture | 10.0 |

### Quality
- Whenever the window is still, the clip holds its exact pixels (the still).
- While it changes, the video was close to the source: 0.98 on the real page, 0.997 on mixed interface work.
- Claude read the same text, character for character, from the source and from the Intel HEVC, x264 and SVT-AV1 outputs, in a typing frame and in a frame taken in the middle of a scroll.

### CPU
- Intel hardware HEVC: 0.13–0.15 of one core on the real page. The two Intel hardware encoders were the lowest of everything measured (H.264: 0.10–0.18).
- Fallback x264 4:4:4: 0.37–0.60 of one core on the test scenes.

### Why not the alternatives (all measured here)
- **Stills only, on every change:** 790 MB per minute on the real page, and encoding them took 361 s for 40 s of recording. With a video on screen, 742 of 800 pictures changed.
- **Lossless video:** 11–28 MB per minute on text and interface scenes, 4,631 on the test video.
- **The current design:** 187 MB per minute, and it lost frames when its moving rectangle changed (issue #290).
- **AV1:** Intel's hardware AV1 does not exist on this PC ("Current codec type is unsupported"). Software libaom AV1 matched Intel HEVC's size on the mixed scene (0.5 MB per minute) but used 0.3–0.5 of one core and did not reach closeness 0.98 on the real page within the range tested. NVIDIA's encoders could not be tested (driver too old for this ffmpeg).
- **HEVC screen profile:** not smaller than the plain profile at the setting compared, and ffmpeg's software decoder cannot read it.
- **Plain H.264 from the graphics chip:** works, about 1.8 times the size of HEVC at equal closeness on the real page (7.2 against 4.0 MB per minute).

## What is NOT yet known (must be measured before this is built as the product)

- **RAM of a purpose-built recorder.** Only ffmpeg's own peak was measured (484–568 MB for the Intel encoders, including its source reader).
- **Start-up.** Through ffmpeg the Intel encoders started in about 300–400 ms, once 919 ms; x264 in about 45 ms. Earlier, through the Media Foundation sink writer, an Intel encoder start took 1.7 s after idle time. Which programming route the product uses decides this; frames from the first moments must be held until the encoder is ready, or the software encoder used for the start.
- **Reading back.** Whether every PC that reads the clip can decode HEVC, and how fast one exact moment can be fetched with a long key-picture distance. The exact stills double as entry points; not measured.
- **Text that sits beside a playing video.** With whole-window settling it stays video-quality until the whole window stops. The real page had only 4 settle points in 40 s. Citrix's newer default settles by region (P1); that is more complex and was not measured.
- **Small and coloured text.** The reading test used dark text of about 16–18 pixels.
- **Other PCs.** One PC, one run per test, test pages plus one real page.
- **HEVC licence terms** for shipping an encoder path. Not researched.

## Suggested first step
Build the smallest version of steps 1 and 2 as a probe (not in the product), record the same real page through it, and measure size, CPU, RAM, start-up and read-back time against the numbers above.
