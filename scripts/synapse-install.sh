#!/usr/bin/env bash
# =============================================================================
# Synapse installer — WSL entry point.
#
# Synapse's controlling body is ALWAYS the Windows-native synapse-mcp.exe HTTP
# daemon (only Windows has real SendInput / UI Automation / WGC capture; it
# drives Windows windows AND WSLg GUI windows, and reaches WSL CLIs via wsl.exe).
# Installing "in WSL" therefore means: build + run that Windows daemon through
# interop, then point the WSL-side MCP clients (Claude Code, Codex) at it.
#
# This script:
#   1. Verifies it is running in WSL with working interop + a Windows Rust
#      toolchain.
#   2. Syncs this source tree to a LOCAL Windows path (building over
#      \\wsl.localhost bakes transient drive paths into the binary).
#   3. Invokes scripts/synapse-setup.ps1 on the Windows side to build, install,
#      deploy profiles, register the auto-start daemon, and wire Windows clients.
#   4. Wires the WSL-side Claude Code + Codex to the daemon over Streamable HTTP.
#
# Fail-loud: every prerequisite is checked; on any failure the script stops and
# prints exactly what failed and how to fix it. No silent fallbacks.
# =============================================================================
set -euo pipefail

say()  { printf '\033[36m[synapse-install]\033[0m %s\n' "$*"; }
die()  { printf '\033[31m[synapse-install] FATAL:\033[0m %s\n' "$*" >&2; exit 1; }

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIND="127.0.0.1:7700"

# --- 1. Environment checks --------------------------------------------------
say "Checking environment"
grep -qiE 'microsoft|wsl' /proc/version 2>/dev/null \
  || die "Not running under WSL. On native Windows run scripts/synapse-setup.ps1 in PowerShell instead."

CMD="/mnt/c/Windows/System32/cmd.exe"
PWSH="/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
[ -x "$CMD" ]  || die "cmd.exe not found at $CMD — WSL interop to Windows is required."
[ -x "$PWSH" ] || die "powershell.exe not found at $PWSH — WSL interop to Windows is required."

# Resolve the Windows user profile (the Windows user can differ from the WSL user).
WIN_USERPROFILE="$("$CMD" /c 'echo %USERPROFILE%' 2>/dev/null | tr -d '\r')"
[ -n "$WIN_USERPROFILE" ] || die "Could not read Windows %USERPROFILE% via interop."
WIN_HOME_WSL="$(wslpath "$WIN_USERPROFILE")"
say "Windows profile: $WIN_USERPROFILE  ($WIN_HOME_WSL)"

WIN_CARGO="$WIN_HOME_WSL/.cargo/bin/cargo.exe"
[ -x "$WIN_CARGO" ] || die "Windows cargo not found at $WIN_CARGO. Install the Rust toolchain on Windows (https://rustup.rs) — the daemon is a Windows binary and must be built with the Windows toolchain."

WIN_EXE_WSL="$WIN_HOME_WSL/.cargo/bin/synapse-mcp.exe"   # /mnt path used by WSL clients
WIN_EXE_WIN="$WIN_USERPROFILE\\.cargo\\bin\\synapse-mcp.exe"

