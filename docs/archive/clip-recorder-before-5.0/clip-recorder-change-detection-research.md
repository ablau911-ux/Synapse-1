# Clip recorder: detecting meaningful visual change in screen recordings

Research report, 2026-11-22 16:29 JST (machine clock). Read and search only; no code was changed.

Question: how do others find meaningful visual changes in desktop, web and mobile screen recordings, so that an image-only AI agent sees the right key frames? What does that mean for our viewer's change detector, and what is the simplest design that fixes review problems (a) to (e) within budget (f)?

## Legend

- **[S]** the cited source states this (quoted or closely paraphrased).
- **[C]** I computed this with a short script (formula given, reproducible).
- **[I]** my inference or recommendation. It is not stated by a source and must be proven on our proof clip.
- **[N]** a source that I found only through a search summary or abstract and did not open in full.

## Our current design and the review problems (from the brief)

20 fps H.264 segments (quality 70, GOP 20, so one keyframe per second). Consecutive frames are compared in 8×8 blocks. A block is changed when at least 3 pixels differ by more than 64 in max(|ΔR|,|ΔG|,|ΔB|). Changed blocks form regions. A region is short-lived when it returns within 500 ms and settled after 200 ms quiet. A block is animating when it changed in at least half the frames within 500 ms. Nearby changes are absorbed into animation intervals. Caret toggles (thin, about 500 ms period) are ignored. A 48 px box around the logged cursor is masked.

| Id | Problem |
|---|---|
| (a) | Fades and gradual changes made of small steps are never detected. |
| (b) | Absorption next to spinners swallows real changes (a label beside a spinner). |
| (c) | The all-or-nothing return test over wide regions hides a flash when a nearby region changes in the same frames. |
| (d) | Low-contrast changes are missed at 64: list selection #cce8ff on white differs by 51, a grey fill by 47. Keyframe encoder noise reached 7 px per block above 32 and 1 px above 64. |
| (e) | The caret rule drops thin real changes (playhead, meter). |
| (f) | Key frames must fit an image/token budget. |

## 1. Numbers recomputed for our cases [C]

Per-pixel colour metrics for our example pairs. YIQ is the pixelmatch ≤ v7 metric, expressed as the equivalent pixelmatch threshold t = sqrt(δ/35215). OKLab HyAB is the pixelmatch v8 metric (1 = black vs white). ΔE00 is CIEDE2000 (sRGB, D65).

| Pair | max-channel Δ | Δ luma (BT.601) | YIQ t | OKLab HyAB | ΔE00 |
|---|---|---|---|---|---|
| white vs #cce8ff (list selection) | 51 | 28.8 | 0.130 | 0.139 | 12.84 |
| white vs #d0d0d0 (grey fill) | 47 | 47.0 | 0.178 | 0.166 | 10.18 |
| #f3f3f3 vs #e5e5e5 (hover tint) | 14 | 14.0 | 0.053 | 0.049 | 2.97 |
| #808080 vs #a0a0a0 (noise, +32 all channels) | 32 | 32.0 | 0.121 | 0.122 | 10.85 |
| #808080 vs #a08080 (noise, +32 red only) | 32 | 9.6 | 0.068 | 0.075 | 13.59 |
| white vs #ffffdf (noise, −32 blue only) | 32 | 3.6 | 0.041 | 0.051 | 12.41 |

Findings from this table:

- [C] At pixelmatch's default threshold 0.1, both real changes are detected (0.130 and 0.178 > 0.1). So is a 32-level noise pixel (0.121). CIEDE2000 rates noise pixels (10.9 to 13.6) the same as or higher than the real changes (10.2 to 12.8).
- [I] **A better per-pixel colour metric does not separate our real low-contrast changes from encoder noise.** What separates them is *spatial support*: a selection or fill changes every pixel of an interior block (64 of 64 above 24), while keyframe noise touched at most 7 pixels above 32 (from the brief). That is why the recommendation counts pixels per block instead of switching to YIQ, OKLab or CIEDE2000.

SSIM on flat UI areas [C]. For two flat patches the SSIM contrast and structure terms are 1, so SSIM equals the luminance term (2μxμy + C1)/(μx² + μy² + C1), with C1 = (0.01·255)².

- White to #cce8ff: SSIM 0.9929. White to #d0d0d0: 0.9796. #808080 to #a0a0a0: 0.9756.
- Black to #202020: 0.0063. The luminance term is a ratio, so it is very sensitive near black and nearly blind near white.
- [I] GIFdroid's "similar" threshold of 0.95 (section 5) would call both of our real changes "no change", even on the changed block itself. SSIM is a poor primary detector for flat light-theme UI changes.

