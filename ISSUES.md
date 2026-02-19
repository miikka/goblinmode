* The user has to manually create Tailscale auth key
* Tailscale key expiry is not enabled for new devices if they're tagged by the auth key.
* The Hetzner API key and Tailscale auth keys are in config.toml. They should be in a password manager.
* It should be investigated if the server can be run with IPv6 connectivity only.
* There are no tests for anything.
* There should be a way to shutdown the server with a snapshot and restore it later. Like `down` but not destroying everything.
* One of the coding agents - maybe Claude Code or OpenCode - should be set up on the VM.
* There should be some way of automatically shutting down or pausing the VM so that forgotten VMs do not incur costs. This may need the deployment of separate watcher process.
* `gob pause` is damn slow. Note: this needs a lot of thought.
* `gob up` is damn slow. Note: this needs a lot of thought.
* `reqwest` crate is out of date
* `toml` crate is out of date`
* The server type should be configurable - put it into the repo config!
* `gob up` should have `--reset` flag that destroys the VM if one is running and recreates it.
