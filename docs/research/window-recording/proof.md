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

---

## NOT PROVEN (claim is in the log; text not found, or page not readable)

| Claim in `research-log.md` | What happened when Claude read the page |
|---|---|
| Recall takes snapshots only when the screen differs from the previous one (finding 2) | Not found on https://support.microsoft.com/en-us/topic/2c35b596-5a96-4090-b791-c27fae75f660 or on the "Retrace your steps with Recall" page. |
| Steps Recorder saves a ZIP holding an MHT file (finding 21) | Not found on the Microsoft page read. |
| Intel's hardware HEVC encoder supports screen content coding from Ice Lake on (finding 29) | The two ffmpeg pages returned almost no text (876 characters); the claim was not read. |
| Zoom sends a shared screen as JPEG and switches to H.264 above about 5 frames per second (finding 48) | Page not read yet. |
| The baseline of the 66 %, 63 %, 61 % screen-content figures (finding 3) | Not stated in the sentence read (P5). |

## REMOVED (was from memory, not from a source)
- "Older integrated Intel graphics decode AV1 but cannot encode it" (finding 8). Claude wrote this from memory. It is withdrawn.

## CORRECTIONS to the log
- **Finding 36 was wrong in one point.** The log says Citrix "made the whole-screen version its default, in place of selecting regions". The page says the default "uses a video codec on large, actively changing screen regions" (P1). It selects regions; it is not whole-screen.
- **Finding 18 used wording that is not on the page.** The log quotes "for most workloads, TurboVNC does a much better job of compression than H.264". Those words are not there. The page's own words are in P3.

## SUMMARY ONLY (from search summaries; pages not read yet; not findings until proven)
Findings 1, 4, 5, 6, 7, 10, 11, 13, 14, 15, 16, 17, 19, 20, 22, 24, 25, 26, 28, 30, 31, 32, 34, 35, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, and the Webex part of 48. Each will be opened and either moved to PROVEN with its quote or listed under NOT PROVEN.

---

## Claude's reading (inference, NOT proven fact)
Kept apart on purpose. From P1–P11 only:
- Two commercial remote-desktop systems and one conferencing system treat still screen content and changing screen content differently (P1, P2, P11).
- One study found a changed-region encoder ahead of H.264 on most of its desktop datasets, with H.264 ahead on game-like ones (P3).
- One vendor measured a 63 % saving from AV1 against H.264 on screen sharing, and under 200 kbit/s for static content (P4); another states 50–75 kbit/s for screen sharing (P6).
- Models take in frames at limited detail by default and need a higher setting or a higher-resolution tier to read small text (P7, P8).

These are readings of the quotes, not measurements of a recorder. No method is chosen.
