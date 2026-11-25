# Live test drivers for the clip recorder

Used for the proof cases in `docs/clip-recorder-plan.md` (section Proof). Both start a given
`synapse-mcp` build as a separate stdio MCP instance. Give that instance its own `--db` folder and set
`SYNAPSE_SHELL_JOB_ROOT` to an empty folder, or it refuses to start beside the running service.

- `record-drive.mjs <exe> <db> <out_dir> <json_options>`: records one window and polls `record_status`.
  Options: `hwnd`, `duration_ms`, `resize: [{at_ms, w, h}]`, `stop_at_ms` (calls `record_stop` twice),
  `kill_server_at_ms` with `restart_after_kill`, `overview`, `max_wait_ms`. Writes `log.json` and a
  copy of `clip.json` into `out_dir`.
- `view-call.mjs <exe> <db> <out_dir> <json_args>`: calls one `screenshot` operation (for example
  `clip_overview`) and saves its text and images.

Test window: open `../flash-page.html` in a Chrome `--app` window with
`--disable-backgrounding-occluded-windows --disable-renderer-backgrounding --disable-background-timer-throttling`,
place it behind other windows, and close it in the same script that records (owner rule).

Shared code lives in `mcp-stdio.mjs` (the stdio MCP instance, `record_status`, window placement
through user32); the drivers import it.

## D5 acceptance (`d5-proof.mjs`)

    node d5-proof.mjs <synapse-mcp exe> <scratch dir> [--clips name,name]

Records the six clips of `../spec/d5.mjs` (plan, "D5 as an equation" item 7), writes
`viewer-output.json` and `schedule.json` in the shape of the `d5.mjs` header, then runs
`node ../spec/run-spec.mjs d5 viewer-output.json schedule.json` and exits with its status. Needs
Node 22 or later and Chrome (`CHROME_PATH` or its default install folder); no npm packages.

- The build runs as its own stdio instance: `--db <scratch>/db/synapse.db`,
  `SYNAPSE_SHELL_JOB_ROOT=<scratch>/shell-jobs`.
- Each clip gets a Chrome `--app` window with its own profile and DevTools port: launched minimized
  without activation, shown without activation, given two animation frames to paint, put behind
  all other windows, recorded, read over DevTools and closed in the same step. Its browser PID is
  found right after the launch; closing ends that process tree and every process of the profile
  and verifies none is left. A window that cannot be closed stops the whole round.
- `proof`, `boundary_below`, `boundary_at`: `../ui-events-page.html`. `still`, `spinner`, `flash`:
  `../d5-scenes.html?scene=...` (`flash-page.html` cannot give the flash clip: its counter, clock
  and log change on every instant, and it does not expose when each flash was presented). The
  flash scene shows a fixed number of flashes; the clip records past the last one by
  `SHORT_LIVED_MS + SETTLE_MS` plus slack, and every flash shown must lie inside it.
- Every MCP call has a deadline; when the instance exits or its stdin breaks, pending calls fail
  at once, so the window cleanup always runs.
- A boundary page is loaded with its start an hour away; once `record` has started, it is reloaded
  with `?start=` set from the clip's s_0 so that the clip stops 150 ms or 200 ms after the first
  instant that shows the last change.
- `viewport` assumes the captured frame is the whole window with equal borders left, right and
  below the page; `scale` is `devicePixelRatio` times the recorder's pixel duplication.
- `d5-runs.json` lists per clip its folder, `clip.json`, `frames.jsonl`, `overview_analysis.json`,
  s_0 and what the page gave, or the error. `--clips boundary_at` records only that clip again (for
  example after `ambiguous_onset`) and keeps the others.
- Tunables (window size, lead and clip lengths, onset phase) are in `CONFIG` at the top.
