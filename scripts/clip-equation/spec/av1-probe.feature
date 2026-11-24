Feature: AV1 window-recording probe (docs/research/window-recording/probe-plan.md, Proof)
  Scenario: every instant is accounted for
    # Equation: lines(frames.jsonl)=instants of the scene, in order, each with exactly one of enc, same or lost; probe.json encoded+same+lost=instants | Known: true
  Scenario: nothing is lost silently
    # Equation: every lost line has a non-empty cause | Known: lost=0 expected; a named loss passes and is shown
  Scenario: the video decodes
    # Equation: sum(ffprobe -count_frames nb_read_frames over video/part-*.ivf)=enc lines; codec=av1; first enc=[0,0]; every enc [part,index] exists; a new part starts at index 0 | Known: true
  Scenario: a still is exact
    # Equation: still files = still:true instants; bgr24(still-NNNNNNNN.webp)=corpus picture of instant NNNNNNNN, byte for byte; stills compared over all scenes >= 1 | Known: true
  Scenario: a still appears when the window settles
    # Equation: every scene: still:true iff pixels(k)!=pixels(k-1) (instant 0 counts as a change) and pixels(k+1..k+9)=pixels(k) (settle=10 instants on screen, the changed one included); mix corpus settle points >= 1 | Known: true
  Scenario: unchanged instants cost nothing
    # Equation: same iff pixels(k)=pixels(k-1) (SHA-256 of the corpus B,G,R bytes); instant 0 is enc | Known: true
  Scenario: read-back
    # Equation: ffmpeg fetch of one picture (first, middle, last enc instant and the enc line with the largest index-in-part, per scene) < 500 ms | Known: true
