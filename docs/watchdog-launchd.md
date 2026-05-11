# Scheduling `gob watchdog` on macOS (launchd)

To clean up forgotten VMs automatically, schedule `gob watchdog` with a
user-level launchd agent. The example below runs it every hour at :00.

1. Find the absolute path to your `gob` binary — launchd does not inherit
   your shell `PATH`:

   ```bash
   which gob
   # e.g. /Users/you/.cargo/bin/gob
   ```

2. Create `~/Library/LaunchAgents/com.goblinmode.watchdog.plist`. Replace
   the `<string>` values marked `REPLACE_ME` with the path from step 1 and
   your home directory:

   ```xml
   <?xml version="1.0" encoding="UTF-8"?>
   <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
     "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
   <plist version="1.0">
   <dict>
       <key>Label</key>
       <string>com.goblinmode.watchdog</string>
       <key>ProgramArguments</key>
       <array>
           <string>REPLACE_ME/.cargo/bin/gob</string>
           <string>watchdog</string>
           <string>--max-age</string>
           <string>8</string>
       </array>
       <key>StartCalendarInterval</key>
       <dict>
           <key>Minute</key>
           <integer>0</integer>
       </dict>
       <key>StandardOutPath</key>
       <string>REPLACE_ME/Library/Logs/goblinmode-watchdog.log</string>
       <key>StandardErrorPath</key>
       <string>REPLACE_ME/Library/Logs/goblinmode-watchdog.log</string>
       <key>EnvironmentVariables</key>
       <dict>
           <key>PATH</key>
           <string>/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin</string>
       </dict>
   </dict>
   </plist>
   ```

3. Load it (and start it on each login automatically):

   ```bash
   launchctl load ~/Library/LaunchAgents/com.goblinmode.watchdog.plist
   ```

4. Tail the log to confirm it's running:

   ```bash
   tail -f ~/Library/Logs/goblinmode-watchdog.log
   ```

To stop it: `launchctl unload ~/Library/LaunchAgents/com.goblinmode.watchdog.plist`.

To run it once on demand without waiting for the next tick:
`launchctl start com.goblinmode.watchdog`.

## Secrets note

Launchd jobs run with a restricted environment. If your `config.toml`
uses `_cmd` fields that depend on tools requiring a terminal/keychain
unlock (e.g. `op`, `security`), the watchdog may fail to authenticate.
The most reliable options for an unattended agent are:

- plain-text tokens in `~/.config/goblinmode/config.toml`, or
- `EnvironmentVariables` in the plist (e.g. `HETZNER__API_TOKEN`,
  `TAILSCALE__API_KEY`).

Either way, treat the plist file as sensitive and `chmod 600` it if you
embed secrets.
