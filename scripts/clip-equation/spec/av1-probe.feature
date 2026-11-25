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
  Scenario: each instant logs the cursor of its frame
    # Equation: every scene, exp-av1 hands the product cursor [k mod 300, k mod 200] with the picture of instant k: every index line except a failed capture has cursor = [k mod 300, k mod 200]; a failed capture's line has no cursor | Known: true
  Scenario: a torn last part does not stop recovery
    # Equation: killed real-page recording, last part cut to 10 bytes: product recovery succeeds; every picture of the earlier parts stays enc and comes back as video; every picture of the torn part is named lost | Known: true
  Scenario: recovery keeps a complete still
    # Equation: killed text-page recording, its part cut to the 32-byte header: after product recovery the still file exists and its instant (and the same instant after it) comes back as exact still | Known: true
  Scenario: a lost line names no missing still
    # Equation: killed text-page recording, its part cut to the 32-byte header and the file of its first still deleted: after product recovery that instant is lost and no lost line has still or still_of naming the deleted file | Known: true
  Scenario: a line already lost names no missing still
    # Equation: killed text-page recording whose index already holds its first still's line as lost with still:true, the still file deleted: after product recovery no lost line has still or still_of naming the deleted file, and a same line after it is lost | Known: true
  Scenario: recovery names every planned instant
    # Equation: killed text-page recording with end_ms 25 ms off the 50 ms grid: index lines after recovery = ceil(end_ms/50), the last one named lost | Known: true
  Scenario: recovery makes a failed clip agree with its pictures
    # Equation: killed text-page recording made failed in clip.json, its last part cut to the 32-byte header: after product recovery every enc line of that part is lost encoder_failed, index lines after = before, every lost line is in clip.json lost, state = failed; a second recovery leaves frames.jsonl and clip.json byte-equal and writes neither clip.json nor any part (their modified times are unchanged) | Known: true
  Scenario: recovery names the losses of a clip whose recorder ended
    # Equation: killed text-page recording made failed in clip.json as a recorder thread that ended without finishing leaves it (lost_cause recorder_ended), its last part cut to the 32-byte header: after product recovery every enc line of that part is lost recorder_ended, every lost instant in clip.json has cause recorder_ended, state = failed | Known: true
  Scenario: recovery refuses a failed clip with zero fps
    # Equation: killed text-page recording made failed with fps 0 in clip.json, its index cut to the line of instant 1 only: product recovery ends with the error "zero fps" and exit 0 (no crash) | Known: true
  Scenario: recovery caps a clip longer than the longest recording
    # Equation: killed text-page recording with end_ms = 600000 + 50 in clip.json: product recovery succeeds, state = ready, index lines = 600000/50 = 12000; each recorded line equals the line a recovery of the unchanged recording makes; every line after the recorded ones is lost | Known: true
  Scenario: a clip with zero fps is refused when it is read
    # Equation: killed text-page recording made ready with fps 0 in clip.json: the product's read of clip.json ends with the error "zero fps" and exit 0 | Known: true
  Scenario: recovery skips a clip of another format quietly
    # Equation: killed text-page recording whose clip.json lacks method (an older format): product recovery ends without an error (ok) and clip.json, frames.jsonl and the list of files are unchanged | Known: true
  Scenario: recovery says a recovered clip needs no more recovery
    # Equation: killed text-page recording: product recovery answers finished = true (the registry marks the clip finished and the next start does not visit it); a second recovery of the now ready clip answers finished = true | Known: true
  Scenario: recovery treats an empty still as missing
    # Equation: killed text-page recording whose first still on an encoded instant is cut to 0 bytes: product recovery succeeds; no index line names that still (still or still_of); its file is not on disk; the product decoder returns that instant as video | Known: true
  Scenario: recovery never deletes a still it could not read
    # Equation: killed text-page recording whose first still is held by another process during product recovery (opened for reading, sharing only delete): recovery ends with an error "could not read" naming the still; every file of the folder before recovery is still there, byte-equal | Known: true
  Scenario: a registry change never writes from a failed read
    # Equation: registry with 2 entries under the bench's own LOCALAPPDATA, held by another process (opened for reading, sharing only delete) while the product marks one clip finished (clip-probe exp-av1-registry): the change ends with an error (ok = false); the registry file is byte-equal | Known: true
  Scenario: a failed atomic write removes its temp file
    # Equation: registry listing one clip under the bench's own LOCALAPPDATA, made read-only so the rename of the written temp file over it fails, while the product marks that clip finished (clip-probe exp-av1-registry action=mark_finished): ok = false; the registry file is byte-equal; no active-clips.json.tmp is left | Known: true
  Scenario: a failed atomic write says when it could not remove its temp file
    # Equation: registry listing one clip under the bench's own LOCALAPPDATA, its active-clips.json.tmp held by another process (opened for reading, sharing read and write, not delete) while the product marks that clip finished (clip-probe exp-av1-registry action=mark_finished), so neither the rename nor the removal of the temp file can happen: ok = false; the error says "could not remove <temp path>"; the registry file is byte-equal | Known: true
  Scenario: recovery at start reaches an unregistered clip in the default folder
    # Equation: killed text-page recording copied to <bench LOCALAPPDATA>\synapse\clips\<its clip_id>, no registry file; the product's recovery at start (clip-probe exp-av1-registry action=recover): clip.json state = ready; the registry lists that clip id with that folder, finished = true | Known: true
  Scenario: recovery at start registers a default-folder clip under the id its clip.json names
    # Equation: killed text-page recording copied to <bench clips>\clip-other (its own clip_id), the registry already listing clip_id "clip-other" (finished, another folder); a second copy in <bench clips>\explicit-b whose clip.json says clip_id <its id>-b; the product's recovery at start: both clips state = ready; the registry lists <its id> with folder clip-other and <its id>-b with folder explicit-b, both finished; no entry has clip_id "explicit-b" | Known: true
  Scenario: recovery at start reports a clip it could not recover
    # Equation: registry under the bench's own LOCALAPPDATA listing one unfinished clip in <bench>\elsewhere\clip-broken whose clip.json is not JSON; the product's recovery at start (clip-probe exp-av1-registry action=recover): ok = false with an error naming that clip.json; the registry still lists the clip unfinished | Known: true
  Scenario: recovery at start reports every failure of registering the default folder
    # Equation: under the bench's own LOCALAPPDATA, the default clips folder holds an unregistered killed text-page recording (clip-readable), a folder clip-broken whose clip.json is not JSON, and a read-only registry []; the product's recovery at start (clip-probe exp-av1-registry action=recover): ok = false with an error naming the clip.json of clip-broken and naming the registry | Known: true
  Scenario: recovery at start reports an entry of the default folder it cannot read
    # Equation: under the bench's own LOCALAPPDATA, the default clips folder holds clip-loop, a junction that points to itself, so its clip.json cannot be resolved (an error other than not found); the product's recovery at start (clip-probe exp-av1-registry action=recover): ok = false with an error naming clip-loop\clip.json | Known: true
  Scenario: a failed start leaves no registry entry and never deletes the registry or its lock file
    # Equation: bench clips folder holding a registry that lists the failed start's clip, and the registry's lock file; the product drops that entry (clip-probe exp-av1-registry action=remove): ok; the registry file holds [] and the lock file stays. Same with a second clip listed: ok; the registry lists exactly the second clip and the lock file stays | Known: true
  Scenario: a clip folder never is or holds the clips folder or the registry
    # Equation: under the bench's own LOCALAPPDATA with no synapse folder, the product's clip folder check (clip-probe exp-av1-registry action=check_folder): the clips folder, it in other letter case with a `..` and a trailing separator, the clips folder as a path relative to the working folder, the synapse folder above it, and the registry path are each refused (ok = false, the error says it is or holds the clips folder); <clips>\clip-new and <bench>\elsewhere\clip are accepted (ok) | Known: true
  Scenario: two folder names Windows keeps apart are never the same clip folder
    # Equation: folders <bench>\i̇ (U+0069 U+0307) and <bench>\İ (U+0130) created side by side (the file system keeps them apart); LOCALAPPDATA = <bench>\i̇; the product's clip folder check (clip-probe exp-av1-registry action=check_folder): <bench>\i̇\synapse\clips refused (ok = false); <bench>\İ and <bench>\İ\synapse\clips accepted (ok) | Known: true
  Scenario: two folders a case-sensitive folder keeps apart are never the same clip folder
    # Equation: <bench> made case sensitive (fsutil file setCaseSensitiveInfo <bench> enable); folders <bench>\a and <bench>\A created side by side (two folders); LOCALAPPDATA = <bench>\a; the product's clip folder check (clip-probe exp-av1-registry action=check_folder): <bench>\a\synapse\clips refused (ok = false); <bench>\A and <bench>\A\synapse\clips accepted (ok) | Known: true
  Scenario: a clip folder whose existing part cannot be resolved is refused
    # Equation: LOCALAPPDATA = <bench>\appdata (no synapse folder); <bench>\link a junction to <bench>\appdata\synapse (dangling), <bench>\loop a junction to itself; the product's clip folder check (clip-probe exp-av1-registry action=check_folder): <bench>\link, <bench>\link\clips and <bench>\loop\clip refused (ok = false) with an error naming the junction; <bench>\elsewhere\clip accepted (ok); no synapse folder is made | Known: true
  Scenario: a late picture comes back in time
    # Equation: product decoder (clip-probe exp-av1-fetch) on the recorded real-page clip, the enc line with the largest index-in-part: total_ms < 500, pictures_decoded = index+1, kind = video | Known: true
  Scenario: the fetched picture is the recorded one
    # Equation: sha256(RGBA of the fetched picture) = reference hash, for the first, the deepest and the last encoded instant of the recorded real-page clip | Known: true
  Scenario: read-back
    # Equation: ffmpeg fetch of one picture (first, middle, last enc instant and the enc line with the largest index-in-part, per scene) < 500 ms | Known: true
