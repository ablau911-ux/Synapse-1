# Contributing to Synapse

Thanks for your interest in Synapse. This document covers how to propose changes
and, importantly, the **licensing terms for contributions** — Synapse is
dual-licensed (noncommercial + paid commercial), so contributions need to be
made under terms that keep that model possible.

## Licensing of contributions

By submitting a contribution (a pull request, patch, or any other work) to this
project, you agree that:

1. **You have the right to submit it.** The contribution is your original work,
   or you otherwise have the rights to submit it under these terms, and you sign
   off on it under the [Developer Certificate of Origin](https://developercertificate.org/)
   (use `git commit -s` to add a `Signed-off-by` line).
2. **You license it to the maintainer broadly.** You grant Chris Royse (the
   project maintainer / licensor) a perpetual, worldwide, non-exclusive,
   royalty-free, irrevocable copyright and patent license to use, reproduce,
   modify, distribute, and **sublicense** your contribution, **including the
   right to license it under the project's [PolyForm Noncommercial
   License](LICENSE.md) and under separate paid [commercial
   licenses](COMMERCIAL-LICENSE.md)**.
3. **You keep your rights too.** You retain copyright in your contribution and
   may use it elsewhere. This grant is in addition to your own rights, not a
   transfer of them.

If you cannot agree to these terms, please do not submit a contribution. If your
employer has rights to work you create, make sure you have permission to
contribute under these terms before doing so.

## Before you start

For anything larger than a small fix, **open an issue first** to discuss the
approach. This avoids wasted work on changes that don't fit the architecture or
the roadmap.

## Development workflow

1. Use the toolchain pinned in `rust-toolchain.toml` (currently Rust 1.96.1);
   rustup installs it automatically. The exact version is pinned so `cargo fmt`
   and `cargo clippy` are reproducible across contributors.
2. Build and check the workspace:
   ```bash
   cargo build --workspace
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   ```
   The repository carries **no automated tests** by rule: there are no
   `tests/` or `benches/` directories, no `#[test]`/`#[tokio::test]`/
   `#[cfg(test)]` code, and no `[dev-dependencies]`. Do not add any; the
   pre-push gate below rejects them.
3. **Enable the local pre-push gate (once per clone):**
   ```bash
   git config core.hooksPath .githooks
   ```
   This repo uses no CI (GitHub Actions are forbidden), so `.githooks/pre-push`
   is the automated backstop. On every push it runs "Gate 0", which blocks any
   commit whose tree carries an automated test surface (test/bench paths, test
   attributes, or `[dev-dependencies]`/`[[test]]`/`[[bench]]` manifest
   sections). When the push touches `.rs`/`Cargo.*` files it also runs
   `cargo fmt --all --check` and
   `cargo clippy --workspace --all-targets -- -D warnings` and blocks the push on
   failure (docs-only pushes skip those two). It is a compile+lint gate, not a
   substitute for manual verification. Bypass only for a genuine non-compiling
   emergency with `git push --no-verify`.
4. **Iterate with fast builds.** Use `cargo check` (~15 s) or `cargo build`
   (dev, ~45 s incremental) for the edit loop. Run `cargo build --release` ONLY
   to ship/run the real `synapse-mcp.exe` daemon — never as compile feedback.
   See [docs/BUILD-AND-MAINTENANCE.md](docs/BUILD-AND-MAINTENANCE.md) for the
   tuned profiles (rust-lld, thin-LTO release, `--profile release-max` for the
   max-runtime binary) and the worktree/`target` disk-hygiene tooling
   (`scripts/repo-maintenance.ps1`) — don't leave throwaway worktrees with their
   own multi-GB `target/` lying around.
5. Synapse is **Windows-native** for its real perception/action paths (Win32
   `SendInput`, UI Automation, WGC/DXGI). Behavior that touches those
   surfaces must be verified on Windows — the project uses manual Full State
   Verification (FSV) on the configured Windows host as the shipping gate: run
   the change through the real Synapse daemon and read back the physical source
   of truth.
6. Keep commits focused and write clear messages. Reference the issue number
   where applicable.

## Pull requests

- Keep PRs scoped to one logical change.
- Include the reasoning and any verification you performed.
- Make sure `cargo fmt --all --check`, the clippy gate, and the build pass
  locally before requesting review.
- Sign off your commits (`git commit -s`) to assert the DCO and the contribution
  terms above.

## Reporting security issues

Do **not** open a public issue for security problems. See
[SECURITY.md](SECURITY.md).
