CLEAN RUN: 3559 passed, 0 failed (HEAD 4229982c)
fmt: clean
Task: suite10; issue #2425; full-run evidence for Release R1 #560 and R2 #574.
Command: cargo test --workspace --no-fail-fast; exit 0; exactly one full run; no test reruns.
Counts: 257 targets, 3559 passed, 0 failed, 15 ignored (existing test annotations unchanged).
Duration: 19.67 minutes; 2026-11-20 12:42:17 through 13:01:57 (runner clock).
Screen: 0 Kernel-Power 506 events during the run; newest event remained 507 at 2026-11-20T09:25:59.325Z.
Foreground: initial sample plus 2 changes; HWND/PID/time only, no owner window contents read; see watch.jsonl.
Historical #2425 test: server::tests::shell_user_active_268::r2::must_pass_shell_r2_argument_verbs_execute_console_268 passed; original timeout did not recur.
Historical #2427 test: must_fail_cross_origin_agent_terminal_websocket_upgrade_is_403 passed.
Watch: identity=(PID, creation time), 150 ms polling, 105 PowerShell/pwsh children observed, 158 test binaries observed.
Watch maximum completed child lifetime upper bound: 2.672492 seconds; 0 children over 8 seconds; 0 captures (cap 5).
Blocking call: none captured; no qualifying long-lived child existed. No failure cause or product/test fault established by this run.
Cleanup: watch stopped automatically after DONE; 265 runner/watch/test/child identities checked, 0 alive; no cargo/rustc; no helper remains.
Runner PID 19468 and watch PID 8252 are stopped; previous orphan watch PID 20408 was stopped before launch and observed no tests.
Evidence: C:/Users/Adam/.claude/synapse-agents/merge/jobs/r1-fullsuite-10/{meta.txt,full.log,failed.txt}.
Watch and cleanup evidence: C:/Users/Adam/.claude/synapse-agents/merge/jobs/day-1120/suite10/{watch.py,watch.jsonl,watch-summary.json,cleanup-final.json}.
Tested branch: merge/wp1-safety; full HEAD 4229982cb1493120b45221747987581055545e87; HEAD unchanged throughout the run.
Scope: no code, tests, assertions, thresholds, behavior tables or frozen table files edited; no second full run; no deployment or live FSV/VBV acceptance claimed.
Principles: this was verification only; BDD/TDD specifications and KISS/OOP implementation remained unchanged.
The local evidence commit contains only this report; pre-existing repository edits remain untouched. REVIEW.md remains uncommitted.
