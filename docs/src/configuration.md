# Configuration

Goblin Mode uses two config files:

- **User config** (`~/.config/goblinmode/config.toml`) — API credentials,
  dotfiles, and VM defaults that apply to all projects.
- **Project config** (`.config/goblinmode.toml` in the project root) —
  per-project settings checked into the repository.

## User config (`~/.config/goblinmode/config.toml`)

### Full example

```toml
[hetzner]
# Hetzner Cloud API token (required).
# Option A: plain text (simple, least secure)
api_token = "hzn-..."
# Option B: shell command — stdout becomes the token
api_token_cmd = "op read 'op://Personal/Hetzner/api token'"

[tailscale]
# Tailscale API key for removing old devices (required).
api_key = "tskey-api-..."
api_key_cmd = "security find-generic-password -a goblinmode -s tailscale-api-key -w"

# Tailscale auth key for joining the tailnet (optional).
# When omitted, goblinmode mints a one-time preauthorized key via the
# Tailscale API key (using the configured `tags`, if any).
auth_key = "tskey-auth-..."
auth_key_cmd = "op read 'op://Personal/Tailscale/auth key'"

# ACL tags applied to the VM when it joins the tailnet (optional).
tags = ["tag:goblinmode"]

[dotfiles]
# Git repo to clone as ~/dotfiles on the VM (optional).
repo = "git@github.com:yourname/dotfiles.git"
# Script to run after cloning, relative to ~/dotfiles (optional).
install = "./install.sh"

[vm]
# Extra APT packages to install on every VM (optional).
packages = ["jq", "ripgrep", "tmux"]

# Coding agents to install on every VM (optional).
# Supported values: "claude-code", "opencode", "pi"
coding_agents = ["claude-code"]

# Extra packages to install via cargo-binstall on every VM (optional).
binstall_packages = ["jj-cli"]
```

### Secrets (API tokens and keys)

Secrets (`api_token`, `api_key`, `auth_key`) can be provided in three ways,
checked in this priority order:

1. **Environment variable** — always wins if set and non-empty
2. **Plain text value** — stored directly in the config file
3. **`_cmd` field** — runs a shell command; stdout becomes the secret value

| Secret | Config field | `_cmd` field | Environment variable |
|---|---|---|---|
| Hetzner API token | `hetzner.api_token` | `hetzner.api_token_cmd` | `HETZNER__API_TOKEN` |
| Tailscale API key | `tailscale.api_key` | `tailscale.api_key_cmd` | `TAILSCALE__API_KEY` |
| Tailscale auth key | `tailscale.auth_key` | `tailscale.auth_key_cmd` | `TAILSCALE__AUTH_KEY` |

Environment variables use `__` (double underscore) as a separator for nested
keys. Each `_cmd` is run with `sh -c`, so shell features (pipes,
substitutions) work. The output is trimmed of leading and trailing whitespace.

### Reference

| Key | Type | Required | Description |
|---|---|---|---|
| `hetzner.api_token` | string | yes* | Hetzner Cloud API token |
| `hetzner.api_token_cmd` | string | yes* | Shell command to fetch the token |
| `tailscale.api_key` | string | yes* | Tailscale API key |
| `tailscale.api_key_cmd` | string | yes* | Shell command to fetch the API key |
| `tailscale.auth_key` | string | no | Tailscale auth key for VM enrollment |
| `tailscale.auth_key_cmd` | string | no | Shell command to fetch the auth key |
| `tailscale.tags` | string[] | no | ACL tags applied to the VM |
| `dotfiles.repo` | string | no | Git URL for dotfiles repo |
| `dotfiles.install` | string | no | Install script path relative to `~/dotfiles` |
| `vm.packages` | string[] | no | Extra APT packages installed on the VM |
| `vm.coding_agents` | string[] | no | Coding agents to install (`"claude-code"`, `"opencode"`, `"pi"`) |
| `vm.binstall_packages` | string[] | no | Extra packages installed via cargo-binstall (e.g. `"jj-cli"`) |

\* At least one of the plain-text or `_cmd` variants is required.

## Project config (`.config/goblinmode.toml`)

Place this file at `.config/goblinmode.toml` in the root of your project and
commit it to the repository. It lets each project customize the VM it gets.

### Full example

```toml
# Hetzner server type (default: "cx23").
# See https://www.hetzner.com/cloud for available types.
server_type = "cx33"

# Ports exposed via `tailscale serve` on the VM (optional).
serve_ports = [3000, 8080]

# Extra APT packages installed on this project's VM, in addition to
# the packages listed in the user config (optional).
packages = ["nodejs", "postgresql-client"]

# Extra cargo-binstall packages for this project's VM (optional).
binstall_packages = ["git-absorb"]

# Coding agents to install on this project's VM (optional).
# Supported values: "claude-code", "opencode", "pi"
coding_agents = ["claude-code"]
```

### Reference

| Key | Type | Default | Description |
|---|---|---|---|
| `server_type` | string | `"cx23"` | Hetzner server type for the VM |
| `serve_ports` | integer[] | `[]` | Ports exposed via `tailscale serve` on the VM |
| `packages` | string[] | `[]` | Extra APT packages installed on the VM, merged with user config `vm.packages` |
| `binstall_packages` | string[] | `[]` | Extra packages installed via cargo-binstall, merged with user config `vm.binstall_packages` |
| `coding_agents` | string[] | `[]` | Coding agents to install (`"claude-code"`, `"opencode"`, `"pi"`), merged with user config `vm.coding_agents` |
