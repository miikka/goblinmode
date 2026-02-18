* The user has to manually create Tailscale auth key
* Tailscale key expiry is not enabled for new devices if they're tagged by the auth key.
* The Hetzner API key and Tailscale auth keys are in config.toml. They should be in a password manager.
* It should be investigated if the server can be run with IPv6 connectivity only.
* There are no tests for anything.
* There should be a repo-local config file (.config/goblinmode.toml) that specifies which ports should be exposed via Tailscale serve.
* `jj` is not installed on the VM
* There should be a way to shutdown the server with a snapshot and restore it later. Like `down` but not destroying everything.
* One of the coding agents - maybe Claude Code or OpenCode - should be set up on the VM.
* There should be some way of automatically shutting down or pausing the VM so that forgotten VMs do not incur costs. This may need the deployment of separate watcher process.
* There should be an explicit `gob prune` command that looks at the VMs running on Hetzner and shuts them down (possibly make use of Hetzner's labels feature to identify them)
* If you press Ctrl-C while waiting the server to be created when running `gob up`, `gob ps` and `gob down` will think that there's no server even though that's not true.
