# Proof ledger: what is proven from text, and what is not

**Owner's rule (2026-11-23):** all research must be backed by proof from text. Nothing is invented; nothing is stated from memory.

**How this file works:**
- **PROVEN** = Claude opened the page itself and copied the words. The quote is verbatim.
- **NOT PROVEN** = the claim is in `research-log.md`, but the text was not found on the page read, or the page could not be read.
- **SUMMARY ONLY** = the claim came from a search-result summary; Claude has not read the page yet. It is not a finding until it moves to PROVEN.
- Claude's own conclusions are not in this file. They are kept apart, at the end, and marked as such.

Pages were read on 2026-11-23 with a small script that downloads the page and prints the sentences around a search word.

---

## PROVEN (verbatim quotes)

### P1. Citrix: video while the screen changes, exact picture when it stops; default mode
Source: https://docs.citrix.com/en-us/citrix-virtual-apps-desktops/graphics/thinwire/intelligent-build-to-lossless.html (page says "Last published: Nov 19, 2026")
- "Designed for mixed workloads in mind, Intelligent Build to Lossless is an evolution of Thinwire Selective Encoding and is the new default graphics mode."
- "Similar to the existing Build to Lossless setting, Intelligent Build to Lossless uses a video codec on large, actively changing screen regions to maintain smooth session graphics."
- "When the screen activity ceases, it progressively refines the image until it is pixel perfect."

### P2. Citrix: still text and simple graphics go to a lossless codec
Source: https://docs.citrix.com/en-us/citrix-virtual-apps-desktops/2203-ltsr/graphics/thinwire
- "…most of the imagery is simple graphics or text regions. Thinwire determines where these regions are and selects these areas for lossless encoding using the 2DRLE codec."

### P3. TurboVNC's study of H.264 against its own changed-region encoder
Source: https://turbovnc.org/About/H264
- "With H.264, on the other hand, it is necessary to encode the entire remote framebuffer image each time it changes (even if only one pixel changed) and to let the H.264 encoder analyze the image for differences relative to the previous image."
- "There are still only a handful of datasets for which H.264 can be shown to compress better than the TurboVNC encoder, but for those datasets, the improvement is now very compelling."
- "However, as slow as CL 9 is, it is still (in the aggregate) about 80-90 times faster than H.264 when compressing 2D app workloads and about 5-6 times faster than H.264 when compressing 3D app workloads."
- "Setting i_keyint_max to a high value improved the 2D compression ratios by 2-3x."
- "…H.264 can provide very compelling compression ratio improvements on certain types of workloads-- particularly games and other application workloads that produce relatively predictable motion between frames."
- Method, from the page: "A strawman H.264 encoder for the TurboVNC Benchmark Tools was developed using the open source x264 library and compared against the existing TurboVNC encoder at the low level."

### P4. Microsoft Teams: AV1 against H.264 for screen sharing
Source: https://techcommunity.microsoft.com/blog/microsoftteamsblog/enhancing-screen-sharing-with-av1-in-microsoft-teams/4096056
- "Using AV1, users can experience lowered bandwidth for Teams screensharing by 63% compared to the H.264 video codec, as measured by A/B tests."
- "For instance, sharing static content usually requires less than 200Kbps bandwidth, while sharing content that moves, such as when scrolling, needs about 1.2Mbps."

### P5. HEVC and its screen content extensions against H.264
Source: https://www.microsoft.com/en-us/research/publication/compression-performance-of-hevc-and-its-format-range-and-screen-content-coding-extensions/
- "The experimental results show that, when compared with H.264/MPEG-4 AVC, HEVC version 1 provides a bit rate savings for equal PSNR of about 23% for all-intra coding, 34% for random access coding, and 38% for low-delay coding."
- "For lossy coding of screen content, the HEVC screen content coding extensions achieve a bit rate savings of about 66%, 63%, and 61% for all-intra coding, random access coding, and low-delay coding, respectively."
- Not stated in the second sentence: what the 66 %, 63 % and 61 % are measured against. The baseline stays unproven.

### P6. Zoom: bandwidth of screen sharing
Source: https://library.zoom.com/admin-corner/network-management/quality-of-service-and-network-best-practices-explainer/calculating-bandwidth-usage-for-zoom-meetings-and-phone
- "For screen sharing only (no video thumbnail): 50-75kbps For screen sharing with video thumbnail: 50-150kbps"
- Same page, for comparison: "1080p HD 3.8 Mbps".

