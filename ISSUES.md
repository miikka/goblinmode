* The user has to manually create Tailscale auth key
* Tailscale key expiry is not enabled for new devices if they're tagged by the auth key.
* The Tailscale machine is not removed after `gob down`
* The Hetzner API key and Tailscale auth keys are in config.toml. They should be in a password manager.