Fades at 20 fps [C]. Per-frame step = total change / number of frames.

| Fade length | Frames | Black text on white, step per frame | #cce8ff selection, step per frame |
|---|---|---|---|
| 150 ms | 3 | 85.0 | 17.0 |
| 200 ms | 4 | 63.8 | 12.8 |
| 300 ms | 6 | 42.5 | 8.5 |
| 500 ms | 10 | 25.5 | 5.1 |

- [C] Any fade of 200 ms or longer, even black on white, never has a single step above 64. This explains problem (a): with consecutive-frame differencing, no threshold that rejects noise can see a slow fade.

## 2. Block and frame difference with noise tolerance

### ffmpeg `mpdecimate`

- [S] It drops frames that do not differ greatly from the previous frame. Options `hi`, `lo`, `frac`: "Values for hi and lo are for 8x8 pixel blocks and represent actual pixel value differences, so a threshold of 64 corresponds to 1 unit of difference for each pixel, or the same spread out differently over the block." Source: https://ffmpeg.org/ffmpeg-filters.html#mpdecimate
- [S] Defaults (source code): `hi = 64*12` (768), `lo = 64*5` (320), `frac = 0.33`, `max = 0`, `keep = 0`. Source: https://github.com/FFmpeg/FFmpeg/blob/master/libavfilter/vf_mpdecimate.c
- [S] Algorithm (source code): an 8×8 SAD is computed in windows on a 4-px stride, on every plane including chroma. The frame is "different" as soon as one window's SAD exceeds `hi`, or when more than `t = (w/16)·(h/16)·frac` windows exceed `lo`.
- [S] **It compares against a reference frame, not the previous frame**: `decimate->ref` is replaced only when a frame is kept (`DECIMATE_KEEP_UPDATE`). Slow drift therefore accumulates against the reference until it crosses a threshold.
- [I] Relevance: `hi` is a mean of 12 levels per pixel. A uniform #cce8ff block gives luma SAD 64 × 28.8 ≈ 1840 [C], well above 768. Keyframe noise of 7 px at about 32 plus small residue on the rest gives roughly 450 to 680 [C, estimate] — close to 768, so a SAD threshold has little margin. A pixel count (n ≥ 32 of 64 above 24) has more margin; see the design.

### ffmpeg `freezedetect`, `select=gt(scene,x)`, `scdet`

- [S] `freezedetect` compares "the mean average absolute difference of all the components of video frames … to a noise floor". `noise` defaults to −60 dB (0.001), `duration` to 2 s. https://ffmpeg.org/ffmpeg-filters.html#freezedetect
- [S] `select` with the `scene` value: "Comparing scene against a value between 0.3 and 0.5 is generally a sane choice." https://ffmpeg.org/ffmpeg-filters.html#select_002c-aselect
- [S] `scdet` threshold is a percentage of maximum change, default 10. "Good values are in the [8.0, 14.0] range." https://ffmpeg.org/ffmpeg-filters.html#scdet-1
- [I] All three are whole-frame scores built for film cuts. A list selection covers about 1.85 % of a 1080p frame [C]. Whole-frame scores do not fit UI micro-changes.

### PySceneDetect

Source for all items: https://www.scenedetect.com/docs/latest/api/detectors.html (v0.7.3). CLI: https://www.scenedetect.com/docs/latest/cli.html

- [S] `ContentDetector(threshold=27.0, min_scene_len=15, weights=(hue 1, sat 1, lum 1, edges 0))` "Detects fast cuts using changes in colour and intensity between frames", in HSV.
- [S] `AdaptiveDetector(adaptive_threshold=3.0, window_width=2, min_content_val=15.0, min_scene_len=15)` is a two-pass detector. It "applies a rolling average … that can help mitigate false detections in situations such as camera movement".
- [S] `ThresholdDetector(threshold=12.0, fade_bias=0.0, min_scene_len=15)` "Detects fast cuts/slow fades in from and out to a given threshold level". This is the fade-to-black case only.
- [S] `HashDetector(threshold=0.35, size=8, lowpass=2)` uses a perceptual hash. `HistogramDetector(threshold=0.2, bins=128)` uses the YUV Y histogram.
- [S] `min_scene_len` defaults to 15 frames in Python and 0.6 s in the CLI.
- [S] In GIFdroid's evaluation on GUI recordings, PySceneDetect scored keyframe F1 0.475 against GIFdroid's 0.880 (precision 0.418, recall 0.550). https://arxiv.org/abs/2112.04128
- [I] Natural-video scene detectors are the wrong tool for UI state changes. The lesson for fades: `ThresholdDetector` handles fades only by comparing against an absolute level, not a frame-to-frame delta.

### pixelmatch, odiff, Resemble.js

