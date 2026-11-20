#!/usr/bin/env python3
"""Synapse MCP daemon - authoritative control logic + CLI.

Single source of truth for daemon state and control. Imported by the GUI
(synapse_panel.py) and called by the PowerShell shims (synapse-ensure.ps1,
synapse-ctl.ps1) so the daemon logic lives in exactly one place.

The daemon is the external HTTP MCP server `synapse-mcp.exe` on
http://127.0.0.1:7700/mcp. Default --mode is stdio; HTTP mode needs --mode http.

CLI:
    python synapse_core.py status     # one-line: daemon=ACTIVE autostart=ENABLED ...
    python synapse_core.py start
    python synapse_core.py stop
    python synapse_core.py reclaim     # safe recovery; refuses to kill unknown owners
    python synapse_core.py enable      # autostart on (remove disable flag) + start
    python synapse_core.py disable     # autostart off (write flag) + stop
    python synapse_core.py ensure      # SessionStart: start unless the disable flag is set

stdlib + psutil only.
"""
from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
import threading
import urllib.request
from datetime import datetime
from pathlib import Path

try:
    import psutil
except ImportError:  # graceful: still works, just no owner/pid detail
    psutil = None

HOST = "127.0.0.1"
PORT = 7700
ENDPOINT = f"http://{HOST}:{PORT}/mcp"
EXE = Path(os.environ["USERPROFILE"]) / ".cargo" / "bin" / "synapse-mcp.exe"
FLAG = Path(os.environ["USERPROFILE"]) / ".claude" / "synapse-disabled.flag"
TOKEN = Path(os.environ["APPDATA"]) / "synapse" / "token.txt"

# Windows detached-launch flags
_DETACHED = 0x00000008 | 0x00000200 | 0x08000000  # DETACHED | NEW_GROUP | NO_WINDOW
_NOWINDOW = 0x08000000


