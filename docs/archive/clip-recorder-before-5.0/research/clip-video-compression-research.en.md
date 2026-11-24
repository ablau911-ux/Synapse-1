# Video compression and creation for screen clips: text quality, file size and system resources

*Research. Written 2026-11-23. It covers both compression and video creation: capture, timestamps, container, seeking, encoding, the production chain, editing, export, color grading, audio, distribution, sync and HDR. It combines a literature review (150 external sources, 18 verified quotes) with experiment E1 and measured experiments on the clip recorder's corpus: 2,808 H.264 runs and 6,195 zstd runs. Internal sources: commit `2ff3db5a`, Gitea #290.*

## Abstract

The goal is a small, high-quality screen clip whose creation uses minimum RAM and CPU. Quality means fully readable text: `R = 1`, where R is 1 minus the edit distance between the OCR text of the clip and the OCR text of the source, divided by the length of the source text.

We compared two ways. Way D: the H.264 encoders of Windows Media Foundation (Intel Quick Sync and Microsoft software). Way Z: lossless zstd. We tested each on 24 scenes: 6 content types times 4 resolutions.

No H.264 setting reached `R = 1`; the best was 0.9996. zstd reached `R = 1` in every run. The price is size: full-frame zstd is 2.7–12 times larger than H.264 at H.264's best readability, and thousands of times larger on video content.

The literature points to one main cause of the text loss in H.264: 4:2:0 chroma subsampling. Professional screen-sharing systems solve it with 4:4:4, AVC444, "build to lossless" or encoding by content type. Our data fits this too: maximum quality did not reach `R = 1`, and at σ = 2 R went up.

Experiment E1 tested the conversion to NV12 alone, without H.264. That alone dropped R below 1 in 8 of 12 scenes. In typing at full resolution R fell to 0.965. In scrolling and UI, the R of the conversion alone is almost the same as the best R of H.264. So the limit comes from the conversion to YUV, not from the compression, and even 4:4:4 will not guarantee `R = 1`.

Conclusion: for text, lossless zstd on RGB. To cut the size: a zstd format that writes only what changed (7–12 times smaller in the partial measurement). For video content, lossy encoding.

## 1. Introduction

The Synapse clip recorder records a window or a screen, so that an agent or a person can later see what happened. The main use is reading: what the window said, at what moment a message appeared, what changed. So a wrong letter is a failure, not just a cosmetic flaw. "1,243.94" read as "1,248.94" is wrong information.

Research question: which compression way gives the smallest file, with the lowest resources, on condition that all the text is kept (`O = [R = 1]`)? The decision function:

```
J = bytes / bytes* + CPU / CPU* + RAM / RAM*      (over the ways that meet O = [R = 1]; * = the best in the set)
```

## 2. Theoretical background

### 2.1 Screen capture and pixel format

Windows always delivers the screen image as 8-bit BGRA [13]. The Desktop Duplication API also returns "dirty rectangles", areas that changed since the previous frame, and "move rectangles" for scrolling [10, 11, 12]:

> Gets information about dirty rectangles for the current desktop frame.
>
> — Microsoft, GetFrameDirtyRects [11] ✓ verified

According to the documentation, the rectangles do not overlap and mark what changed since the previous frame. The operating system can merge areas, so the rectangles are an upper bound, not an exact difference [13]. FreeRDP uses exactly this pattern: move rectangles first, then dirty rectangles [14]. This is the cheapest way to know what changed without comparing all the pixels.

### 2.2 Color spaces: RGB and YCbCr

Video encoders work in the YCbCr space: brightness (Y) and two color channels. The conversion by BT.601 or BT.709 needs rounding at 8 bits, and so does the reverse conversion. So the round trip itself loses information [19]. A forum reports that at 8 bits about 5/6 of the RGB values change in the round trip, but never by more than 2 units [17, 18]. By contrast, YCoCg-R is exactly reversible in integers, at the cost of one extra bit in each color channel [15, 16]:

> The scaled version uses a lifting scheme to make it exactly invertible
>
> — Wikipedia, YCoCg [112] ✓ verified

A deviation of 2 units barely hurts OCR. It explains why "lossless" in YUV is still not bit-identical, but it does not explain the wrong letters.

### 2.3 Chroma subsampling

In 4:2:0 each color channel is kept at half the width and half the height, so a block of 2×2 pixels shares one color [20]:

```
bits per pixel (8-bit):  4:4:4 = 8 + 8 + 8 = 24     4:2:2 = 8 + 2·8/2 = 16     4:2:0 = 8 + 2·8/4 = 12
chroma samples kept:     4:4:4 = 100 %              4:2:2 = 50 %               4:2:0 = 25 %
```

> Colorful computer text and line graphics can become blurry during reduced color resolution during 4:2:2 or 4:2:0 chroma resolution.
>
> — TestUFO [22] ✓ verified

According to RTINGS, the artifacts stand out especially in text on a solid-color background, and for desktop use 4:4:4 matters "a lot" [21]. This is a paraphrase, because the page did not load for verification. Verified sources back the same claim: TestUFO above, and the quotes below.

> YUV 4:4:4 does not downsample chroma at all, so this is more appropriate for screen capture
>
> — FFmpeg-user 2018 [26] ✓ verified

> chroma subsampling will introduce visually disturbing artifacts, such as in images containing text
>
> — US patent 9153017 [24, 110] ✓ verified

Windows draws text with ClearType, which uses each color channel separately for the edges of the letters [30, 31]. 4:2:0 deletes exactly this information. More sources document the same effect in recordings: in H.264 even "lossless" goes to 4:2:0 [25], text blurs in yuv420p [26], and so in the OBS forum [27], at RED [23] and at VideoHelp [32].

### 2.4 Hybrid video coding (H.264 and HEVC)

H.264 and HEVC are "hybrid" encoders: prediction (within the frame and between frames), transform, quantization and entropy coding [1, 2]. HEVC aimed at a saving of about 50% in bitrate over H.264 at the same perceived quality [2]. The loss comes from quantization, and rate control sets the QP: constant CQP, constant-quality CRF, or bitrate [58, 59].

> A constant QP encode at QP=18 will stay at QP=18 regardless of the frame (there is some small offset for different frame types, but it is negligible here). Constant Rate Factor at CRF=18 will increase the QP to, say, 20, for high motion frames ...
>
> — slhck, CRF guide [59] ✓ verified

On Windows, the Media Foundation H.264 encoder supports only the Baseline, Main and High profiles [60], and the HEVC encoder only Main [61]. They have no 4:4:4 and no lossless mode.

### 2.5 Screen content tools

HEVC SCC added tools for screen content: block copy within the same picture (IBC), color palette, adaptive color transform and adaptive motion resolution [69]. In the official verification test:

> For TGM video, the estimated bit-rate savings ranged from 60–90% relative to the JM and 40–80% relative to the HM
>
> — Verification testing of HEVC SCC, Microsoft Research [68] ✓ verified

AV1 has matching tools: IBC, palette and lossless mode [70, 71, 74], and they also exist in SVT-AV1 [72]. According to an FFmpeg patch, Intel's hardware AV1 encoder offers them too:

