# ADR: Multi-Browser Bridge — Declared Browser Rows, Per-Kind Hosts, Token-Bound Kinds

Status: Proposed (foundation phase 1 landed; phase 2 call-site switch pending)
Date: 2026-10-15

## Context

The normal (non-CDP) browser bridge was Chrome-only: the extension id, origin,
token file, tab-target prefix, endpoint, health key, policy root, user-data root,
process names and installer UI names were literals spread over the daemon, the
service worker and the installer, and the daemon kept exactly one active bridge
host, so a second browser would evict Chrome. The owner wants bridges for Edge,
Brave, Opera / Opera GX, Vivaldi and Firefox in addition to Chrome, without
changing the public MCP tool surface.

## Decision

1. **One declared data table.** Every bridgeable browser is one row in
   `crates/synapse-browsers/browsers/<id>.json` (`BrowserRow`, serde
   `deny_unknown_fields`, validated, unique id / tab prefix / health key /
   endpoint base / token file). The daemon (`synapse_browsers::BrowserRegistry`),
   the raw-CDP lane, the seats and the PowerShell installers
   (`scripts/synapse-browser-rows.ps1`) read the same files. Setup deploys the
   rows beside the exe (`<exe dir>\browsers`); the daemon falls back to the
   crate dir (dev) and to the compiled-in `chrome.json` so Chrome never breaks
   when no rows are deployed.
2. **Kind is data, family is code.** `BrowserKindId` is a validated string;
   only `BrowserFamily { Chromium, Gecko }` selects behaviour objects
   (`BrowserProfileInspector`, origin rule, extension adapter). Gecko ships as a
   fail-closed stub owned by the Firefox package.
3. **Chrome wire ids are unchanged.** The `chrome` row keeps `chrome-tab:`,
   `chrome-extension://leoocgnkjnplbfdbklajepahofecgfbk/chrome.tabs`,
   `chrome_bridge`, `chrome-bridge-token.txt` and
   `Software\Policies\Google\Chrome`; parity tests pin them. Persisted session
   owner rows need no migration: their kind is derived from the existing
   `endpoint` / `cdp_target_id` (no stored `browser_kind` field).
4. **Per-kind active hosts.** `BridgeHostTable` (one active host per kind)
   replaces `active_host_id`; a same-kind reconnect still replaces and fails only
   that kind's host.
5. **Token-bound kind.** Each row has its own registration token file under
   `%APPDATA%\synapse\`. `/register` derives the kind from which single token file
   matches (constant time); Origin and UA cannot distinguish Chromium browsers
   (same extension id). The worker's hello `browserKind` is cross-checked; a
   mismatch makes the host stale (fail closed). Bearer registrations without a
   token (legacy native host) bind the legacy kind.
6. **Implicit routing, no schema change.** Every bridge command payload carries
   `hwnd` and usually `targetIdHint` / `elementId`; the bridge resolves the kind
   from the window's process (rows sharing an image are split by
   `exe_path_markers`) and the target prefix, refuses cross-kind commands, and
   never guesses (`BROWSER_BRIDGE_KIND_UNRESOLVED`). A non-browser window routes
   to the only active kind (legacy behaviour) and is refused when several kinds
   are active.
7. **Per-row capabilities.** A command whose capability the row lists in
   `unsupported_capabilities` fails per command with
   `BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED` before it is queued; health requires
   the global capability set minus the row's unsupported ones, and the debugger
   API only when `debugger_api_expected`.
8. **Health.** `chrome_bridge` is unchanged (read by `synapse-setup.ps1`).
   Every other row reports `health.subsystems.<health_key>`; a browser that is
   not installed reports `not_installed`, which never degrades overall `ok`.
9. **Extension and installer.** The shared worker reads `bridge-config.json`
   (written by the installer from the row) and stays dormant without it; family
   adapters are static imports. `install-synapse-browser-bridge.ps1 -Browser <id>`
   dispatches to the family installer; setup installs the legacy row as before
   and every other row best-effort (not installed = skip with readback).

## Consequences

- Adding a Chromium browser is a data change plus row tests (no shared file).
- Firefox needs its own gecko inspector, adapter and installer (package-owned
  files) and an owner credential decision (AMO unlisted signing).
- The 9k-line `chrome_debugger_bridge.rs` keeps its Chrome popup-risk scan but
  now runs it per row (`chrome_profile_scan_for(row)`); moving the scan fully
  into `synapse-browsers::chromium` remains follow-up debt.
- New error codes: `BROWSER_BRIDGE_KIND_UNRESOLVED`,
  `BROWSER_BRIDGE_CAPABILITY_UNSUPPORTED`.