# Single-quote a value for safe embedding in generated shell code (#259: quote all paths).
synapse_sq() {
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

install_synapse_token_loader() {
  local loader="$HOME/.config/synapse/mcp-env.sh"
  mkdir -p "$(dirname "$loader")"
  cat > "$loader" <<EOF
# Synapse MCP bearer token bridge for WSL agent clients.
# Source this from shell startup before launching Codex/Claude HTTP MCP clients.
# #259: sourcing is idempotent. It only exports the bearer token and defines codex(); it
# creates no files, sets no traps and exports no start-snapshot variables.
_synapse_mcp_token_path=$(synapse_sq "$TOKEN_WSL")

if [ ! -r "\$_synapse_mcp_token_path" ]; then
    printf '%s\n' "SYNAPSE_MCP_TOKEN_UNREADABLE path=\$_synapse_mcp_token_path remediation=start the Windows synapse-mcp daemon or repair token generation" >&2
else
    _synapse_mcp_token="\$(tr -d '\r\n' < "\$_synapse_mcp_token_path")"
    if [ -z "\$_synapse_mcp_token" ]; then
        printf '%s\n' "SYNAPSE_MCP_TOKEN_EMPTY path=\$_synapse_mcp_token_path remediation=regenerate the Synapse MCP bearer token" >&2
    elif [ "\${SYNAPSE_BEARER_TOKEN:-}" != "\$_synapse_mcp_token" ]; then
        export SYNAPSE_BEARER_TOKEN="\$_synapse_mcp_token"
    fi
    unset _synapse_mcp_token
fi
unset _synapse_mcp_token_path

# #259: each codex start gets its own snapshot codex-tool-surface-o<shell PID>-<random>.json.
# The start-snapshot variables reach only the codex child. After codex exits, the snapshot is
# deleted, then proven orphans are pruned: o<pid> only if /proc is readable, /proc/<pid> is
# gone and the file is older than the grace minutes; legacy <n>-<n> names only past the
# retention hours; anything else is kept. Codex's exit status is returned unchanged.
codex() {
    _synapse_codex_surface=$(synapse_sq "$SURFACE_WSL")
    _synapse_codex_dir="\$HOME/.config/synapse/codex-start-snapshots"
    _synapse_codex_grace=$(synapse_sq "$SNAPSHOT_GRACE_MINUTES")
    _synapse_codex_retention=$(synapse_sq "$SNAPSHOT_RETENTION_HOURS")
    _synapse_codex_own=
    _synapse_codex_hash=
    _synapse_codex_count=
    if [ ! -r "\$_synapse_codex_surface" ]; then
        printf '%s\n' "SYNAPSE_CODEX_TOOL_SURFACE_SNAPSHOT_MISSING path=\$_synapse_codex_surface remediation=run scripts/synapse-setup.ps1 to write the current daemon tools/list fingerprint before starting Codex" >&2
    else
        _synapse_codex_hash="\$(sed -n 's/.*"tool_surface_sha256"[[:space:]]*:[[:space:]]*"\([0-9a-fA-F][0-9a-fA-F]*\)".*/\1/p' "\$_synapse_codex_surface" | head -n 1)" || _synapse_codex_hash=
        _synapse_codex_count="\$(sed -n 's/.*"tool_count"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "\$_synapse_codex_surface" | head -n 1)" || _synapse_codex_count=
        if [ -z "\$_synapse_codex_hash" ]; then
            printf '%s\n' "SYNAPSE_CODEX_TOOL_SURFACE_SNAPSHOT_INVALID path=\$_synapse_codex_surface remediation=delete the invalid snapshot and rerun scripts/synapse-setup.ps1" >&2
        else
            _synapse_codex_nonce="\$(od -An -N16 -tx1 /dev/urandom 2>/dev/null | tr -d ' \n')" || _synapse_codex_nonce=
            [ -n "\$_synapse_codex_nonce" ] || _synapse_codex_nonce="\$(date +%s)"
            _synapse_codex_try="\$_synapse_codex_dir/codex-tool-surface-o\$\$-\$_synapse_codex_nonce.json"
            if mkdir -p "\$_synapse_codex_dir" && (set -C; cat "\$_synapse_codex_surface" > "\$_synapse_codex_try") 2>/dev/null; then
                _synapse_codex_own="\$_synapse_codex_try"
            else
                printf '%s\n' "SYNAPSE_CODEX_TOOL_SURFACE_START_SNAPSHOT_FAILED path=\$_synapse_codex_try remediation=repair permissions on \$_synapse_codex_dir before starting Codex" >&2
            fi
        fi
    fi
    # The invocations sit in if-conditions so a caller's set -e cannot abort before cleanup.
    if [ -n "\$_synapse_codex_own" ]; then
        if SYNAPSE_TOOL_SURFACE_HASH_AT_CODEX_START="\$_synapse_codex_hash" \\
            SYNAPSE_TOOL_SURFACE_TOOL_COUNT_AT_CODEX_START="\$_synapse_codex_count" \\
            SYNAPSE_TOOL_SURFACE_SNAPSHOT_AT_CODEX_START="\$_synapse_codex_own" \\
            command codex "\$@"; then
            _synapse_codex_status=0
        else
            _synapse_codex_status=\$?
        fi
    else
        if command codex "\$@"; then _synapse_codex_status=0; else _synapse_codex_status=\$?; fi
    fi
    [ -z "\$_synapse_codex_own" ] || rm -f -- "\$_synapse_codex_own"
    for _synapse_codex_old in "\$_synapse_codex_dir"/codex-tool-surface-*.json; do
        [ -f "\$_synapse_codex_old" ] || continue
        _synapse_codex_tail="\${_synapse_codex_old##*/codex-tool-surface-}"
        case "\$_synapse_codex_tail" in
            o[0-9]*-*.json)
                _synapse_codex_owner="\${_synapse_codex_tail#o}"
                _synapse_codex_owner="\${_synapse_codex_owner%%-*}"
                # Owner and grace are validated separately as non-empty decimals before any
                # /proc lookup; a malformed owner proves nothing, so the file is kept.
                case "\$_synapse_codex_owner" in
                    ''|*[!0-9]*) continue ;;
                esac
                case "\$_synapse_codex_grace" in
                    ''|*[!0-9]*) continue ;;
                esac
                if [ -d /proc/self ] && [ ! -d "/proc/\$_synapse_codex_owner" ] \\
                    && [ -n "\$(find "\$_synapse_codex_old" -prune -mmin +"\$_synapse_codex_grace" 2>/dev/null)" ]; then
                    rm -f -- "\$_synapse_codex_old"
                fi
                ;;
            [0-9]*)
                case "\$_synapse_codex_retention" in
                    ''|*[!0-9]*) continue ;;
                esac
                if [ -n "\$(find "\$_synapse_codex_old" -prune -mmin +"\$((_synapse_codex_retention * 60))" 2>/dev/null)" ]; then
                    rm -f -- "\$_synapse_codex_old"
                fi
                ;;
        esac
    done
    set -- "\$_synapse_codex_status"
    unset _synapse_codex_surface _synapse_codex_dir _synapse_codex_grace _synapse_codex_retention _synapse_codex_own
    unset _synapse_codex_hash _synapse_codex_count _synapse_codex_nonce _synapse_codex_try _synapse_codex_status
    unset _synapse_codex_old _synapse_codex_tail _synapse_codex_owner
    return "\$1"
}
EOF
  chmod 600 "$loader"

  local marker='if [ -f "$HOME/.config/synapse/mcp-env.sh" ]; then'
  local block='# Synapse MCP HTTP bearer token bridge for WSL agent clients.
