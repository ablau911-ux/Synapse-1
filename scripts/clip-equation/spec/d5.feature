# Clip viewer D5: the acceptance of the change detection on the proof clips.
# Spec: docs/clip-recorder-plan.md, "D5 as an equation", item 7 (acceptance) and the findings of
# Gitea issue #293 (Codex review of ea3e8514 and 3077349b).
# Checked by: node scripts/clip-equation/spec/run-spec.mjs d5 <viewer-output.json> <schedule.json>
#
# Inputs (written in the verification round; see the header of d5.mjs for every field):
#   viewer-output.json  one object per clip: proof (the ui-events-page.html clip), still, spinner
#                       (a lone spinner), flash (the flash page clip). Each holds the viewer's
#                       overview_analysis.json timeline and picks, frame_ms, end_ms, keyframes_ms;
#                       proof also schedule_start_ms (clip time of the page's schedule start) and
#                       flash also flashes (the flashes the page showed).
#   schedule.json       the page's window.schedule, window.actualTiming and window.scheduleBoxFits.
# Symbols: one instant = frame_ms (50 ms at 20 fps); K_s = SETTLE_MS / frame_ms = 4;
# an event = a timeline entry of kind change, short_lived or animating.

Feature: The viewer reports each scheduled UI event once, with its type and start, and nothing else

  # D5 item 7: the acceptance lines.

  Scenario: every scheduled event reported
    Given the proof clip and the page's schedule
    Then each scheduled row has an event of its expected kind whose start is within one instant of the row's presented start
    # Equation: |start_ms(event) - (schedule_start_ms + t_presented)| <= frame_ms, kind(event) = expect | Known: every row matched

  Scenario: still period gives no event
    Given the proof clip
    Then no event starts in the scheduled still period, while the caret blinks
    # Equation: count(events starting in [19000, 29000]) = 0 | Known: 0

  Scenario: still clip gives no event
    Given the still clip
    Then its timeline has no event
    # Equation: count(events) = 0 | Known: 0

  Scenario: lone spinner gives exactly one animation
    Given the lone spinner clip
    Then its timeline has one animating event and no other event
    # Equation: count(animating) = 1, count(change) + count(short_lived) = 0 | Known: 1, 0

  Scenario: keyframe instants give no event
    Given the proof and still clips with the keyframe instants of their segments
    Then no event starts within one instant of a keyframe unless a scheduled row starts there
    # Equation: count(events at a keyframe instant and at no scheduled row) = 0 | Known: 0

  Scenario: flash clip gives one FLASH per flash
    Given the flash clip and the number of flashes its page showed
    Then the timeline has one short_lived event per flash and no other event
    # Equation: count(short_lived) = flashes, count(change) = 0 | Known: equal, 0

  # Issue #293: one scenario per finding.

  Scenario: #293-1 a fade is one change from its start
    Given the 300 ms dialog fade of the proof clip
    Then exactly one change starts in the fade, within one instant of the fade start: undecided steps never move the anchor
    # Equation: count(change starting in [t, t + 300 + 200]) = 1, |start - t| <= frame_ms | Known: 1, true

  Scenario: #293-2 an animation that stays SAME to its anchor
    Given the grey square that alternates 100/156 around its grey 128 (SAME to the anchor, DIFF frame to frame)
    Then one animating event starts within one instant of the alternation start, and no change or short_lived event starts in it
    # Equation: count(animating at t) = 1, count(change, short_lived in [t, t_end)) = 0 | Known: 1, 0

  Scenario: #293-3 a change is pictured at its confirmed settle frame
    Given every settled change of the proof clip
    Then its pick "settled frame after change" is at least K_s instants after its last change
    # Equation: frame(pick) >= end_frame(change) + K_s | Known: every settled change

  Scenario: #293-4 quiet for SETTLE_MS before the clip end is settled
    Given every change the viewer marks as still changing at the clip end
    Then it was quiet for less than SETTLE_MS before the clip end
    # Equation: end_ms(clip) - end_ms(change) < 200 | Known: every unsettled change

  Scenario: #293-5 only the animated stretch is exempt from FLASH
    Given the grey square's alternation and its 50 ms dark flash 400 ms after the alternation ends
    Then the alternation gives no short_lived event, and the later flash gives one short_lived event within one instant of its start
    # Equation: count(short_lived in [t_alt, t_alt_end)) = 0, |start(short_lived) - t_flash| <= frame_ms | Known: 0, true

  Scenario: #293-6 flashes are timed from their presentation
    Given the page's actual timing of every flash
    Then each flash was presented, and shown for at least FLASH_MS from its presentation
    # Equation: hidden_ms - shown_ms >= 50 for every flash row | Known: every flash

  Scenario: #293-7 the schedule box shows every row
    Given the page's schedule box
    Then its content fits the box: nothing is cut off
    # Equation: scrollHeight <= clientHeight | Known: true