### P7. How Claude takes in an image
Source: https://platform.claude.com/docs/en/build-with-claude/vision.md
- Table rows: "High-resolution | Claude 4.7 and later models | 2,576 px | 4,784" and "Standard | All other models | 1,568 px | 1,568".
- "Each patch is a 28×28-pixel block of the image, referred to as a visual token."
- "When an image is downsized, Claude scales it to the largest size that fits the tier's limits while preserving its aspect ratio."
- Table row: "3840x2160 px (8.29 megapixels) | 1456x819 px | 1,560 | 2576x1449 px | 4,784".

### P8. How Gemini takes in video frames
Source: https://ai.google.dev/gemini-api/docs/media-resolution.md.txt
- Table: video costs 70 per frame at `unspecified`, `low` and `medium`, and 280 at `high`.
- "For video, `low` and `medium` settings are treated identically (70 tokens) to optimize context usage. This is sufficient for most action recognition and description tasks."
- "**Video** (Text-heavy) | `high` | 280 (per frame) | Required only when the use case involves reading dense text (OCR) or small details within video frames."

### P9. Windows reports changed and moved areas (Desktop Duplication)
Source: https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api
- "IDXGIOutputDuplication::GetFrameDirtyRects returns dirty regions, which are non-overlapping rectangles that indicate the areas of the desktop image that the operating system updated since you processed the previous desktop image."
- "IDXGIOutputDuplication::GetFrameMoveRects returns move regions, which are rectangles of pixels in the desktop image that the operating system moved to another location within the same image."

### P10. Windows Steps Recorder
Source: https://support.microsoft.com/en-us/windows/steps-recorder-deprecation-a64888d7-8482-4965-8ce3-25fb004e975f
- "It automatically records screenshots of the desktop along with descriptive, annotated steps as you perform a task, making it easier to reproduce and troubleshoot issues."
- "The recorded screenshots and step‑by‑step text are saved to a file for later review or sharing with support teams."
- The page is titled "Steps Recorder deprecation".

### P11. Remote Desktop (RDP): different codecs for text, images and video
Source: https://learn.microsoft.com/en-us/azure/virtual-desktop/graphics-encoding
- "Mixed-mode : separates text, image and video encoding using different codecs to provide the best quality and lowest encoding cost for each type of detected content. Mixed-mode is only available with software encoding."
- "By default, graphics data is separated depending on its content. Text, images, and videos are encoded using a mix of codecs to achieve optimal encoding performance across different content types when using software encoding only."

### P12. Microsoft Recall: snapshots only while the screen differs
Source: https://learn.microsoft.com/en-us/windows/client-management/manage-recall
- "Snapshots are taken periodically while content on the screen is different from the previous snapshot."

### P13. screenpipe: event-driven screenshots with accessibility data
Source: https://docs.screenpi.pe/architecture
- Page heading: "Screenpipe architecture: event-driven capture and storage".
- "…it listens for meaningful OS events and captures only when something actually changes — pairing each screenshot with accessibility tree data for maximum quality at minimal cost."
- "How Screenpipe uses event-driven capture, accessibility tree extraction, OCR fallback, and SQLite storage to build a searchable local memory of your screen."

### P14. W3C content hints for screen content
Source: https://www.w3.org/TR/mst-content-hint/
- "Screencast content of presentations or webpages with a lot of text content is completely unintelligible if the quantization levels are too high or if the content is downscaled or otherwise blurry."
- "\"detail\" The track should be treated as if video details are extra important."

### P15. Playwright trace
Source: https://playwright.dev/docs/trace-viewer
- "Hover over each action of your test and visually see the change in the DOM snapshot."
- "When tracing with the screenshots option turned on (default), each trace records a screencast and renders it as a film strip."

### P16. Lossless WebP against PNG
Source: https://developers.google.com/speed/webp/docs/webp_lossless_alpha_study
- "WebP lossless compression is shown to be 23% better than ZopfliPNG and 42% better than libpng."

### P17. QOI and lossless WebP on non-photographic images
Source: https://cloudinary.com/blog/jpeg-xl-and-the-pareto-front
- "Not shown on the chart is QOI, which clocked in at 154 Mpx/s to achieve 17 bpp, which may be \"quite OK\" but is quite far from Pareto-optimal…"
- "For these images, lossless AVIF is not useful — it compresses worse than PNG, and reaches about the same density as QOI but is much slower. Lossless WebP on the other hand achieves very good compression for such images."

