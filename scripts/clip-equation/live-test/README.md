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