> Use palette mode in Screen Content Tool. ... Use intra block copy in Screen Content Tool.
>
> — FFmpeg-devel, QSV AV1 [73] ✓ verified

### 2.6 Lossless compression

Zstandard is documented in RFC 8878 [6]. Compression levels range from −131072 (fastest) to 22, and negative levels give up compression ratio for speed [7, 8, 9]. In the Silesia benchmark, zstd 1.5.7 at level −3 compressed 2.24 times at a speed of about 635MB per second [8].

On a screen, most of the frame does not change. A codec that compresses each frame on its own misses this:

> In screen captures where > 95% of the frame stays exactly the same, this approach doesn't work well indeed.
>
> — FFmpeg-devel 2022 [84] ✓ verified

So codecs built for screen recording (TSCC, ZMBV) are better than general codecs [82]. FFV1 is also meant for screen recording [83], and there are broad comparisons of lossless codecs [89]. For still images, lossless WebP shrank a screenshot more than JPEG XL and PNG in one test [86]. JPEG XL is stronger on photos [85, 87, 88].

### 2.7 Quality metrics

PSNR measures average error. SSIM measures how well structure is kept [3]. VMAF combines several metrics and is calibrated for compression and scaling artifacts [4, 5]. None of them answers the question "did the text stay correct". So we used an OCR-based metric, which is the complement of the character error rate (CER) [79, 80]:

```
CER = edit_distance(reference, ocr) / |reference|          R = 1 − CER
```

On the common scale for printed text, a CER of 1–2% counts as "good" [81]. So `R = 0.98–0.99` counts as good for document scanning. Our requirement is stricter: `R = 1`.

The literature shows that compression hurts text recognition:

> harmful compression artifacts in textual regions that reduced OCR accuracy
>
> — GDELT [75] ✓ verified

License plate recognition drops with quality along a logistic curve [77]. See also [78].

> lack of clarity in the text areas is noticeable making the text difficult to discern and in some cases render it unreadable
>
> — US patent 8064516 [76, 111] ✓ verified

### 2.8 Video creation: from the screen to a file

**Capture.** Windows.Graphics.Capture delivers frames through a pool (Direct3D11CaptureFramePool) and a FrameArrived event, which fires when a new frame is stored in the pool [90, 91]. Each frame has a SystemRelativeTime, and this is the basis for exact timestamps and for sync [92, 93]:

> The QPC (Query Performance Counter) time at which the compositor rendered the frame.
>
> — Microsoft, SystemRelativeTime [92] ✓ verified

The API sends a frame only when the screen changes, so the actual rate varies by itself. Desktop Duplication (section 2.1) adds information about the areas that changed.

**Timestamps and frame rate.** In Media Foundation each sample gets a time and a duration before it is written to the Sink Writer [94, 95]. When there are B-frames, the MP4 sink needs both a presentation time and a decode time:

> When writing B-frames into an MP4 file, you must set both the presentation time stamp and the decode time stamp.
>
> — Microsoft, MPEG-4 File Sink [96] ✓ verified

There is also a report of a corrupt MP4 when samples are fed too fast [97].

Screen recordings usually have a variable frame rate (VFR). Editors like Premiere work better with a constant frame rate (CFR), and VFR causes stutter and audio drift [102, 103].

Our recorder solves this differently: it saves frames only when the picture changes, that is VFR. The frames.jsonl file gives a fixed timeline of 50 milliseconds for every moment and points to the frame shown at it. So the timeline has CFR without duplicating frames.

**Container and crash resilience.** In a regular MP4 the index (moov) is written only at the end of the recording, so a crash can leave a file that cannot play [98, 101]. MKV and fragmented MP4 lose only the last fragment in a crash. OBS 30.2 writes "Hybrid MP4": it is fragmented during recording, and it gets a full index at the end [98, 99, 100]:

> It remains recoverable even if writing the file is aborted, e.g. due to system crashes or power outages
>
> — OBS knowledge base, Hybrid MP4 [99] ✓ verified

In a zstd clip each frame is a separate file, and the index (frames.jsonl) is written line by line. A crash loses at most the last moment, and this is the basis for recovery (B10).

**Keyframes and seeking.** A seek to a point in the clip lands on a keyframe and decodes forward to the requested frame. A long GOP compresses better but slows seeking. In practice a GOP of 0.5 to 2 seconds is common [104, 105, 106]. In full-frame zstd each frame stands on its own, so the number of frames to decode on a seek is `N_dec = 1`. In zstd formats that store only differences, N_dec ≤ K_z.

**Hardware vs software encoding.**

> Hardware encoders ... are generally recommended for best performance as they take the workload off the CPU
>
> — OBS knowledge base, Hardware Encoding [107] ✓ verified

In a Wowza test, CPU load fell from about 50% with x264 to about 20% with NVENC, and Quick Sync freed CPU in a similar way [108]. x264 gives better quality per bit, at a CPU cost [109].

In our case the software encoder (DW2) used about 2.2 seconds of CPU and about 700MB of RAM per 20 seconds of typing at 1080p. zstd −3 compression is orders of magnitude cheaper: zstd reaches about 635MB per second [8].

### 2.9 Video creation in general: from shooting to viewing

**The production chain.** Video production commonly uses three codec roles:

- **Capture codec:** in the camera.
- **Intermediate (mezzanine) codec:** for editing. ProRes and DNxHR keep high quality and hold up under repeated edits. They come in tiers: 4444 and 444 for finishing, 422 HQ for intermediate work, and Proxy for light editing [113–116]. An intermediate codec does not improve the picture, but a wrong choice can harm it [115].
- **Distribution codec:** for viewing. H.264, HEVC, AV1 and VVC, built for small size and playback.

Our clip is a special case: it is both the capture and the archive, and a machine (OCR) reads it, not only an eye. So it is closer to a lossless intermediate codec than to a distribution codec.

**Distribution and bitrate.** Instead of a fixed bitrate ladder, Netflix encodes each title at several resolutions and bitrates, and picks the points on the "convex hull" of the rate-quality curve [117, 119]. According to Mux, the cited saving is about 20% per title and about 30% per scene [118]. This is the same principle as our J: measure all the options and pick by the optimal frontier.

**New codecs.** VVC (H.266) was finalized in 2020, with a saving of about 50% over HEVC according to the standard itself [120]. Independent tests found 31–40%, with encoding at least 8 times more complex and decoding 1.5 times more complex [121]. The more efficient the codec, the more CPU the encoding costs, and that is exactly the resource we want to save.

**Audio-video sync.** According to ITU-R BT.1359, the offset is noticeable from +45 to −125 milliseconds, and acceptable up to +90/−185 (plus = sound before picture) [122]. Other sources cite the same numbers with the opposite sign [123]. Our clip has no audio. If audio is added, the QPC timestamp of each frame (section 2.8) is the basis for sync.

**Color and dynamic range.** BT.2100 defines HDR with PQ or HLG curves, BT.2020 colors and a depth of 10 or 12 bits [124–126]. The Windows screen is captured as 8-bit BGRA in SDR (section 2.1), so our clip stays 8-bit RGB. An HDR screen will need a separate decision.

### 2.10 Video production: editing, export, color and audio

