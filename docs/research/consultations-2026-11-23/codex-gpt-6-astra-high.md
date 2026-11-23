Model: **GPT-6 (Codex).**

1. **Direction sensible; hybrid not yet justified.**

Temporal compression is the strongest next step. Region classification, two codecs, and lossless refresh are premature.

The literature study optimizes for **exact pixels**; your goal says **readable text**. Those are different requirements. The OCR noise experiment invalidates `R = 1` as the gate used to reject H.264 in plan 4.2. It does **not** prove H.264 looks acceptable; that needs direct inspection.

Remote-desktop designs offer useful techniques, but optimize interactive transmission. Recordings additionally need seeking, portability, and faithful playback of earlier events. A lossless refresh after motion stops cannot restore text lost during motion.

My recommendation: **benchmark one simple temporal zstd format against whole-window Quick Sync before committing to hybrid.** If ordinary MP4 is essential and Quick Sync text passes inspection, Quick Sync is the simpler product.

2. **Simplest design: one compression path, bounded history.**

For a Synapse-native clip, start with:

- Initial full RGB frame.
- Previous-frame prediction plus fast zstd; benchmark the already-tested prefix method before assuming tiles win.
- Skip identical images; preserve timestamps, durations, and cursor behavior.
- Periodic full frames plus an index for bounded seeking and recovery.
- Fixed buffer limits; explicitly record missed frames.

Changed tiles are a candidate implementation, not a requirement. Batch changed tiles into one compressed payload per update; tiny independently compressed tiles can sacrifice compression and add overhead.

Keep whole-frame zstd as the baseline. At **6,668 MB/min for video**, it cannot satisfy the general “small file” goal.

3. **Drop, add, change; main risks.**

**Drop initially:** video-region detection, palettes, codec switching, lossless refresh, and exhaustive parameter sweeps.

**Add:** seek/recovery design, total resource accounting, and an explicit decision about custom clips versus standard video files.

**Change:** treat WGC dirty regions as an optional optimization with a verified fallback. The literature report itself says unchanged-frame suppression is uncertain; its “idle cost is zero” claim is therefore unsupported.

Main risks:

- Scrolling changes nearly every fixed tile. Temporal prefix matching may beat same-position tiles; motion copying adds complexity.
- Video remains expensive in lossless formats. If common, test whole-window hardware encoding before region hybrid.
- “Changes constantly” also describes scrolling text, terminals, and subtitles.
- Hybrid retains encoder overhead; smaller regions do not guarantee proportionally smaller RAM.
- Published ~12 MB and ~164 MB figures need matching accounting boundaries. Include capture textures, buffers, queues, worker, and GPU memory.
- Earlier Quick Sync runs had **228 frame-mapping failures**. Timing correctness needs fixing before judging that path production-ready.

4. **Next measurement: small, decisive comparison.**

Reuse four **20-second 1080p** scenes: typing, UI, scrolling, video. Three repetitions each; four candidates: whole-frame zstd −3, prefix zstd, changed-tile zstd, Quick Sync q70. **48 runs**, no broad sweep.

Measure final bytes including indexes/checkpoints; total CPU seconds; peak RAM/GPU memory; capture/copy cost; missed frames; stop latency; seek latency.

Verify every reconstructed lossless frame against source. For H.264, inspect native-size small text, colored text, ClearType, and moving text; report crop SSIM and worst examples. OCR remains secondary, with source-noise controls. Validate real playback through Synapse.

Choose the simplest candidate meeting explicit size, memory, readability, and timing limits. Hybrid earns its complexity only if simpler candidates fail.

Read-only consultation; no edits, builds, or commits.
hook: Stop
hook: Stop
hook: Stop Completed
hook: Stop Failed
tokens used
54,462
Model: **GPT-6 (Codex).**

1. **Direction sensible; hybrid not yet justified.**

Temporal compression is the strongest next step. Region classification, two codecs, and lossless refresh are premature.

The literature study optimizes for **exact pixels**; your goal says **readable text**. Those are different requirements. The OCR noise experiment invalidates `R = 1` as the gate used to reject H.264 in plan 4.2. It does **not** prove H.264 looks acceptable; that needs direct inspection.

Remote-desktop designs offer useful techniques, but optimize interactive transmission. Recordings additionally need seeking, portability, and faithful playback of earlier events. A lossless refresh after motion stops cannot restore text lost during motion.

My recommendation: **benchmark one simple temporal zstd format against whole-window Quick Sync before committing to hybrid.** If ordinary MP4 is essential and Quick Sync text passes inspection, Quick Sync is the simpler product.

2. **Simplest design: one compression path, bounded history.**

For a Synapse-native clip, start with:

- Initial full RGB frame.
- Previous-frame prediction plus fast zstd; benchmark the already-tested prefix method before assuming tiles win.
- Skip identical images; preserve timestamps, durations, and cursor behavior.
- Periodic full frames plus an index for bounded seeking and recovery.
- Fixed buffer limits; explicitly record missed frames.

Changed tiles are a candidate implementation, not a requirement. Batch changed tiles into one compressed payload per update; tiny independently compressed tiles can sacrifice compression and add overhead.

Keep whole-frame zstd as the baseline. At **6,668 MB/min for video**, it cannot satisfy the general “small file” goal.

3. **Drop, add, change; main risks.**

**Drop initially:** video-region detection, palettes, codec switching, lossless refresh, and exhaustive parameter sweeps.

**Add:** seek/recovery design, total resource accounting, and an explicit decision about custom clips versus standard video files.

**Change:** treat WGC dirty regions as an optional optimization with a verified fallback. The literature report itself says unchanged-frame suppression is uncertain; its “idle cost is zero” claim is therefore unsupported.

Main risks:

- Scrolling changes nearly every fixed tile. Temporal prefix matching may beat same-position tiles; motion copying adds complexity.
- Video remains expensive in lossless formats. If common, test whole-window hardware encoding before region hybrid.
- “Changes constantly” also describes scrolling text, terminals, and subtitles.
- Hybrid retains encoder overhead; smaller regions do not guarantee proportionally smaller RAM.
- Published ~12 MB and ~164 MB figures need matching accounting boundaries. Include capture textures, buffers, queues, worker, and GPU memory.
- Earlier Quick Sync runs had **228 frame-mapping failures**. Timing correctness needs fixing before judging that path production-ready.

4. **Next measurement: small, decisive comparison.**

Reuse four **20-second 1080p** scenes: typing, UI, scrolling, video. Three repetitions each; four candidates: whole-frame zstd −3, prefix zstd, changed-tile zstd, Quick Sync q70. **48 runs**, no broad sweep.

Measure final bytes including indexes/checkpoints; total CPU seconds; peak RAM/GPU memory; capture/copy cost; missed frames; stop latency; seek latency.

Verify every reconstructed lossless frame against source. For H.264, inspect native-size small text, colored text, ClearType, and moving text; report crop SSIM and worst examples. OCR remains secondary, with source-noise controls. Validate real playback through Synapse.

Choose the simplest candidate meeting explicit size, memory, readability, and timing limits. Hybrid earns its complexity only if simpler candidates fail.

Read-only consultation; no edits, builds, or commits.
