# goblinmode

CLI tool (`gob`) for managing ephemeral Hetzner dev VMs with Tailscale networking.

## Debug Traces

Use `--trace` to write structured JSON logs for debugging command execution:

```bash
gob up --trace
```

This writes a file like `gob-trace-<timestamp>.jsonl` in the current directory.
You can also choose the output path:

```bash
gob up --trace /tmp/gob-up-trace.jsonl
```

## Configuration

Config file: `~/.config/goblinmode/config.toml`

### Secrets (API tokens and keys)

Secrets can be provided in three ways, in priority order:

1. **Environment variable** — always wins if set and non-empty
2. **`_cmd` field** — shell command whose stdout is the secret value
3. **Plain text value** — stored directly in the config file

#### Example config

```toml
[hetzner]
# Option A: plain text (simple, least secure)
api_token = "hzn-..."

# Option B: macOS Keychain
api_token_cmd = "security find-generic-password -a goblinmode -s hetzner-api-token -w"

# Option C: 1Password CLI
api_token_cmd = "op read 'op://Personal/Hetzner/api token'"

[tailscale]
# Mix and match sources for different secrets
api_key_cmd = "security find-generic-password -a goblinmode -s tailscale-api-key -w"
auth_key_cmd = "op read 'op://Personal/Tailscale/auth key'"
tags = ["tag:goblinmode"]
```

Supported `_cmd` fields:
- `hetzner.api_token_cmd` (env: `HETZNER_API_TOKEN`)
- `tailscale.api_key_cmd` (env: `TAILSCALE_API_KEY`)
- `tailscale.auth_key_cmd` (env: `TAILSCALE_AUTH_KEY`)

The command is run with `sh -c`, so shell features (pipes, substitutions) work.
Output is trimmed of leading/trailing whitespace.