**Non-linear editing.** The common editing workflow has two stages.

- **Offline:** you edit with light proxy files instead of the original media [127, 128].
- **Online:** you bring back the original media at full resolution and match it to the edit (conform). Then come effects, color grading and graphics [127].

For the link to work, the proxy must have the same name, the same timecode and the same audio channels as the original [129, 130]. Premiere and Avid handle this themselves [130, 131].

**Export.** On export you pick a rate control mode. CRF fits when you want a known quality, and two-pass encoding (2-pass) when you want a known file size [134]. According to a Streaming Learning Center test, two-pass takes 2 times as long and mainly improves long content, and it barely helps short clips [132]. YouTube recommends an MP4 container and BT.709 for SDR content, and it re-encodes every upload [133].

**Color grading.** Log footage needs a conversion before the creative work, otherwise the tools work in the wrong space [137]. A LUT can serve as an input transform (camera to Rec.709) or as an output transform at the end of the chain [137]. There are two approaches:

- **Scene-referred:** you grade before the conversion to display space, and you keep the full range the camera captured [139].
- **Display-referred:** you grade after the conversion to display space.

ACES is a scene-referred pipeline with wide-gamut colors and a rendering transform for the display [135, 136, 138].

**Audio.** 48 kHz is the common rate for audio in video [145]. AAC is the default for distribution, and Opus is better at low bitrates and in real time [144, 150]. On loudness:

- EBU R 128 sets −23 LUFS for broadcast [140, 141, 142].
- Streaming platforms normalize to about −14 LUFS, Apple Music to −16 [143].

So one master needs a loudness match for each target.

**Screencast production.** The practical recommendations:

- Record at high quality and edit afterwards [147].
- Enlarge the user interface so that the text is readable [146].
- Move the cursor slowly, and use cursor highlight and zoom only where needed [147, 148].
- Avoid private details during the recording [149].

**Link to our product.** The recorder's clip is the "source media" in this chain. It is stored lossless, so you can export from it at any moment to any distribution codec without losing a generation.

The Synapse viewer is the counterpart of offline editing: it reads the clip, marks moments of change (D5) and lets you jump to any moment. The cursor is sampled with each frame, so a future export stage can add cursor highlight or zoom. For now there is no audio and no HDR color (section 2.9).

## 3. Research method

### 3.1 The corpus

24 scenes: typing, user interface, scrolling, flicker, static screen and video, each at 506×333, 640×360, 1280×720 and 1920×1080. Each scene is 20 seconds in 400 moments of 50 milliseconds. Headless Chrome drew the scenes with virtual time, so every run gives the same pixels. The corpus is stored as zstd frames at level −3. The encoder gets the small scenes upscaled by σ, by pixel duplication:

```
σ(1920×1080) = σ(1280×720) = 1      σ(640×360) = 2 → 1280×720      σ(506×333) = 3 → 1518×1000
```

### 3.2 The ways and the equations

**Way D:** a live recorder writes frames, and a separate process encodes them to H.264. We tested four encoders: DW1 Intel Quick Sync, DW2 Microsoft software, DW3 NVIDIA and DW4 MS AVC DX12. Settings: q from 30 to 100, τ_f (keyframe interval), content hint, and bitrate β. Each setting ran 3 times, 2,808 runs in total.

**Way Z:** lossless zstd in 54 mapped ways: full frame, XOR, tiles, rectangles, prefix, stream, dictionary, thread count and layouts.

```
bytes      B     = Σ frame files + frames.jsonl + clip.json
quality    Z:    Σ|decoded − source| = 0  ⇒  SSIM = 1, PSNR = ∞, R = 1
CPU        C     = N_new · (c_0 + px · κ_c)                      (cycle counter, all threads)
RAM        M     = M_0 + M_cctx(L, W) + B_c · px · 3             (OS peak private bytes − M_0)
ready      t_ready = t_close ≤ 50 ms
```

Measurement rules: the input is always BGRA, and the conversion is measured. CPU comes from the cycle counter, and RAM from the peak that the operating system reports. The bytes are all the files that are saved. R is measured in the text areas of each scene, on 10 frames, with Windows OCR. Each run is tied to its run list by hash.

## 4. Results

### 4.1 Way D (H.264)

Of 2,808 runs: 1,428 valid, 228 with a frame-mapping failure (all of them Quick Sync on content with gaps), 576 that did not run because the NVIDIA encoder did not start (0x8000FFFF), and 576 that did not run because DX12 has no product path. In both encoders an extra keyframe was added. The checks: 17 passed, 2 failed on these known exceptions.

**Table 1.** All 24 scenes, in MiB per minute. zstd = full frame at level −3; always R = 1. The H.264 R is in parentheses.

| Content | Resolution | zstd | H.264 smallest (R) | H.264 largest (R) | H.264 at best R (size) |
|---|---|---:|---:|---:|---:|
| Typing | 506×333 | 1.63 | 0.45 (0.9730) | 1.32 (0.9959) | 0.9959 (1.07) |
| Typing | 640×360 | 1.67 | 0.30 (0.9897) | 0.65 (0.9721) | 0.9984 (0.52) |
| Typing | 1280×720 | 1.82 | 0.17 (0.9343) | 0.34 (0.9564) | 0.9918 (0.26) |
| Typing | 1920×1080 | 2.02 | 0.25 (0.9343) | 0.42 (0.9564) | 0.9918 (0.34) |
| UI | 506×333 | 24.97 | 4.32 (0.9806) | 16.89 (0.9866) | 0.9888 (14.05) |
| UI | 640×360 | 28.53 | 2.07 (0.9830) | 24.30 (0.9940) | 0.9958 (7.31) |
| UI | 1280×720 | 36.63 | 1.38 (0.9531) | 24.30 (0.9974) | 0.9974 (6.81) |
| UI | 1920×1080 | 38.21 | 1.44 (0.9733) | 53.97 (0.9996) | 0.9996 (13.92) |
| Scrolling | 506×333 | 24.07 | 10.34 (0.9878) | 97.91 (0.9926) | 0.9929 (22.66) |
| Scrolling | 640×360 | 32.51 | 6.88 (0.9892) | 29.17 (0.9959) | 0.9959 (29.17) |
| Scrolling | 1280×720 | 92.23 | 6.82 (0.9947) | 50.80 (0.9972) | 0.9978 (12.10) |
| Scrolling | 1920×1080 | 142.13 | 9.92 (0.9957) | 76.26 (0.9965) | 0.9976 (17.92) |
| Flicker | 506×333 | 4.94 | 1.33 | 39.66 | n/a |
| Flicker | 640×360 | 4.98 | 0.82 | 24.30 | n/a |
| Flicker | 1280×720 | 14.08 | 0.92 | 24.30 | n/a |
| Flicker | 1920×1080 | 14.80 | 0.97 | 53.97 | n/a |
| Static screen | 506×333 | 0.03 | 0.08 | 0.30 | n/a |
| Static screen | 640×360 | 0.04 | 0.06 | 0.20 | n/a |
| Static screen | 1280×720 | 0.04 | 0.03 | 0.09 | n/a |
| Static screen | 1920×1080 | 0.05 | 0.03 | 0.10 | n/a |
| Video | 506×333 | 570.67 | 3.93 | 391.37 | n/a |
| Video | 640×360 | 770.68 | 1.95 | 232.76 | n/a |
| Video | 1280×720 | 3,004.82 | 1.34 | 364.88 | n/a |
| Video | 1920×1080 | 6,667.54 | 2.63 | 819.58 | n/a |