- [S] pixelmatch ≤ v7 uses the YIQ metric of Kotsarenko and Ramos, with `maxDelta = 35215 * threshold²` and default `threshold: 0.1`. `includeAA: false` means anti-aliased pixels are detected and not counted. https://github.com/mapbox/pixelmatch/blob/v6.0.0/index.js
- [S] pixelmatch v8.0.0 (released 2026-10-06) replaced YIQ with OKLab plus HyAB. The default stays 0.1. It added `windowSize` and `ignoreMask`. https://github.com/mapbox/pixelmatch/releases
- [S] On `windowSize`: "This helps with noise. GPU dithering and sub-pixel anti-aliasing scatter stray pixels all over the image, so they never fill up one small square, while a real regression usually changes a compact area." The README example is `windowSize: 16`, changed if the count is > 28. https://github.com/mapbox/pixelmatch
- [I] This is the same idea as the recommended "n ≥ 32 of 64 pixels above 24" block rule: count compact support, not peak difference.
- [S] odiff: YIQ NTSC colour difference, `threshold` (0 to 1), an `antialiasing` flag and `ignoreRegions`. https://github.com/dmtrKovalenko/odiff
- [S] Resemble.js: `ignoreAntialiasing`, `ignoreColors`, `ignoredBoxes`, `ignoreAreasColoredWith`, `largeImageThreshold: 1200`. Its antialiasing algorithm was developed "over several days of trial-and-error", with no published basis. https://github.com/rsmbl/Resemble.js
- [S] Playwright `toHaveScreenshot`: `threshold` is "acceptable perceived color difference in the YIQ color space … Defaults to 0.2". Other options are `maxDiffPixels`, `mask` (pink box over locators), `animations: "disabled"` (default) and `caret: "hide"` (default). It waits "until two consecutive page screenshots yield the same result". https://playwright.dev/docs/api/class-pageassertions
- [S] Chromatic: default `diffThreshold` 0.063, which "balances high visual accuracy with low false positives (for example, from artifacts like anti-aliasing)". https://www.chromatic.com/docs/threshold/

### SSIM / MS-SSIM and perceptual colour difference

- [S] scikit-image `structural_similarity`: to match Wang et al. 2004, "set gaussian_weights to True, sigma to 1.5, use_sample_covariance to False". The paper is https://ece.uwaterloo.ca/~z70wang/publications/ssim.pdf, via https://scikit-image.org/docs/stable/api/skimage.metrics.html
- [N] MS-SSIM (Wang, Simoncelli, Bovik 2003) evaluates SSIM over several scales. Not opened.
- [S] CIEDE2000 implementation notes and test data: https://hajim.rochester.edu/ece/sites/gsharma/ciede2000/
- [C]/[I] For our problem, SSIM is nearly blind to flat light-area changes (0.993 and 0.980 above). ΔE00 and YIQ do not separate noise from real low-contrast change per pixel (table above). The max-channel difference is cheap and adequate, provided the decision is made on per-block pixel counts.

## 3. Gradual changes and fades

- [S] mpdecimate compares to the last kept frame (above).
- [S] psc2code "computes the normalized root-mean-square error (NRMSE) … If the dissimilarity … is less than a user-specified threshold (0.05 in this work), psc2code discards [the frame] … Otherwise, it keeps [it] … and uses [it] as a new starting point to compare its subsequent frames." https://arxiv.org/abs/2103.11610
- [S] PSFinder does the same: it deletes successive frames "until the dissimilarity … is above a threshold. We set the threshold to 0.05 rather than 0 to account for small differences … (e.g, consecutive frames change due to a mouse cursor movement or different time shown in the desktop clock)." https://arxiv.org/abs/2203.04519
- [S]/[N] The twin-comparison method (Zhang, Kankanhalli, Smoliar 1993, *Multimedia Systems* 1:10–28, DOI 10.1007/BF01210504) is the classic gradual-transition (dissolve, fade) detector. A lower threshold marks a candidate transition start. Differences are accumulated while they stay above it, and a higher threshold confirms the transition. Abstract: https://www.oreilly.com/library/view/readings-in-multimedia/9781558606517/xhtml/B9781558606517501145.htm. I read only secondary descriptions; the full text was not opened.
- [S] Running-average background: OpenCV `accumulateWeighted`, `dst ← (1−alpha)·dst + alpha·src`. "alpha regulates the update speed". https://docs.opencv.org/4.x/d7/df3/group__imgproc__motion.html
- [S] MOG2: `createBackgroundSubtractorMOG2(history=500, varThreshold=16, detectShadows=true)`. KNN: `history=500, dist2Threshold=400`. https://docs.opencv.org/4.x/de/de1/group__video__motion.html
- [I] For UI recordings the simplest correct baseline is a **per-block anchor**: the block's last settled picture. It is mpdecimate's or psc2code's reference frame applied per block instead of per frame, so one moving area does not reset the baseline for the whole screen. Twin-comparison hysteresis gives the two thresholds. Running averages and MOG2 add a learning rate that blurs the exact before/after pictures we want to show the agent, and they target natural-scene backgrounds. They are not needed.

