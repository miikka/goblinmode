---
name: cargo-insta
description: Use when the user wants to run, review, accept, or reject snapshot tests with cargo-insta.
---

**Key commands:**

- `cargo insta test` — run tests and collect snapshot changes
- `cargo insta review` — interactively review pending snapshots (accept `a`, reject `r`, skip `s`)
- `cargo insta accept` — accept all pending snapshots without review
- `cargo insta reject` — reject all pending snapshots without review
- `cargo insta pending-snapshots` — list snapshots awaiting review

**Typical workflow:**
1. Run `cargo insta test` (or `cargo test`) to generate `.snap.new` files
2. Run `cargo insta review` to walk through each diff and accept or reject

**Useful flags:**
- `--snapshot <path>` — target a specific snapshot file
- `--include-ignored` — include gitignore'd snapshots