if [ -f "$HOME/.config/synapse/mcp-env.sh" ]; then
    . "$HOME/.config/synapse/mcp-env.sh"
else
    printf '"'"'%s\n'"'"' "SYNAPSE_MCP_ENV_MISSING path=$HOME/.config/synapse/mcp-env.sh remediation=restore the WSL Synapse MCP env loader" >&2
fi'
  for rc in "$HOME/.profile" "$HOME/.bashrc"; do
    [ -f "$rc" ] || continue
    if ! grep -Fq "$marker" "$rc"; then
      printf '\n%s\n' "$block" >> "$rc"
    fi
  done

  # Populate this process too, so the commands below see the same SoT.
  # shellcheck disable=SC1090
  . "$loader"
  [ -n "${SYNAPSE_BEARER_TOKEN:-}" ] || die "SYNAPSE_BEARER_TOKEN did not load from $TOKEN_WSL."
}

ensure_codex_synapse_policy() {
  local cfg="$1"
  local bind="$2"
  local tmp
  mkdir -p "$(dirname "$cfg")"
  [ -f "$cfg" ] || : > "$cfg"
  tmp="$(mktemp)" || die "Could not allocate a temp file to update $cfg."
  awk -v bind="$bind" '
    function emit() {
      print "[mcp_servers.synapse]"
      print "url = \"http://" bind "/mcp\""
      print "bearer_token_env_var = \"SYNAPSE_BEARER_TOKEN\""
      print "required = true"
      print "default_tools_approval_mode = \"approve\""
      emitted = 1
    }
    BEGIN { in_synapse = 0; found = 0; emitted = 0 }
    /^\[mcp_servers\.synapse\][[:space:]]*$/ {
      found = 1
      in_synapse = 1
      emit()
      next
    }
    /^\[/ && in_synapse {
      in_synapse = 0
    }
    in_synapse {
      if ($0 ~ /^[[:space:]]*(url|bearer_token_env_var|required|default_tools_approval_mode)[[:space:]]*=/) {
        next
      }
      if ($0 ~ /^[[:space:]]*$/) {
        next
      }
      print
      next
    }
    { print }
    END {
      if (!found) {
        if (NR > 0) {
          print ""
        }
        emit()
      }
    }
  ' "$cfg" > "$tmp" || die "Failed to rewrite $cfg with Synapse Codex MCP policy."
  mv "$tmp" "$cfg" || die "Failed to replace $cfg with repaired Synapse Codex MCP policy."
  chmod 600 "$cfg" 2>/dev/null || true

  grep -Fqx "[mcp_servers.synapse]" "$cfg" \
    || die "Codex config $cfg missing [mcp_servers.synapse] after repair."
  grep -Fqx "url = \"http://$bind/mcp\"" "$cfg" \
    || die "Codex config $cfg missing Synapse HTTP URL after repair."
  grep -Fqx 'bearer_token_env_var = "SYNAPSE_BEARER_TOKEN"' "$cfg" \
    || die "Codex config $cfg missing SYNAPSE_BEARER_TOKEN bearer env after repair."
  grep -Fqx 'required = true' "$cfg" \
    || die "Codex config $cfg missing required=true after repair."
  grep -Fqx 'default_tools_approval_mode = "approve"' "$cfg" \
    || die "Codex config $cfg missing default_tools_approval_mode=approve after repair."
}

# --- 2. Sync source to a local Windows path ---------------------------------
SRC_WSL="$WIN_HOME_WSL/synapse-src"
SRC_WIN="$WIN_USERPROFILE\\synapse-src"
say "Syncing source -> $SRC_WIN"
mkdir -p "$SRC_WSL"
rsync -a --delete \
  --exclude='/target' --exclude='/.git' \
  --exclude='/.playwright-mcp' --exclude='*.log' \
  "$REPO_ROOT/" "$SRC_WSL/"
[ -f "$SRC_WSL/Cargo.toml" ] || die "Source sync failed: $SRC_WSL/Cargo.toml missing."
[ -d "$SRC_WSL/crates/synapse-profiles/profiles" ] || die "Source sync failed: bundled profiles missing."

# --- 3. Build + configure the Windows daemon via the PowerShell setup --------
say "Running Windows-side setup (build + daemon + Windows clients) — this can take ~25 min on a cold build"
PS1_WIN="$SRC_WIN\\scripts\\synapse-setup.ps1"
"$PWSH" -NoProfile -ExecutionPolicy Bypass -File "$PS1_WIN" -SourceDir "$SRC_WIN" -Bind "$BIND" \
  || die "Windows-side setup failed. See the [synapse-setup] output above for the exact failing step."

# Read the bearer token before client wiring. The raw token is never printed.
TOKEN_WSL="$(wslpath "$("$CMD" /c 'echo %APPDATA%' 2>/dev/null | tr -d '\r')")/synapse/token.txt"
[ -f "$TOKEN_WSL" ] || die "Token not found at $TOKEN_WSL — the Windows setup did not complete."
TOK="$(tr -d '\r\n' < "$TOKEN_WSL")"
[ -n "$TOK" ] || die "Token at $TOKEN_WSL is empty."
SURFACE_WSL="$(wslpath "$("$CMD" /c 'echo %APPDATA%' 2>/dev/null | tr -d '\r')")/synapse/codex-tool-surface.json"
[ -f "$SURFACE_WSL" ] || die "Codex tool-surface snapshot not found at $SURFACE_WSL — the Windows setup did not complete."
# #259: setup persisted the start-snapshot pruning limits as Windows User variables; read the
# registry value directly (interop children may hold a pre-setup environment block).
read_windows_user_env() {
  "$PWSH" -NoProfile -NonInteractive -Command "[Environment]::GetEnvironmentVariable('$1','User')" 2>/dev/null | tr -d '\r\n'
}
SNAPSHOT_RETENTION_HOURS="$(read_windows_user_env SYNAPSE_CODEX_START_SNAPSHOT_RETENTION_HOURS)" || SNAPSHOT_RETENTION_HOURS=""
SNAPSHOT_GRACE_MINUTES="$(read_windows_user_env SYNAPSE_CODEX_START_SNAPSHOT_GRACE_MINUTES)" || SNAPSHOT_GRACE_MINUTES=""
case "$SNAPSHOT_RETENTION_HOURS:$SNAPSHOT_GRACE_MINUTES" in
  *[!0-9:]*|:*|*:) die "Windows User SYNAPSE_CODEX_START_SNAPSHOT_RETENTION_HOURS/GRACE_MINUTES missing or invalid ('$SNAPSHOT_RETENTION_HOURS'/'$SNAPSHOT_GRACE_MINUTES') — the Windows setup did not complete." ;;