## 4. Animation, spinner and video-region suppression

- [S] GIFdroid failure modes: "some GUIs may contain animated app elements such as advertisement or movie playing, which will change dynamically, resulting in no steady keyframes being localized". Also: slow loading where "the partial GUI may stay for a relatively long time", and users clicking "before the GUI is fully rendered". https://arxiv.org/abs/2112.04128
- [I] A whole-screen steady test fails as soon as any part animates. Steadiness must be judged per block.
- [S] Visual-testing tools suppress animation **at the source and scoped to the element**:
  - Playwright `animations: "disabled"`: finite animations are fast-forwarded and infinite ones reset. `mask` covers only the locator's bounding box.
  - Chromatic pauses CSS animations at the last frame (`pauseAnimationAtEnd`) and videos/GIFs at the first frame. https://www.chromatic.com/docs/animations/ Its ignore applies to the element with `.chromatic-ignore` or `data-chromatic="ignore"` and its bounding box. https://www.chromatic.com/docs/ignoring-elements/
  - Percy will "Freeze animated GIFs on the first frame. Freeze most CSS animation and transition styles." https://www.browserstack.com/docs/percy/stabilize-screenshots/animations
  - Applitools: match levels Strict, Layout, Ignore Colors and Dynamic, per checkpoint or region. "Ignore regions will prevent Visual AI comparison from occurring in the specified section. Floating regions allow you to specify a 'range' where a certain element can be positioned". https://applitools.com/docs/eyes/concepts/working-with-eyes/match-levels
- [I] None of these tools absorbs *neighbours* of a moving element. Exclusion stops at the element's own box. Our "absorb nearby changes" rule has no precedent and causes (b).
- [S]/[N] Periodic motion: Cutler and Davis (IEEE PAMI 22(8), 2000) detect periodicity from the self-similarity of a tracked object over time. https://www.cse.buffalo.edu/courses/cse725/peter/Cutler_2000.pdf (search summary only).
- [S]/[N] Seenomaly (ICSE 2020) lints GUI animations from screencasts with an unsupervised adversarial autoencoder. It is a classifier, not a change detector. https://chunyang-chen.github.io/publication/Seenomaly_ICSE20.pdf (search summary only).
- [I] For spinners a per-block activity count is enough: changed in ≥ 50 % of frames within 500 ms, which is our existing rule. The fix is to keep that status per block, freeze the block's anchor while it animates, and never let it suppress other blocks beyond one 8 px ring.

## 5. GUI screen-recording key-frame extraction (bug reports, tutorials, agents)

