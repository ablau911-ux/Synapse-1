# Experiments step 1: the corpus (docs/clip-recorder-experiments-plan.md,
# "Acceptance (BDD)", step 1). One scenario per row of the step-1 table.
# Checked by: node scripts/clip-equation/spec/run-spec.mjs step1
# The runner matches each scenario by its title, computes the equation from
# the inputs in scripts/clip-equation/corpus/corpus.json, reads the outcome
# from the corpus folders and compares both with the "known:" value.
# Plan 1336612e: the encoder row is checked in step 2 for way D and prints
# "step 2"; frames are stored at the window size (sigma = 1 for way Z; the
# product's sigma for way D is recorded in each folder's meta.json).

Feature: Step 1, a complete and exactly repeatable frame corpus
  Every scenario page is rendered by headless Chrome for t = 20 s, one
  screenshot per 50 ms instant of virtual time, stored as zstd -3 B,G,R
  frames at the window size, with sigma recorded beside them.

  Background:
    Given the inputs in corpus/corpus.json: t = 20 s, f = 20 fps, 6 scenarios, 4 window sizes
    And the corpus folders in the corpus root, one per scenario and window size

  Scenario: list encoders
    Given results/encoders.json written by clip-probe exp-encoders
    When the entries are grouped by distinct (codec, name, vendor)
    Then equation: checked in step 2 for way D (activation of every H.264 encoder)
    And known: –

  Scenario: capture folders
    Given 6 scenarios and 4 window sizes
    When the corpus root is listed
    Then equation: folders = scenarios * sizes
    And known: 24

  Scenario: capture instants
    Given any corpus folder
    When its frames.jsonl lines are counted
    Then equation: N_inst = t * f
    And known: 400

  Scenario: lost instants
    Given any corpus folder
    When new and repeat instants are counted
    Then equation: N_lost = N_inst - N_new - N_rep
    And known: 0

  Scenario: frame size
    Given every stored frame of every folder
    When it is zstd-decompressed
    Then equation: bytes(frame) = w * h * 3 at the window size, sigma = 1
    And known: 1920x1080=6220800; 1280x720=2764800; 640x360=691200; 506x333=505494

  Scenario: repeats
    Given any corpus folder
    When its frame files and repeat lines are read
    Then equation: stored files = N_new, and every repeat names an earlier stored frame
    And known: files = N_new

  Scenario: still page
    Given the still folders
    When their new frames are counted
    Then equation: N_new(still) = 1
    And known: 1

  Scenario: flash page
    Given the flash folders
    When the flash box is tested pixel by pixel in every instant's frame
    Then equation: flashes = t * 1 per s
    And known: 20

  Scenario: repeatability
    Given typing at 1280x720 rendered twice
    When the two frames of every instant are compared
    Then equation: differing instants = count(frame_a(j) != frame_b(j))
    And known: 0

  Scenario: waiting bytes
    Given any corpus folder
    When the zstd bytes in frames.jsonl and the frame file sizes on disk are summed
    Then equation: D_wait = sum of zstd(L = -3) bytes of the stored frames
    And known: equals the folder's file sizes summed, to the byte
