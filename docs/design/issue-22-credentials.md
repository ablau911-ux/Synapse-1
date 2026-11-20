# Issue #22: child environments and scoped spawn credentials

## Shared child environment policy

`crates/synapse-mcp/src/m4_child_environment.rs` is the one policy every spawn
site in `crates/synapse-mcp/src` uses:

| Class | Sites | Environment |
|---|---|---|
| Untrusted command | `act_launch` (std + `CreateProcessW` block), `act_run_shell`, durable shell jobs | Reconstructed base env; no `SYNAPSE_*TOKEN*/*CREDENTIAL*/*CAPABILITY*`; no `APPDATA`, `LOCALAPPDATA`, `USERPROFILE`, `HOME*` (caller overrides filtered too) |
| Internal helper | ssh JSONL tail, ssh shell cleanup, kill/taskkill, Codex interrupt/steer helpers, context-graph server, video decoders, desktop worker | Inherited daemon env minus Synapse credentials (`internal_helper_command`, `internal_helper_tokio_command`, `inherited_environment_block`) |
| Trusted bootstrap | `TrustedBootstrap::{SharedDaemon, SetupRepair, SpawnedAgent}` | Listed in one enum with reasons; see below |

Trusted bootstrap exceptions:

- `SharedDaemon` (`connect.rs`): the daemon must load the master token.
- `SetupRepair` (`setup.rs`): operator setup manages `token.txt`.
- `SpawnedAgent` (`act_spawn_agent`): keeps profile paths so Claude/Codex find
  their login and config; the only credentials admitted are a live scoped spawn
  credential and the local-agent capability. The master is never admitted.

Test-only `Command::new` calls are out of scope.

## Scoped spawn credential

`crates/synapse-mcp/src/spawn_credentials.rs`:

- Issue: `act_spawn_agent` mints `synapse-spawn-v1.` + hex
  `HMAC-SHA256(master, "synapse-spawn-credential-v1" 0 spawn_id 0 nonce32)`
  (RFC 2104 over `sha2`, RFC 4231 vectors in tests). The daemon stores only its
  SHA-256 digest, keyed by spawn id, in memory.
- Delivery: the value goes into `SYNAPSE_BEARER_TOKEN` in the spawned agent's
  environment, the name already read by the generated Claude MCP config and
  HTTP hooks, the Codex runner (`bearer_token_env_var`), the Codex notify
  script, and the local-model runner. `client_bearer_token()` makes the
  local-model runner and `--mode connect` prefer a scoped env credential over
  `token.txt`; the Codex runner no longer requires `token.txt` to equal the
  spawn bearer.
- Scope (`http/auth.rs` → `authorize_spawn_scoped`): master token first and
  unchanged. Otherwise a scoped credential is accepted only for
  - `/mcp`: one session. The first sessionless `POST` claims the binding; the
    response's `Mcp-Session-Id` binds it; a failed or cancelled initialize
    reopens it. Later requests must carry exactly that session id.
  - `POST /agent-events` whose every `spawn_id` query value is its own spawn.
  - `POST /codex-app-server/request` whose JSON body `spawn_id` is its own.
  - `GET /health` (connect adapter probe; same data as the `health` tool).
  An `X-Synapse-Spawn-Id` header naming another spawn is refused. Every other
  route (shutdown, events, dashboard data, chrome bridge, stats) is refused.
- Revocation: `SpawnCredentialLease` revokes on drop for every failed or
  cancelled spawn path; on success it hands off to the bound session, and
  `teardown_session_with_options_report` revokes it right after marking the
  session terminated (HTTP DELETE, explicit end, idle/stale expiry, adoption).
  A daemon restart revokes everything (in-memory registry).

## Residual exposure and limits

- Environment policy is not an OS boundary. A same-user child can still read a
  discovered `%APPDATA%\synapse\token.txt`; spawned agents keep profile paths by
  design. Closing that needs an OS identity/ACL boundary.
- One MCP session per spawn: a spawned client that re-initializes while its
  session is live is refused. A `--mode connect` adapter cannot reconnect after
  a daemon restart (credential gone).
- Unbound credentials of a successful spawn (should not occur: readiness
  requires the session) live until daemon restart.

## Verification

No cargo or live trigger was run by the fixer. Regression tests:
`crates/synapse-mcp/src/tests/child_environment_22.rs` and
`crates/synapse-mcp/src/tests/spawn_credentials_22.rs`. Manager FSV must cover a
real spawned Claude and Codex agent (MCP tool call, hook/notify ingress, Codex
bridge), cross-spawn and out-of-scope refusals, revocation after session end,
and unchanged operator `token.txt` clients.