| Work | How stable states / transitions are found | Reported parameters | Source |
|---|---|---|---|
| GIFdroid (ICSE 2022) | SSIM on the Y (luma) channel of consecutive frames. Patterns: instantaneous transition, animation transition, steady. | [S] "0.95 as the threshold to decide whether two frames are similar, and 5 frames as the threshold to indicate a steady state" (pilot study). Recordings were 7–30 fps. Keyframe F1 0.880 vs PySceneDetect 0.475. | https://arxiv.org/abs/2112.04128 |
| CAPdroid (ICSE 2023) | Same Y-SSIM similarity sequence. TAP = sharp drop; SCROLL = drop then slow rise; INPUT = repeated drop/rise plus OCR keyboard detection ("qwert", "123"). | [S] Action segmentation 0.84 video F1 and 0.93 accuracy, best of five baselines. | https://arxiv.org/abs/2302.00886 |
| Video2Action (UIST 2023) | Y-SSIM. A shot is a fully rendered UI, "the steady state where the consecutive frames are similar for a relatively long duration". | [S] "we set a duration of 1 second as a relatively long duration". Scene generation video F1 81.6 %. | https://arxiv.org/abs/2308.03252 |
| V2S / V2S+ (ICSE 2020, TSE) | No pixel diff. A Faster R-CNN detects the Android "show touches" indicator and an opacity CNN classifies press or lift. | [S] Input video must be at least 30 fps. | https://arxiv.org/abs/2005.09057, https://arxiv.org/abs/2301.01191 |
| psc2code (TOSEM 2020) | NRMSE against the last kept frame. | [S] NRMSE < 0.05 means non-informative. | https://arxiv.org/abs/2103.11610 |
| PSFinder (2022) | Samples 1 frame per 30 s, then NRMSE against the last kept frame. | [S] Threshold 0.05 to tolerate cursor movement and clock changes. | https://arxiv.org/abs/2203.04519 |
| SeeHow (2023) | ActionNet classifies HCI actions. Frames with no action, move-mouse or move-cursor are discarded. | [S] Active text line overlap threshold 0.75. | https://arxiv.org/abs/2304.14042 |
| CodeMotion (L@S 2018) | [N] Segments code regions, OCR, merges edits into contiguous intervals. | [N] Not opened. | https://pg.ucsd.edu/publications/Codemotion-programming-tutorial-video-interfaces_LAS-2018.pdf |
| GUI-World (2024) | Benchmark of key-frame strategies for MLLMs: Random (fixed interval), Extracted (Katna), Human. | [S] 10 frames for Random and Extracted; human sets average 6.719 frames. Random and human frames scored "approximately 0.2–0.3 points higher … than those using programmatic extraction … traditional keyframe technologies, designed for natural videos, are less effective for detecting essential GUI operations". More keyframes and higher resolution improved results. | https://arxiv.org/abs/2406.10819 |
| ScreenLLM (WWW 2025) | Top-k frames by second-order pixel change (change of the frame-difference), then rectangles where pixel change exceeds a threshold, then OCR. | [S] "We set the threshold for bounding box detection to 30." | https://arxiv.org/abs/2503.20978 |
| Mobile-Agent-V (2025) | Uniform sampling, drop near-duplicates, drop frames too close in time. Similarity = share of non-zero pixels in the grayscale absdiff. Keyframes are shown as a mosaic. | [S] Threshold 0.3 (text-heavy Settings app) or 0.5 (visual Weather app). Sliding window of 4 keyframes "as a balanced trade-off" against token cost. | https://arxiv.org/abs/2505.13887 |
| VG-GUIBench / TASKER (ECCV 2026) | Key-frame extraction combining task relevance and scene dynamics. | [S] "performance … on both VideoQA and video-guided agentic tasks critically depends on effective keyframe extraction". | https://arxiv.org/abs/2606.29445 |
| Watch & Learn (2025), VideoAgentTrek (2025) | Inverse-dynamics models predict the action from two consecutive screen states. No hand-tuned thresholds reported. | [S] W&L reports over 53K trajectories. | https://arxiv.org/abs/2510.04673, https://arxiv.org/abs/2510.19488 |

[I] Common pattern: a key frame is the end of a transition, once the UI has been steady for a minimum time. Reported minimums are 5 frames (GIFdroid), 1 s (Video2Action), and two identical consecutive screenshots (Playwright). Our 200 ms settle (4 frames at 20 fps) is in the same range as GIFdroid. These works detect *screen* transitions on mobile. None of them addresses fades, small low-contrast changes, or partial-screen flashes. Our requirements are stricter, which is why the design below works per block rather than per frame.

## 6. Damage tracking (dirty rectangles) as a change source

- [S] Desktop Duplication (Windows 8+): `GetFrameDirtyRects` returns "non-overlapping rectangles that indicate the areas of the desktop image that the operating system updated". `GetFrameMoveRects` returns moved regions. "Process all the move regions and then process all the dirty regions." When the OS runs out of space it coalesces, so it "covers pixels that it has not actually updated". https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api and https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-getframedirtyrects
- [S] Desktop Duplication also reports the mouse pointer separately (position per frame, shape on change) when the adapter overlays it. (same page)
- [S] Windows Graphics Capture: `Direct3D11CaptureFrame.DirtyRegions` (an `IVectorView<RectInt32>`) and `GraphicsCaptureSession.DirtyRegionMode` (`ReportOnly` = 0, `ReportAndRender` = 1). The contract version is 1245184, and the docs list them for the winrt-26100 API set onward. Microsoft documents no semantics beyond the signatures.
  - https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframe.dirtyregions
  - https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturedirtyregionmode
  - https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.dirtyregionmode
- [I] 26100 corresponds to Windows 11 24H2.
- [S] VNC/RFB: the client asks for incremental `FramebufferUpdateRequest`. The server sends only changed rectangles, and CopyRect for moved areas. https://www.rfc-editor.org/rfc/rfc6143
- [S] X11 DAMAGE: "Monitoring the regions affected by rendering has wide-spread use, from VNC-like systems scraping the screen…" https://www.x.org/releases/X11R7.7/doc/damageproto/damageproto.txt
- [S] RDP's graphics pipeline (MS-RDPEGFX) is also region-based. https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpegfx/da5c75f9-cd99-450c-98c4-014a496942b0
- [S] ffmpeg can export H.264 motion vectors (`export_mvs`, shown by `codecview`). https://ffmpeg.org/ffmpeg-filters.html#codecview
- [I] Dirty rectangles are a superset of real change: repaints with identical pixels and coalescing. They say *where to look*, not *what changed visibly*. Their main benefit for us would be removing encoder noise outside dirty areas entirely, at the root of (d).
- [I] They must be captured by the recorder and stored as per-frame sidecar metadata. The viewer cannot recover them from H.264. Recommended as an optional phase 2 recorder change. The pixel-count design below does not depend on them.

