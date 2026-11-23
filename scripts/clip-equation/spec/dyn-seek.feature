Feature: Numbered pictures survive random seeks
  Scenario: Seek backwards through a moving segment
    Given hashes of every sequentially decoded picture
    When the same frames are decoded in reverse order
    Then every decoded pixel hash is identical
