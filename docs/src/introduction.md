<!--
SPDX-FileCopyrightText: 2026 Miikka Koskinen

SPDX-License-Identifier: MIT
-->

# Introduction

**Goblin Mode** is a command-line tool that spins up a per-project development virtual machine (VM).
The goal: after you've run `gob up`, you can SSH in and start developing.
Goblin Mode is opinionated — it sets things up just the way [the author](#author) likes them.

Here's what Goblin Mode does when you run `gob up`:

- A Debian VM is started on [Hetzner](https://www.hetzner.com/).
- [Tailscale](https://tailscale.com/) is set up for VPN connectivity.
  - You can configure which ports to expose with `tailscale serve`.
- cloud-init configures the VM. Packages are installed with apt and [cargo-binstall](https://github.com/cargo-bins/cargo-binstall).
- The local git repository is pushed to the VM.
  - The VM repo is added as a remote called `gob` in the local repo, so you can sync changes back with `git fetch gob`.
- Goblin Mode detects the programming language for your project and installs the toolchain.
  - Supported languages: Rust (rustup/cargo) and Python (uv).

Once you're done, destroy the VM with `gob down`. A snapshot is saved by default, so you can pick up where you left off.

## Author

Goblin Mode was created by [Miikka Koskinen](https://miikka.me/).
I've written about it [here on my blog](https://quanttype.net/p/goblin-mode/).
It's [software that I made for myself](https://quanttype.net/p/software-for-myself/).


## License

MIT. See `LICENSES` and `REUSE.toml` for full details.
