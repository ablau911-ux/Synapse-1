Feature: DYN1 recorder folder
  Scenario: A recorded clip is immediately ready
    Then schema 3 stores exact groups and cropped moving intervals
    And all references and byte totals match the files
    And live ready time is at most 1000 ms
    And repeated stop calls return the same state
