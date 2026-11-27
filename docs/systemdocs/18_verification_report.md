# 18. Verification Report

**Source files covered:**
- Whole-workspace counts gathered by enumerating `crates/**/*.rs`
- `Cargo.toml` (workspace), `clippy.toml`, `deny.toml`
- `crates/synapse-core/src/defaults.rs` (`SCHEMA_VERSION`)
- `.githooks/pre-push`, `README.md`
- Reconciled against companion documents 02, 04, 16, 17

> This document is a measured snapshot. Codebase counts in §1–§2 were re-measured on 2026-11-27 (#288) from tracked files (`git ls-files` / `git grep`, workspace version `0.1.0`). They are reproducible with the commands listed in §6.

---

## 1. Codebase metrics

| Metric | Value | How measured |
|---|---|---|
| Workspace crates | 13 | `Cargo.toml` `[workspace].members` |
| Rust source files (`crates/**/*.rs`) | 429 | `git ls-files 'crates/*.rs' \| wc -l` |
| Lines of Rust (`crates/**/*.rs`) | 339,050 | `git ls-files -z 'crates/*.rs' \| xargs -0 cat \| wc -l` |
| MCP tool attributes (`#[tool(` occurrences) | 245 | `git grep '#\[tool('` over `crates/` (all in `synapse-mcp`) |
| Default public MCP tools | 40 facades | `PUBLIC_TOOL_NAMES` in `crates/synapse-mcp/src/server/tool_profiles.rs`; see [16_api_tools_reference.md](16_api_tools_reference.md) §16.0 |
| Debug-only MCP routes | 4 | `DEBUG_ONLY_TOOL_ROUTES` in `crates/synapse-mcp/src/server.rs` |
| RocksDB column families (named) | 17 (+ implicit `default`) | [04_storage_and_persistence.md](04_storage_and_persistence.md) (not re-checked 2026-11-27) |
| Storage `SCHEMA_VERSION` | 1 | `crates/synapse-core/src/defaults.rs` |
| Error codes (catalog) | ~120 across 9 groups | `crates/synapse-core/src/error_codes.rs` (not re-checked 2026-11-27) |
| Telemetry metrics | 19 (12 counter / 5 gauge / 2 histogram) | [14_core_telemetry_overlay.md](14_core_telemetry_overlay.md) (not re-checked 2026-11-27) |
| Automated tests | 0 | `git grep -E '#\[(tokio::)?test'` over `crates/` finds none; no `tests/` or `benches/` directories |
| Binaries / entry points | 3 | `synapse-mcp`, `synapse-overlay`, `synapse-chrome-native-host` |

### 1.1 Source files per crate

| Crate | `.rs` files |
|---|---|
| synapse-mcp | 217 (214 under `src/`, 3 under `examples/`) |
| synapse-action | 49 |
| synapse-a11y | 37 (incl. 7 under `examples/`) |
| synapse-reflex | 28 |
| synapse-core | 26 |
| synapse-capture | 20 |
| synapse-storage | 15 (incl. 1 under `examples/`) |
| synapse-profiles | 10 |
| synapse-perception | 9 |
| synapse-audio | 8 |
| synapse-models | 7 |
| synapse-telemetry | 2 |
| synapse-overlay | 1 |

---

## 2. Count reconciliations

| Item | Raw signal | Documented figure | Explanation |
|---|---|---|---|
| MCP tools | 245 `#[tool(` attributes | 40 default public facades | The 40 facades are the default public surface; the other attributes are hidden implementation tools behind them, including the 4 debug-only routes. |
| Workspace crates | 13 | earlier drafts said 14 | `synapse-test-utils` was removed with the test suite. |
| Test functions | 0 | earlier drafts said 2,163 | All automated tests were removed by policy; see [17_test_suite.md](17_test_suite.md). |

---

## 3. Lint & dependency configuration

| Control | Status |
|---|---|
| Clippy | `clippy::unwrap_used` / `clippy::expect_used` **denied** workspace-wide (`[workspace.lints.clippy]`). `clippy.toml` still sets `allow-unwrap-in-tests = true` / `allow-expect-in-tests = true`, but there is no test code. |
| Pre-push gate | `.githooks/pre-push`: Gate 0 (rejects any test surface) on every push; `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` when Rust/Cargo files change. |
| Dependency/license gate | `cargo-deny` (`deny.toml`): targets x86_64 linux-gnu + windows-msvc; allowed licenses MIT, Apache-2.0 (+LLVM-exception), BSD-2/3-Clause, MPL-2.0; advisories version 2. |
| CI | None (no `.github/` workflows, Makefile, justfile, or nextest config). |
| Lint status at snapshot | Not executed in this pass (`cargo clippy` not run here) — "Not determined from source". |

---

## 4. Test results

| Item | Result |
|---|---|
| Automated tests | None exist; there is nothing to run. |
| Shipping gate | Manual Full State Verification ("FSV") performed by the agent on the configured Windows host; surfaces in code only as `minimum_manual_fsv` manifest metadata. It is never automated. |

See [17_test_suite.md](17_test_suite.md) for the gate and the FSV process.

---

## 5. Notable constants & magic numbers

| Constant | Value | Source |
|---|---|---|
| `SCHEMA_VERSION` (storage) | 1 (stored as 4-byte BE `u32` under `__schema_version`) | `crates/synapse-core/src/defaults.rs` |
| `DEFAULT_BIND` (daemon) | `127.0.0.1:7700` | mcp config |
| `DEFAULT_AIM_TRACK_EMA_ALPHA` | exported reflex EMA alpha | `crates/synapse-core/src/defaults.rs` |
| Reflex scheduler target | 1 ms (MMCSS waitable timer; 2 ms tokio fallback) | [10](10_reflex_subsystem.md) |
| Reflex starvation threshold | `STARVATION_AFTER = 2 s` | `crates/synapse-reflex/src/conflict.rs` |
| GC eviction per pass | 25% of byte budget | [04](04_storage_and_persistence.md) |
| Disk-pressure state machine | 5 levels (fs2) | [04](04_storage_and_persistence.md) |
| Audio ring | 30 s, 48 kHz, f32, stereo | [08](08_audio_subsystem.md) |
| STT | Whisper tiny INT8, 16 kHz mono, English | [08](08_audio_subsystem.md) |
| Template match | NCC clamped [-1,1]; 10 slots, min conf 0.85 | [07](07_perception_subsystem.md) |
| Frame channel | capacity 2, drop-oldest | [05](05_capture_subsystem.md) |
| Double-click delay | clamp(`GetDoubleClickTime`/4, 30..150) ms | [09](09_action_subsystem.md) |
| Bigram typing speedup | 0.75× over 50 common English bigrams | [09](09_action_subsystem.md) |
| Registered ML models | 1 (RT-DETRv2-S COCO) | [13](13_models_subsystem.md) |
| ORT version | 2.0.0-rc.12 | `Cargo.toml` |
| rmcp version | 1.7.0 | `Cargo.toml` |
| Single-instance duplicate exit code | 3 | `crates/synapse-mcp/src/main.rs` |

---

## 6. Reproducing these counts

```bash
# from the repository root (Git Bash), tracked files only
git ls-files 'crates/*.rs' | wc -l                                  # source files (429)
git ls-files -z 'crates/*.rs' | xargs -0 cat | wc -l               # LOC (339,050)
git ls-files 'crates/*.rs' | awk -F/ '{print $2}' | sort | uniq -c # files per crate
git grep -h '#\[tool(' -- 'crates/*.rs' | wc -l                     # tool attributes (245)
git grep -h -E '#\[(tokio::)?test' -- 'crates/*.rs' | wc -l          # automated tests (0)
```

---

## 7. Documentation series completeness

| Doc | File | Status |
|---|---|---|
| 01 | system_overview.md | ✅ |
| 02 | source_code_map.md | ✅ |
| 03 | configuration.md | ✅ |
| 04 | storage_and_persistence.md | ✅ |
| 05 | capture_subsystem.md | ✅ |
| 06 | accessibility_and_cdp_subsystem.md | ✅ |
| 07 | perception_subsystem.md | ✅ |
| 08 | audio_subsystem.md | ✅ |
| 09 | action_subsystem.md | ✅ |
| 10 | reflex_subsystem.md | ✅ |
| 11 | profiles_subsystem.md | ✅ |
| 13 | models_subsystem.md | ✅ |
| 14 | core_telemetry_overlay.md | ✅ |
| 15 | mcp_server_architecture.md | ✅ |
| 16 | api_tools_reference.md | ✅ |
| 17 | test_suite.md | ✅ |
| 18 | verification_report.md | ✅ (this file) |