### P18. Lossless x264 screen recording is feasible on an old CPU
Source: https://obsproject.com/forum/resources/almost-lossless-local-recoding-using-obs-studio.200/
- "My CPU is Intel i7-3770 @ 3.40GHz. It handles x264 lossless @ 48fps effortlessly."
- The size figure in the log (62–70 MB per second) was not in the text read. Not proven.

### P19. The H.264 encoder that ships with Windows: quality setting
Source: https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder
- "CODECAPI_AVEncCommonQuality Sets the quality level. This property applies when the rate control mode is quality-based VBR ( eAVEncCommonRateControlMode_Quality )."
- "The default mode is unconstrained variable bit rate (VBR)."

### P20. Codecs made for screens
- TSCC, source https://wiki.multimedia.cx/index.php/TSCC : "TSCC combines run length encoding (RLE) with standard zlib compression. The algorithm can operate in 24-bit RGB, 16-bit RGB, and palettized 8-bit RGB modes."
- ZMBV, source https://wiki.multimedia.cx/index.php/ZMBV : "This codec employs ZLIB compression and has intraframes and delta frames. Delta frames seem to have blocks either copied from the previous frame or XOR'ed with som…" (sentence cut by the reader).
- Windows Media Video 9 Screen, source https://learn.microsoft.com/en-us/windows/win32/medfound/windowsmediavideo9screenencoder : "The Windows Media Video 9 Screen encoder is optimized for encoding sequentia…" (sentence cut by the reader).

### P21. rrweb
Source: https://raw.githubusercontent.com/rrweb-io/rrweb/HEAD/docs/observer.md
- "After completing a full snapshot, we need to record events that change the state."
- "Fortunately, modern browsers have provided us with a very powerful API which can do exactly this: [MutationObserver]…"

### P22. OpenH264 screen content mode is slow
Source: https://bugzilla.mozilla.org/show_bug.cgi?id=1942245
- "Frames from screen capture are often large and with OpenH264 in screen content mode, encoding them takes several times longer than encoding non-screen content."

### P23. YouTube at 1080p
Source: https://streaminglearningcenter.com/codecs/which-codec-does-youtube-use-part-iii.html
- "However, the savings over AVC really kicked in at 1080p which, at ~3.6 Mbps, are probably viewed by a significant chunk of viewers."
- The 47 % figure in the log was not in the text read. Not proven.

### P24. AV1 in Chrome and in Google Meet
Source: https://developer.chrome.com/blog/av1
- "The 3.6.0 release of libaom also includes improvements targeting screen sharing. Compared to VP9 speed 7, AV1 Speed 10 provides 45% better compression while running 35% faster."
- "Google Meet has tested AV1 successfully in extremely low bandwidth conditions—down to 40 kbps…"

Source: https://webrtchacks.com/the-hidden-av1-gift-in-google-meet/
- Section heading on the page: "AV1 screen content coding is better". The 25 % figure in the log was not in the text read. Not proven.

### P25. Parsec's settings for colour accuracy
Source: https://support.parsec.app/hc/en-us/articles/32381785123860-Improve-Stream-Quality-and-Color-Accuracy
- "Settings H.265 encoding/decoding 4:4:4 color Constant FPS".
- "If you require greater color accuracy than enabling 4:4:4 color can provide, you can try enabling 10-bit color."

### P26. Direct3D 12 video encoding in Windows 11
Source: https://learn.microsoft.com/en-us/windows-hardware/drivers/display/video-encoding-d3d12
- "…the supported codecs are H.264 and HEVC, although the D3D12 video encoding framework provides open extensibility for newer codecs such as AV1."
- "Support for AV1 Encode is added in Windows 11, version 24H2 (WDDM 3.2)."

### P27. AV2
Source: https://aomedia.org/press%20releases/Alliance-for-Open-Media-Releases-AV2-Codec/
- "Wakefield, Mass. – June 9, 2026 - The Alliance for Open Media (AOMedia) today announced the release of the AV2 specification."
- "Developed under AOMedia's royalty-free patent policy, AV2 is the next-generation video compression standard…"

### P28. NVIDIA hardware AV1
Source: https://blogs.nvidia.com/blog/av1-obs29-youtube (vendor's own claim)
- "All GeForce RTX 40 Series GPUs — including laptop GPUs and the recently launched GeForce RTX 4070 — support real-time AV1 hardware encoding, providing 40% more efficient encoding on average…"

