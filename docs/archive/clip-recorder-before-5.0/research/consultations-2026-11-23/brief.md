READ-ONLY CONSULTATION. Do not edit any file, do not run builds or long scripts. Read and think, then answer.

Goal of the product (owner's words): a screen-clip recorder for Synapse (Windows, records an application window so an AI agent or a person can later see what happened). The end result wanted: a small video file, high quality (text readable), whose creation uses as little RAM and CPU as possible.

Read these (repo C:\Users\Adam\Synapse-clip):
1. docs/research/screen-clip-compression-from-scratch.md: literature-only study, 85 refs, ranking of options.
2. docs/research/clip-video-compression-research.en.md: earlier study with OUR measurements (2,808 H.264 runs via Media Foundation, partial zstd run at 506x333, experiment E1).
3. scripts/clip-equation/results/e1-nv12.json and results/ocr-noise.json: E1 (R after NV12 conversion alone) and the OCR noise test (Windows OCR reading the SOURCE with invisible ±1/±2 noise gives R 0.974-0.9998, so R<1 is within OCR noise and is not a fidelity measure).
4. docs/clip-recorder-plan-4.2.md: the current building plan (zstd -3 whole frames become the clip, no H.264 worker).

Key measured numbers at 1920x1080 per 20 s scene: zstd -3 whole frames: CPU 0.27-2.1 s, RAM ~12 MB, size 2 (typing) / 38 (ui) / 142 (scroll) / 6,668 (video) MB/min. H.264 Quick Sync q70 worker: CPU 4.5-5.1 s, RAM ~164 MB, 2.4 ui / 22 scroll / 221 video MB/min. H.264 MS software q70: CPU 2.2-13 s, RAM ~671 MB. At 506x333, zstd formats that store only changes (prefix/XOR/rect) were 7-12x smaller than whole frames.

Current proposal (Claude's synthesis): hybrid like remote-desktop protocols: WGC capture only on change + dirty regions checked by tile compare; lossless RGB tiles with fast zstd for text/UI; H.264 Quick Sync only for regions that change constantly (video/animation); lossless refresh when a region settles; measure fidelity by pixel equality and SSIM on text crops, not OCR. Build order: (a) zstd changed-tiles first, (b) video regions with H.264 later if needed.

Your task: think independently. Challenge the proposal. Answer:
1. Is this the best way to reach the goal? If not, what is, and why?
2. What is the simplest design that gets most of the benefit (the owner wants no over-complication)?
3. What would you drop, add or change, and what are the main risks?
4. What should be measured next to confirm the choice, and how (concrete, quick)?
State which model you are. Answer in under 600 words.
