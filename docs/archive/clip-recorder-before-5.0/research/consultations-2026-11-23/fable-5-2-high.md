**Model:** Claude Fable 5.2 (`claude-fable-5-2`). Read-only; nothing edited or run.

**Verdict:** stage (a) is right, but as a previous-frame prefix, not tiles. Stage (b) with H.264 should leave the plan; a byte-rate limit covers it more simply.

### 1. Is it the best way?

Half. Lossless change-only zstd is the right core, but for a different reason than the plan records.

- **The recorded reason is void.** Plan 4.2 §1 drops H.264 because R < 1. Your noise test shows R < 1 is OCR noise, so R neither convicts nor clears H.264.
- **The reason that holds is cost.** Per 20 s scene, Quick Sync used ~164 MB and 4.5–5.1 s CPU, against ~12 MB and 0.27–2.1 s for zstd.
- **Lossless removes the quality question.** Decoded = source is a yes/no check, so stage (a) needs no SSIM work.
- **Whole frames fail "small file".** At 1080p they cost 38 (UI) and 142 (scroll) MB/min, against 2.4 and 22 for Quick Sync q70. Change-only storage is the plan itself, not a later optimisation.

### 2. Simplest design

One format, one code path:

1. On each new frame, compare with the previous one and find the changed row span.
2. Compress only that span with zstd, using the same span of the previous frame as prefix.
3. Write a whole frame every few seconds so seek stays bounded.
4. Apply a byte-rate limit: when a region stays over budget for N frames, store it at a lower frame rate, then one exact frame once it settles.

Prefix beats tiles in your own data: in the 506×333 rows that ran, prefix was the smallest for typing, UI and scroll at 2.8 MB RAM. A scrolled page is the same bytes at an offset, which prefix matching finds and changed tiles do not.

### 3. Drop, add, change, risks

**Drop**
- H.264 regions: a second pipeline, NV12 conversion, a ~164 MB encoder and two decoders in the viewer.
- Tile classification, palette and colour counting.
- WGC dirty regions: a full-frame compare is cheap (my estimate: under 1 ms at 1080p).
- SSIM on text crops, until a lossy path exists.

**Add**
- Keyframes, because plan 4.2's "seek decodes one frame" no longer holds.
- The byte-rate limit from day one; without it one video costs 6.7 GB/min.
- A corrected decision line in the plan.

**Risks**
- **Prefix CPU at 1080p is unmeasured.** zstd indexes the prefix on every call (from its docs, from memory), which is why step 1 crops to the changed span.
- **The corpus is synthetic.** Real smooth scrolling re-renders text at sub-pixel offsets, so prefix matches may vanish.
- **Full-screen video stays large even when throttled lossless.** If that matters, a stateless JPEG of the busy region at ~2 fps is roughly 40–50 MB/min at 1080p (my estimate), and still far simpler than H.264.
- **The format is custom.** Sharing needs an MP4 export, paid only on request.

### 4. Measure next

- **24 rows, not 42,000.** At 1920×1080 only, run the six scenes through whole-frame −3, full-frame prefix L1, full-frame prefix L3, and changed-span + prefix L1. Report MB/min, CPU seconds and peak RAM. Pass: UI and scroll at or below 2.4 and 22 MB/min, CPU under 2 s, RAM under 40 MB.
- **Five real 60 s recordings.** Use an editor, real browser scrolling, a settings UI, a page with a spinner and a page with video. Capture raw once and replay through the same four formats. Also count the share of frames over budget; that number decides whether any lossy path is ever built.
- **Seek.** Measure decode time to a random frame with keyframes every 2 s and every 10 s.
- **Only if the busy share is material:** compare bytes for throttled lossless, throttled JPEG and Quick Sync on the video scene.