*R was measured only in scenes with text. Scene = 20 seconds, times 3 per minute. The H.264 bytes are the MP4 file.*

### 4.2 Way Z (zstd)

Every Z row that ran passed the round-trip check: `Σ|decoded − source| = 0`, 6,105 of 6,105. The clip is ready within 50 milliseconds at most in all rows. The RAM of single-thread compression matched the equation within ±10% in all rows. 90 runs of dictionary compression failed on the static scene, as expected: a dictionary needs at least 7 frames.

The run stopped after 6,195 of 48,330 rows, so it covers only 506×333:

**Table 2.** The smallest zstd format found, at 506×333 (MiB per minute)

| Content | Full frame (corpus) | Smallest found | Way | CPU / RAM |
|---|---:|---:|---|---:|
| Typing | 1.63 | 0.22 | ZW12 prefix L3 | 0.27 s / 2.8 MB |
| UI | 24.97 | 5.39 | ZW12 prefix L3 | 0.80 s / 2.8 MB |
| Scrolling | 24.07 | 2.02 | ZW12 prefix L3 | 0.78 s / 2.8 MB |
| Flicker | 4.94 | 1.54 | ZW13 stream L4 | 0.49 s / 132 MB |
| Static screen | 0.03 | 0.13 | ZW13 stream L1 | 0.02 s / 130 MB |
| Video | 570.67 | 350.90 | ZW6 L3 | 3.66 s / 2.3 MB |

*CPU is per whole scene (20 seconds). RAM is above M_0. In the static scene, "smallest found" is larger than the corpus, probably because it also includes frames.jsonl and clip.json. This was not checked.*

### 4.3 Resources

In the forecast check of Way D, the worker of the software encoder (DW2) on typing at 1920×1080 used about 700MB of RAM above the baseline, and 2.2 seconds of CPU per 20 seconds. Full-frame zstd compression uses, by the equation, `M_cctx + px·3`, that is about 6.2MB of frame buffer at 1080p, plus the compression context.

### 4.4 Observations

- **Obs1:** Even at maximum quality (q100) R stays below 1: typing at 1920×1080 0.9564, scrolling at 1920×1080 0.9965. So there is a loss that raising the quality does not remove.
- **Obs2:** At σ = 2 (640×360, each pixel duplicated to 2×2) the typing R was 0.9984, versus 0.9918 at σ = 1.
- **Obs3:** R does not rise consistently with q. In typing at 640×360: 0.9984 at q80, and 0.9721 at q100.
- **Obs4:** In typing, 1280×720 and 1920×1080 got exactly the same R values. We did not check why.

### 4.5 Experiment E1: the conversion to NV12 alone

Method: the same probe and the same converter that the encoder gets (`Nv12Canvas::draw`, BT.709 limited range, color averaged over 2×2). The reverse conversion goes to BGRA by BT.709, and each 2×2 block gets its own color, without interpolation. The OCR reads this image instead of the decoded frame, on the same 10 frames and in the same areas.

```
R_420 = R(source → NV12 → BGRA)          no H.264 at all          probe flag e1_nv12=1 (commit below)
```

**Table 4.** R of the conversion alone, versus the best R of H.264 (Table 1)

| Scene | σ | R_420 (conversion only) | H.264, best R |
|---|---:|---:|---:|
| Typing 1920×1080 | 1 | 0.9652 | 0.9918 |
| Typing 1280×720 | 1 | 0.9652 | 0.9918 |
| Typing 640×360 | 2 | 1 | 0.9984 |
| Typing 506×333 | 3 | 1 | 0.9959 |
| Scrolling 1920×1080 | 1 | 0.9975 | 0.9976 |
| Scrolling 1280×720 | 1 | 0.9976 | 0.9978 |
| Scrolling 640×360 | 2 | 0.9960 | 0.9959 |
| Scrolling 506×333 | 3 | 0.9955 | 0.9929 |
| UI 1920×1080 | 1 | 1 | 0.9996 |
| UI 1280×720 | 1 | 0.9969 | 0.9974 |
| UI 640×360 | 2 | 0.9972 | 0.9958 |
| UI 506×333 | 3 | 0.9898 | 0.9888 |

- **E1-a:** The conversion alone drops R below 1 in 8 of 12 scenes.
- **E1-b:** In scrolling and UI, R_420 is almost equal to the best R of H.264, within a difference of up to 0.003. So at the good settings the compression itself adds very little, and most of the loss already happened in the conversion.
- **E1-c:** In typing at σ = 1 the conversion alone gives 0.965, lower than H.264 (0.9918). The H.264 decoder upscales the color by interpolation, not by blocks, and this probably suits the OCR better. So R is sensitive to very small changes in the pixels.
- **E1-d:** At σ = 2 the 2×2 color block falls exactly on a duplicated source pixel, so the subsampling loses no color. Only rounding of up to 2 units remains. Even so, in scrolling `R = 0.9960` and in UI 0.9972. So even rounding that the eye cannot see changes what the OCR reads.

## 5. Discussion

### 5.1 The cause of the text loss: three hypotheses

- **H1, 4:2:0 chroma subsampling (the main one):** The literature supports it [21–29], and Obs1 fits it, because a loss that does not depend on q fits the stage before encoding. Obs2 fits it too: when σ is even, the 2×2 color block falls exactly on one source pixel, and the sampling loses no color. This holds on condition that the decoder does not mix neighbors when it upscales, and this was not checked.
- **H2, 8-bit YCbCr rounding:** It exists, but only up to 2 units [17]. It is unlikely to swap letters.
- **H3, quantization:** It causes part of the loss at low q. In typing at 1080p R rises from 0.9343 at q30 to 0.9918 at q70. It does not explain the ceiling.

**E1 result (section 4.5):** H1 is partly confirmed. 4:2:0 at σ = 1 drops the typing R to 0.965. But E1-d shows that even without subsampling, the rounding of H2 is enough to drop R below 1. H3, quantization, is the smallest factor at the good settings (E1-b).

The key conclusion: no path that goes through 8-bit YUV will guarantee `R = 1`, because the OCR reacts even to changes that the eye cannot see. This includes 4:4:4, AVC444 and lossless H.264 in yuv444. Only a lossless RGB path guarantees `R = 1`: zstd, or x264rgb at qp 0.

### 5.2 How the industry solves it

All the professional screen-sharing systems offer a special mode for text:

> In the case of H.264 4:2:0 (the Visually lossless policy is disabled), the final image is not pixel perfect (lossless)
>
> — Citrix Thinwire [39] ✓ verified

> YUV444 produces approximately 1.5–2 times more data than YUV420 for the same frame.
>
> — Citrix YUV444 support [40] ✓ verified

According to the VMware Blast Extreme guide, High Color Accuracy turns off 4:2:0 and switches to 4:4:4 when text is not sharp [42, 43]. This is a paraphrase, because the site was not available for verification. The two Citrix quotes above verify the same principle.

