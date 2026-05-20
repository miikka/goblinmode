<!--
SPDX-FileCopyrightText: 2026 Miikka Koskinen

SPDX-License-Identifier: MIT
-->

# Goblinmode

CLI tool (`gob`) for managing ephemeral Hetzner dev VMs with Tailscale networking.

## Build & Test

```
cargo build          # build
cargo build 2>&1     # build, check for warnings
cargo test           # run unit tests
just ci              # fmt check + clippy + coverage check (must pass before PR)
```

Binary is `gob`. Tests use the dependency-injection pattern: commands expose a `run_with(deps, ...)` function taking trait objects so pure logic can be tested without hitting real APIs.

The coverage baseline is in `.config/coverage-baseline-linux.json`. When coverage drops (e.g. after removing code), **add tests** — never update the baseline with `--save-baseline` to paper over a regression.

`GOBLINMODE_DATA_DIR` env var overrides the state directory (`~/.local/share/goblinmode/`) for tests that exercise `save_state`/`load_state`/`delete_state`.

## Architecture

- `src/main.rs` — CLI entrypoint using clap. Subcommands: `up`, `down`, `status`/`ps`, `mosh`, `zed`, `prune`
- `src/cmd/up.rs` — Provisions a VM: creates Hetzner server with cloud-init, waits for SSH and cloud-init, syncs project via rsync, optionally sets up dotfiles, adds git remote
- `src/cmd/down.rs` — Snapshots the VM and destroys the server by default; use `--destroy` to skip snapshotting (resume paused VM with `gob up`)
- `src/cmd/status.rs` — Shows VM status for the current project
- `src/cmd/prune.rs` — Lists and deletes all goblinmode-labeled servers on Hetzner
- `src/cmd/mosh.rs` — Connects to VM via mosh
- `src/cmd/zed.rs` — Opens remote project in Zed
- `src/config.rs` — Loads config from `~/.config/goblinmode/config.toml` with env var overrides
- `src/hetzner.rs` — Hetzner Cloud API client (blocking reqwest). Servers are labeled `managed-by=goblinmode`
- `src/tailscale.rs` — Tailscale API client
- `src/project.rs` — Detects project root by walking up to find `.git`
- `src/project_config.rs` — Per-project config (e.g. `serve_ports`) from `.goblinmode.toml`
- `src/state.rs` — Per-project state (server ID, IP, username) in `~/.local/share/goblinmode/`

## Key patterns

- Config values resolve env vars first, then config file (`resolve_value` in config.rs)
- VM provisioning is cloud-init based (user creation, packages, tailscale setup)
- `ensure_running` in up.rs returns early if server already exists and is running
- Dotfiles setup and cloud-init wait only run on initial provisioning, not reconnect
- State is saved immediately after server creation (before polling), so Ctrl-C doesn't orphan servers
- Non-fatal operations (dotfiles, git remote) warn on failure instead of aborting

## Issues and PRs

- The project is hosted with Forgejo at https://forgejo.sargo-hamlet.ts.net/miikka/goblinmode/
- You can use `fj` command-line tool to interact with Forgejo
- Run `cargo fmt` before committing.
- `just ci` should pass before you submit a PR.
- `main` is a protected branch — always work on a feature branch and open a PR.
