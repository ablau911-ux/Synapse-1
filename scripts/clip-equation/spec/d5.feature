# Clip viewer D5: the acceptance of the change detection on the proof clips.
# Spec: docs/clip-recorder-plan.md, "D5 as an equation", item 7 (acceptance) and the findings of
# Gitea issue #293 (Codex reviews of ea3e8514, 3077349b and 8d9af525).
# Checked by: node scripts/clip-equation/spec/run-spec.mjs d5 <viewer-output.json> <schedule.json>
#
# Inputs (written in the verification round; the header of d5.mjs lists every field):
#   viewer-output.json  one object per clip: proof (the ui-events-page.html clip), still, spinner
#                       (a lone spinner), flash (the flash page clip), boundary_below and
#                       boundary_at (page runs ending 150 ms and exactly 200 ms after the "last
#                       change" row). Each holds the viewer's overview_analysis.json timeline (and
#                       picks), fps and frame_ms (= 1000 / fps), and as needed end_ms, keyframes_ms,
#                       schedule_start_ms, viewport (CSS px to recorded px), shown_ms (flash clip),
#                       actual (a boundary run's window.actualTiming) and folder (the clip folder:
#                       boundary clips, and the proof and still clips for their keyframes).
#   schedule.json       the proof run's window.schedule, window.actualTiming, window.always and
#                       window.scheduleBoxFits.
# Symbols: one instant = frame_ms (50 ms at 20 fps); K_s = SETTLE_MS / frame_ms = 4;
# an event = a timeline entry of kind change, short_lived or animating; t = the clip time at which
# the page presented a row (never its nominal time); region = the row's element box in recorded px.

