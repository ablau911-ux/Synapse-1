Feature: Dynamic YouTube recorder acceptance
  Scenario: The clip is complete
    # Equation: ready completed 60000 ms and 1200 lines | Known: true
  Scenario: Nothing was lost
    # Equation: lost empty and C and T held | Known: true
  Scenario: The test ran at 1080p
    # Equation: width >= 1900 and height >= 1000 | Known: true
  Scenario: RAM
    # Equation: peak private bytes minus baseline <= 500 MB | Known: true
  Scenario: CPU
    # Equation: processor share <= 10 percent and B held | Known: true
  Scenario: The encoder
    # Equation: hardware quality 90 and Q held | Known: true
  Scenario: The video moves
    # Equation: moving share before scroll >= 0.95 | Known: true
  Scenario: The box is the video
    # Equation: IoU at 5000 ms >= 0.80 and padding <= 24 px | Known: true
  Scenario: Text beside the video is exact
    # Equation: moving rectangle intersects right column zero times | Known: true
  Scenario: The scroll is stored
    # Equation: exact record within 2000 ms of scroll | Known: true
  Scenario: After the scroll the box is the video again
    # Equation: IoU at scroll plus 3000 ms >= 0.80 | Known: true
  Scenario: Typing is exact and does not move
    # Equation: all 12 keys have exact search-row bands within 300 ms and box unchanged | Known: true
  Scenario: The stop
    # Equation: final moving instant <= pause plus 950 ms | Known: true
  Scenario: One exact picture after the stop
    # Equation: following exact bands cover the previous rectangle rows | Known: true
  Scenario: Nothing changes nothing written
    # Equation: repeat without rectangle change has no exact record | Known: true
  Scenario: Sample count
    # Equation: every segment complete and frames = last minus first plus one | Known: true
  Scenario: Size bound
    # Equation: retained bytes <= measured moving rate plus exact and start allowances | Known: true
  Scenario: The picture is the screen
    # Equation: screenshot crop max byte difference below label = 0 | Known: true
  Scenario: The viewer sees it
    # Equation: overview succeeds and reports video animation scroll and typing changes | Known: true
  Scenario: The video is in the clip
    # Equation: both video crops vary and differ from each other | Known: true
  Scenario: Ready time
    # Equation: ready latency <= 1000 ms | Known: true
  Scenario: The window is closed
    # Equation: no process of the test profile remains | Known: true
