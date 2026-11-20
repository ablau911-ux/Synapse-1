# Synapse control panel

A small Windows window to see and control the local Synapse daemon
(`synapse-mcp.exe`, `http://127.0.0.1:7700/mcp`): four lamps (Daemon, Server,
MCP tools, Autostart), an on/off slider for the daemon and for autostart, and a
Safe Reclaim button that starts the daemon only when the port is free. It never
stops a process it cannot verify as the authenticated Synapse daemon.

## Start it
- Desktop shortcut "Synapse Control", or
  `pythonw tools\control-panel\synapse_panel.py`
- Command line: `python tools\control-panel\synapse_core.py status`
  (also `start`, `stop`, `reclaim`, `enable`, `disable`, `ensure`).
  `synapse-ctl.ps1` forwards its arguments to it; `synapse-ensure.ps1` runs
  `ensure` (start the daemon unless autostart is off).

## Files
- `synapse_core.py`: all daemon logic (`SynapseService`) and the command line.
- `synapse_panel.py`: the Tkinter window; it only calls `SynapseService`.
- `synapse-ctl.ps1`, `synapse-ensure.ps1`: PowerShell shims.

## Needs
Windows, Python 3.13 with tkinter, `Pillow` and `psutil` (process ids; `stop`
needs it), and `synapse-mcp.exe` in `%USERPROFILE%\.cargo\bin`. The daemon
token is read at run time from `%APPDATA%\synapse\token.txt`; it is never
stored here. Autostart off = the file `%USERPROFILE%\.claude\synapse-disabled.flag`.

Every command acts on the live shared daemon of this PC.
