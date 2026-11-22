# Clip experiments, step 1: corpus

Written by `clip-probe exp-corpus` (scripts/clip-equation/probe/src/exp_corpus.rs) from the capture folders in `C:\Users\Adam\clip-corpus\` (not in the repository). Plan: docs/clip-recorder-experiments-plan.md, F6, F7, Procedure step 1.

Each capture: t = 20 s at f = 20 instants (N_inst = 400); frames stored at the window size as zstd level −3 of B,G,R, one file per new frame; `frames.jsonl` has one line per instant. Recorded size = σ · stored size; the encode step scales by σ. n_new = new frames per second; bytes/frame = mean zstd −3 bytes of the stored frames; D_wait = their sum; lost = instants without any frame; delivered = frames Windows Graphics Capture delivered.

| folder | scenario | window | stored | recorded | σ | instants | N_new | N_rep | n_new (1/s) | zstd −3 bytes/frame | D_wait bytes | lost | flashes | delivered |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| typing-1280x720 | typing | 1280x720 | 1280x720 | 1280x720 | 1 | 400 | 160 | 240 | 8.00 | 5589 | 894163 | 0 | - | 163 |
| typing-1280x720-repeat | typing | 1280x720 | 1280x720 | 1280x720 | 1 | 400 | 160 | 240 | 8.00 | 5582 | 893063 | 0 | - | 163 |
| typing-1920x1080 | typing | 1920x1080 | 1920x1080 | 1920x1080 | 1 | 400 | 160 | 240 | 8.00 | 6101 | 976122 | 0 | - | 163 |
