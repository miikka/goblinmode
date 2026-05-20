# Getting started

## Installation

There's no stable release of Goblin Mode yet, so you'll need to install from source:

```bash
git clone https://github.com/miikka/goblinmode.git
cd goblinmode
cargo install --path .
```

This puts the `gob` binary on your `PATH`.

## Initial configuration

Create the config file at `~/.config/goblinmode/config.toml` with your credentials.

* To create a Hetzner API token, open the [Hetzner Console](https://console.hetzner.com/), choose a project, then go to _Security_ → _API tokens_ and click _Generate API token_.
  Goblin Mode needs _Read & Write_ permissions.
* To create a Tailscale API key, open the [Tailscale Admin Console](https://login.tailscale.com/admin/), go to _Settings_ → _Keys_, and click _Generate access token..._

```toml
[hetzner]
api_token = "hzn-..."

[tailscale]
api_key = "tskey-api-..."
```

If you'd prefer to keep the API tokens and keys in a password manager, see the `_cmd` fields in the [Configuration](./configuration.md) section.

## Usage

`cd` into a Git project directory and spin up a VM:

```bash
gob up       # provision a VM and sync the project
gob mosh     # connect to the VM with mosh
gob zed      # open the remote repository in Zed
gob down     # snapshot and destroy the VM when done
```

To see all subcommands, run `gob help`.
