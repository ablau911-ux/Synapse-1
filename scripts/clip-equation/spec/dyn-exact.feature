Feature: DYN1 exact layer (architecture sections 4.2 and 7, step 1)

  Scenario: corpus pictures round trip independently
    # Equation: mismatching BGR bytes over every instant = 0 | Known: 0
  Scenario: product reader matches every instant and backward seeks
    # Equation: reader_mismatches = 0 | Known: 0
  Scenario: groups are complete independent zstd frames
    # Equation: records/group <= EXACT_GROUP_PICTURES; first offset = 0; first raw = 6 + w*h*3 | Known: true
  Scenario: idle writes one record
    # Equation: records(still) = 1 | Known: 1
  Scenario: typing records contain only changed rows
    # Equation: mean(non-key raw)/(w*h*3) < 0.05 | Known: true
  Scenario: exact size meets whole-frame stream figures
    # Equation: MiB/min <= typing:0.5, ui:6.6, scroll:9.4, flash:2.8 | Known: true
  Scenario: exact writer RAM stays within budget
    # Equation: OS peak private bytes - M0 <= 141 MiB * 1.10 | Known: true
  Scenario: mixed clip stays within ten percent
    # Equation: total retained bytes <= 1.10 * 3.24 MiB | Known: true