## 7. Caret and cursor in screencast analysis

- [S] Playwright hides the caret by default in visual comparisons (`caret: "hide"`). Visual-test tools treat the caret as noise at the source.
- [S] `GetCaretBlinkTime` "Retrieves the time required to invert the caret's pixels" and returns `INFINITE` when the caret does not blink. https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getcaretblinktime
- [S] `GUITHREADINFO.rcCaret` is "The caret's bounding rectangle, in client coordinates". https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-guithreadinfo
- [I] Custom-drawn UIs (browsers, Electron) may not expose a system caret there; UI Automation is the fallback.
- [S] psc2code: the cursor overlapping code causes OCR errors, and cross-frame information corrects them. PSFinder and psc2code set the NRMSE threshold to 0.05 partly to tolerate cursor movement. SeeHow (ActionNet) discards move-mouse and move-cursor frames.
- [S] V2S relies on the Android touch indicator, a semi-transparent circle that is solid on press and fades on lift.
- [S] Windows Graphics Capture: `GraphicsCaptureSession.IsCursorCaptureEnabled` "specifying whether the capture session will include the cursor in the captured content". https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.iscursorcaptureenabled
- [S] `SM_CXCURSOR` is "The nominal width of a cursor, in pixels". https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getsystemmetrics
- [I] Best practice: keep the cursor out of the pixels (capture without the cursor, draw it from the log), so that no mask is needed and hover effects under the cursor stay visible. Short of that, mask only the old and new cursor rectangles, and only in frames where the logged position or shape changed. A stationary cursor creates no diff, so it never needs a mask.

## 8. Method comparison

| Method | Solves | Key parameters | Source |
|---|---|---|---|
| Reference-frame (anchor) comparison | a, c | Compare to the last kept/settled picture, not the previous frame. | mpdecimate source; psc2code; PSFinder |
| Twin-comparison hysteresis | a, d | Low threshold starts a candidate, high threshold confirms, differences accumulate in between. | Zhang et al. 1993 [N] |
| mpdecimate hi/lo/frac | d | SAD per 8×8: hi 768, lo 320, frac 0.33, 4-px stride, all planes. | ffmpeg docs + source |
| pixelmatch `windowSize` count | d | Count of diff pixels per N×N window (example N = 16, > 28); threshold 0.1. | pixelmatch README v8 |
| YIQ / OKLab / CIEDE2000 per pixel | does not separate noise from low contrast [C] | pixelmatch 0.1, Playwright 0.2, Chromatic 0.063. | pixelmatch, Playwright, Chromatic |
| SSIM (Y channel) steady detection | screen transitions only; misses (d) [C] | 0.95 plus 5 frames (GIFdroid); 1 s (Video2Action). | GIFdroid, Video2Action |
| Whole-frame scene scores | none of a–e | scene 0.3–0.5; scdet 8–14; content 27; adaptive 3.0; min_scene_len 15. | ffmpeg; PySceneDetect |
| Element-scoped ignore / mask / freeze | b, e (principle) | Mask the element box only; pause at end or first frame. | Playwright, Chromatic, Percy, Applitools |
| Per-block activity / periodicity | b | ≥ 50 % of frames in 500 ms; self-similarity for period. | our rule; Cutler & Davis [N] |
| Dirty rects (DDA / WGC / RFB / DAMAGE) | d (noise gating), cost | Superset rectangles; needs recorder sidecar; WGC from API 26100. | Microsoft docs; RFC 6143; X DAMAGE |
| Caret signature (blink time, rcCaret) | e | Half-period = `GetCaretBlinkTime`; caret rect. | Win32 docs |
| Key-frame budgeting: crops, mosaics, top-k | f | 1920×1080 = 1,560 tokens (standard tier) or 2,691 (high-res); 1000×1000 = 1,296; 200×200 = 64; Mobile-Agent-V window 4. | Claude vision docs; Mobile-Agent-V; ScreenLLM |

## 9. Recommended design: per-block anchor with two-tier counts

[I] The whole section is my recommendation. Every parameter is a starting value to be confirmed on the proof clip (section 10). It keeps our 8×8 grid, the max-channel metric, the 200 ms settle and 500 ms windows. It changes *what each block is compared against*, *how a block is judged* and *where decisions are made*: per block, not per merged region.