### P29. Neural codec DCVC-RT
Source: https://arxiv.org/html/2502.20762v2
- "…it enables 1080p coding on consumer GPUs like the NVIDIA RTX 2080Ti with an average speed of 40 fps for encoding and 34 fps for decoding. On an NVIDIA A100 GPU, it reaches an impressive 125 fps for encoding…"

### P30. VVC screen tools against HEVC
Source: https://www.microsoft.com/en-us/research/publication/overview-of-the-screen-content-support-in-vvc-applications-coding-tools-and-performance/
- "…enables the VVC reference software implementation (VTM) to achieve average bit-rate savings of about 41% to 61% relative to the HEVC test model (HM) reference software implementation using the Main 10 profile for 4:2:0 screen conte…" (cut by the reader)
- "Compared to the HM using the Screen-Extended Main 10 profile and the same 4:2:0 test sequences, the VTM provides about 19% to 25% bit-rate savings. The same comparison with 4:4:4 test sequences revealed bit-rate savings of about 13% to 27% for Y'CBCR and of about 6% to 14% for R'G'B'…"

### P31. Webex content sharing
Source: https://help.webex.com/en-us/gw8u4j/Known-Issues-and-Limitations-for-Webex-Video
- "Content sharing is only supported using H.264 video codec."
- "Although the Meeting Client can receive 1440p5, 1080p12, or 720p30, the cascade link between the device and the client limits content to 10 fps."

### P32. Time Travel Debugging
Source: https://learn.microsoft.com/en-us/windows-hardware/drivers/debugger/time-travel-debugging-overview
- "TTD is efficient and works to add as little overhead as possible as it captures code execution in trace files."
- The trace sizes in the log (hundreds of megabytes to gigabytes) were not in the text read. Not proven.

### P33. Windows.Graphics.Capture has change-related properties
Source: https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession
- The property table lists "DirtyRegionMode" and "MinUpdateInterval". The page gives no description for either. What they do is not proven.

### P34. Citrix's older "Build to lossless" setting works on the whole screen
Source: https://docs.citrix.com/en-us/citrix-virtual-apps-desktops/2203-ltsr/graphics/thinwire
- "Build to lossless compresses the screen using H.264 (or H.265) during screen activity and sharpens to pixel perfect (lossless) when activity stops."
- Compare P1: the newer default, "Intelligent Build to Lossless", uses the video codec "on large, actively changing screen regions".

### P35. Desktop Duplication returns nothing new while the desktop is still
Source: https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-acquirenextframe
- "The time-out interval, in milliseconds. This interval specifies the amount of time that this method waits for a new frame before it returns to the caller."
- "DXGI_ERROR_WAIT_TIMEOUT if the time-out interval elapsed before the next desktop frame was available."

### P36. Compression damages text for OCR
Source: https://arxiv.org/html/2503.19495v2
- "Compression techniques, especially at low bitrates, can introduce significant distortions, such as blurring, blocking artifacts, and loss of fine details, which can severely impact text readability."

### P37. RenderDoc sits between the application and the graphics interface
Source: https://renderdoc.org/docs/behind_scenes/how_works.html
- "After this point all accesses to the API remain wrapped and the driver essentially sets itself up as a \"man-in-the-middle\" between the application and the real API."

### P38. Order of calls for NVIDIA's hardware encoder
Source: https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-video-encoder-api-prog-guide/index.html
- "After loading the NVENC Interface, the client should first call NvEncOpenEncodeSessionEx to open an encoding session."
- No start-up time is given on the page.

### P39. PrintWindow
Source: https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-printwindow
- "The PrintWindow function copies a visual window into the specified device context (DC), typically a printer DC."
- The only flag the page documents is "PW_CLIENTONLY". The flag `PW_RENDERFULLCONTENT` is not on the page. Not proven.

### P40. RDP: images and frequently changing content
Source: https://learn.microsoft.com/en-us/azure/virtual-desktop/graphics-encoding
- "The rest of the content is separated to images and video: Images are software encoded with either AVC/H.264 or RemoteFX graphics, depending on the capabilities of the local device."

### P41. UI Automation events (batch 13)
Source: https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-eventsforclients
- "Microsoft UI Automation allows clients to subscribe to events of interest. This capability improves performance by eliminating the need to continually poll the UI elements in the system to see if any information, structure, or state has changed."
- No cost figures are given on the page.

