"""Regression for normal-profile access to a hidden shell delegate through act."""

import json
import os
import socket
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

import jsonschema
import requests


class BackgroundRouterProfileGateTest(unittest.TestCase):
    def test_act_shell_delegate_obeys_profile_without_disabling_shell_facade(self):
        repo = Path(__file__).resolve().parents[2]
        exe = repo / "target" / "release" / "synapse-mcp.exe"
        profiles = repo / "crates" / "synapse-profiles" / "profiles"
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        url = f"http://127.0.0.1:{port}"
        token = "issue54-regression-synthetic-token"
        with tempfile.TemporaryDirectory(prefix="synapse-issue54-") as root:
            env = os.environ.copy()
            env["SYNAPSE_BEARER_TOKEN"] = token
            appdata = Path(root) / "appdata" / "synapse"
            appdata.mkdir(parents=True)
            (appdata / "token.txt").write_text(token, encoding="ascii")
            env["APPDATA"] = str(appdata.parent)
            with open(Path(root) / "stdout.log", "wb") as stdout, open(
                Path(root) / "stderr.log", "wb"
            ) as stderr:
                process = subprocess.Popen(
                    [str(exe), "--mode", "http", "--bind", f"127.0.0.1:{port}",
                     "--db", str(Path(root) / "db"), "--profile-dir", str(profiles)],
                    env=env,
                    stdout=stdout,
                    stderr=stderr,
                    creationflags=subprocess.CREATE_NO_WINDOW,
                )
                try:
                    headers = {
                        "Authorization": f"Bearer {token}",
                        "Accept": "application/json, text/event-stream",
                        "Content-Type": "application/json",
                    }
                    for _ in range(100):
                        try:
                            health = requests.get(f"{url}/health", headers=headers, timeout=1)
                            if health.ok:
                                break
                        except requests.RequestException:
                            pass
                        time.sleep(0.1)
                    else:
                        self.fail(
                            "isolated Synapse daemon did not become healthy: "
                            + (Path(root) / "stderr.log").read_text(errors="replace")[-1500:]
                        )

                    def rpc(method, params, request_id=None):
                        payload = {"jsonrpc": "2.0", "method": method, "params": params}
                        if request_id is not None:
                            payload["id"] = request_id
                        response = requests.post(
                            f"{url}/mcp", headers=headers, json=payload, timeout=15
                        )
                        response.raise_for_status()
                        data = next(
                            (line[5:].strip() for line in response.text.splitlines()
                             if line.startswith("data:") and line[5:].strip()),
                            None,
                        )
                        return (json.loads(data) if data else None), response.headers

                    _, init_headers = rpc(
                        "initialize",
                        {"protocolVersion": "2025-03-26", "capabilities": {},
                         "clientInfo": {"name": "issue54-regression", "version": "1"}},
                        1,
                    )
                    headers["Mcp-Session-Id"] = init_headers["Mcp-Session-Id"]
                    rpc("notifications/initialized", {})
                    listed, _ = rpc("tools/list", {}, 2)
                    tools = {tool["name"]: tool for tool in listed["result"]["tools"]}
                    self.assertEqual(len(tools), 42)
                    self.assertIn("act", tools)
                    self.assertIn("shell", tools)
                    self.assertNotIn("act_run_shell", tools)
                    command = r"C:\Windows\System32\whoami.exe"
                    delegated = {"operation": "invoke", "action": {
                        "verb": "run_shell", "command": command}}
                    jsonschema.validate(delegated, tools["act"]["inputSchema"])
                    denied, _ = rpc("tools/call", {"name": "act", "arguments": delegated}, 3)
                    self.assertEqual(
                        denied["error"]["data"]["code"], "TOOL_PROFILE_POLICY_DENIED"
                    )
                    allowed = {"operation": "run", "command": command}
                    jsonschema.validate(allowed, tools["shell"]["inputSchema"])
                    result, _ = rpc("tools/call", {"name": "shell", "arguments": allowed}, 4)
                    self.assertEqual(result["result"]["structuredContent"]["run"]["exit_code"], 0)
                finally:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=10)


if __name__ == "__main__":
    unittest.main()
