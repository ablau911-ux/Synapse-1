# Research log: best way to record an application window for an AI to read back

**Task (the only thing given):** record a clip of an application window on Windows so that an AI model can read back what happened. Goals: maximum quality, small files, low CPU, low RAM. No earlier design, numbers or decisions are used.

**Rules of this log:** every finding is written here with its source, as found. "Not found" results are logged too. A finding marked UNVERIFIED comes from a search summary or secondary source and was not checked against the primary page.

Started 2026-11-23, by Claude, searching directly (no agents).

---

## Batch 1 (searches 1–4)

### 1. screenpipe (open-source screen recorder built for AI)
- Captures per-monitor frames at a configurable rate. Default 1.0 frame per second (0.5 on macOS). Source: https://docs.screenpi.pe/architecture
- Stores MP4 video chunks (5–10 minute chunks) under `~/.screenpipe/data/`; metadata and text go into a SQLite database. Sources: https://docs.screenpi.pe/architecture , https://www.mintlify.com/screenpipe/screenpipe/advanced/performance
- Text comes first from the accessibility tree; OCR is the fallback (Windows native OCR on Windows). Source: https://mintlify.com/screenpipe/screenpipe/architecture
- A newer doc page describes event-driven capture instead of a fixed rate ("no FPS slider; capture happens when events occur"). Source: https://mintlify.com/screenpipe/screenpipe/introduction UNVERIFIED which version is current.
- Disk use claims conflict: about 30 GB/month at 1 fps (architecture docs); 5–10 GB/month (site FAQ, https://screenpi.pe/about); about 20 GB/month (README); a sample "45 GB in 18 days" on the performance page.
- Not found: which codec and settings its MP4 uses. H.265 not confirmed.
- **Meaning:** a recorder made for AI uses a very low frame rate plus a text index, not smooth video.

### 2. Microsoft Recall
- Takes snapshots periodically, only while the screen content differs from the previous snapshot. Source: https://support.microsoft.com/en-us/topic/2c35b596-5a96-4090-b791-c27fae75f660
- Storage allocation: 25 GB default, 10 GB on a 256 GB device; oldest snapshots are deleted when full. Source: https://learn.microsoft.com/en-us/windows/client-management/manage-recall
- "Every five seconds" interval: secondary source only (Yahoo Tech). UNVERIFIED.
- Not found: per-snapshot size, image format, how change is detected.
- **Meaning:** Microsoft's own AI screen memory stores stills on change, not video.

### 3. Screen content coding: HEVC, VVC
- HEVC version 1 against H.264/AVC, equal PSNR: about 23 % smaller (all-intra), 34 % (random access), 38 % (low delay). Source: Li, Xu, Sullivan, "Compression performance of HEVC and its format range and screen content coding extensions", https://www.microsoft.com/en-us/research/?p=798826
- HEVC screen content coding (SCC) extensions on screen content: about 66 %, 63 %, 61 % smaller (all-intra, random access, low delay). Same source. The baseline of this figure was not visible in the summary. UNVERIFIED baseline.
- VVC against HEVC on 4:2:0 screen content: 41–61 % smaller; against HEVC-SCC: 19–25 % smaller. Source: https://www.microsoft.com/en-us/research/?p=798274
- Why screen content is special: text has very high frequencies; the same patterns repeat inside one picture, so intra block copy and palette modes work well. Source: https://www.cambridge.org/core/services/aop-cambridge-core/content/view/E12DCF69B0A59DD269EFE14CD1AFBFBE/S2048770315000116a.pdf/overview-of-hevc-extensions-on-screen-content-coding.pdf
- **Meaning:** codecs with screen content tools can be far smaller than plain H.264 on text and UI. Whether such an encoder is available in real time on a normal PC is still open (see 8).

### 4. How browsers send screen content (WebRTC)
- The `contentHint` values "detail" and "text" tell the encoder to keep sharpness and drop frame rate instead. Source: https://www.w3.org/TR/mst-content-hint
- The spec states the reason: text-heavy screencasts are "completely unintelligible if the quantization levels are too high or if the content is downscaled". Same source.
- Chrome's default for screen share is `maintain-resolution`: it lowers frame rate before it lowers resolution. Sources: https://lists.w3.org/Archives/Public/public-webrtc-logs/2024Nov/0035.html , https://groups.google.com/a/chromium.org/g/blink-dev/c/WPvxi5nUK2E/m/sGUnH8DdAgAJ
- Not found: Chromium's exact encoder settings for screen content.
- **Meaning:** for screen content, the proven trade is resolution and sharpness first, frame rate last.

## Batch 2 (searches 5–8)

### 5. H.264 (x264) settings and screen recordings
- Each +6 of CRF roughly halves the size; each −6 doubles it. Source: https://trac.ffmpeg.org/wiki/Encode/H.264
- For screencasts of bugs, a high CRF (31) is visibly bad on flat areas, text and icons. Source: https://bugs.kde.org/show_bug.cgi?id=476186
- Screen recordings hold many duplicate frames; removing them, or recording at about 5 frames per second, shrinks the file a lot. Source: Stack Overflow thread mirrored at https://kiwix.lema.org/stackoverflow.com_en_all_2023-11/questions/64518242/what-ffmpeg-arguments-will-approximate-zoom-recording-quality UNVERIFIED numbers.
- Not found: a measured MB-per-minute figure for static desktop text.

### 6. AV1 screen content tools
- AV1 has palette mode and intra block copy for screen content; both are in the libaom reference encoder. Sources: https://aomedia.org/docs/AV1_ToolDescription_v11-clean.pdf , https://www.jmvalin.ca/papers/AV1_tools.pdf
- AV1 overall is about 30 % smaller than VP9 (whole codec, not screen content alone). Source: https://research.facebook.com/file/115187374892843/Analysis_of_AV1_Coding_Tools.pdf
- Not found: a percentage for AV1's screen tools on screen content.

### 7. Windows.Graphics.Capture: change information
- `GraphicsCaptureSession` has `DirtyRegionMode` and `MinUpdateInterval`; `Direct3D11CaptureFrame` has `DirtyRegions`. Sources: https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession , https://microsoft.github.io/windows-docs-rs/doc/windows/Graphics/Capture/struct.GraphicsCaptureSession.html
- These are new (Windows build 26100) and barely documented. Source: https://github.com/robmikh/Win32CaptureSample/issues/82
- Chromium's WebRTC capturer detects dirty-region support at run time and uses it to improve capture performance and its "zero-hertz" mode (no frames while nothing changes). Source: https://arai.searchfox.org/firefox-main/source/third_party/libwebrtc/modules/desktop_capture/win/wgc_capturer_win.cc
- Not confirmed: that a session delivers a frame only when the window changed.
- **Meaning:** Windows can tell the recorder which parts changed, so the recorder need not compare pictures itself. To verify on a real PC.

### 8. Hardware AV1 encoding
- All GeForce RTX 40 series GPUs, laptop GPUs included, encode AV1 in real time. Source: https://blogs.nvidia.com/blog/av1-obs29-youtube
- Intel: AV1 encode arrived with Arc graphics. Source: https://www.pcgamer.com/uk/intel-arc-gpus-will-support-av1-encode-and-decode . Older integrated Intel graphics (UHD of 12th/13th gen): AV1 decode only, no encode. UNVERIFIED (from memory, not from this search).
- An Intel community report: the AV1 hardware encoder of an Arc 140T is reachable through Quick Sync/oneVPL but is not registered in Media Foundation. Source: https://community.intel.com/t5/Graphics/AV1-encoding-not-available-Poor-video-encoding/m-p/1754079
- **Meaning:** hardware AV1 encoding exists only on some PCs, and Windows' built-in media interface does not always expose it.

## Batch 3 (searches 9–12)

### 9. What an AI model can take in as an image
- Claude: images are scaled down to a long edge of at most 1568 pixels (standard models) or 2576 pixels (newest high-resolution models). Cost is one token per 28 × 28 pixel block; cap 1568 tokens (standard) or 4784 tokens (newest). Source: https://platform.claude.com/docs/en/build-with-claude/vision.md
- A larger image is resized before the model sees it; the extra pixels add delay and no accuracy. Anthropic suggests at most about 1.15 megapixels for fast answers. Same source.
- Not found in this search: Gemini's video sampling rate and tokens per frame.
- **Meaning:** a full 1920 × 1080 frame (2.07 megapixels) is shrunk before a standard model reads it. For small text the model needs a crop of the area at full resolution, not the whole frame. A recorder for AI must keep full resolution and serve crops.

### 10. Text recognition against compression and resolution
- No rigorous study found that gives OCR accuracy against compression quality and character height in pixels.
- Practitioner guidance: JPEG quality 90 % and above reads about as well as PNG for clear, large print (https://dev.to/designs_forus_0fc9a45b6f/jpg-vs-png-for-ocr-which-image-format-gives-better-text-extraction-3ega); quality 0.8 "does not degrade OCR accuracy significantly" (patent text, https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/10198628). UNVERIFIED, vendor and blog level.
- Strong lossy compression blurs sharp edges and puts halos around characters; resolution matters more than file format (https://how-ocr-works.com/images/compression.html , IBM guidance https://ibm.com/docs/SSYHZ8_24.0.0/com.ibm.dba.dp/topics/con_ocr_best_practices.html).
- A 2025 preprint on OCR-aware compression reports that JPEG-type codecs blur and block text at low bitrates: https://arxiv.org/html/2503.19495v2
- **Meaning:** no published threshold. Legibility at each setting must be measured on real frames with the real reader (the model).

### 11. Playwright (browser test recorder)
- A trace is a zip: full DOM snapshots per action, plus screencast frames stored as separate JPEG files. Sources: https://playwright.dev/docs/trace-viewer , https://docs.rs/playwright-rs-trace
- One measured trace: about 3.4 MB for 276 screenshot JPEGs and 40 DOM snapshots; the screenshots were most of it. Source: https://dzone.com/articles/playwright-trace-debugging (one data point).
- Its video recording uses VP8 at a fixed 1 Mbit/s target, quality not configurable; users call the result visibly compressed. Sources: https://github.com/microsoft/playwright/issues/31424 , https://dev.to/mutsuntsai/replacing-playwrights-hardcoded-vp8-encoder-a-deep-dive-into-pagescreencast-43ee
- **Meaning:** the main tool for recording automated runs stores structure (DOM) plus JPEG stills per action for analysis, and treats video as a rough extra. 1 Mbit/s is 7.5 MB per minute.

### 12. Remote desktop (RDP) graphics encoding
- RDP classifies screen content and uses a different codec for each kind: H.264 for images and frequently updated areas, a separate codec for text. Sources: https://learn.microsoft.com/en-us/azure/virtual-desktop/graphics-encoding , https://techcommunity.microsoft.com/t5/microsoft-security-and/remote-desktop-protocol-rdp-10-avc-h-264-improvements-in-windows/ba-p/249588
- AVC444 mode sends full colour resolution (4:4:4) through H.264, made for text sharpness. Source: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpegfx/844018a5-d717-4bc9-bddb-8b4d6be5dd3f
- RemoteFX Progressive sends a rough picture first and refines it while the area stays unchanged. Source: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpegfx/f8c3c134-bf99-45eb-be7e-d22c8178255f
- Not found: bandwidth figures; confirmation that unchanged regions cost nothing.
- **Meaning:** the mature screen-remoting system does NOT use one method. It sends moving areas as video, text with a text-friendly codec, and refines still areas to full quality over time.

## Batch 4 (searches 13–16)

### 13. Lossless still-image formats for screenshots
- No benchmark found that tests PNG, WebP lossless, JPEG XL and QOI together on UI screenshots.
- WebP lossless is 23 % smaller than ZopfliPNG and 42 % smaller than libpng on a web-image corpus (Google's own study): https://developers.google.com/speed/webp/docs/webp_lossless_alpha_study
- 2018 test on screenshots (4 KB to 12 MB): WebP lossless much smaller than PNG; AVIF lossless often larger than PNG: https://lafibre.info/images/format/lossless_compression_png_webp_avif.pdf
- JPEG XL lossless: smaller than PNG, lossless WebP and lossless AVIF across speeds; at low effort it is twice as fast as QOI and 31 % smaller on non-photographic images (Cloudinary, the format's co-author): https://cloudinary.com/blog/jpeg-xl-and-the-pareto-front
- Academic comparison: FLIF smallest, JPEG XL close behind: https://arxiv.org/pdf/2108.02557
- A 2025 benchmark with about 2,100 desktop-application screenshots exists (WebP, AVIF, JPEG XL): https://bensuperpc.github.io/benchmark/webp ; its results were not in the summary. TO READ.

### 14. Colour resolution (4:4:4 against 4:2:0) and text
- 4:2:0 halves colour resolution; the blur shows mainly on small coloured text and text on a coloured background. Sources: https://www.rtings.com/tv/learn/chroma-subsampling , https://www.displayninja.com/chroma-subsampling/
- NVIDIA forum case: text colour was blurred in H.264 until the pipeline was switched to YUV 4:4:4: https://forums.developer.nvidia.com/t/the-text-blurred-in-output-h264-created-by-nvfbctoglenc/231994
- NVENC can output 4:4:4. Quick Sync through OBS has been forced to 4:2:0 (NV12) for years; reports conflict on whether Quick Sync can do 4:4:4 at all: https://obsproject.com/forum/threads/quicksync-4-4-4-chroma-encoding-not-working.176685/
- Not found: whether Intel's HEVC 4:4:4 hardware encode is reachable on Windows.

### 15. Lossless video of a screen (x264 at qp 0)
- It is large. OBS forum test at 1900 × 1200, 48 fps, x264 lossless ultrafast: 62–70 MB per second. Source: https://obsproject.com/forum/resources/almost-lossless-local-recoding-using-obs-studio.200/
- ffmpeg guide: lossless output "will likely be huge", and plain libx264 at qp 0 converts colour, so only libx264rgb is truly lossless: https://trac.ffmpeg.org/wiki/Encode/H.264 , https://trac.ffmpeg.org/wiki/Capture/Desktop
- Screen grabs change little frame to frame, which is why they compress well when the encoder exploits it (ffmpeg-user list): https://ffmpeg.org/pipermail/ffmpeg-user/2014-November/024217.html
- **Meaning:** lossless for moving content is out. Lossless is affordable only for content that does not change.

### 16. The H.264 encoder that ships with Windows
- Quality mode (`eAVEncCommonRateControlMode_Quality`) holds a constant quality given by `CODECAPI_AVEncCommonQuality`, range 1–100, default 70. Source: https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder , https://learn.microsoft.com/en-us/windows/win32/codecapi/avenccommonquality-property
- Profiles: Baseline, Main, High. Same source.
- Not found: a screen-content setting (`AVEncVideoContentType`), and the built-in HEVC encoder's availability. TO SEARCH.

## Batch 5 (searches 17–20)

### 17. Codecs made only for screens (family A2)
- **TSCC (TechSmith, Camtasia):** picture difference coded as run-length (like Microsoft RLE8, extended to 16 and 24 bit) and then packed with zlib. Lossless. Made for tutorials where only the cursor moves. Sources: https://wiki.multimedia.cx/index.php/TSCC , https://ffmpeg.org/doxygen/trunk/tscc_8c.html
- **TSC2 (TechSmith 2):** 16 × 8 macroblocks, skip/coded flag per frame, two quantiser values (so it can be lossy). Thin documentation. Source: https://wiki.multimedia.cx/index.php/TechSmith_Screen_Capture_Codec_2
- **ZMBV (DOSBox):** key frames and delta frames, zlib; in a delta frame each block is copied from the previous frame (with a motion vector) or XOR-ed against it. Lossless. Source: https://wiki.multimedia.cx/index.php/ZMBV
- **Windows Media Video 9 Screen (MSS2):** ships with Windows as a Media Foundation transform; "optimized for compressing consecutive screen shots", exploits simple pictures and little motion; poor on photographic content; one-pass only. Sources: https://learn.microsoft.com/en-us/windows/desktop/medfound/windowsmediavideo9screenencoder , https://learn.microsoft.com/en-us/previous-versions/bb288697(v=vs.85)
- **Meaning:** all of these share one idea: store only what changed since the last frame, without loss, using run-length or block copy plus a general compressor. They are cheap on CPU and exact, and weak on video or photos.

### 18. VNC encodings and H.264 (family A4)
- Tight encoding against Hextile: 52–98 % fewer bytes, largest gain on uniform screen data (old TightVNC data): https://www.tightvnc.com/archive/compare.html
- Tight compression level 3 → 6: only 4.3 % better ratio for 56 % more CPU; level 6 → 9: ratio often worse, CPU 4.4 times higher. Source: https://turbovnc.org/pmwiki/uploads/About/tighttoturbo.pdf
- **TurboVNC's H.264 study:** "for most workloads, TurboVNC does a much better job of compression than H.264". H.264 won only on game-like or video-like workloads. Reason given: H.264 must produce a whole new frame whenever anything changes, "even if the change is only 1 pixel". Software H.264 was also far too slow on 2D data. Source: https://turbovnc.org/About/H264 (2014-era study, old hardware).
- TigerVNC's developer agreed later: H.264 "still doesn't compress as well" as the Tight-type encoder for most workloads. Source: https://groups.google.com/g/tigervnc-devel/c/kGS73nsjXBg
- **Meaning:** for ordinary desktop content, "send only the changed rectangles, each in the cheapest suitable encoding" beat video coding in these studies; video coding wins when large areas change all the time.

### 19. Recording structure instead of pixels (family H)
- rrweb: one full snapshot of the page's DOM (each node gets an id), then a stream of changes (nodes added or removed, attributes, text, mouse, input) with timestamps; replay applies them in order. Works only inside a web page. Sources: https://raw.githubusercontent.com/rrweb-io/rrweb/HEAD/docs/observer.md , https://raw.githubusercontent.com/rrweb-io/rrweb/HEAD/docs/replay.md
- Not found: measured size of such a recording against video. Not found: anything on Windows Steps Recorder. TO SEARCH.

### 20. Layered compression of mixed text and pictures (family A5)
- Mixed Raster Content (ITU-T T.44): a page is split into a full-resolution binary mask (the sharp text edges) plus a foreground and a background layer; the mask is coded without loss, the two image layers with lossy coders at lower resolution. Sources: https://engineering.purdue.edu/~bouman/publications/pdf/tip19.pdf , https://engineering.purdue.edu/~bouman/publications/pdf/jei5.pdf
- DjVu is the best-known product of this kind. Same sources.
- Weakness: a wrong split between layers makes visible artefacts.
- **Meaning:** "text exact, pictures lossy" is an established technique for documents. It is the same idea RDP uses for screens (finding 12).

## Batch 6 (searches 21–24)

### 21. Windows Steps Recorder; Chrome DevTools screencast
- Steps Recorder (psr.exe) saves a ZIP holding an MHT file; each step has a screenshot plus automatic notes about the action. Source: https://support.microsoft.com/en-us/windows/steps-recorder-deprecation-a64888d7-8482-4965-8ce3-25fb004e975f
- Microsoft deprecated it in February 2024 and points to Snipping Tool, Game Bar and Clipchamp instead. Same source; https://www.ghacks.net/2024/01/08/microsoft-is-removing-a-tool-from-windows-11-that-you-may-have-never-used/
- Chrome DevTools `Page.startScreencast` sends frames as JPEG or PNG with a quality of 0–100 and an "every n-th frame" setting. Source: https://chromium.googlesource.com/devtools/devtools-frontend/+/HEAD/node_modules/devtools-protocol/pdl/domains/Page.pdl
- Not confirmed: that screencast frames arrive only when the page changes.
- **Meaning:** Windows' own bug-report recorder stored one screenshot per user action with a text description, not video.

### 22. OBS capture methods
- Desktop Duplication dates from Windows 8; Windows.Graphics.Capture from Windows 10 version 1803. Source: https://obsproject.com/forum/threads/windows-graphics-capture-vs-dxgi-desktop-duplication.149320
- Not found: OBS's recommended quality settings for screen content, or file sizes per hour. TO SEARCH.

### 23. Desktop Duplication
- Works per display, not per window; one window means capturing the display and cropping. Source: https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api
- Gives changed rectangles (`GetFrameDirtyRects`) and moved rectangles (`GetFrameMoveRects`); updates that arrive while the program is busy are merged into one. Source: https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-getframedirtyrects
- `AcquireNextFrame` times out when nothing changed, so no frame is produced for a still desktop. Same source.
- Becomes invalid on desktop switches (for example the secure desktop). Source: https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nn-dxgi1_2-idxgioutputduplication

### 24. Software encoders with screen modes
- OpenH264 has a screen content mode (`SCREEN_CONTENT_REAL_TIME`); a Mozilla bug reports it takes several times longer than normal mode and runs on one thread. Sources: https://pkg.go.dev/github.com/pion/mediadevices/pkg/codec/openh264 , https://bugzilla.mozilla.org/show_bug.cgi?id=1942245
- SVT-AV1 added screen tools for real-time use: intra block copy on key frames, palette on intra blocks. Source: https://videohelp.com/software/SVT-AV1/version-history UNVERIFIED details.
- x264: "veryfast" costs about twice the work of "ultrafast" at 1080p30 on one community benchmark: https://github.com/cmoore1776/x264-benchmark
- Not found: CPU percentages for any of them on screen content.

## Batch 7 (searches 25–28): how big are real videos of screens

### 25. YouTube delivery sizes at 1080p
- One measured video (content not stated): H.264 stream about 1,041 kbit/s, VP9 about 801 kbit/s, AV1 about 469 kbit/s. Source: https://forum.videohelp.com/showthread.php?p=2783787
- Typical ranges from a community table: 1080p H.264 and VP9 about 2.5–3 Mbit/s for ordinary video. Source: https://gist.github.com/AgentOak/34d47c65b1d28829bb17c24c04a0096f
- At 1080p, AV1 saves about 47 % against H.264 and 21 % against VP9 (Streaming Learning Center measurements): https://streaminglearningcenter.com/codecs/which-codec-does-youtube-use-part-iii.html
- Slide and screencast videos are far smaller because nothing moves: one user's slide video needed 0.2 Mbit/s where the default target was 4.2 Mbit/s (21 times less): https://obsproject.com/forum/threads/screencasts-reducing-file-size.151146/
- **Check of the owner's reference (50 MB for 30 minutes):** that is about 220 kbit/s, or 1.7 MB per minute. It fits a mostly still screencast in VP9 or AV1 after YouTube's slow offline encoding. It does not fit film content, which runs 5–15 times higher.

### 26. AV1 for screen sharing in browsers
- libWebRTC switches AV1 into "screen content coding" for tracks that come from screen sharing or carry the hint "detail"; reported effect: frames 25 % or more smaller. Source: https://webrtchacks.com/the-hidden-av1-gift-in-google-meet/ (third-party analysis)
- libaom 3.6: AV1 at speed 10 compresses 45 % better than VP9 at speed 7 and runs 35 % faster, with improvements aimed at screen sharing; available to any WebRTC app since Chrome 113. Source: https://developer.chrome.com/blog/av1
- Meet tested AV1 down to 40 kbit/s. Same source (Google's claim).
- **Meaning:** a software AV1 encoder fast enough for real-time screen content exists and is free to use (libaom). CPU and RAM cost on a PC still to be found.

### 27. Screen sharing in conferencing products
- Zoom: screen sharing alone needs 50–75 kbit/s; with a video thumbnail 50–150 kbit/s. Full-frame-rate content sharing "may increase bandwidth substantially". Source: https://library.zoom.com/admin-corner/network-management/quality-of-service-and-network-best-practices-explainer/calculating-bandwidth-usage-for-zoom-meetings-and-phone
- Teams: screen sharing frame rate adapts between 7.5 and 30 frames per second. Source: https://learn.microsoft.com/en-us/training/modules/plan-optimize-network-performance-for-teams-phone-system/2-understand-teams-network-requirements
- **Meaning:** 50–75 kbit/s is about 0.4–0.6 MB per minute for a shared screen that is mostly still. Products built for screens reach sizes hundreds of times below ordinary video settings by sending almost nothing while the screen is still.

### 28. Game and desktop streaming
- Parsec's advice for people who read text (programmers, artists): H.265 with 4:4:4 colour; for artists a 25 Mbit/s or higher limit. Sources: https://support.parsec.app/hc/en-us/articles/32381785123860-Improve-Stream-Quality-and-Color-Accuracy , https://support.parsec.app/hc/en-us/articles/32361396676116-Artists-and-Animators
- Sunshine/Moonlight: native 4:4:4 encoding on Windows was in testing; not confirmed as shipped. Source: https://github.com/orgs/LizardByte/discussions/220
- **Meaning:** products that must keep text sharp move to full colour resolution (4:4:4); products for games accept high bitrates, which is the opposite of the small-file goal.

## Batch 8 (searches 29–32): encoders in the PC, Windows tools, containers

### 29. Intel hardware encoder: screen content and colour
- Intel's hardware HEVC encoder supports the HEVC Screen Content Coding extension on Ice Lake (10th generation) and later; ffmpeg's Quick Sync encoder gained support for it in 2022 (needs libmfx 1.32 or newer). Sources: https://lists.mplayerhq.hu/pipermail/ffmpeg-devel/2022-January/291206.html , https://code.ffmpeg.org/Traneptora/FFmpeg/commit/c4ae6908f28ceacb39b96de9f8761aae1968bf42
- A user reported the SCC profile "unsupported" on one platform; Intel staff ran it successfully. Source: https://community.intel.com/t5/Media-Intel-Video-Processing/Some-problems-about-HEVC-HW-encoder/m-p/1195872
- Intel 10th generation and newer: hardware HEVC with 4:2:2 colour. Source: https://www.intel.com/content/www/us/en/support/articles/000057555/graphics.html . 4:4:4 not covered by this result.
- Not found: whether Windows' own media interface exposes Intel's screen content mode, or only Intel's library (oneVPL) does.
- **Meaning:** the graphics chip in recent Intel PCs can encode HEVC with the screen tools (palette, block copy) in hardware. That would combine the screen-content gains of finding 3 with low CPU. How to reach it from a program on Windows is the open point.

### 30. Windows Snipping Tool and Game Bar
- Snipping Tool records to MP4 only, no format setting; saved under Videos\Screen Recordings. Sources: https://pureinfotech.com/screen-record-snipping-tool-windows-11/ , https://www.elevenforum.com/t/record-screen-with-snipping-tool-app-in-windows-11.11190/latest
- Not found: its frame rate, bitrate or size per minute; Game Bar's quality settings. No published figures; would need measuring.

### 31. File containers and crashes
- Ordinary MP4 writes its index at the end; a crash leaves the file unreadable. Source: https://obsproject.com/forum/threads/video-formats-that-survives-after-crash.120856/
- Fragmented MP4 writes each group of frames as its own fragment; a crash loses at most the fragment being written. Source: https://obsproject.com/forum/resources/how-to-fix-mp4-mov-files-corrupting-when-obs-studio-crashes.1293/updates
- Matroska (MKV) also survives interruption. OBS 30.2 added "Hybrid MP4": fragmented while recording, rewritten as a normal MP4 at the end. Source: https://obsproject.com/blog/obs-studio-hybrid-mp4
- Not found: seek accuracy and index behaviour per container.

### 32. Direct3D 12 Video Encode
- Windows 11 has a Direct3D 12 video encoding interface: H.264 and HEVC from the start, AV1 since version 24H2, plus HEVC 4:2:2 and 4:4:4 extensions. Sources: https://learn.microsoft.com/en-us/windows-hardware/drivers/display/video-encoding-d3d12 , https://learn.microsoft.com/windows-hardware/drivers/display/video-encoding-d3d12-av1 , https://microsoft.github.io/DirectX-Specs/d3d/D3D12_Video_Encoding_HEVC_422_444.html
- Support depends on the graphics vendor's driver. ffmpeg has added encoders on top of it. Source: https://ffmpeg.org/pipermail/ffmpeg-devel/2024-April/325692.html
- Not found: whether Windows ships a built-in HEVC encoder transform for Media Foundation.
- **Meaning:** a second built-in Windows route to hardware encoders exists besides Media Foundation, and it is the one that exposes HEVC 4:4:4 and AV1.

## Batch 9 (searches 33–36): the reader, structure recording, newest codecs, "build to lossless"

### 33. How models take in video
- Gemini: each video frame costs about 70 tokens at the default setting (64 on older models) and 280 tokens at "high"; Google says "high" is "required only when the use case involves reading dense text (OCR) or small details within video frames". Source: https://ai.google.dev/gemini-api/docs/media-resolution
- Not confirmed in this search: the sampling rate (commonly stated as 1 frame per second).
- **Meaning:** a model watching video at default settings sees each frame at low detail. Reading text needs high-detail frames, which cost four times more. So a recorder for AI should let the reader pick few frames at high detail, not many at low detail.

### 34. UI tree against screenshots for AI agents
- A UI-tree snapshot costs about 273 tokens against 1,000–1,800 for a screenshot (one source's figures): https://byteiota.com/?p=26671 . Vercel Labs self-reports 31,000 against 198,000 tokens on a 52-step task: https://agentconn.com/blog/vercel-agent-desktop-computer-use-land-grab/ . UNVERIFIED vendor figures.
- The tree is only as good as the application's accessibility support; old applications need screenshots. Source: https://techjacksolutions.com/?p=103965
- Some faults show only in pixels, for example text overflowing its box. Source: https://www.greptile.com/blog/trex-computer-use
- Some systems return both and prefer the tree. Same sources.
- Not found: recording UI Automation events over time on Windows.
- **Meaning:** text and structure from the UI tree are the cheapest thing for a model to read, but they cannot replace pixels. The two complement each other.

### 35. Newest codecs
- AV2: final specification 1.0.0 released about 28 May 2026; claimed about 30 % smaller than AV1; no hardware yet (first chips expected 2027); ffmpeg had no AV2 support as of October 2026. Sources: https://aomedia.org/press%20releases/Alliance-for-Open-Media-Releases-AV2-Codec/ , https://en.wikipedia.org/wiki/AV2 , https://phoronix.com/news/av2-1.0-specification-released
- Not covered by this search: real-time neural codecs; real-time VVC encoders. TO SEARCH.
- **Meaning:** AV2 is not usable for a product today.

### 36. "Build to lossless" in remote desktop products
- **Citrix HDX Thinwire:** "Build to lossless compresses the screen using H.264 (or H.265) during screen activity and sharpens to pixel perfect (lossless) when activity stops." Source: https://docs.citrix.com/en-us/citrix-virtual-apps-desktops/graphics/thinwire/intelligent-build-to-lossless.html
- Citrix can also use the video codec only "for actively changing regions"; still regions go to a lossless run-length codec (2DRLE). Source: https://docs.citrix.com/en-us/citrix-virtual-apps-desktops/2203-ltsr/graphics/thinwire
- "Intelligent Build to Lossless" is Citrix's default graphics mode from version 2503 when a GPU is present; it decides by screen activity, per monitor. Same first source.
- **PCoIP (VMware Horizon):** text is always sent lossless on a LAN; under low bandwidth it shows a lossy picture first and "rapidly builds the image to a lossless state". Source: https://docs.vmware.com/en/VMware-Horizon/2206/horizon-remote-desktop-features/GUID-FB9598F4-EBBD-4CFF-B24E-CF9D18E729CD.html
- **Meaning:** the two leading commercial remote-desktop systems converged on the same method: lossy video while the screen changes, then an exact picture once it stops. Citrix made the whole-screen version its default, in place of selecting regions.

## Batch 10 (searches 37–40): software AV1 speed, frame skipping, lossless codecs, hardware encoders

### 37. Software AV1 in real time
- libaom (version 3.5) in real-time mode at speed 10 on a 1080p test clip: roughly 140–170 frames per second on the benchmark machines of OpenBenchmarking (camera content; the CPUs behind each figure were not clear in the summary). Source: https://openbenchmarking.org/test/pts/aom-av1&eval=ce004df6382a0005912fcfb18262b1b1f1419af5 UNVERIFIED per-CPU detail.
- Speed 10 exists since libaom 3.2.0 (October 2021). Source: https://fftrac-bg.ffmpeg.org/ticket/9336
- Not found: CPU load and RAM on screen content; rav1e real-time figures.
- **Meaning:** software AV1 has headroom for real time at 1080p on a desktop CPU. Its cost at the low frame rates a screen recorder needs must be measured.

### 38. Not encoding when nothing changes
- libWebRTC has a "zero-hertz" mode for screen sharing: the capture rate may fall to zero while the screen is still, and the latest frame is re-sent only when needed. Sources: https://bugzilla.mozilla.org/show_bug.cgi?id=1920687 , https://webrtc.googlesource.com/src/+/8935a7e8ebb322114a2241f659eb776997fe853e
- With variable frame rate, each frame carries its own time; a quiet screen holds one picture for a long time. Software that assumes a constant rate may seek wrongly. Source: https://transloadit.com/glossary/variable-frame-rate/
- Not found: OBS's behaviour; measured savings from dropping duplicate frames.
- **Meaning:** frames should be produced by change, not by a clock, and each must carry its own timestamp.

### 39. Lossless video codecs
- MSU's 2007 comparison: Lagarith best for capture, FFV1 the most balanced. Source: https://videoprocessing.ai/codecs/lossless-codecs-comparison-2007.html (old).
- One user's 2013 test on the same clip: uncompressed 4.28 GB; Huffyuv 2.06; UT Video 2.03; Lagarith 1.97; FFV1 1.83 GB. Source: https://forum.videohelp.com/showthread.php?p=2252130 . So these codecs reach only about 2:1 on camera video.
- Not found: results on screen content; animated WebP, APNG and GIF sizes.
- **Meaning:** general lossless video codecs are built for camera footage and give about half size. They are not candidates for small files.

### 40. Hardware encoders from NVIDIA and AMD
- NVIDIA RTX 40 series (laptops included): real-time AV1 encoding, claimed about 40 % more efficient than H.264. Source: https://blogs.nvidia.com/blog/av1-obs29-youtube (vendor claim)
- AMD: AV1 encoding on Radeon RX 7000 series. Source: https://wccftech.com/handbrake-adds-av1-support-for-nvidia-rtx-40-ada-amd-rx-7000-rdna-3-gpus/amp/
- Older NVIDIA cards (RTX 30) decode AV1 but cannot encode it. Source: https://wccftech.com/nvidia-av1-nvenc-encorder-obs-studio-support-geforce-rtx-40-graphics-cards/amp/
- Not found: 4:4:4, lossless and constant-quality modes per encoder; independent tests on screen text.
- **Meaning:** which hardware codec a PC has depends on its graphics chip and its age. A recorder that must run on any PC needs a method that does not depend on one hardware codec.

## Batch 11 (searches 41–44): neural codecs, VVC encoders, drawing-command recording, program replay

### 41. Neural (learned) video codecs
- DCVC-RT (Microsoft, CVPR 2025): 1080p at about 40 frames per second encoding and 34 decoding on an NVIDIA RTX 2080 Ti; 125 and 113 on an A100. About 21 % smaller than the VVC reference software in one test setting, 14 % in RGB. Sources: https://arxiv.org/html/2502.20762v2 , https://dcvccodec.github.io
- An independent review confirms the speed and rates it "GPU only today; NPU/CPU not yet viable", and not integrated into ffmpeg or WebRTC. Source: https://streaminglearningcenter.com/articles/evaluating-dcvc-rt-a-real-time-neural-video-codec-that-delivers-on-speed-and-compression.html
- Not found: results on screen content; any CPU speed.
- **Meaning:** neural codecs compress best on paper but need a strong graphics card for both recording and reading back. That fails "low CPU, low RAM, any PC" today.

### 42. VVC (H.266) encoders
- VVC's screen tools: intra block copy, palette, BDPCM, transform-skip residual coding, adaptive colour transform. VVenC implements all of them for its main profile. Sources: https://www.microsoft.com/en-us/research/?p=798274 , https://github.com/fraunhoferhhi/vvenc/wiki
- Speed: VVenC measured 6.6 frames per second at 1080p on its "faster" preset with 4 threads (Arm example): https://learn.arm.com/learning-paths/servers-and-cloud-computing/vvenc/vvenc/ . A 2021 paper says it cannot reach real time. uvg266 is an open VVC encoder strongest in intra coding: https://tie-ultravideo.rd.tuni.fi/uvg266.html
- Not found: a current real-time 1080p figure for either; hardware VVC encoders in PCs.
- **Meaning:** VVC has the best screen tools of any finished standard, but no encoder found runs it in real time cheaply. A possible use: recompress later in the background, not during recording.

### 43. Recording drawing commands instead of pixels
- apitrace records OpenGL, Direct3D and DirectDraw calls to a file and can replay them; RenderDoc captures single frames by sitting between the application and the graphics interface, storing only what that frame needs; GFXReconstruct does the same for Vulkan and DirectX 12. Sources: https://github.com/apitrace/apitrace (via forks found), https://renderdoc.org/docs/behind_scenes/how_works.html , https://www.lunarg.com/mastering-gfxreconstuct-part-1/
- Not found: trace sizes; anything on RDP drawing orders as a recording method.
- **Meaning:** exact and potentially small for applications that draw with one graphics interface, but it must hook into each application and replay needs the same graphics setup. It does not cover every window. Tools exist for debugging, not for general recording.

### 44. Recording the program's execution (replay)
- WinDbg Time Travel Debugging records a process so it can be replayed exactly. Trace files run from hundreds of megabytes to gigabytes, index files about twice the trace; recording has "large overhead" and needs an elevated debugger. Sources: https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/time-travel-debugging-overview , https://learn.microsoft.com/en-us/archive/blogs/windbg/time-travel-debugging-ttd-file-size , https://wiki.mozilla.org/Time_Travel_Debugging
- **Meaning:** the most complete record possible, and the worst on size, CPU and RAM. Out for this task.

---

# WHAT THE EVIDENCE SAYS SO FAR (after 40 searches; not final)

**1. The systems closest to this task do not record smooth video.**
- Microsoft Recall: stills, only when the screen changed (2).
- Windows Steps Recorder: one screenshot per user action plus a text note (21).
- Playwright traces: structure of the page plus JPEG stills per action; its video is a rough extra (11).
- screenpipe: about 1 frame per second plus a text index (1).

**2. The systems that must show a live screen converged on a mix, not on one codec.**
- RDP: video codec for moving areas, a separate codec for text (12).
- Citrix: video codec while the screen changes, then an exact picture when it stops; this is now its default (36).
- PCoIP: text lossless; lossy first and built up to lossless under pressure (36).
- TurboVNC's study: changed rectangles beat H.264 on ordinary desktop content; H.264 wins only when large areas change constantly (18).

**3. Sizes that products reach on mostly still screens are tiny.**
- Zoom screen share: 50–75 kbit/s, about 0.5 MB per minute (27).
- A screencast on YouTube at about 220 kbit/s, about 1.7 MB per minute, is consistent with the measurements found (25).
- These come from sending almost nothing while the screen is still, and from codecs with screen tools.

**4. Codecs with screen tools are much smaller than plain H.264 on text and UI.**
- HEVC with screen content coding: about 60 % smaller on screen content (3). Intel's graphics chips from 10th generation encode it in hardware (29).
- AV1 with screen tools: a free software encoder runs in real time (26, 37); hardware only on recent graphics chips (8, 40).
- AV2 is not usable yet (35).

**5. Lossless video of changing content is out; lossless stills of settled content are affordable.**
- Lossless x264 at 1900 × 1200: 62–70 MB per second (15). General lossless video codecs: about 2:1 (39).
- Still-image formats (WebP lossless, JPEG XL) beat PNG by 20–40 % and more (13).

**6. For text, colour resolution and sharpness matter more than frame rate.**
- Browsers keep resolution and drop frame rate for screen content (4).
- Half colour resolution (4:2:0) blurs coloured text; products for reading text use 4:4:4 (14, 28).

**7. The reader is a model with limits.**
- A model shrinks a full frame before reading it (9), and reads video frames at low detail unless told otherwise (33).
- So the recording must keep full resolution and be able to hand out a crop or a single frame at full detail.
- Text and structure from the UI tree cost a model far less than pictures, but cannot replace them (34).

**8. Windows can report what changed.**
- Both capture interfaces give changed regions; Desktop Duplication also gives moved regions and produces nothing while the screen is still (7, 23).

## Candidate methods the evidence points to (to be compared by measurement, none chosen yet)
- **A. Stills on change:** exact pictures of the changed regions, taken when the system reports a change, with timestamps; no video codec.
- **B. Video while changing, exact still when settled** (the Citrix default): one video stream of the whole window with a screen-content codec, plus a lossless still each time the picture stops changing.
- **C. Video only with a screen-content codec** (HEVC with screen tools or AV1), full colour resolution, frames only on change.
- **D. Any of the above plus a text and event track** from the UI tree and input, for the model to read first.

## Combined solutions already found in real products (owner request: hybrid, triple, quad)
- **Two parts.** Citrix: video + exact still when settled (36). RDP and Citrix selective mode: changed rectangles + video for moving areas (12, 36). Recall: stills on change + text (2). Steps Recorder: still per action + note (21).
- **Three parts.** RDP: changed rectangles + video + a text codec (12). screenpipe: video + UI-tree text + OCR (1).
- **Four parts.** Playwright trace: page structure + stills + video + console and network logs (11). screenpipe with audio: video + UI-tree text + OCR + audio (1).
- Not yet found in any product: video + settled stills + UI-tree text + event timeline together. The full table of combinations to study is in `subject-list.md`, group 5b.

## Still open (see `subject-list.md`)
Drawing-command and replay recording; neural codecs; real-time VVC; Windows' other capture routes; start time of encoders; CPU and RAM figures for every candidate; containers and exact seeking; conferencing products' own screen codecs; animated image formats; a measured test on a real PC with a real model as the reader.

---

## Coverage map: replaced by the full subject list in `subject-list.md` (same folder), 21 groups. The table below is the first, shorter version. Each family gets searched and logged; status is updated here.

| # | Family | Status |
|---|---|---|
| A1 | General video codecs: H.264, HEVC, VVC, AV1, AV2, VP8, VP9, MPEG-2/4, Theora, EVC, LCEVC | partly (3, 5, 6) |
| A2 | Screen-specific codecs: TSCC/TSC2 (Camtasia), ZMBV, Microsoft Screen codecs (MSS1/MSS2), Flash Screen Video, CamStudio, QuickTime Animation, GoToMeeting G2M, VMware | open |
| A3 | Lossless and intermediate video codecs: FFV1, UT Video, MagicYUV, Lagarith, HuffYUV, ProRes, DNxHR, MJPEG | partly (15) |
| A4 | Remote display protocols: RDP, VNC encodings, SPICE, PCoIP, Citrix HDX, NICE DCV, Parsec, Sunshine/Moonlight | partly (12) |
| A5 | Learned (neural) video and image codecs; text-aware and layered compression (mixed raster content, DjVu) | open |
| B1 | Still-image formats: PNG, WebP, JPEG, JPEG XL, AVIF, HEIC, QOI, JPEG 2000, JPEG XS | partly (13) |
| B2 | Animated image formats: GIF, APNG, animated WebP, animated AVIF | open |
| C1 | Software encoders: x264, x265, OpenH264, libvpx, libaom, SVT-AV1, rav1e, VVenC | open |
| C2 | Hardware encoders and how programs reach them: Quick Sync, NVENC, AMF, Media Foundation, D3D12 Video Encode | partly (8, 14, 16) |
| D | Decoders and reading one exact frame back | open |
| E | Capture methods on Windows: Windows.Graphics.Capture, Desktop Duplication, GDI/PrintWindow, DWM, Magnification, hooks, mirror and indirect display drivers | partly (7) |
| F | Containers and streaming formats: MP4, fragmented MP4, Matroska/WebM, MPEG-TS, FLV, HLS, DASH, RTMP, WebRTC, SRT, NDI | open |
| G | Broadcasting and recording products: OBS, ShareX, Game Bar, Snipping Tool, Loom, Zoom, Teams, Camtasia, vMix; platform bitrates | open |
| H | Recording structure instead of pixels: DOM replay (rrweb), accessibility tree and UI Automation events, input logs, Windows Steps Recorder, Chrome DevTools screencast, Playwright traces | partly (11) |
| I | Recorders built for AI: screenpipe, Recall, Rewind and clones; agent trajectory formats | partly (1, 2) |
| J | What the AI reader needs: input limits, legibility, frame selection | partly (9, 10) |

---

## Open questions so far
- Measured sizes of screen recordings per approach (still missing).
- What an AI model can actually read: resolution limits, effect of compression on text.
- Lossless stills on change: formats and sizes.
- Remote desktop encodings (changed regions only).
- Which encoders every Windows PC has, and their cost in CPU and RAM.
