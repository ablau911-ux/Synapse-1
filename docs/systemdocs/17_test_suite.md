# 17. Test Suite

**Source files covered:**
- `.githooks/pre-push` (the only automated gate)
- `CONTRIBUTING.md` (developer workflow)
- `docs/foreground-capability-acceptance-runbook.md` (manual FSV runbook)

Cross-reference: see [18_verification_report.md](18_verification_report.md).

> Updated 2026-11-27 (#288). Earlier versions of this page described an upstream test suite (per-crate `tests/` directories, inline `#[cfg(test)]` modules, the `synapse-test-utils` crate with its `StdioMcpClient`, workspace `tests/fixtures/`, and about 1,590 test functions). None of that exists in this repository any more.

---

## 1. Overview

The repository carries **no automated tests** by rule:

- no `tests/` or `benches/` directory, at workspace level or in any crate;
- no `#[test]`, `#[tokio::test]` or `#[cfg(test)]` code;
- no `[dev-dependencies]`, `[[test]]` or `[[bench]]` manifest sections;
- no `synapse-test-utils` crate (the workspace has 13 crates; see [02_source_code_map.md](02_source_code_map.md)).

There is **no CI**: no `.github/` directory, no `Makefile`, `justfile` or `.config/nextest.toml`. Do not run or document `cargo test`; there is nothing for it to run.

---

## 2. The automated gate: `.githooks/pre-push`

Enable once per clone with `git config core.hooksPath .githooks`. On every push the hook runs, in order:

1. **Gate 0 (every push, including docs-only pushes).** Scans the pushed commit's tree (not the working copy) and blocks the push if it finds any test surface: paths matching `tests/`, `benches/`, `tests.rs` or `_fsv_(driver|support)`; `#[test]`, `#[tokio::test]` or `#[cfg(test)]` in `*.rs`; `[dev-dependencies]`, `[[test]]` or `[[bench]]` in any `Cargo.toml`.
2. **rustfmt gate** (only when the push changes `.rs` or `Cargo.toml`/`Cargo.lock`): `cargo fmt --all --check` under the toolchain pinned in `rust-toolchain.toml` (`1.96.1`).
3. **clippy gate** (same condition): `cargo clippy --workspace --all-targets -- -D warnings`. Warning lines that do not fail the deny level are counted and the log is kept under the Git-private `synapse-pre-push-logs` directory.

Bypass only for a genuine non-compiling emergency: `git push --no-verify`.

---

## 3. Manual Full State Verification (FSV)

**FSV = Full State Verification.** It is the project's manual acceptance gate, performed by the agent on the configured Windows host. It is not an automated test type.

- `docs/foreground-capability-acceptance-runbook.md` is the manual acceptance runbook. It directs the agent to trigger behaviour through the real wired MCP client, then read the source of truth with a separate operation, and not to use any script, helper client or harness as acceptance.
- `manual-fsv-tmp/` is a gitignored scratch directory (`.gitignore` line 63: `/manual-fsv-tmp/`).
- `minimum_manual_fsv` / `manual_fsv_evidence_ref` appear as data fields in bundled profile manifests (e.g. `crates/synapse-profiles/profiles/*.toml`) and in the profile quality and registry code (`crates/synapse-mcp/src/m3/profile_quality*.rs`, `profile_registry.rs`). They record the manual-FSV checklist a curated profile must pass; nothing in the code performs that checklist.

---

## 4. Supporting tools that are not tests

- `crates/*/examples/*.rs` — manual probes and dump utilities (see [02_source_code_map.md](02_source_code_map.md)). They are run by hand and are not acceptance evidence.
- `SYNAPSE_DEBUG_TOOLS=1` registers four debug-only MCP routes (`DEBUG_ONLY_TOOL_ROUTES` in `crates/synapse-mcp/src/server.rs`) used for diagnostics.

---

## 5. What is NOT covered

- Nothing is verified automatically beyond formatting, lint and compilation of all targets.
- Real Windows perception/action behaviour is verified only by manual FSV on the configured host.
- Non-Windows builds compile, but their Windows-only subsystems fail loud and are not exercised.

See [18_verification_report.md](18_verification_report.md) for verification-status reporting.
