# DYN1 builder status, 2026-11-23

Status: incomplete, stopped at the step 1 scroll-size acceptance gate. No threshold was loosened.

| Check | Result | Evidence |
|---|---|---|
| Initial dyn-exact RED | 8 failures; exp-dyn was absent before implementation | dyn-exact-red.txt |
| Final dyn-exact | 7 passes, 1 failure; scroll 10.1841716766 MiB/min > 9.4 | dyn-exact-failed.txt |
| Byte equality | Zero mismatches, all eight inputs; product reader and independent decoder | dyn-exact-metrics.json |
| Mixed clip | 2.7776966095 MiB <= 3.564 MiB | dyn-exact-metrics.json |
| Writer RAM | Final run maximum 160,428,032 bytes <= 155.1 MiB | dyn-exact-metrics.json |
| Whole-frame scroll baseline | 9.4372158051 MiB/min, zero mismatches | dyn-scroll-baseline.json |
| Workspace cargo fmt --all -- --check | Exit 0 | dyn-root-fmt.txt |
| Probe entry point formatting | Exit 0, cargo fmt with skip_children=true | dyn-probe-fmt.txt |
| New Rust modules formatting | Exit 0, rustfmt --check | dyn-files-fmt.txt |
| Workspace strict cargo clippy | Exit 0, -j 4, Idle, --offline, -D warnings | dyn-root-clippy.stderr.txt |
| Probe strict cargo clippy | Exit 101; 11 existing findings, none in added modules | dyn-probe-clippy.stderr.txt |
| Probe release build | Exit 0, -j 4, Idle | dyn-exact-build.stderr.txt |
| Workspace release build | Exit 101; missing ONNX Runtime, undefined OrtGetApiBase | dyn-root-build.stderr.txt |
| Node syntax checks | Both dyn-exact.mjs and run-spec.mjs exit 0 | Builder tool output |
| Git diff --check | Exit 0; only LF-to-CRLF notices | Builder tool output |
| Git commit | Blocked before staging by denied index.lock creation outside writable root | dyn-commit-blocked.txt |
| Synapse | Health passed; DYN1 live UX was not verified | Not integrated |

Step 1 adds the exact layer, shared rectangle geometry, direct product-code probe, and independent format/size checks. It is not green and is not wired into the server. Steps 2–9 were not started. The H.264 worker remains; recovery, viewer/D5 integration, and youtube-proof.mjs are not implemented. No live YouTube test was run. No commit or push was made. REVIEW.md remains uncommitted.

The final probe outputs remain in `.runs/dyn-exact-final`. Automatic approval review rejected cleanup of the redundant `.runs/dyn-exact-red`, `.runs/dyn-exact-first`, `.runs/dyn-exact-second`, and `.runs/dyn-scroll-baseline` folders with `blocked by policy`; they also remain. No build or probe process was left running.
