# Research log: best way to record an application window for an AI to read back

**Task (the only thing given):** record a clip of an application window on Windows so that an AI model can read back what happened. Goals: maximum quality, small files, low CPU, low RAM.

**This file no longer holds findings.** Its first version carried 51 findings written from search-result summaries, before Claude had opened the pages. The owner's rule is that all research is proven from text, so that version was replaced on 2026-11-23:

- Every claim that Claude then proved by opening the page and copying its words is in `proof.md` (P1–P67).
- Every claim that could not be proven was removed. `proof.md` names them once, as withdrawn, so nobody re-uses them.
- Measurements made on this PC are in `measurements.md`.
- The recommendation is in `recommendation.md`.
- The first version of this log remains in the Git history (commit `4b5498cb`) as a record of how the research started.

## What was searched (51 searches, 2026-11-23, by Claude directly)

| batch | subjects |
|---|---|
| 1 | screenpipe; Microsoft Recall; screen content coding in HEVC and VVC; WebRTC content hints |
| 2 | x264 settings for screencasts; AV1 screen tools; change information in Windows.Graphics.Capture; hardware AV1 encoders |
| 3 | how models take in images; text recognition against compression; Playwright traces and video; Remote Desktop encoding |
| 4 | lossless still-image formats; colour resolution and text; lossless screen video; Windows' H.264 encoder |
| 5 | codecs made for screens (TSCC, ZMBV, Windows Media Video 9 Screen); VNC encodings against H.264; rrweb; mixed raster content |
| 6 | Windows Steps Recorder and Chrome DevTools screencast; OBS capture methods; Desktop Duplication; software encoders with screen modes |
| 7 | YouTube delivery sizes; AV1 for screen sharing in browsers; Zoom and Teams screen sharing; game and desktop streaming |
| 8 | Intel hardware screen content coding; Snipping Tool and Game Bar; containers and crashes; Direct3D 12 video encoding |
| 9 | how models take in video; UI tree against screenshots for agents; AV2; "build to lossless" in Citrix and PCoIP |
| 10 | software AV1 speed; not encoding unchanged frames; lossless video codecs; NVIDIA and AMD hardware encoders |
| 11 | neural video codecs; VVC encoders; recording drawing commands; recording program execution |
| 12 | encoder start-up; other Windows capture routes; reading one moment from a video; conferencing screen codecs |
| 13 | VP9 screen mode; UI Automation events; choosing frames from a recording for a model |

Later reading passes for proof: licence terms of HEVC, x264 and AV1; archived copies of pages that could not be read directly.

The full list of subjects, with what is covered, is in `subject-list.md`.
