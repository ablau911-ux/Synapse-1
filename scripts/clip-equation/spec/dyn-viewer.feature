Feature: DYN1 composition and seek
  Scenario: Exact text surrounds a decoded moving box
    Then source pixels outside the box match returned pixels below the image label
    And moving pictures change across the interval
    And seeks apply at most 20 exact records and 20 moving frames
