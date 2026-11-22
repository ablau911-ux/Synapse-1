# Experiments step 1: the corpus (docs/clip-recorder-experiments-plan.md,
# "Acceptance (BDD)", step 1). One scenario per row of the step-1 table.
# Checked by: node scripts/clip-equation/spec/run-spec.mjs step1
# The runner matches each scenario by its title, computes the equation from
# the inputs in scripts/clip-equation/corpus/corpus.json, reads the outcome
# from the corpus folders and compares both with the "known:" value.
# Owner, 2026-11-22 21:30: zstd is the only coder; the encoder rows are kept
# for the record and print "not needed: zstd only".

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
    Then equation: unique encoders = |{(codec, name, vendor)}|, each with a name and a hardware flag
    And known: not needed: zstd only

  Scenario: activate encoders
    Given every unique encoder
    When IMFActivate::ActivateObject is called on it
    Then equation: activated = unique encoders
    And known: not needed: zstd only

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
    Then equation: bytes(frame) = w_win * h_win * 3
    And known: 1920x1080=6220800; 1280x720=2764800; 640x360=691200; 506x333=505494

  Scenario: encoded size
    Given the meta.json of every folder
    When its recorded size is read
    Then equation: (w, h) = sigma * (w_win, h_win)
    And known: 640x360=1280x720; 506x333=1518x999

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