Feature: The viewer reports each scheduled UI event once, with its type, region and start, and nothing else

  Scenario: a moving area is one animation
    Given the video clip with its stored moving rectangle
    Then one animation has that rectangle and no pixel-noise event lies inside it while moving
    # Equation: count(animating) = 1; bbox = stored rect; inner changes = 0 | Known: true

  Scenario: a moving area that stops gives one change
    Given the video clip after its moving interval ends
    Then one change begins within one instant of its last recorded motion
    # Equation: count(change at moving end +/- one instant) = 1 | Known: true

  # D5 item 7: the acceptance lines.

  Scenario: every scheduled event reported
    Given the proof clip, the page's schedule with its presentation times and regions, and the spinners running from page load
    Then each row takes exactly one event of its kind, in its region, starting within one instant of t, and no event is left over
    And typing is matched key by key: every key presentation is covered by exactly one change, every change covers at least one key and starts within one instant of its first key, and the keys of one change form a chain allowed by D5 step 4 (each next key on the same line, at most REGION_GAP_BLOCKS = 24 columns aside, shown within SETTLE_MS after the previous one)
    # Equation: one-to-one: kind = expect, bbox meets region, |start_ms - t| <= frame_ms; missed = duplicate = key problems = unexpected = 0 | Known: 0, 0, 0, 0

  Scenario: still period gives no event
    Given the proof clip
    Then no event starts in the scheduled still period, while the caret blinks
    # Equation: count(events starting in [t_still, t_still + 10000)) = 0 | Known: 0

  Scenario: still clip gives no event
    Given the still clip
    Then its timeline has no event
    # Equation: count(events) = 0 | Known: 0

  Scenario: lone spinner gives exactly one animation
    Given the lone spinner clip
    Then its timeline has one animating event and no other event
    # Equation: count(animating) = 1, count(change) + count(short_lived) = 0 | Known: 1, 0

  Scenario: keyframe instants give no event
    Given the proof and still clips with keyframes_ms equal to every keyframe instant of the segments in their clip.json
    And at least one keyframe after the first instant when the clip has more encoded frames than GOP_FRAMES = tau * f = 20 (a still clip, one encoded frame, is exempt)
    Then no event starts within one instant of a keyframe, except a proof event matched to its scheduled row
    # Equation: keyframes_ms = sorted(segments.keyframe_instants) * frame_ms; encoded > GOP_FRAMES => some keyframe > frame_ms; count(events at a keyframe instant and not matched to a row) = 0 | Known: 0

  Scenario: flash clip gives one FLASH per flash
    Given the flash clip and the clip time of each presented flash
    Then each flash has exactly one short_lived event within one instant, and there is no other event of any kind
    # Equation: per flash count(short_lived, |start_ms - t| <= frame_ms) = 1; other events = 0 | Known: 1, 0

  # Issue #293: one scenario per finding.

  Scenario: #293-1 a fade is one change from its start
    Given the 300 ms dialog fade of the proof clip
    Then the fade row takes exactly one change, in its region, within one instant of the fade start: undecided steps never move the anchor
    # Equation: count(matched change of the fade row) = 1 | Known: 1

  Scenario: #293-2 an animation that stays SAME to its anchor
    Given the grey square that alternates 100/156 around its grey 128 (SAME to the anchor, DIFF frame to frame)
    Then its row takes one animating event, and no other event starts in its region during the alternation
    # Equation: count(matched animating) = 1, count(other events in region in [t, t_end)) = 0 | Known: 1, 0

  Scenario: #293-3 a change is pictured at its confirmed settle frame
    Given every settled change of the proof clip, with the confirmed frame N its timeline text names
    Then a "settled frame after change" pick lies at N, and N is at least end_frame + K_s (clipped to the last frame) and at most the last frame
    # Equation: pick.frame = N, min(end_frame + K_s, last) <= N <= last | Known: every settled change

  # Onset rule of the boundary runs (and of any exact-duration check): the page knows only that
  # the change reached the screen within [set_ms, shown_ms] (shown_ms is the next animation frame,
  # an upper bound). Both ends, shifted by page_start - s_0, must fall to the same recorder instant
  # ceil(t / frame_ms); a run whose interval straddles an instant fails as ambiguous_onset, is
  # never classified, and that boundary run is recorded again.

  Scenario: #293-4a quiet one instant less than SETTLE_MS before the clip end is not settled
    Given the boundary_below clip, its page run's timing and the recorder's clip.json and frames.jsonl
    Then the page and the recorder alone give 150 ms of quiet after the last change, and the viewer reports that change as still changing when the clip ended
    # Equation: onset = ceil((page_start - s_0 + set_ms) / frame_ms) = ceil((page_start - s_0 + shown_ms) / frame_ms), else ambiguous_onset; quiet = stop(frames.jsonl) - onset * frame_ms = SETTLE_MS - frame_ms; outcome = unsettled | Known: 150, unsettled

  Scenario: #293-4b quiet exactly SETTLE_MS before the clip end is settled
    Given the boundary_at clip, its page run's timing and the recorder's clip.json and frames.jsonl
    Then the page and the recorder alone give exactly 200 ms of quiet after the last change, and the viewer reports that change as settled
    # Equation: onset = ceil((page_start - s_0 + set_ms) / frame_ms) = ceil((page_start - s_0 + shown_ms) / frame_ms), else ambiguous_onset; quiet = stop(frames.jsonl) - onset * frame_ms = SETTLE_MS; outcome = settled | Known: 200, settled

  Scenario: #293-5 only the animated stretch is exempt from FLASH
    Given the grey square's alternation and its 50 ms dark flash 400 ms after the alternation ends
    Then no short_lived event starts in the square during the alternation, and the dark flash row takes exactly one short_lived event
    # Equation: count(short_lived in region in [t_alt, t_alt_end)) = 0, count(matched short_lived of the dark flash) = 1 | Known: 0, 1

  Scenario: #293-6 flashes are timed from their presentation
    Given the page's actual timing of every flash row
    Then its times are finite and ordered, and the hide was decided at least FLASH_MS after the presentation
    # Equation: set_ms <= shown_ms <= hide_ms <= hidden_ms, hide_ms - shown_ms >= 50 | Known: every flash row

  Scenario: #293-7 the schedule box shows every row
    Given the page's schedule box, measured at load and on every resize
    Then its content fits the box, the box lies inside every viewport edge, and no card covers it
    # Equation: scroll size <= client size, 0 <= left, top; right <= innerWidth; bottom <= innerHeight; no card overlaps (window.scheduleBoxFits) | Known: true