### P42. Models reading recordings of user interfaces (batch 13)
Source: https://arxiv.org/html/2406.10819v2 (GUI-World)
- "Empirical results suggest that enhancing vision perception, such as more keyframes and higher resolution, can lead to substantial performance improvements in GUI tasks."
- The benchmark covers "nine mainstream MLLMs", "five keyframe selection strategies" and "six GUI scenarios".

Source: https://arxiv.org/html/2512.19107v1 (FC-MIR)
- "We propose an adaptive, training-free keyframe sampling and stitching algorithm for video sequences, which significantly reduces frame redundancy while preserving critical trajectory information…"

### P43. VP9 has a content setting in ffmpeg's libvpx wrapper (batch 13)
Source: https://ffmpeg.org/pipermail/ffmpeg-devel/2017-November/219896.html
- Subject line of the patch: "lavc/libvpxenc: add tune-content option".
- The values of the option (default, screen, film) were not in the text read. Not proven. Nothing was found on x265 or Kvazaar and screen content.

## Proven in the retry pass (other addresses for the same sources)

### P44. Lossless screen recording sizes (finding 15)
Source: https://obsproject.com/forum/resources/almost-lossless-local-recoding-using-obs-studio.200/
- "Data volumes (input is 1900x1200, fps varies): x264 lossless ultrafast (48 fps): 62-70 MB/s x264 lossless veryfast (48 fps): ~52 MB/s Lagarith lossless (20 fps): ~66 MB/s MJPG lossy, 85% quality (30 fps): ~14 MB/s"
- "It is almost lossless because the conversion from RGB to YUV 4:4:4 is not truly lossless…"

### P45. Intel hardware HEVC with screen content coding in ffmpeg (finding 29)
Source: https://github.com/FFmpeg/FFmpeg/commit/c4ae6908f28ceacb39b96de9f8761aae1968bf42.patch
- "Subject: [PATCH] lavc/qsvenc: add encode support for screen content coding extension Enables HEVC Screen Content Coding extension support on ICL+ platform"
- "Set the encoding profile (scc requires libmfx >= 1.32)."

### P46. Zoom: JPEG pictures, then H.264 (finding 48)
Source: https://devforum.zoom.us/t/136028.json (a developer's post, citing Zoom's documentation)
- "For screensharing, we are successfully receiving data, but it is always a sequence of individual JPEG images."
- "According to the Zoom RTMS documentation, the stream should dynamically switch from JPEG … [when the frame ra]te exceeds a certain threshold (around 5 FPS)."

### P47. AV1 screen content coding in WebRTC (finding 26)
Source: https://webrtchacks.com/the-hidden-av1-gift-in-google-meet/
- "…running two AV1 encoders in parallel with the same content – a shared tab with slides that are being paged through slowly. The result is a 25%+ reduction in the frame sizes and hence bitrate."

### P48. JPEG XL against QOI (finding 13)
Source: https://cloudinary.com/blog/jpeg-xl-and-the-pareto-front
- "…QOI is indeed quite OK for its speed (and simplicity), though far from Pareto-optimal: low-effort JPEG XL encoding is twice as fast and 31% smaller."

### P49. RDP AVC444 (finding 12)
Source: https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpegfx/844018a5-d717-4bc9-bddb-8b4d6be5dd3f
- "…encapsulates regions of a graphics frame compressed using MPEG-4 AVC/H.264 compression techniques [ITU-H.264-201201] in YUV444 mode."

### P50. UI tree against screenshots, in tokens (finding 34)
Source: https://byteiota.com/?p=26671 (a blog; its figures are not independently checked)
- "Accessibility tree snapshots run about 273 tokens each. Screenshot-based computer use runs 1,000 to 1,800."

### P51. PCoIP builds unchanged regions to lossless (finding 36)
Source: https://docs.omnissa.com/bundle/Horizon-Remote-Desktop-FeaturesVmulti/page/PCoIPGeneralSettings.html
- "Unchanged regions of the image progressively build to a lossless (perfect) quality regardless of this value."

