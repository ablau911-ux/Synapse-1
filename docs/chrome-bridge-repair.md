# Chrome bridge repair recipes

Error and health text from `crates/synapse-mcp/src/chrome_debugger_bridge.rs` stays short:
a stable machine code, `key=value` tokens, one remediation sentence, and a pointer to a section
below. The long recipes live here.

Constants used by the recipes:

- Extension id: `leoocgnkjnplbfdbklajepahofecgfbk` (health reports it as `expected_extension_id=`).
- Installer: `scripts\install-synapse-chrome-debugger.ps1` (run from the interactive Windows desktop).
- Install directory: `%LOCALAPPDATA%\synapse\chrome-extension\<build-id>` (stable unpacked directory).
- Health: the `chrome_bridge` subsystem in `health`.

## install

Install the bundled Synapse Chrome extension with `scripts\install-synapse-chrome-debugger.ps1`.
The installer deploys the bridge to `%LOCALAPPDATA%\synapse\chrome-extension\<build-id>` and
auto-loads that stable unpacked directory into the already-open active Chrome profile. It refuses
to launch a second Chrome profile.

What the normal end-user bridge does:

- Uses `chrome.tabs`, `chrome.scripting`, `chrome.downloads`, `chrome.webNavigation` and
  `chrome.webRequest` over a direct localhost WebSocket, plus `chrome.alarms` as the MV3 reconnect wake.
- Debugger-free `pageScreenshot` capture through `chrome.tabs.captureVisibleTab` stitching.
- `chrome.downloads` list/wait/event capture for `browser_downloads` save/move.
- `browser_file_upload` with target-scoped `DOM.setFileInputFiles` / `Page.fileChooserOpened` for
  session-owned Chrome bridge tabs.
- Never uses `nativeMessaging` or helper Chrome windows.

Explicit `browser_debugger`-profile `chrome.debugger` lanes (break-glass):

- Target-scoped hover/tap/active-tab drag, `Page.printToPDF`, `Runtime.evaluate`,
  `Page.addScriptToEvaluateOnNewDocument` init scripts, `Runtime.addBinding` /
  `Runtime.bindingCalled` capture, `Page.handleJavaScriptDialog`.
- Viewport, device, geolocation, locale/timezone and media emulation, network conditions.
- Inactive-tab synthetic mouse drag and HTML5 DataTransfer drag dispatch.
- Entering the profile: `profile operation=set profile=browser_debugger confirm_break_glass=true`
  with a non-empty reason.

## no-active-host

Symptom: `reason=no_active_chrome_bridge_host`, `tab_control_available=false`.

1. Use the already-open authenticated Chrome profile only. Do not launch a second Chrome
   process/profile.
2. Wait for the installed bridge worker `alarmReconnect` registration, then re-read health.
3. If an active stale host appears, set `profile=browser_debugger` and call
   `browser_debugger.reload_bridge` through the public facade.
4. If no host registers, run the installer from the interactive desktop so it auto-loads the bundled
   unpacked extension into the existing active Chrome profile.
5. If health reports `installed=false`, `reload_bridge` cannot repair: Chrome has no loaded extension
   host to receive `reloadSelf` (`cdp_bridge_reload_can_install_absent_extension=false`). Run the installer.

If external debugger/nativeMessaging rows exist while the host is absent
(`external_chrome_popup_risk_scope=host_unavailable_no_live_management`), live suppression state cannot
be read. Restore the host first, then re-read health before classifying those rows.

## stale-extension

Symptom: `CHROME_BRIDGE_EXTENSION_STALE`, `reason=...` tokens naming the build/capability mismatch.

1. Run the installer to deploy the bundled bridge into the stable `<build-id>` directory. It stamps
   the installed `service_worker.js` with `BRIDGE_CODE_STAMP` (SHA-256 of the deployed source).
2. `reason=code_stamp=<running|not_seen_yet> expected=<installed>` means Chrome still runs a
   cached older worker. **A Chrome restart does not fix this**: Chrome keeps the cached MV3 worker
   until the extension itself reloads (#237). The daemon heals it automatically: the first command
   that hits this reason triggers one `reloadSelf` and is retried. Health reports
   `extension_code_stamp`, `expected_code_stamp` and `last_stale_code_reload=[...]` (outcome
   `healed` or `failed code=...`). If a reload reconnects but the worker is still stale, the daemon
   stops auto-reloading for that installed stamp and fails closed.
3. For other reasons, or to force it, call `browser_debugger.reload_bridge` through the public
   facade from a bridge that advertises `reloadSelf`.
4. If the loaded worker predates `reloadSelf`, fail closed and reload the extension from the
   operator's side. Do not use foreground `chrome://extensions` automation.

## unsupported-attach-command

Symptom: `A11Y_CDP_DEBUGGER_WARNING_UNSUPPRESSED`, "refused unsupported attach-capable command".

Only explicit `browser_debugger`-profile lanes may use `chrome.debugger` (see the lane list under
`install`). Other commands need a dedicated raw-CDP automation profile.

1. Run the installer and `browser_debugger.reload_bridge` so the current bridge is installed.
2. For supported browser instrumentation: set `profile=browser_debugger` with `confirm_break_glass=true`
   and a reason.
3. For full DOM/action CDP or screenshots: use raw CDP from a dedicated Synapse-launched automation
   profile.

## self-native-messaging

Symptom: `synapse_chrome_bridge_permission_blocking=true`, `synapse_chrome_bridge_permission_risk=...`.

The Synapse extension profile row still exposes active/manifest `nativeMessaging`; normal bridge
commands fail closed. Rerun the installer to preserve the self `ExtensionSettings`
`blocked_permissions` shield (HKCU policy blocks `nativeMessaging` for the Synapse extension id), then
reload the existing extension with `browser_debugger.reload_bridge` when available, or keep commands
failed closed until Chrome reloads it.

## external-popup-risk

Symptom: `external_chrome_popup_risk=...` unsuppressed.

Let the installed bridge management fallback disable the named extension ids, disable them in Chrome,
or rerun `scripts\synapse-setup.ps1` from an elevated PowerShell so `ExtensionSettings` applies
`blocked_permissions` for debugger/nativeMessaging. Do not weaken the admin-only HKCU Chrome policy ACL.

## profile-install

Symptom: `synapse_chrome_bridge_profile_installation ... installed=false`.

Run the installer from the interactive Windows desktop with the target Chrome profile already open.
`browser_debugger.reload_bridge` can only reload an already-registered bridge host; it cannot install an
absent Chrome extension.
