* The user has to manually create Tailscale auth key
* Tailscale key expiry is not enabled for new devices if they're tagged by the auth key.
* The Hetzner API key and Tailscale auth keys are in config.toml. They should be in a password manager.
* It should be investigated if the server can be run with IPv6 connectivity only.
* There are no tests for anything.
* There should be a repo-local config file (.config/goblinmode.toml) that specifies which ports should be exposed via Tailscale serve.
