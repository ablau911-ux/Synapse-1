# Experiments step 5: the results file.
# Spec: docs/clip-recorder-experiments-plan.md, "Method" 4 (J), "Procedure" 5, "Acceptance (BDD)" step 5,
# "Choosing the configuration".
# Checked by: node scripts/clip-equation/spec/run-spec.mjs step5 [results/fit.json]
# Built by:   node scripts/clip-equation/fit/results.mjs [results/fit.json]
# Symbols: bytes = bytes per minute of clip, CPU = CPU seconds per minute of clip, RAM = peak private bytes
# above the process base; * = the least value among the kept configurations of the same corpus folder.

Feature: One results table with every configuration, J, the Pareto front and the best way

  Scenario: rows equal configurations
    Given the input CSVs named in fit.json
    Then results/results.csv has one row per configuration found in the CSVs, and the results file's table has the same rows
    # Equation: rows(results.csv) = rows(results.md table) = count(distinct configuration keys in the CSVs) | Known: equal

  Scenario: every CSV row present
    Given the input CSVs named in fit.json
    Then every CSV row belongs to exactly one configuration row and the repeats add up
    # Equation: sum(runs) over results.csv = rows of the CSVs | Known: equal; no CSV row missing

  Scenario: per-minute values
    Given every configuration row
    Then bytes per minute equals the mean bytes of its CSV rows times (N_new / frames) * 60 / t
    # Equation: bytes/min = mean(bytes) * (N_new / frames) * 60 / t | Known: within 1e-9 relative on every row

  Scenario: J for every kept row
    Given the kept rows of each corpus folder
    Then J is given for every kept row and for no dropped row, and each dropped row names its failed clause
    # Equation: J = bytes / bytes* + CPU / CPU* + RAM / RAM* | Known: recomputed J within 1e-9 on every kept row

  Scenario: J* = 3
    Given a configuration that is least on bytes, CPU and RAM at once
    Then its J is exactly 3
    # Equation: J = 1 + 1 + 1 | Known: 3 exactly (synthetic set, and every folder whose least-J row is least on all three)

  Scenario: Pareto front
    Given the kept rows of each corpus folder
    Then no front row is dominated, and every kept row off the front is dominated by a front row
    # Equation: dominated(a) = exists b: b <= a on bytes, CPU and RAM and b < a on one | Known: verified by brute force

  Scenario: best way and its equations
    Given the kept configurations
    Then the results file names the configuration with the least J and lists its equations with coefficients
    # Equation: best = argmin mean J over the shared corpus folders | Known: the same configuration as recomputed; equations present

  Scenario: results file header
    Given the results file
    Then its header names the input CSVs with their row counts and says PILOT, not final when an input is a pilot CSV
    # Equation: label = PILOT when any input file is a pilot CSV | Known: header present and label right