### 9.1 Block test (fixes d)

For a block and a reference picture R, count pixels by max(|ΔR|, |ΔG|, |ΔB|):

- `n24` = pixels with Δ > 24.
- `n64` = pixels with Δ > 64.

Predicates:

- **DIFF(R)**: `n64 ≥ 3` OR `n24 ≥ 32`. The first part is the existing rule for thin, high-contrast changes. The second is new: half the block changed by a low-contrast amount.
- **SAME(R)**: `n64 ≤ 1` AND `n24 ≤ 16`.
- Between the two: undecided. This is the hysteresis band.

Check against our data:

- #cce8ff (Δ 51) and the grey fill (Δ 47) give `n24 = 64` on interior blocks, so DIFF.
- Keyframe noise (≤ 7 px above 32, 1 px above 64) is expected to stay under `n24 ≤ 16` and `n64 ≤ 1`, so SAME. This must be measured (M1).
- Deliberately out of scope: hover tints of about 14 levels and 1-px low-contrast borders.

### 9.2 Per-block anchor and state (fixes a)

Each block keeps an anchor A: its last settled picture, 192 bytes, about 6 MB at 1080p. Each frame t:

1. **Departure.** The first frame where not SAME(A) is stored as `depart_t`. The block becomes ACTIVE when DIFF(A) holds. A slow fade crosses DIFF(A) at some point even though no single step does, and `depart_t` back-dates the start.
2. **Settle.** SAME(f_t, f_t−k) holds for k = 1..4 (200 ms at 20 fps). Comparing against t−4, not only t−1, stops a slow fade from "settling" in mid-ramp.
3. **On settle:**
   - If DIFF(A): **CHANGED** event (start `depart_t`, end t), then A := f_t.
   - If SAME(A) and ACTIVE: **RETURNED**. It is a **FLASH** if t − 4 − `depart_t` ≤ 500 ms, otherwise **REVERTED**. A is unchanged.
   - If undecided: A := f_t silently. This absorbs noise drift and emits no event.

### 9.3 Animation (fixes b)

- A block is **animating** while DIFF(f_t, f_t−1) held in ≥ 5 of the last 10 frames. This is our existing rule, kept per block.
- An 8-neighbour of an animating block counts as animating at ≥ 4 of 10, to catch spinner edges. This one-block ring replaces "absorb nearby changes".
- While a block animates it emits no CHANGED, FLASH or REVERTED events, and **its anchor is frozen**. When it settles, the normal settle rule applies. A spinner that ends on new content produces CHANGED; a spinner that disappears and leaves the old picture produces RETURNED.
- A label next to a spinner is not animating. It settles 200 ms after its change and emits CHANGED while the spinner keeps spinning. If the label shares a block with the spinner, that block's event is deferred to the spinner's end, not lost.
- Connected animating blocks (8-neighbourhood, overlapping in time) form one **ANIMATION** interval. It is reported with its first and last frame.

### 9.4 Flash next to a change (fixes c)

- The return test is per block (9.2), never over a merged wide region.
- Events are then grouped by **spatio-temporal connected components**, separately for each event type: blocks that are 8-neighbours *and* whose [`depart_t`, settle] windows overlap.
- A flash beside a persistent change therefore yields two events, one FLASH and one CHANGED.
- A moving thin object (a playhead sweeping across blocks) chains into one component, which avoids one event per block.

### 9.5 Caret (fixes e)

A group is treated as a caret and dropped only if **all** of these hold:

1. The pixel footprint (from the diff mask, not the block box) is ≤ 3 px wide and 8–48 px tall, or the transposed shape for underline carets.
2. It has two states, and the "off" state is SAME(A). Each toggle returns.
3. At least 2 on/off cycles at a fixed position (same footprint box).
4. The half-period is within ±50 ms (1 frame) of `GetCaretBlinkTime()`, logged by the recorder. If the recorder has no value, use 530 ms ±50 ms ([I] the commonly cited Windows default; verify on the test machine).
5. Optional: the footprint overlaps the logged `rcCaret` or UI Automation caret rectangle.

A playhead moves (fails 3). A meter has more than two states and no blink period (fails 2 and 4). Both fall through to ANIMATION or CHANGED.

Cursor: replace the always-on 48 px box with a mask of the old and new logged cursor rectangles (cursor size from the logged shape or `SM_CXCURSOR` × DPI scale, +2 px). Apply it only in frames where the logged position or shape changed. Phase 2 option: record with `IsCursorCaptureEnabled = false` and draw the cursor from the log.

### 9.6 Key-frame selection within budget (fixes f)

