# Live check of the stress-batch fixes, one issue at a time

Written 2026-11-25 01:50 Jerusalem Standard Time by claude. Owner request (2026-11-25): "check live VBV all the fixes on baches"; VBV = visual behaviour verification (owner, 01:52).

## Goal

For every issue that a commit on `fix/stress-batch2` names, run the issue's own failing input against a release build of that branch in the real app, and verify the behaviour visually: what the target window shows before and after the call (screenshot), next to the tool's answer. Record PASS (the reported wrong behaviour is gone, the window shows what the answer claims, and the new behaviour is the one the commit states) or FAIL (with the observed answer and picture). One check per issue, run once.

## Facts (from the repository and this session)

- Branch `fix/stress-batch2`, head `b69f35a4`; 126 commits since `main` (`b3a546c4`) name 68 issues: 144, 146, 148, 162, 163, 174, 180, 181, 183, 184, 186, 189, 191, 193, 194, 195, 199, 200, 206, 207, 209, 210, 211, 212, 218, 219, 220, 221, 222, 223, 224, 226, 227, 229, 235, 236, 243, 245, 248, 249, 250, 251, 252, 253, 254, 256, 258, 259, 260, 263, 264, 265, 266, 267, 270, 271, 272, 273, 274, 277, 278, 283, 284, 289, 311, 312, 313, 316 (`git log --format=%s b3a546c4..HEAD`).
- Each issue body (Gitea `adam/synapse`) holds the failing input and the observed wrong answer; each fix commit message holds the new behaviour and, for most, a "how to see it live" line.
- A working MCP stdio client that starts a release `synapse-mcp.exe` with its own database exists on `feature/clip-capture`: `scripts/clip-equation/live-test/mcp-stdio.mjs` (128 lines). It is not on this branch.
- The owner's own Synapse server (pid started 18:04) serves this session; the live check must never stop it or share its database or port.

## Decisions

1. The check runs a separate release binary (`target/release/synapse-mcp.exe` of this branch) with its own database folder under `.runs/live-check/` and, for HTTP checks, its own loopback port. It never touches the owner's running server.
2. One Node driver, `scripts/live-check/batch-live.mjs`, with one named check per issue. Each check prints one line: `#N PASS|FAIL|SKIP <observed answer, at most 300 characters>`. SKIP only when the input needs something the PC cannot provide right now (an SSH host, a disk that fills, a second machine), with the reason.
3. Stdio checks use a copy of the clip branch's `mcp-stdio.mjs` client in `scripts/live-check/` (the clip branch is not merged; when it is, one copy is removed).
4. Visual evidence: every check whose issue touches a window (act, browser, observe, read_text, screenshot, target, process launch) takes a screenshot of the target window before and after the call through the test server's own `screenshot` tool, saved as `.runs/live-check/<run>/<N>-before.png` / `-after.png`; Claude looks at each pair and states what it shows. Checks with no window (transport, storage, task, shell records) verify the observable behaviour: the HTTP answer, the stored row, the process that runs or does not run. Windows are opened, checked and closed in the same check (owner rule: visible test windows only while the check runs).
5. Permission (`sev:perm`) issues are not checked: they wait for the owner's decision.
6. Claude runs the driver once and reads every line; a FAIL is a bug: it is logged on its issue and fixed, then only that check is run again.

## Parts and files

- `scripts/live-check/PLAN.md` (this file).
- `scripts/live-check/mcp-stdio.mjs` (client copy).
- `scripts/live-check/batch-live.mjs` (driver and checks).
- Output: `.runs/live-check/<run>/results.txt` (not committed).

## Proof

`results.txt` of one run, with a line for each of the 68 issues; the PASS lines are posted on their issues as evidence before they are closed.