- **Microsoft AVC444:** two H.264 streams in 4:2:0 format, one for brightness and one for color, from the same encoder and the same decoder. Together they give 4:4:4 on 4:2:0 hardware [29, 33, 34, 38]. This mode is on in Azure VD and in Windows 365 [35–37].
- **Citrix:** "Build to lossless" sharpens to a lossless image when the screen settles. YUV444 produces 1.5–2 times more data [39, 40]. In one test: 195MB versus 655MB for H.264 YUV444 [41].
- **VMware, Parsec, Splashtop and Sunshine** offer 4:4:4 [42–48].
- **Teradici PCoIP** picks a codec by content type and sends text lossless [49–51].
- **RDP 8.1** sent text in a dedicated codec and images in H.264 [29].

Even 4:4:4 is not bit-identical. Only lossless RGB encoding is:

> the conversion from RGB to YUV 4:4:4 is not truly lossless (you can't get back bitwise identical images), but it is way much better than using 4:2:0
>
> — OBS guide [53] ✓ verified

See also [52, 54–57].

### 5.3 What is available on this computer

Media Foundation does not offer 4:4:4 [60, 61]. Quick Sync worked in OBS only in NV12 for about 8 years [64]. HEVC in 4:2:2 exists only from generation 10 [62], and the question about 4:4:4 in H.264 stayed without a clear answer [63]. NVENC supports 4:4:4 [65–67], but on this computer the NVIDIA encoder did not start.

### 5.4 The options for the product

**Table 3.** Options, by criteria O and J

| Option | R | Size | CPU / RAM | Risk |
|---|---|---|---|---|
| Full-frame zstd (plan 4.2) | 1 | Large (Table 1) | Low / a few MB | Video: 6.5GB per minute at 1080p |
| zstd that writes only what changed (prefix, XOR, rectangles) | 1 | 7–12 times smaller than full frame (506×333) | Low / 2.8MB | Measured only at 506×333 |
| zstd for text, H.264 for video (by content) | 1 in text | Small even in video | H.264 in the worker used about 700MB | Content detection; two paths |
| AVC444 on Quick Sync | < 1 per E1 | 1.5–2 times 4:2:0 | 2 times the encoding time | Own implementation; probably not R = 1 |
| AV1 with screen content tools (Intel QSV) | Unknown | Large saving on text | Hardware, if present | Unknown whether the processor here supports it |
| Lossless RGB H.264 (x264 in software) | 1 | Medium | High CPU | New dependency and GPL licensing |

## 6. Limitations

- The Z run is partial: only 506×333.
- The OCR is Windows OCR. Another engine will give other R values, but a comparison on the same engine is fair.
- The corpus is synthetic, from one computer and one graphics card.
- Every direct quote in the document (inside a quote box) was checked against the full page on 2026-11-23 and is marked ✓. Two checks, 26 pages, 18 quotes verified. Four pages did not load: RTINGS (content did not load), VMware/Omnissa (DNS, 403), Blender (403), FFmpeg wiki (bot protection). Their claims appear as marked paraphrases, and a verified source also backs each one: TestUFO and FFmpeg-user for subsampling, Citrix for 4:4:4, OBS for RGB versus YUV 4:4:4. The other claims are paraphrases from the sources.
- Section 2.10 (production) is based on professional guides, standards documents (EBU, ITU) and vendor blogs, not on our own measurement. The recorder does not edit, does not grade color and does not record audio.
- In E1 the reverse conversion from NV12 uses the color of each block without interpolation. Real decoders interpolate, and this explains E1-c.
- Some sources are forums or marketing material, and they carry less weight. The description marks them.

## 7. Conclusions and recommendations

1. H.264 with Media Foundation on this computer does not meet the requirement `R = 1` in any setting (2,808 runs).
2. zstd always meets it. For text and UI it is the right choice.
3. To cut the size: finish the Z run, and pick the most economical zstd format for each resolution.
4. Video content on the screen needs lossy encoding, otherwise the size is unreasonable. This is a product decision: by content, or a documented limit.
5. E1 is done. The conversion to 8-bit YUV alone prevents `R = 1`, so 4:4:4 and AVC444 solutions will not meet the requirement either. Only lossless RGB meets it.
6. Next experiments:
   - **E2:** finish the Z run, when the owner decides, after the build by plan 4.2.
   - **E3:** dirty rectangles from Desktop Duplication to save CPU [10–14].
   - **E4:** check whether the GPU here supports AV1 with screen content tools [73].

## References

1. Wiegand, Sullivan, Bjøntegaard, Luthra (2003). Overview of the H.264/AVC video coding standard. IEEE TCSVT 13(7):560–576. [pdf](https://iphome.hhi.de/wiegand/assets/pdfs/csvt_overview_0305.pdf)
2. Sullivan, Ohm, Han, Wiegand (2012). Overview of the HEVC standard. IEEE TCSVT 22(12):1649–1668. [link](https://www.microsoft.com/en-us/research/?p=798934)
3. Wang, Bovik, Sheikh, Simoncelli (2004). Image quality assessment: from error visibility to structural similarity. IEEE TIP 13(4):600–612. [link](https://ece.uwaterloo.ca/~z70wang/publications/ssim.html)
4. Netflix. VMAF. [github.com/Netflix/vmaf](https://github.com/Netflix/vmaf)
5. Bampis et al. SpatioTemporal feature integration and model fusion for FR-VQA. [arXiv:1804.04813](https://arxiv.org/pdf/1804.04813)
6. Collet, Kucherawy (2021). RFC 8878: Zstandard compression and application/zstd. [link](https://rfc-editor.org/rfc/rfc8878.html)
7. Zstandard project. [facebook.github.io/zstd](https://facebook.github.io/zstd)
8. zstd docs: compression levels. [link](https://www.mintlify.com/facebook/zstd/concepts/compression-levels)
9. LKML: btrfs zstd negative compression levels (2025). [link](https://lkml.indiana.edu/hypermail/linux/kernel/2501.3/06350.html)
10. Microsoft Learn. Desktop Duplication API. [link](https://learn.microsoft.com/en-ie/Windows/Win32/direct3ddxgi/desktop-dup-api)
11. Microsoft Learn. IDXGIOutputDuplication::GetFrameDirtyRects. [link](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-getframedirtyrects)
12. Microsoft Learn. IDXGIOutputDuplication::GetFrameMoveRects. [link](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgioutputduplication-getframemoverects)
13. Microsoft Learn. DXGI_OUTDUPL_FRAME_INFO. [link](https://learn.microsoft.com/windows/win32/api/dxgi1_2/ns-dxgi1_2-dxgi_outdupl_frame_info)
14. FreeRDP. server/Windows/wf_dxgi.c. [link](https://git.stg.centos.org/source-git/freerdp/blob/5a9772fdc85fecb7337184707f474e5788ebd87e/f/server/Windows/wf_dxgi.c)
15. Malvar, Sullivan. YCoCg-R: a color space with RGB reversibility (JVT-I014r3). [pdf](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/06/Malvar_Sullivan_YCoCg-R_JVT-I014r3-2.pdf)
16. Microsoft Research. Lifting-based reversible color transformations for image compression. [link](https://www.microsoft.com/en-us/research/?p=158074)
17. VideoHelp forum: RGB↔YUV round trip at 8 bit. [link](https://forum.videohelp.com/showthread.php?p=2476477)
18. Gist: BT.709/BT.601 round-trip simulation. [link](https://gist.github.com/linrock/5be4f365c9c9e61eee9e8984ba13cb25)
19. JVT-J020 (2003). [link](https://www.itu.int/wftp3/av-arch/jvt-site/2003_12_Waikoloa/JVT-J020r1.DOC)
20. Linux kernel docs. YUV formats. [link](https://docs.kernel.org/userspace-api/media/v4l/yuv-formats.html)
21. RTINGS. Chroma subsampling. [link](https://www.rtings.com/tv/learn/chroma-subsampling)
22. TestUFO. Chroma subsampling test. [link](https://testufo.com/chroma)
23. RED. Video chroma subsampling. [link](https://www.red.com/red-101/video-chroma-subsampling)
24. US patent 9153017. System and method for optimized chroma subsampling. [link](https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/9153017)
25. Blender #61569. Lossless H.264 not lossless because of 4:2:0. [link](https://projects.blender.org/blender/blender/issues/61569)
26. FFmpeg-user (2018). Text blurry after rgb→yuv420p. [link](https://ffmpeg.org/pipermail/ffmpeg-user/2018-June/040281.html)
27. OBS forum. Color quality loss due to chroma subsampling. [link](https://obsproject.com/forum/threads/color-quality-loss-due-to-chroma-subsampling.123738/)
28. Citrix. Thinwire color space. [link](https://docs.citrix.com/en-us/citrix-virtual-apps-desktops/graphics/thinwire/colorspace)
29. Microsoft (2016). RDP 10 AVC/H.264 improvements. [link](https://techcommunity.microsoft.com/t5/microsoft-security-and/remote-desktop-protocol-rdp-10-avc-h-264-improvements-in-windows/ba-p/249588)
30. Microsoft Research. Optimal filtering for patterned displays (ClearType). [pdf](https://www.microsoft.com/en-us/research/wp-content/uploads/2000/07/optfilt.pdf)
31. Wikipedia. Subpixel rendering. [link](https://www.Wikipedia.org/wiki/BGR_(subpixels))
32. VideoHelp forum: chroma and text sharpness. [link](https://forum.videohelp.com/showthread.php?p=2211745)
33. MS-RDPEGFX. RFX_AVC444V2_BITMAP_STREAM. [link](https://msdn.microsoft.com/en-us/library/3b337b87-f478-4786-a63b-97794aa72075)
34. MS-RDPEGFX. YUV444 modes. [link](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpegfx/844018a5-d717-4bc9-bddb-8b4d6be5dd3f)
35. Azure Virtual Desktop. Graphics encoding. [link](https://learn.microsoft.com/en-us/azure/virtual-desktop/graphics-encoding)
36. Azure Virtual Desktop. Increase chroma to 4:4:4. [link](https://learn.microsoft.com/bg-bg/azure/virtual-desktop/graphics-chroma-value-increase-4-4-4)
37. Windows 365. Chroma value. [link](https://learn.microsoft.com/en-us/windows-365/enterprise/chroma-value-change-default)
38. msandbu. Windows Server 2016 and AVC 444 mode. [link](https://msandbu.org/windows-server-2016-and-introducing-avc-444-mode/)
39. Citrix. Thinwire (2402 LTSR). [link](https://docs.citrix.com/en-us/citrix-virtual-apps-desktops/2402-ltsr/graphics/thinwire.html)
40. Citrix. YUV444 support (Mac VDA). [link](https://docs.citrix.com/en-us/mac-vda/configure/graphics/yuv444-support.html)
41. NVIDIA GTC 2019 S9884. Color accuracy in VDI. [pdf](https://developer.download.nvidia.com/video/gputechconf/gtc/2019/presentation/s9884-user-experience-is-key-to-vdi-success-color-accuracy-is-key-to-user-experience.pdf)
42. VMware. Blast Extreme optimization guide. [link](https://techzone.vmware.com/resource/vmware-blast-extreme-optimization-guide)
43. Omnissa. Blast Extreme optimization guide. [link](https://techzone.omnissa.com/resource/omnissa-blast-extreme-optimization-guide)
44. VMware Horizon 2309. Blast policy settings. [link](https://docs.vmware.com/en/VMware-Horizon/2309/horizon-remote-desktop-features/GUID-220442CF-EA01-470E-A381-1BED9BC0B81C.html)
45. Parsec. Improve stream quality and color accuracy. [link](https://support.parsec.app/hc/en-us/articles/32381785123860-Improve-Stream-Quality-and-Color-Accuracy)
46. Parsec. Programmers and writers. [link](https://support.parsec.app/hc/en-us/articles/32361397075604-Programmers-and-Writers)
47. Splashtop. Superior color accuracy and image clarity (vendor). [link](https://splashtop.com/blog/superior-color-accuracy-and-image-clarity)
48. LizardByte (Sunshine) discussion #220: 4:4:4. [link](https://github.com/orgs/LizardByte/discussions/220)
49. Teradici. The importance of lossless support (vendor). [link](https://connect.teradici.com/blog/the-importance-of-lossless-support)
50. Teradici. What is PCoIP (vendor). [link](https://teradici.com/what-is-pcoip)
51. VMware Horizon 2206. PCoIP build-to-lossless. [link](https://docs.vmware.com/en/VMware-Horizon/2206/horizon-remote-desktop-features/GUID-FB9598F4-EBBD-4CFF-B24E-CF9D18E729CD.html)
52. FFmpeg wiki. Capture/Desktop. [link](https://trac.ffmpeg.org/wiki/Capture/Desktop?version=22)
53. OBS. Almost lossless local recording. [link](https://obsproject.com/forum/resources/almost-lossless-local-recoding-using-obs-studio.200/)
54. OBS forum. Best color settings for x264 qp=0. [link](https://obsproject.com/forum/threads/best-color-settings-for-x264-qp-0-lossless-recording.49456/)
55. Libav-user (2012). Lossless encoding: red color sharpness. [link](https://ffmpeg.org/pipermail/libav-user/2012-May/002104.html)
56. GitHub gist. gdigrab lossless recording. [link](https://gist.github.com/JanTvrdik/8adfb7de7b5d9c2ca0aac43777adf3a1)
57. FFmpeg-user (2014). Best codec for screen recording. [link](https://ffmpeg.org/pipermail/ffmpeg-user/2014-November/024217.html)
58. FFmpeg wiki. Encode/H.264. [link](https://trac.ffmpeg.org/wiki/Encode/H.264?version=87)
59. Robitza (2017). CRF guide. [link](https://slhck.info/video/2017/02/24/crf-guide.html)
60. Microsoft Learn. H.264 video encoder. [link](https://learn.microsoft.com/en-ie/Windows/Win32/medfound/h-264-video-encoder)
61. Microsoft Learn. H.265/HEVC video encoder. [link](https://learn.microsoft.com/en-us/windows/win32/medfound/h-265---hevc-video-encoder)
62. Intel. HEVC chroma support. [link](https://www.intel.com/content/www/us/en/support/articles/000057555/graphics.html)
63. Intel community. H.264 profiles supported by Quick Sync. [link](https://community.intel.com/t5/Media-Intel-Video-Processing/h-264-profiles-supported-by-Quick-Sync/td-p/1029859)
64. OBS forum. QuickSync 4:4:4 chroma encoding not working. [link](https://obsproject.com/forum/threads/quicksync-4-4-4-chroma-encoding-not-working.176685/post-651371)
65. NVIDIA. NVENC application note (SDK 12.2). [link](https://docs.nvidia.com/video-technologies/video-codec-sdk/12.2/nvenc-application-note/index.html)
66. NVIDIA forum. YUV444 HEVC encoding in D3D12. [link](https://forums.developer.nvidia.com/t/yuv44-hevc-encoding-in-d3d12/197834)
67. NVIDIA. Video Codec SDK 13.0 (Blackwell). [link](https://developer.nvidia.com/blog/nvidia-video-codec-sdk-13-0-powered-by-nvidia-blackwell/)
68. Microsoft Research. Verification testing of the compression performance of HEVC SCC. [link](https://www.microsoft.com/en-us/research/?p=798757)
69. Overview of HEVC extensions on screen content coding (APSIPA). [pdf](https://www.cambridge.org/core/services/aop-cambridge-core/content/view/E12DCF69B0A59DD269EFE14CD1AFBFBE/S2048770315000116a.pdf/overview-of-hevc-extensions-on-screen-content-coding.pdf)
70. AOMedia. AV1 tool description v11. [pdf](https://aomedia.org/docs/AV1_ToolDescription_v11-clean.pdf)
71. libaom. Algorithm description. [link](https://aomedia.googlesource.com/aom/+show/HEAD/doc/AlgorithmDescription.md)
72. SVT-AV1. Version history. [link](https://videohelp.com/software/SVT-AV1/version-history)
73. FFmpeg-devel (2024). libavcodec/qsvenc: Screen Content Tool for AV1. [link](https://ffmpeg.org/pipermail/ffmpeg-devel/2024-September/333792.html)
74. ITU Journal ICT Discoveries 3(1), 2020. [link](https://itu.int/en/publications/Documents/tsb/2020-ITU_Journal-ICT_Discoveries/Vol3-No1/files/basic-html/page99.html)
75. GDELT. At-scale OCR of TV news: frame quality levels. [link](https://blog.gdeltproject.org/?p=22550)
76. US patent 8064516. Text recognition during video compression. [link](https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/8064516)
77. Scitepress (2014). Quality assessment of compressed video for ALPR. [link](https://www.scitepress.org/Papers/2014/46712/)
78. Forasoft. PaddleOCR text detection in video (vendor). [link](https://www.forasoft.com/learn/ai-for-video-engineering/articles-ai/paddleocr-text-detection-video)
79. OCR-D. Quality assurance (CER definition). [link](https://ocr-d.de/en/spec/ocrd_eval)
80. Towards Data Science. Evaluating OCR with CER and WER. [link](https://towardsdatascience.com/evaluating-ocr-output-quality-with-character-error-rate-cer-and-word-error-rate-wer-853175297510)
81. OCR improves MT for low-resource languages (Holley scale). [arXiv:2202.13274](https://arxiv.org/pdf/2202.13274)
82. Speed Demos Archive. Lossless codecs comparison. [link](https://kb.speeddemosarchive.com/Lossless_Codecs_Comparison)
83. RFC 9043 (2021). FFV1 video coding format. [link](https://www.rfc-editor.org/rfc/rfc9043)
84. FFmpeg-devel (2022). FFV1 on screen content. [link](https://ffmpeg.org/pipermail/ffmpeg-devel/2022-June/297414.html)
85. Google Research. Benchmarking JPEG XL lossy/lossless image compression. [link](https://research.google/pubs/benchmarking-jpeg-xl-lossylossless-image-compression/)
86. LogRocket. Image format comparison (screenshot test). [link](https://blog.logrocket.com/?p=215712)
87. The JPEG XL image coding system. [arXiv:2506.05987](https://arxiv.org/pdf/2506.05987)
88. Analysis and enhancement of lossless image compression in JPEG XL. [arXiv:2404.19755](https://arxiv.org/pdf/2404.19755)
89. MSU (2007). Lossless video codecs comparison. [pdf](https://www.compression.ru/video/codec_comparison/pdf/msu_lossless_codecs_comparison_2007_eng.pdf)
90. Microsoft Learn. Direct3D11CaptureFramePool class. [link](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframepool)
91. Microsoft Learn. Direct3D11CaptureFramePool.FrameArrived event. [link](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframepool.framearrived)
92. Microsoft Learn. Direct3D11CaptureFrame.SystemRelativeTime. [link](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframe.systemrelativetime?view=winrt-26100)
93. Microsoft Learn. Screen capture (Windows.Graphics.Capture). [link](https://learn.microsoft.com/en-us/windows/uwp/audio-video-camera/screen-capture)
94. Microsoft Learn. Tutorial: using the Sink Writer to encode video. [link](https://learn.microsoft.com/en-gb/windows/win32/medfound/tutorial--using-the-sink-writer-to-encode-video)
95. Microsoft Learn. Time stamps and durations. [link](https://learn.microsoft.com/en-ie/Windows/Win32/medfound/time-stamps-and-durations)
96. Microsoft Learn. MPEG-4 file sink. [link](https://learn.microsoft.com/en-ie/Windows/Win32/medfound/mpeg-4-file-sink)
97. Microsoft Q&A. Media Foundation generates a bad MP4 when fed frames too quickly. [link](https://docs.microsoft.com/answers/questions/349726/media-foundation-generates-a-bad-mp4-video-when-it.html)
98. OBS blog. Writing an MP4 muxer for fun and profit (hybrid MP4). [link](https://obsproject.com/cs/blog/obs-studio-hybrid-mp4)
99. OBS knowledge base. Hybrid MP4 format. [link](https://obsproject.com/kb/hybrid-mp4)
100. OBS forum. How to fix MP4/MOV files corrupting when OBS crashes. [link](https://obsproject.com/forum/resources/how-to-fix-mp4-mov-files-corrupting-when-obs-studio-crashes.1293)
101. OneStream. MKV vs MP4 for pre-recorded streaming. [link](https://onestream.live/blog/mkv-vs-mp4-for-pre-recorded-streaming/)
102. Adobe community. FAQ: issues with screen-recorded footage in Premiere Pro (VFR). [link](https://community.adobe.com/t5/premiere-pro-discussions/faq-issues-with-screen-recorded-footage-in-premiere-pro/td-p/10874355)
103. VideoHelp forum. VFR recordings and audio sync. [link](https://forum.videohelp.com/showthread.php?p=2586137)
104. AWS Media blog. Back to basics: GOPs explained. [link](https://aws.amazon.com/blogs/media/part-1-back-to-basics-gops-explained)
105. ZimaSpace. Why long-GOP video slows seeking. [link](https://shop.zimaspace.com/blogs/tech-ai-hub/long-gop-video-slow-seeking-home-media-server)
106. Transloadit. Keyframe interval. [link](https://transloadit.com/glossary/keyframe-interval/)
107. OBS knowledge base. Hardware encoding. [link](https://obsproject.com/kb/Hardware-Encoding)
108. Wowza. Hardware vs software streaming encoders. [link](https://www.wowza.com/?p=23557)
109. OBS forum. NVENC vs x264 for local recording. [link](https://obsproject.com/forum/threads/nvenc-vs-x264-for-local-recording-some-thoughts.56737/latest)
110. Google Patents. US9153017B1, system and method for optimized chroma subsampling. [link](https://patents.google.com/patent/US9153017B1/en)
111. Google Patents. US8064516B2, text recognition during video compression. [link](https://patents.google.com/patent/US8064516B2/en)
112. Wikipedia. YCoCg. [link](https://en.wikipedia.org/wiki/YCoCg)
113. Frame.io workflow guide. Mezzanine workflow. [link](https://workflow.frame.io/guide/mezzanine-workflow)
114. Lowepost. The ultimate DNxHR and ProRes guide. [link](https://lowepost.com/finishing/insights/dnxhr-vs-prores-r4/)
115. OWC. Video editing codecs explained. [link](https://www.owc.com/blog/video-editing-codecs-explained)
116. ProVideo Coalition. Why ProRes. [link](https://www.provideocoalition.com/why-prores/)
117. Streaming Media. How Netflix pioneered per-title video encoding optimization. [link](https://streamingmedia.com/Articles/Editorial/Featured-Articles/How-Netflix-Pioneered-Per-Title-Video-Encoding-Optimization-108547.aspx)
118. Mux (2018). Instant per-title encoding (webinar). [link](https://www.mux.com/webinars/instant-per-title-encoding)
119. Convex hull prediction methods for bitrate ladder construction. [arXiv:2310.15163](https://arxiv.org/html/2310.15163v3)
120. Bross et al. (2021). Overview of the Versatile Video Coding (VVC) standard and its applications. IEEE TCSVT 31(10):3736–3764. [link](https://www.microsoft.com/en-us/research/?p=798226)
121. Amestoy et al. (2023). Video quality assessment and coding complexity of VVC. [arXiv:2310.13093](https://arxiv.org/pdf/2310.13093)
122. ITU-R BT.1359-1 (1998). Relative timing of sound and vision for broadcasting. [pdf](https://www.itu.int/dms_pubrec/itu-r/rec/bt/R-REC-BT.1359-1-199811-I!!PDF-E.pdf)
123. NTIA report 11-474. Audio-video synchronization. [pdf](https://its.ntia.gov/publications/download/11-474.pdf)
124. ITU-R Report BT.2390-8 (2020). High dynamic range television. [pdf](https://www.itu.int/dms_pub/itu-r/opb/rep/R-REP-BT.2390-8-2020-PDF-E.pdf)
125. AJA (2019). High dynamic range broadcasting. [pdf](https://www.aja.com/pdf/2019/High_Dynamic_Range_Broadcasting.pdf)
126. Wikipedia. Hybrid log–gamma. [link](https://wikipedia.com/wiki/HLG10)
127. OWC. Online vs offline editing: why you still need proxy videos. [link](https://www.owc.com/blog/online-vs-offline-editing-why-you-still-need-to-consider-proxy-videos)
128. Videomaker. Getting to know offline editing. [link](https://www.videomaker.com/article/c3/17050-getting-to-know-offline-editing)
129. Frame.io. C2C complete proxy workflow guide. [link](https://help.frame.io/en/articles/6079079-c2c-complete-proxy-workflow-guide)
130. Frame.io workflow guide. Premiere Pro proxies. [link](https://workflow.frame.io/guide/premiere-pro-proxies)
131. Adobe. Convert online clips to offline clips (Premiere Pro). [link](https://helpx.adobe.com/uk/premiere/desktop/organize-media/ingest-proxy-workflow/convert-online-clips-to-offline-clips.html)
132. Streaming Learning Center. Single-pass vs two-pass encoding. [link](https://streaminglearningcenter.com/?p=18071)
133. YouTube Help. Recommended upload encoding settings. [link](https://support.google.com/youtube/answer/1722171)
134. VideoHelp forum. CRF vs 2-pass for upload. [link](https://forum.videohelp.com/showthread.php?p=2522857)
135. Netflix Partner Help. What is color management. [link](https://backlothelp.netflix.com/hc/en-us/articles/360025502033-What-is-Color-Management)
136. ARRI. About ACES. [link](https://www.arri.com/en/learn-help/learn-help-camera-system/image-science/about-aces)
137. Videomaker. A guide to working with LUTs. [link](https://www.videomaker.com/article/c01/18793-a-guide-to-working-with-luts)
138. Mixing Light. ACES from camera to completion. [link](https://mixinglight.com/color-grading-tutorials/aces-from-camera-to-completion/)
139. Understanding ACES scene-referred workflows (2025). [link](https://atulsingh-nikki.github.io/obsidian-notes/2025/01/10/understanding-aces-scene-referred-workflows.html)
140. EBU R 128 v4. Loudness normalisation and permitted maximum level. [pdf](https://tech.ebu.ch/docs/r/r128v4_0.pdf)
141. EBU R 128 s2. Loudness in streaming. [pdf](https://knowledgehub.ebu.ch/files/live/sites/tech/files/shared/r/r128s2v2_0.pdf)
142. ITU-R Report BS.2434 (2018). [pdf](https://www.itu.int/dms_pub/itu-r/opb/rep/R-REP-BS.2434-2018-PDF-E.pdf)
143. Wikipedia. EBU R 128 (platform targets table). [link](https://en.wikipedia.org/wiki/EBU_R_128)
144. FFmpeg wiki. Encode/HighQualityAudio. [link](https://trac.ffmpeg.org/wiki/Encode/HighQualityAudio?version=78)
145. The Broadcast Bridge. Standards for audio coding. [link](https://thebroadcastbridge.com/content/entry/20283/standards-part-5-standards-for-audio-coding)
146. Hanselman. The guide to creating quality technical screencasts. [link](https://www.hanselman.com/blog/the-guide-to-creating-quality-technical-screencasts)
147. TechSmith. Screen recording basics: video editing, part 2. [link](https://www.techsmith.com/learn/tutorials/academy/screen-recording-basics-video-editing-part-2/)
148. Smashing Magazine (2010). Most common mistakes in screencasting. [link](https://www.smashingmagazine.com/2010/12/most-common-mistakes-in-screencasting/)
149. InformIT. Creating a screencast, part 2: recording tips. [link](https://informit.com/articles/article.aspx?p=1762611)
150. Cloudinary. AAC vs Opus. [link](https://cloudinary.com/guides/video-formats/aac-vs-opus)

Internal sources:

- `docs/clip-recorder-experiments-plan.md`
- `docs/clip-recorder-experiments-results.md`
- `scripts/clip-equation/results/d-runs.csv`, `z-runs.csv` (commit `2ff3db5a`)
- `docs/clip-recorder-plan-4.2.md` (`a25eb968`)
- Gitea adam/synapse #290
