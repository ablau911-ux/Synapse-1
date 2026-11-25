Feature: DYN1 moving layer (architecture step 2)
  Scenario: hardware quality grant
    # Equation: hardware and quality_level=90 and b_frames=0 and gop_frames=20 | Known: true
  Scenario: one sample per instant
    # Equation: sum(segment.frames)+waited=instants for full, crop, minimum and paused inputs | Known: true
  Scenario: start stays exact
    # Equation: waited=first_instant of the first segment; behind=0 | Known: true
  Scenario: mapping including repeated instants
    # Equation: SSIM-Y(expected[n], decoded[n]) >= each adjacent-source score for every n | Known: true
  Scenario: box quality
    # Equation: mean SSIM-Y >= 0.95 | Known: true
  Scenario: odd dimensions
    # Equation: coded_size=506x334; visible_size=506x333; mapping exact | Known: true
  Scenario: keyframe spacing
    # Equation: first key=0; maximum key gap including tail <= GOP_FRAMES | Known: true
  Scenario: bounded queue
    # Equation: high_water <= MOVING_QUEUE_FRAMES; behind=0 | Known: true
  Scenario: unavailable stays exact
    # Equation: segments=0 and moving files=0 and unavailable reason present | Known: true
