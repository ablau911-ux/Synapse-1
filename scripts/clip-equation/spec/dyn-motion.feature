Feature: DYN1 temporal change and compression cost
  Scenario: Only sustained expensive motion becomes moving
    Then the independent synthetic scenes satisfy section 7 step 3
    And every composed instant is exact outside its moving rectangle
    And moving pictures have mean SSIM-Y at least 0.95
