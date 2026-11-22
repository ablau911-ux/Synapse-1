# Clip experiments, step 1: corpus

Written by `node scripts/clip-equation/corpus/render.mjs` from the capture folders in `C:\Users\Adam\clip-corpus\` (not in the repository). Plan: docs/clip-recorder-experiments-plan.md, F6, F7, Procedure step 1. Checked by `node scripts/clip-equation/spec/run-spec.mjs step1`.

Each capture: headless Chrome (`--headless=new`, own temporary profile, no window), virtual time advanced 50 ms per instant over the DevTools protocol, one lossless PNG screenshot per instant at the window size, device scale 1; t = 20 s at f = 20 instants (N_inst = 400). A frame equal in every pixel to the previous one is a repeat. New frames are stored at the window size as zstd level −3 of B,G,R, one file per new frame; `frames.jsonl` has one line per instant (its `sha256` is the first 16 hex digits of SHA-256 of the B,G,R bytes); `meta.json` records σ and the encoded size σ · window. n_new = new frames per second; bytes/frame = mean zstd −3 bytes of the stored frames; D_wait = their sum; lost = instants without a frame (always 0: every instant has a screenshot); time ok = `Date.now()` in the page equals s_j at the first and last instant, and on the flash page its counter shows j at every instant; render s = wall time of the capture.

Total corpus size on disk: 4014133606 bytes (3828.2 MiB).

| folder | scenario | window | σ | encoded | instants | N_new | N_rep | n_new (1/s) | zstd −3 bytes/frame | D_wait bytes | lost | time ok | render s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| flash-1280x720 | flash | 1280x720 | 1 | 1280x720 | 400 | 400 | 0 | 20.00 | 12300 | 4920183 | 0 | yes | 27.3 |
| flash-1920x1080 | flash | 1920x1080 | 1 | 1920x1080 | 400 | 400 | 0 | 20.00 | 12929 | 5171687 | 0 | yes | 34.4 |
| flash-506x333 | flash | 506x333 | 3 | 1518x999 | 400 | 400 | 0 | 20.00 | 4316 | 1726353 | 0 | yes | 20.5 |
| flash-640x360 | flash | 640x360 | 2 | 1280x720 | 400 | 400 | 0 | 20.00 | 4349 | 1739618 | 0 | yes | 20.7 |
| scroll-1280x720 | scroll | 1280x720 | 1 | 1280x720 | 400 | 400 | 0 | 20.00 | 80590 | 32235932 | 0 | yes | 27.7 |
| scroll-1920x1080 | scroll | 1920x1080 | 1 | 1920x1080 | 400 | 400 | 0 | 20.00 | 124194 | 49677690 | 0 | yes | 40.9 |
| scroll-506x333 | scroll | 506x333 | 3 | 1518x999 | 400 | 400 | 0 | 20.00 | 21036 | 8414320 | 0 | yes | 20.7 |
| scroll-640x360 | scroll | 640x360 | 2 | 1280x720 | 400 | 400 | 0 | 20.00 | 28409 | 11363526 | 0 | yes | 20.7 |
| still-1280x720 | still | 1280x720 | 1 | 1280x720 | 400 | 1 | 399 | 0.05 | 15218 | 15218 | 0 | yes | 20.0 |
| still-1920x1080 | still | 1920x1080 | 1 | 1920x1080 | 400 | 1 | 399 | 0.05 | 16613 | 16613 | 0 | yes | 26.5 |
| still-506x333 | still | 506x333 | 3 | 1518x999 | 400 | 1 | 399 | 0.05 | 12232 | 12232 | 0 | yes | 20.0 |
| still-640x360 | still | 640x360 | 2 | 1280x720 | 400 | 1 | 399 | 0.05 | 14671 | 14671 | 0 | yes | 20.3 |
| typing-1280x720 | typing | 1280x720 | 1 | 1280x720 | 400 | 143 | 257 | 7.15 | 4444 | 635555 | 0 | yes | 23.2 |
| typing-1280x720-repeat | typing | 1280x720 | 1 | 1280x720 | 400 | 143 | 257 | 7.15 | 4444 | 635555 | 0 | yes | 22.9 |
| typing-1920x1080 | typing | 1920x1080 | 1 | 1920x1080 | 400 | 143 | 257 | 7.15 | 4938 | 706122 | 0 | yes | 30.2 |
| typing-506x333 | typing | 506x333 | 3 | 1518x999 | 400 | 143 | 257 | 7.15 | 3996 | 571359 | 0 | yes | 20.5 |
| typing-640x360 | typing | 640x360 | 2 | 1280x720 | 400 | 143 | 257 | 7.15 | 4089 | 584757 | 0 | yes | 20.8 |
| ui-1280x720 | ui | 1280x720 | 1 | 1280x720 | 400 | 400 | 0 | 20.00 | 32012 | 12804788 | 0 | yes | 27.4 |
| ui-1920x1080 | ui | 1920x1080 | 1 | 1920x1080 | 400 | 400 | 0 | 20.00 | 33385 | 13354072 | 0 | yes | 34.1 |
| ui-506x333 | ui | 506x333 | 3 | 1518x999 | 400 | 372 | 28 | 18.60 | 23465 | 8728929 | 0 | yes | 20.7 |
| ui-640x360 | ui | 640x360 | 2 | 1280x720 | 400 | 400 | 0 | 20.00 | 24927 | 9970857 | 0 | yes | 20.3 |
| video-1280x720 | video | 1280x720 | 1 | 1280x720 | 400 | 400 | 0 | 20.00 | 2625655 | 1050262003 | 0 | yes | 74.4 |
| video-1920x1080 | video | 1920x1080 | 1 | 1920x1080 | 400 | 400 | 0 | 20.00 | 5826182 | 2330472941 | 0 | yes | 123.2 |
| video-506x333 | video | 506x333 | 3 | 1518x999 | 400 | 400 | 0 | 20.00 | 498659 | 199463514 | 0 | yes | 41.5 |
| video-640x360 | video | 640x360 | 2 | 1280x720 | 400 | 400 | 0 | 20.00 | 673429 | 269371540 | 0 | yes | 41.9 |