class SynapseService:
    """All daemon logic, no UI. The one authoritative implementation."""

    def __init__(self):
        self._health_cache: dict | None = None

    def test_port(self, timeout: float = 0.3) -> bool:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.settimeout(timeout)
            return s.connect_ex((HOST, PORT)) == 0

    def port_owners(self) -> list[tuple[int, str]]:
        """[(pid, name)] of processes LISTENing on PORT."""
        if psutil is None:
            return []
        owners: list[tuple[int, str]] = []
        try:
            for c in psutil.net_connections(kind="tcp"):
                if c.laddr and c.laddr.port == PORT and c.status == psutil.CONN_LISTEN and c.pid:
                    try:
                        name = psutil.Process(c.pid).name()
                    except Exception:
                        name = "?"
                    owners.append((c.pid, name))
        except Exception:
            pass
        # de-dup by pid
        seen, out = set(), []
        for pid, name in owners:
            if pid not in seen:
                seen.add(pid)
                out.append((pid, name))
        return out

    def synapse_procs(self) -> list[int]:
        if psutil is None:
            return []
        pids = []
        for p in psutil.process_iter(["name"]):
            try:
                if (p.info["name"] or "").lower() == "synapse-mcp.exe":
                    pids.append(p.pid)
            except Exception:
                pass
        return pids

    def mcp_ready(self, timeout: float = 1.5) -> bool:
        """Probe the MCP layer with a real `initialize` handshake (deeper than a
        port check): proves the daemon is actually serving MCP, so a fresh client
        (Claude) would get its tools. Cleans up the probe session afterwards."""
        if not self.test_port():
            return False
        try:
            token = TOKEN.read_text().strip() if TOKEN.exists() else ""
        except Exception:
            token = ""
        headers = {"Content-Type": "application/json",
                   "Accept": "application/json, text/event-stream"}
        if token:
            headers["Authorization"] = f"Bearer {token}"
        body = json.dumps({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                       "clientInfo": {"name": "synapse-panel", "version": "1"}},
        }).encode()
        try:
            req = urllib.request.Request(ENDPOINT, data=body, headers=headers, method="POST")
            with urllib.request.urlopen(req, timeout=timeout) as r:
                sid = r.headers.get("Mcp-Session-Id")
                text = r.read().decode("utf-8", "replace")
        except Exception:
            return False
        payload = None
        for line in text.splitlines():
            line = line.strip()
            if line.startswith("data:"):
                line = line[5:].strip()
            if line.startswith("{"):
                try:
                    payload = json.loads(line)
                    break
                except Exception:
                    pass
        ok = bool(payload and "result" in payload)
        if sid:  # close the probe session so we don't churn daemon sessions
            try:
                close_headers = {"Mcp-Session-Id": sid}
                if token:
                    close_headers["Authorization"] = f"Bearer {token}"
                urllib.request.urlopen(
                    urllib.request.Request(ENDPOINT, headers=close_headers, method="DELETE"),
                    timeout=timeout)
            except Exception:
                pass
        return ok

    def health_probe(self, timeout: float = 1.5) -> dict | None:
        """Return authenticated live Synapse identity for this endpoint.

        The local listener may be a forwarding proxy, so its process name is
        diagnostic only. A valid authenticated Synapse health payload is the
        positive service-identity proof.
        """
        if not self.test_port():
            return None
        try:
            token = TOKEN.read_text().strip() if TOKEN.exists() else ""
        except Exception:
            token = ""
        if not token:
            return None
        headers = {"Accept": "application/json"}
        if token:
            headers["Authorization"] = f"Bearer {token}"
        health_url = ENDPOINT.removesuffix("/mcp") + "/health"
        try:
            req = urllib.request.Request(health_url, headers=headers, method="GET")
            with urllib.request.urlopen(req, timeout=timeout) as response:
                payload = json.loads(response.read().decode("utf-8", "replace"))
        except Exception:
            return None
        if not isinstance(payload, dict):
            return None
        pid = payload.get("pid")
        build = payload.get("build")
        tool_count = payload.get("tool_count")
        if not (
            payload.get("ok") is True
            and isinstance(pid, int) and pid > 0
            and isinstance(build, str) and bool(build)
            and isinstance(tool_count, int) and tool_count > 0
        ):
            return None
        http = payload.get("subsystems", {}).get("http", {})
        bind_addr = http.get("bind_addr") if isinstance(http, dict) else None
        return {
            "pid": pid,
            "build": build,
            "tool_count": tool_count,
            "bind_addr": bind_addr if isinstance(bind_addr, str) else None,
        }

    def tool_count(self) -> int:
        """Best-effort tool count from the daemon's tool-surface file (-1 if unknown)."""
        surface = Path(os.environ["APPDATA"]) / "synapse" / "codex-tool-surface.json"
        try:
            return int(json.loads(surface.read_text()).get("tool_count", -1))
        except Exception:
            return -1

    def get_state(self, check_mcp: bool = True) -> dict:
        """Cheap status read. Authenticated health identifies the daemon even
        when a local proxy owns the listening socket. The MCP handshake is only
        run when check_mcp is True (callers throttle it — it opens a probe
        session)."""
        bound = self.test_port()
        owners = self.port_owners() if bound else []
        owner_pids = [pid for pid, name in owners if name.lower() == "synapse-mcp.exe"]
        if not bound:
            self._health_cache = None
        elif check_mcp:
            self._health_cache = self.health_probe()
        health = self._health_cache if bound else None
        syn_pids = [health["pid"]] if health else owner_pids
        result = {
            "bound": bound,
            "owners": owners,
            "is_synapse": bool(syn_pids),
            "syn_pids": syn_pids,
            "autostart": not FLAG.exists(),
            "exe_exists": EXE.exists(),
            "mcp_ready": False,
            "tool_count": health["tool_count"] if health else -1,
            "build": health["build"] if health else None,
            "bind_addr": health["bind_addr"] if health else None,
            "proxied": bool(health and owners and health["pid"] not in {pid for pid, _ in owners}),
        }
        if bound and check_mcp:
            result["mcp_ready"] = self.mcp_ready()
            if result["mcp_ready"] and not health:
                result["tool_count"] = self.tool_count()
        return result

    # --- actions (return a human message) --------------------------------- #
    def _owners_text(self) -> str:
        owners = self.port_owners()
        return ", ".join(f"{name}({pid})" for pid, name in owners) or "unknown owner"

    def _verified_local_daemon(self) -> tuple[dict | None, str]:
        """Return a freshly authenticated daemon only if its PID is locally
        re-verified as synapse-mcp.exe. Never infer kill authority from a port
        owner, a process-name scan, or cached health."""
        health = self.health_probe()
        if health is None:
            return None, "no authenticated Synapse health identity"
        if psutil is None:
            return None, "local process verification is unavailable"
        pid = health["pid"]
        try:
            name = (psutil.Process(pid).name() or "").lower()
        except Exception:
            return None, f"health-reported pid {pid} is not a live local process"
        if name != "synapse-mcp.exe":
            return None, f"health-reported pid {pid} is {name or 'unknown'}, not synapse-mcp.exe"
        return health, ""

    def start(self) -> str:
        if self.test_port():
            health = self.health_probe()
            if health is not None:
                route = ""
                owner_pids = {pid for pid, _ in self.port_owners()}
                if health["pid"] not in owner_pids:
                    route = f" through proxy {self._owners_text()}"
                return f"already active: authenticated daemon pid {health['pid']}{route}"
            return (f"start refused: {HOST}:{PORT} is held by unverified "
                    f"{self._owners_text()}; no process was stopped")
        if not EXE.exists():
            return f"binary missing: {EXE}"
        subprocess.Popen(
            [str(EXE), "--mode", "http", "--bind", f"{HOST}:{PORT}"],
            cwd=str(EXE.parent), creationflags=_DETACHED, close_fds=True,
        )
        for _ in range(12):
            if self.test_port():
                return f"started, listening on {HOST}:{PORT}"
            threading.Event().wait(0.5)
        return f"launch issued; port {PORT} not bound yet"

    def _kill(self, pid: int) -> bool:
        return subprocess.run(["taskkill", "/PID", str(pid), "/F"],
                              creationflags=_NOWINDOW, capture_output=True).returncode == 0

    def stop(self) -> str:
        health, reason = self._verified_local_daemon()
        if health is None:
            return f"stop refused: {reason}; no process was stopped"
        pid = health["pid"]
        if not self._kill(pid):
            return f"stop refused: verified daemon pid {pid} could not be stopped"
        return f"stopped verified authenticated daemon pid {pid}"

    def force_reclaim(self) -> str:
        health = self.health_probe()
        if health is not None:
            owner_pids = {pid for pid, _ in self.port_owners()}
            route = ""
            if health["pid"] not in owner_pids:
                route = f" through proxy {self._owners_text()}"
            return (f"safe reclaim blocked: authenticated Synapse daemon pid "
                    f"{health['pid']} is healthy{route}; no process was stopped")
        if self.test_port():
            return (f"safe reclaim refused: {HOST}:{PORT} is held by unverified "
                    f"{self._owners_text()}; no process was stopped")
        return self.start()

    def enable_autostart(self) -> str:
        try:
            FLAG.unlink()
        except FileNotFoundError:
            pass
        msg = "autostart ENABLED"
        if not self.test_port():
            msg += "; " + self.start()
        return msg

    def disable_autostart(self) -> str:
        FLAG.parent.mkdir(parents=True, exist_ok=True)
        FLAG.write_text("disabled " + datetime.now().strftime("%Y-%m-%d %H:%M:%S"))
        return "autostart DISABLED (flag set); " + self.stop()

    def ensure(self) -> str:
        """SessionStart guarantee: bring the daemon up unless manually disabled."""
        if FLAG.exists():
            return "autostart DISABLED (flag set) - skipping"
        return self.start()

    def status_line(self) -> str:
        s = self.get_state()
        d = "ACTIVE" if (s["bound"] and s["is_synapse"]) else ("PORT-BUSY" if s["bound"] else "DOWN")
        a = "ENABLED" if s["autostart"] else "DISABLED"
        m = f"READY({s['tool_count']} tools)" if s["mcp_ready"] else "OFFLINE"
        return f"synapse daemon={d} mcp={m} autostart={a} endpoint={ENDPOINT}"


def _cli(argv: list[str]) -> int:
    svc = SynapseService()
    cmd = (argv[0] if argv else "status").lower()
    actions = {
        "status": svc.status_line,
        "start": svc.start,
        "stop": svc.stop,
        "reclaim": svc.force_reclaim,
        "enable": svc.enable_autostart,
        "disable": svc.disable_autostart,
        "ensure": svc.ensure,
    }
    fn = actions.get(cmd)
    if fn is None:
        print(f"synapse: unknown command '{cmd}'; use one of: {', '.join(actions)}",
              file=sys.stderr)
        return 2
    print(fn())
    return 0


if __name__ == "__main__":
    raise SystemExit(_cli(sys.argv[1:]))
