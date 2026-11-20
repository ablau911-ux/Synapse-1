# Synapse MCP Windows Container

This image packages the `synapse-mcp` daemon for Windows Docker hosts. It builds the Rust `synapse-mcp.exe` binary in a Windows Server Core build stage, copies it into a Windows Server Core runtime image, and serves MCP over HTTP on TCP port 7700.

This is a Windows-container image. Do not build it from Linux containers or Docker Desktop Linux mode.

## Build

On a Windows Docker host running Windows containers:

```powershell
docker build -f Dockerfile.windows -t synapse-mcp:windows-ltsc2022 .
```

The build needs network access. `synapse-mcp` enables `synapse-models/directml`, which enables `ort/download-binaries` and downloads ONNX Runtime binaries during the Rust build. The Dockerfile copies the resulting runtime DLLs from `target\release\` into the final image next to `synapse-mcp.exe`.

Or with Compose:

```powershell
docker compose -f docker-compose.windows.yml build
```

## Run

```powershell
docker run --rm --name synapse-mcp -p 7700:7700 -e SYNAPSE_BEARER_TOKEN=synapse-local-dev-token synapse-mcp:windows-ltsc2022
```

The container starts:

```powershell
C:\synapse\synapse-mcp.exe --mode http --allow-non-loopback
```

The image sets:

```text
SYNAPSE_BIND=0.0.0.0:7700
SYNAPSE_MODE=http
```

The HTTP transport requires a bearer token. The example above uses `synapse-local-dev-token`; replace it with your own value on any shared machine or network.

Health endpoint:

```powershell
$headers = @{
  Authorization = "Bearer synapse-local-dev-token"
  Origin = "http://localhost:7700"
}
Invoke-WebRequest -UseBasicParsing http://localhost:7700/health -Headers $headers
```

MCP endpoint:

```text
http://localhost:7700/mcp
```

## Run With Compose

```powershell
docker compose -f docker-compose.windows.yml up --build
```

Compose publishes `localhost:7700` and checks `GET /health`.

By default, Compose sets `SYNAPSE_BEARER_TOKEN=synapse-local-dev-token`. Override it when starting the service:

```powershell
$env:SYNAPSE_BEARER_TOKEN = "replace-with-a-secret-token"
docker compose -f docker-compose.windows.yml up --build
```

## MCP Client Config

Use the HTTP MCP endpoint from clients running on the Windows host:

Claude Code `.mcp.json`:

```json
{
  "mcpServers": {
    "synapse": {
      "type": "http",
      "url": "http://localhost:7700/mcp",
      "headers": {
        "Authorization": "Bearer synapse-local-dev-token",
        "Origin": "http://localhost:7700"
      }
    }
  }
}
```

Codex `config.toml`:

```toml
[mcp_servers.synapse]
url = "http://localhost:7700/mcp"

[mcp_servers.synapse.headers]
Authorization = "Bearer synapse-local-dev-token"
Origin = "http://localhost:7700"
```

## Capability Matrix

| Capability | Windows container status | Notes |
| --- | --- | --- |
| MCP HTTP transport | Supported | Served by `synapse-mcp --mode http` on `http://localhost:7700/mcp`. |
| Health checks | Supported | `GET http://localhost:7700/health`. |
| Multi-agent TCP access from host | Supported | Publish `7700:7700`; clients connect to `http://localhost:7700/mcp`. |
| Storage, profiles, and daemon state | Supported inside container | State is container-local unless you add a volume for the configured Synapse DB/profile paths. |
| Windows API availability | Partial | The process runs as a Windows process in the container, not on the host desktop. |
| Accessibility/UI Automation against host desktop apps | Not supported by this packaging | Windows containers run in Session 0 isolation and cannot inspect the interactive user desktop. |
| Screen capture of host desktop | Not supported by this packaging | Session 0/container isolation prevents normal capture of the user's interactive desktop. |
| Keyboard/mouse input automation of host desktop | Not supported by this packaging | Input sent from the container is not a reliable way to control the host's interactive desktop. |
| Audio loopback from host session | Not supported by this packaging | The container does not share the interactive user's audio session. |
| Chrome native messaging into host Chrome | Not supported by this packaging | Native messaging hosts must be installed where the browser runs. |

Use the container for no-install MCP daemon access over HTTP. For desktop automation against a logged-in Windows user's apps, run Synapse directly in that user's Windows session instead of inside a container.
