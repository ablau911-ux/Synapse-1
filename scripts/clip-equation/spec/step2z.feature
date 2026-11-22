# Experiments step 2 and 3, way Z (zstd alone): compress, decompress, verify.
# Spec: docs/clip-recorder-experiments-plan.md, "Acceptance (BDD)", the Z rows of
# step 2 and step 3; equations 1 to 6; ways of docs/clip-recorder-ways.md.
# Checked by scripts/clip-equation/spec/step2z.mjs against
# scripts/clip-equation/results/z-runs.csv (one row per `clip-probe exp-z` run).
# Every check computes its known result from its inputs before it reads the outcome.
# Symbols: px = w · h; N_new = new frames of the corpus folder; L = zstd level;
# K_z = keyframe interval; B_c, B_d = frames held while compressing, decoding.

Feature: Way Z, every zstd way compresses the corpus losslessly with known bytes, CPU and RAM

  Background:
    Given the corpus folder C:\Users\Adam\clip-corpus\<scenario>-<w>x<h>
    And N_new = the count of lines with "new": true in its frames.jsonl
    And D_wait = the sum of the sizes on disk of its frame-NNNN.zst files
    And each row of z-runs.csv is one exp-z run of one way, level and option set on that folder

  # Step 2, compress

  Scenario: frames compressed
    When a way compresses the folder
    Then N_comp = N_new of the folder, exactly

  Scenario: bytes
    When a way writes its clip files to the scratch folder
    Then bytes = the sum of the output sizes the way produced
    And bytes = the sum of the clip's file sizes on disk, to the byte

  Scenario: baseline equals the corpus
    Given way ZW1, whole frame, L = -3, independent frames, interleaved B,G,R, nbWorkers 0
    When it compresses the folder
    Then bytes = D_wait of the folder, to the byte (same coder, level and frames)
    # Checked on the product's call (one-shot ZSTD_compress2, drain=0). Rows with drain=16384
    # reproduce the corpus writer's call (Node zlib.zstdCompressSync) and are printed as
    # evidence of the cause when the row misses; they never make it pass.

  Scenario: determinism
    Given one configuration run 3 times (repeat 1, 2 and 3)
    Then bytes of run 1 = run 2 = run 3

  Scenario: RAM while compressing (equation 4)
    Given M_0 = private bytes of the run's process before the first frame
    And M_cctx = zstd's own estimate (ZSTD_estimateCCtxSize_usingCCtxParams, or ZSTD_estimateCStreamSize_usingCCtxParams for one stream)
    And B_c = 1 for independent frames, 2 for ways that keep the previous frame
    Then M_comp = M_0 + M_cctx + B_c · px · 3 is within ±10 % of the measured peak private bytes

  Scenario: CPU (equation 3)
    Given κ_c fitted on the fit set of the full run (step 4)
    Then held-out rows are within ±10 % of C_comp = N_new · (c_0 + px · κ_c)
    # Pending until the full run and the fit; the pilot only reports C_comp per frame.

  Scenario: run time (equation 6)
    Given T = Σ_runs N_new · px · 3 / v(L, m) · repeats from the pilot
    Then the full run's wall time is within ±20 % of T
    # Pending until the full run; z-pilot.md gives T.

  # Step 3, decompress and verify

  Scenario: round trip
    When the clip is decoded frame by frame and compared with the corpus
    Then Σ |decoded − corpus| over every pixel of every frame = 0

  Scenario: seek cost
    When the worst frame of the clip is shown alone
    Then N_dec(worst) = 1 for independent frames (ZW0 to ZW6, ZW16, ZW17)
    And N_dec(worst) <= K_z for the delta, tile, rectangle, prefix, stream and switch ways

  Scenario: decoder RAM (equation 4)
    Given M_dctx = zstd's own estimate (ZSTD_estimateDCtxSize, or ZSTD_estimateDStreamSize for one stream)
    And B_d = 1 for independent frames, 2 for ways that keep the previous frame
    And b_max = the largest clip file, the compressed frame the viewer holds while it decodes (a term added to equation 4)
    Then M_dec = M_0 + M_dctx + B_d · px · 3 + b_max is within ±10 % of the measured peak private bytes of the decode phase

  Scenario: time until ready (equation 5)
    When the last frame has arrived
    Then t_ready = t_close, closing the last file and clip.json, is at most one instant, 50 ms
