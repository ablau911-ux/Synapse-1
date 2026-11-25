Feature: DYN1 service restart
  Scenario: A killed recording preserves every completed exact record
    Then recovery publishes ready with service_restart
    And every missing instant is named once
    And surviving moving samples are counted from the file
    And no partial moving file remains
