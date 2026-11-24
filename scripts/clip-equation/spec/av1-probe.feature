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
    # Equation: every scene: instant k is marked iff pixels(k)!=pixels(k-1) (instant 0 and the instant after a lost one count as a change) and pixels(k+1..k+9)=pixels(k) (settle=10 instants on screen, the changed one included); the mark is still:true when no earlier still holds the same pixels, else still_of=<instant of that still>; mix corpus settle points >= 1 | Known: true
  Scenario: unchanged instants cost nothing
    # Equation: same iff pixels(k)=pixels(k-1) (SHA-256 of the corpus B,G,R bytes) and instant k-1 is not lost; instant 0 is enc; an instant the corpus marks lost is lost | Known: true
  Scenario: a resized window keeps recording
    # Equation: scene whose window size changes: lost=0; the first instant at a new size is enc [part,0] of a new part; the last encoded instant of each size comes back from the product decoder at that size | Known: true
  Scenario: a picture after a lost instant is recorded again
    # Equation: every scene: the first instant with a picture after a lost instant is enc, never same; it and a later same instant come back from the product decoder as pictures | Known: true
  Scenario: a torn last part does not stop recovery
    # Equation: killed real-page recording, last part cut to 10 bytes: product recovery succeeds; every picture of the earlier parts stays enc and comes back as video; every picture of the torn part is named lost | Known: true
  Scenario: recovery keeps a complete still
    # Equation: killed text-page recording, its part cut to the 32-byte header: after product recovery the still file exists and its instant (and the same instant after it) comes back as exact still | Known: true
  Scenario: a lost line names no missing still
    # Equation: killed text-page recording, its part cut to the 32-byte header and the file of its first still deleted: after product recovery that instant is lost and no index line has still or still_of naming the deleted file | Known: true
  Scenario: recovery names every planned instant
    # Equation: killed text-page recording with end_ms 25 ms off the 50 ms grid: index lines after recovery = ceil(end_ms/50), the last one named lost | Known: true
  Scenario: a late picture comes back in time
    # Equation: product decoder (clip-probe exp-av1-fetch) on the recorded real-page clip, the enc line with the largest index-in-part: total_ms < 500, pictures_decoded = index+1, kind = video | Known: true
  Scenario: the fetched picture is the recorded one
    # Equation: sha256(RGBA of the fetched picture) = reference hash, for the first, the deepest and the last encoded instant of the recorded real-page clip | Known: true
  Scenario: read-back
    # Equation: ffmpeg fetch of one picture (first, middle, last enc instant and the enc line with the largest index-in-part, per scene) < 500 ms | Known: true