### P52. Tight encoding against Hextile and raw (finding 18)
Source: https://www.tightvnc.com/archive/compare.html
- One test session: "Data size in screen updates, bytes 32,729,254 [Raw] 1,590,721 [Hextile] 652,572 [Zlib] 518,112 [Tight-1.0] 499,140 [Tight-1.1L] 499,140 [Tight-1.1] Bandwidth savings, Tight-1.1 vs. others 98.47% 68.62% 23.51% 3.66%".
- Another: "111,178,662 [Raw] 2,788,259 [Hextile] … 454,748 [Tight-1.1] … 99.59% 83.69% 75.24% 4.61%".

### P53. Time Travel Debugging trace size (finding 44)
Source: https://learn.microsoft.com/en-us/archive/blogs/windbg/time-travel-debugging-ttd-file-size
- "If you record an app or process even just for a few minutes, the trace file may grow to be several gigabytes in size."

### P54. H.264 and blurred coloured text (finding 14)
Source: https://forums.developer.nvidia.com/t/231994.json (a user's question)
- "When the text color is Geen(ForeColor)/Black(BackColor)， the text in output.h264 created by NvFBCToGLEnc will blurred."
- The fix by switching to YUV 4:4:4 was not in the text read. Not proven.

### P55. HEVC patent pool: what it licenses
Source: https://accessadvance.com/topic-what-do-we-license/
- "We license HEVC Decoders and HEVC Encoders in Consumer HEVC Products, whether made available in devices, software, or through Cloud Based Services."
- "We seek to be paid one royalty per device/software copy on First Sale."
- "Royalties are due for HEVC Software Sold separately from a device…"
- Not on the page: who pays when a program only uses the encoder already in the PC's graphics chip.

### P56. x264 licence
Source: https://www.videolan.org/developers/x264.html
- "x264 is a free software library and application for encoding video streams into the H.264/MPEG-4 AVC compression format, and is released under the terms of the GNU GPL."
- "In addition to being free to use under the GNU GPL, x264 is also available under a commercial license."

### P57. Windows has Media Foundation transforms for HEVC
Sources: https://learn.microsoft.com/en-us/windows/win32/medfound/h-265---hevc-video-decoder , https://learn.microsoft.com/en-us/windows/win32/medfound/h-265---hevc-video-encoder
- "The Media Foundation H.265 video decoder is a Media Foundation Transform that supports decoding H.265/HEVC content…"
- "The Media Foundation H.265 video encoder is a Media Foundation Transform that supports encoding content into the H…" (cut by the reader)

**Still not readable after the retry:** Playwright's video settings (address returned 404), ffmpeg's wiki (no connection), the 47 % YouTube figure (not on the page), WebRTC zero-hertz behaviour (only the bug reference "go/rtc-0hz-present" was readable).

---

## NOT PROVEN (claim is in the log; text not found, or page not readable)

