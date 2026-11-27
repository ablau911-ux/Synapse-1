# Security Policy

Synapse gives an AI agent a real local computer-use body: it can move the mouse,
type, press keys, run allowlisted shell commands, launch processes, capture the
screen, and read accessibility trees. Treat it as a
powerful, security-sensitive tool and run it only on machines and accounts where
that level of control is acceptable.

## Reporting a vulnerability

**Please do not report security vulnerabilities through public GitHub issues,
discussions, or pull requests.**

Instead, report privately:

- Preferred: open a [GitHub private security advisory](https://github.com/ChrisRoyse/Synapse/security/advisories/new)
  ("Report a vulnerability").
- Or email **chrisroyseai@gmail.com** with subject `SECURITY: Synapse`.

Please include:

- A description of the issue and its impact.
- Steps to reproduce, or a proof of concept.
- Affected version / commit, OS, and configuration.
- Any suggested remediation.

You can expect an acknowledgement within a few days. Please give a reasonable
opportunity to investigate and ship a fix before any public disclosure. Reports
made in good faith are welcome and appreciated.

## Scope and operational guidance

Because Synapse is designed to control the local machine, the following are
**expected behavior**, not vulnerabilities, when configured by the operator:

- Sending input (keyboard/mouse) to the focused application.
- Capturing the screen and reading UI Automation / accessibility data.
- Running shell commands and launching processes that the operator has
  explicitly allowlisted.

Genuine security issues include, for example: bypassing the configured
allowlists or safety gates, privilege escalation, authentication bypass on the
HTTP transport, leaking secrets/credentials into logs or stored rows, or remote
code execution beyond the operator's configured surface.

### HTTP authentication model

- The HTTP daemon binds `127.0.0.1:7700` by default and refuses a non-loopback
  bind unless the operator passes `--allow-non-loopback`
  (`SYNAPSE_ALLOW_NON_LOOPBACK`).
- **A loopback bind does not check the bearer token.** On a loopback bind the
  daemon relies on the bind itself plus a `Host`/`Origin` guard (only
  `127.0.0.1`, `localhost` and `::1` are accepted). Any local process that can
  reach the loopback port can call every tool, with or without an
  `Authorization` header. Setting `SYNAPSE_BEARER_TOKEN` does not change this.
- **A non-loopback bind requires the bearer token** on every MCP request. It
  also requires an `Origin` header, and the same `Host`/`Origin` allowlist
  still applies.
- The daemon still loads a token at startup: `%APPDATA%\synapse\token.txt` if
  that file exists, otherwise `SYNAPSE_BEARER_TOKEN`. It is used for
  non-loopback binds, by the `--mode connect` bridge, and to derive the Chrome
  extension bridge registration token.

### Hardening recommendations

- Keep the HTTP transport on loopback. Treat every local process running as
  your user as able to drive Synapse; there is no per-client authentication on
  loopback.
- Use a non-loopback bind only on a trusted network, and only with a strong
  token in `%APPDATA%\synapse\token.txt` or `SYNAPSE_BEARER_TOKEN`.
- Run under a least-privilege user account dedicated to automation.
- Keep the shell and launch policies (the `shell` and `process operation=launch`
  facades, internally `act_run_shell` / `act_launch`) as narrow as possible.
- Do not expose the server to untrusted networks.