One image per event:

| Event | Image |
|---|---|
| CHANGED | The settle frame. Optionally pair it with the anchor crop as "before". |
| FLASH | The frame of maximum `n24` sum. This is the only moment it is visible. |
| REVERTED | The peak frame. |
| ANIMATION | First and last frame. |

Merging and cropping:

- Merge events whose settle times are within 500 ms into one image.
- Crop to the union box of the event's blocks plus a 64 px margin, minimum 256 px per side. Use the full frame only when the box exceeds 25 % of the screen.

Budget in tokens:

- [S] Standard tier: a 1920×1080 frame costs 1,560 tokens (downsized to 1456×819). High-resolution tier: 2,691 tokens. https://platform.claude.com/docs/en/build-with-claude/vision
- [C]/[I] tokens ≈ ⌈w/28⌉·⌈h/28⌉ matches every row of the documentation table. A 448×280 crop is about 160 tokens and 256×256 about 100, so a typical budget can carry about 10 times more crops than full frames.

Ranking when over budget:

- First priority goes to FLASH and CHANGED events of the user-action window (logged input ±1 s). Next comes changed area × mean Δ. ANIMATION events come last.
- If still over budget, tile consecutive crops into a mosaic, as Mobile-Agent-V does with its keyframe mosaic.

### 9.7 Why this is the simplest fix

- It is one comparison-target change (per-block anchor), one extra counter (`n24`), one lagged settle test (t−4), and one grouping rule (spatio-temporal components per event type).
- It needs no colour-space conversion, no SSIM, no ML model and no recorder change.
- What it removes: region-level absorption and the all-or-nothing wide-region return test.
- [I] Cost is about two block comparisons per frame (vs t−1 and vs A) plus the t−2..t−4 checks only for ACTIVE blocks.

## 10. What to measure on the proof clip

| Id | Measure | Pass condition | If it fails |
|---|---|---|---|
| M1 noise floor | On static stretches, per-block `n24` and `n64` vs the previous frame and vs the anchor. Split frame pairs that cross a keyframe (every 20th frame) from those that do not. Report max and p99.99. | Max `n24` ≤ 16 and max `n64` ≤ 1 on static content, so no CHANGED or FLASH events there. | Raise the low threshold from 24 to 32, or the SAME bound. Never lower the DIFF bound below 2× the measured noise. |
| M2 recall per event type | Hand-label the clip: list selection, grey fill, fade of 300 ms or more, label beside spinner, flash beside a persistent change, playhead, meter, typing. | Each yields the expected event type. Start (`depart_t`) within ±1 frame of the label; settle within 4–5 frames after the real end. | |
| M3 false events | A spinner alone; static screen; caret blinking. | Spinner = exactly one ANIMATION interval and 0 CHANGED. Caret = 0 events. Static = 0 events. | |
| M4 fade evidence | Plot the per-frame max Δ vs t−1 and vs A across the fade. | Shows the old failure (never > 64 vs t−1) and the new detection (DIFF vs A). | |
| M5 budget | Images, tokens per clip and per minute; share of labelled events covered by at least one key frame. | Within the caller's budget, 100 % coverage of labelled events. | |
| M6 cost | Analysis time per minute of 1080p video; anchor memory. | | |

## 11. Considered and not recommended now

- **SSIM or MS-SSIM as the detector:** blind to flat light-area changes [C]. Still useful later as a "visually same" check for de-duplicating key frames.
- **CIEDE2000, YIQ or OKLab per pixel instead of max-channel:** no separation from encoder noise [C]. It costs a colour conversion per pixel.
- **MOG2, KNN or running-average background:** adds a learning rate and multi-modal model. Exact before/after pictures are lost. Per-block anchors already give the baseline.
- **Whole-frame detectors (PySceneDetect, scene, scdet, freezedetect):** built for cuts. A UI change covers about 2 % of a frame. GIFdroid measured F1 0.475 for PySceneDetect on GUI recordings.
- **Dirty rectangles:** a good phase 2 noise gate, but they need a recorder sidecar (WGC dirty regions from API 26100, or Desktop Duplication), and they are a superset of real change.
- **Learned models (ActionNet, inverse dynamics, autoencoders):** heavier than needed for change detection. They would matter later only to label *actions*.

## 12. Limitations of this research

- Twin-comparison, Cutler and Davis, MS-SSIM, CodeMotion and Seenomaly were read only through abstracts or search summaries [N].
- Microsoft documents no behaviour for WGC `DirtyRegionMode`, only the signatures.
- The 530 ms caret default and the 26100-to-24H2 mapping are not from the cited pages.
- All thresholds in section 9 are starting values [I]. M1 decides the low threshold and the SAME bounds.