| Claim in `research-log.md` | What happened when Claude read the page |
|---|---|
| Steps Recorder saves a ZIP holding an MHT file (finding 21) | Not found on the Microsoft page read. |
| Intel's hardware HEVC encoder supports screen content coding from Ice Lake on (finding 29) | The two ffmpeg pages returned almost no text (876 characters). Intel's own page failed to load (certificate error). Not read. |
| Zoom sends a shared screen as JPEG and switches to H.264 above about 5 frames per second (finding 48) | Only the thread title was readable: "Zoom RTMS: Consistently Receiving JPEGs for Screenshare, Unable to Trigger Video Stream (h.264)". The 5 frames per second detail was not read. |
| The baseline of the 66 %, 63 %, 61 % screen-content figures (finding 3) | Not stated in the sentence read (P5). |
| CRF ±6 halves or doubles the size; lossless output "will likely be huge"; only libx264rgb is truly lossless (findings 5, 15) | ffmpeg's wiki pages returned almost no text (876 characters). Not read. |
| Lossless x264 at 1900 × 1200 is 62–70 MB per second (finding 15) | Figure not in the text read (P18). |
| JPEG XL at low effort is twice as fast as QOI and 31 % smaller (finding 13) | Not in the text read (P17). |
| Playwright's video is VP8 at a fixed 1 Mbit/s (finding 11) | GitHub page text was navigation only. Not read. |
| H.264 blurred text until switched to 4:4:4 (NVIDIA forum, finding 14) | Forum page text was not readable. Not read. |
| AV1 saves 47 % against H.264 on YouTube at 1080p (finding 25) | Figure not in the text read (P23). |
| AV1 screen content coding makes frames 25 % smaller (finding 26) | Figure not in the text read (P24). |
| PCoIP sends text lossless and "builds to lossless" (finding 36) | The VMware page returned 3,933 characters with no match. Not read. |
| WebRTC zero-hertz mode lets capture fall to zero (finding 38) | Only the bug title was readable: "Enable libwebrtc's ZeroHertzAdapterMode in FrameCadenceAdapter". The behaviour was not read. |
| VVenC ran at 6.6 frames per second at 1080p (finding 42) | The page shows the benchmark command; the result figure was not in the text read. |
| Time Travel Debugging traces run to gigabytes with large overhead (finding 44) | Not in the text read; the page read says the opposite about overhead (P32). |
| RDP AVC444 sends full colour resolution (finding 12) | Only the structure name was readable. Not read. |
| Tight encoding saves 52–98 % against Hextile (finding 18) | Only the page preface was read; the figures were not. |
| A PDF source for AV1's screen tools and for mixed raster content (findings 6, 20) | PDFs were not read. |
| A UI-tree snapshot costs about 273 tokens against 1,000–1,800 for a screenshot (finding 34) | The page timed out. A second page (Greptile) was readable only as headings. Not read. |
| libaom real-time speed 10 runs 140–170 frames per second at 1080p (finding 37) | The page did not load. Not read. |
| Lagarith best for capture, FFV1 most balanced; lossless codecs reach about 2:1 (finding 39) | The page read lists the tested codecs only; the conclusions and sizes were not in the text read. |
| Desktop Duplication dates from Windows 8 and Windows.Graphics.Capture from Windows 10 version 1803 (finding 22) | Forum page text was navigation only. Not read. |
| `PrintWindow` with `PW_RENDERFULLCONTENT` captures covered windows (finding 46) | The flag is not on Microsoft's page (P39). |
| Long distances between key frames slow exact seeking (finding 47) | Page text was navigation only. Not read. |
| apitrace and GFXReconstruct details (finding 43) | Only RenderDoc's page was read (P37). |

## REMOVED (was from memory, not from a source)
- "Older integrated Intel graphics decode AV1 but cannot encode it" (finding 8). Claude wrote this from memory. It is withdrawn.

## CORRECTIONS to the log
- **Finding 36 was wrong in one point.** The log says Citrix "made the whole-screen version its default, in place of selecting regions". The pages say the reverse: the older setting "compresses the screen" (P34), and the new default "uses a video codec on large, actively changing screen regions" (P1). The default selects regions.
- **Finding 18 used wording that is not on the page.** The log quotes "for most workloads, TurboVNC does a much better job of compression than H.264". Those words are not there. The page's own words are in P3.
- **Finding 1 was wrong about screenpipe.** The log says it records MP4 video at 1 frame per second. The architecture page read says it "captures only when something actually changes — pairing each screenshot with accessibility tree data" (P13). The 1 frame per second and MP4 claims were not found there.
- **Finding 24 was wrong about OpenH264.** The log says its screen mode "runs on one thread". On the page, "Its use is single threaded" is about a different thing (a data lock in Firefox's code), not about OpenH264. Only the "several times longer" statement is proven (P22).
- **Finding 35 had the wrong date.** The log says the AV2 specification was released about 28 May 2026. The press release read is dated 9 June 2026 (P27).
- **Finding 44 was contradicted.** The log says Time Travel Debugging has "large overhead". The page read says "TTD is efficient and works to add as little overhead as possible" (P32).

## STATUS OF THE 48 FINDINGS IN THE LOG (second pass, 2026-11-23)
- 39 source pages were opened in this pass, 11 in the first.
- Proven with a verbatim quote: P1–P33 above.
- Failed, unreadable or not yet opened: the table under NOT PROVEN.
- Wrong and corrected: findings 1, 18, 24, 35, 36, 44. Withdrawn: one claim in finding 8.

---

## Claude's reading (inference, NOT proven fact)
Kept apart on purpose. From P1–P11 only:
- Two commercial remote-desktop systems and one conferencing system treat still screen content and changing screen content differently (P1, P2, P11).
- One study found a changed-region encoder ahead of H.264 on most of its desktop datasets, with H.264 ahead on game-like ones (P3).
- One vendor measured a 63 % saving from AV1 against H.264 on screen sharing, and under 200 kbit/s for static content (P4); another states 50–75 kbit/s for screen sharing (P6).
- Models take in frames at limited detail by default and need a higher setting or a higher-resolution tier to read small text (P7, P8).

These are readings of the quotes, not measurements of a recorder. No method is chosen.