esac
install_synapse_token_loader

# --- 4. Wire WSL-side clients to Streamable HTTP ----------------------------
say "Wiring WSL-side MCP clients"

# Claude Code (WSL): Streamable HTTP -> shared Windows daemon.
if command -v claude >/dev/null 2>&1; then
  claude mcp remove synapse -s user >/dev/null 2>&1 || true
  claude mcp add --scope user --transport http synapse "http://$BIND/mcp" --header "Authorization: Bearer $TOK"
  say "Claude Code (WSL) wired -> Streamable HTTP daemon."
else
  say "claude CLI not found in WSL; skipping Claude Code wiring."
fi

# Codex (WSL): Streamable HTTP -> shared Windows daemon.
CODEX_CFG="$HOME/.codex/config.toml"
if [ -n "$(type -P codex)" ]; then
  # #259: the sourced loader defines a codex() function; wire the real binary.
  command codex mcp remove synapse >/dev/null 2>&1 || true
  command codex mcp add synapse --url "http://$BIND/mcp" --bearer-token-env-var SYNAPSE_BEARER_TOKEN
  ensure_codex_synapse_policy "$CODEX_CFG" "$BIND"
  say "Codex (WSL) wired -> Streamable HTTP daemon with required=true and default_tools_approval_mode=approve."
elif [ -f "$CODEX_CFG" ]; then
  die "Codex config exists at $CODEX_CFG but codex CLI is not on PATH, so the installer cannot safely replace stale synapse config. Install/repair Codex CLI, then re-run."
else
  say "codex CLI/config not found in WSL; skipping Codex wiring."
fi

# --- 5. Verify the daemon is reachable from WSL -----------------------------
say "Verifying daemon health from WSL"
if curl -fsS -m 5 -H "Authorization: Bearer $TOK" "http://$BIND/health" >/dev/null 2>&1; then
  say "Daemon healthy and reachable from WSL on http://$BIND."
else
  die "Daemon not reachable from WSL on http://$BIND. The Windows daemon may not have started; check %LOCALAPPDATA%\\synapse\\logs\\daemon.log."
fi

say "Done. Restart Claude Code / Codex; call the synapse 'health' tool to confirm."
