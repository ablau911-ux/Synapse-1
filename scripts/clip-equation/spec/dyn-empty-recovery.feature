Feature: Recovery before the first captured picture
  Scenario: No frame index has been created
    Given a recording clip with no frames.jsonl
    When recovery runs
    Then the state is ready with stop_reason service_restart
    And each planned instant is lost exactly once
