# Recommendation: how to record an application window for an AI

Written by Claude on 2026-11-23, after three measurement rounds on this PC. Each point names its support: a quote in `proof.md` (P-numbers) or a table in `measurements.md`. The owner decides.

**Task:** record a clip of an application window on Windows so that an AI model can read back what happened. Goals: maximum quality, small files, low CPU, low RAM.

## The method

**Video while the screen changes, plus an exact still when it settles.**

1. **Capture** the window with Windows.Graphics.Capture and keep the pictures on the graphics chip.
2. **Encode one video stream of the whole window** at full resolution with the graphics chip's HEVC encoder.
   - Reach the encoder through **Direct3D 12 video encoding** (part of Windows 11) or the chip maker's own library. **Not through the Media Foundation sink writer.**
   - One key picture every 5 seconds.
3. **Write one lossless still of the window each time the picture stops changing** for half a second.
4. **Fallback** on a PC with no hardware HEVC encoder: Windows' own software H.264 encoder.
5. **Later addition:** a text and event track from UI Automation and input, beside the pictures.

There is no "moving rectangle" logic and no lossless storage of changing pixels.

## What was measured for this method

All on this PC, on a real YouTube page (video playing, a scroll, typing, a pause), one run each.

| goal | result | where |
|---|---|---|
| Small files | Video 4.0–8 MB per minute at closeness 0.98 (by route), plus 4.3 for the exact stills. A live recording came to 11.1 MB per minute. Today's recorder: 187. | measurements, second and third parts |
| Small files, interface work | About 0.5–0.7 MB per minute, plus 0.06 for stills | second and third parts |
| Low CPU | Live recorder: 0.034 of one core | third part, live recording |
| Low RAM | Live recorder: 327 MB peak (the whole ffmpeg program) | third part, live recording |
| Starts at once | 113–141 ms through Direct3D 12, also after 150 s idle | third part, start-up |
| Quality when still | Exact pixels (the lossless still) | by construction |
| Quality when changing | Closeness 0.98 on the real page, 0.997 on interface work | second part |
| A model can read it | Claude read the same text from source and encodes: typing, mid-scroll, and 12–13 pixel grey text beside the playing video | first and third parts |
| Reading one moment back | 171 ms worst case with a key picture every 5 s (software decoder) | third part |

Against today's recorder, the recommended method needs about 5–9 % of the file size on the real page.

## Why each choice

- **Video plus exact still:** it is what the leading screen products do. Citrix: "Build to lossless compresses the screen using H.264 (or H.265) during screen activity and sharpens to pixel perfect (lossless) when activity stops." (P34) PCoIP: "Unchanged regions of the image progressively build to a lossless (perfect) quality…" (P51)
- **HEVC from the graphics chip:** smallest at equal closeness of everything that ran (4.0 MB per minute against 6.9 for x264 4:2:0 and 7.2 for hardware H.264), at 0.13–0.15 of one core.
- **Direct3D 12, not Media Foundation:** the same Intel encoder started in 113–141 ms through Direct3D 12 and in 0.9–2.7 s through Media Foundation. The current recorder's 1.7 s start came from that route.
- **Key picture every 5 s:** costs 47 % more than every 30 s (4.7 against 3.2 MB per minute) and cuts the worst-case fetch from 460 ms to 171 ms.
- **Windows' own software H.264 as fallback:** it ships with Windows. x264 was smaller (4.3 against about 18 MB per minute at closeness 0.98) but it is "released under the terms of the GNU GPL" (P56).
- **Text and event track later:** screenpipe pairs "each screenshot with accessibility tree data" (P13); UI Automation events remove "the need to continually poll the UI elements" (P41). Codex and Opus both put this in their first choice (opinions).

## Why not the alternatives (measured)

| alternative | result |
|---|---|
| Lossless stills of every changed picture | 790 MB per minute on the real page; encoding took 361 s for 40 s |
| Lossless video | 11–28 MB per minute on text and interface scenes; thousands on video |
| Today's design (lossless layer + moving rectangle) | 187 MB per minute; frames lost when the rectangle changed (#290) |
| AV1 from the graphics chip | Not usable on this PC: Intel's library says "Current codec type is unsupported"; the Media Foundation AV1 encoder wrote files with no readable video |
| AV1 in software (libaom) | Same size as Intel HEVC on interface work (0.5), but 0.3–0.5 of one core, and below closeness 0.98 on the real page in the range tested |
| HEVC screen profile | Valid file, but not smaller at the setting compared, and ffmpeg's software decoder cannot read it |
| Hardware H.264 | Works; about 1.8 times the size of HEVC at equal closeness |

## Questions that were open and are now answered by measurement

- **The NVIDIA encoders** (measured with ffmpeg 7.1.1). At closeness 0.98 on the real page: NVIDIA HEVC about 6.0 MB per minute, NVIDIA H.264 about 6.3, against 4.0–4.6 for Intel HEVC. NVIDIA AV1 did not reach 0.98 in the range tested. Their first start after idle took about 1.7 s, with the NVIDIA chip read as powered off (D3) just before; with the chip awake, 157–194 ms. **So on a PC with two graphics chips, use the encoder of the integrated chip: it is the smallest here and it never has to wake the second chip.**
- **Can settings close the size gap of Direct3D 12?** No. Five settings were tried; the best wrote 6.9 MB per minute where Intel's library wrote 4.6 at the same closeness. The choice is a trade: Direct3D 12 starts in about 130 ms and costs 50–60 % more size; Intel's library starts in about 400 ms and is the smallest.
- **Exact text beside a playing video.** On the real page the side column changed in 557 of 800 pictures and the header in 699, because the glow around the player follows the video. Settling by region would give exact text there only 5–7 % of the time. The text there stays video quality, which Claude read without error.

## What is still not known

- **Other PCs.** Everything was measured on one PC (Intel plus NVIDIA graphics). AMD graphics and older PCs were not tested.
- **A recorder written for the product.** The figures come from ffmpeg standing in for the recorder. A purpose-built one may use less memory; that can only be measured once it exists, which is the suggested first step below.
- **HEVC licence.** The patent pool says "A royalty is due upon the Sale of a Consumer HEVC Product…" (P61) and that it licenses encoders "in devices, software, or through Cloud Based Services" (P55). It does not say who pays when a program only uses the encoder already in the PC's graphics chip. A trade article on the pool's policy says, for decoders: "If the software uses an existing hardware decoder, of course, there's no royalty, since a royalty on that hardware should have already been paid." (P66) That points the same way for a hardware encoder but does not state it, and it is not the pool's current text. A legal answer is still needed. The fallback (Windows' own H.264) avoids the question at about four times the size.
- **HEVC on the reading PC.** This PC has Microsoft's HEVC extension installed; others may not.

## Suggested first step

Build the smallest version of steps 1–3 as a separate probe (not in the product): Windows.Graphics.Capture → Direct3D 12 HEVC → file, plus the settled still. Record the same real page and compare with the tables in `measurements.md`.
