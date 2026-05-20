// SPDX-FileCopyrightText: 2026 Miikka Koskinen
//
// SPDX-License-Identifier: MIT

use anyhow::Result;
use std::io::{self, Write};
use std::process::Command;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use tracing::{info, instrument, warn};

use crate::config;
use crate::hetzner::{HetznerClient, ServerInfo};
use crate::tailscale::TailscaleClient;

pub const DEFAULT_MAX_AGE_HOURS: u64 = 8;

trait WatchdogRunner {
    fn now(&self) -> OffsetDateTime;
    fn list_servers(&mut self) -> Result<Vec<ServerInfo>>;
    fn pause_server(&mut self, server: &ServerInfo) -> Result<u64>;
    fn delete_tailscale_device(&mut self, hostname: &str) -> Result<bool>;
    fn remove_known_host(&mut self, host: &str);
    fn line(&mut self, message: String);
}

struct RealRunner {
    hetzner: HetznerClient,
    tailscale: TailscaleClient,
}

impl RealRunner {
    fn new(cfg: &config::Config) -> Self {
        Self {
            hetzner: HetznerClient::new(cfg.hetzner_api_token.clone()),
            tailscale: TailscaleClient::new(cfg.tailscale_api_key.clone()),
        }
    }
}

impl WatchdogRunner for RealRunner {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn list_servers(&mut self) -> Result<Vec<ServerInfo>> {
        self.hetzner.list_goblinmode_servers()
    }

    fn pause_server(&mut self, server: &ServerInfo) -> Result<u64> {
        let description = snapshot_description(&server.name);
        print!("  shutting down... ");
        io::stdout().flush().ok();
        self.hetzner.shutdown_server(server.id)?;
        self.hetzner.wait_for_server_off(server.id)?;
        print!("off, snapshotting... ");
        io::stdout().flush().ok();
        let image_id = self.hetzner.create_image(server.id, &description)?;
        self.hetzner.wait_for_image(image_id)?;
        print!("done (image: {}), deleting server... ", image_id);
        io::stdout().flush().ok();
        self.hetzner.delete_server(server.id)?;
        println!("done");
        Ok(image_id)
    }

    fn delete_tailscale_device(&mut self, hostname: &str) -> Result<bool> {
        self.tailscale.delete_device_by_hostname(hostname)
    }

    fn remove_known_host(&mut self, host: &str) {
        let _ = Command::new("ssh-keygen").args(["-R", host]).output();
    }

    fn line(&mut self, message: String) {
        println!("{}", message);
    }
}

/// Snapshot description used by the watchdog, matching the format used by
/// `gob down` so that future `gob up` restoration finds the right image.
fn snapshot_description(server_name: &str) -> String {
    let project = server_name.strip_prefix("gob-").unwrap_or(server_name);
    format!("gob-pause-{}", project)
}

#[instrument(level = "info", skip_all, fields(max_age_hours = max_age_hours, dry_run = dry_run))]
pub fn run(max_age_hours: u64, dry_run: bool) -> Result<()> {
    let cfg = config::load_config()?;
    let mut runner = RealRunner::new(&cfg);
    run_with(&mut runner, max_age_hours, dry_run)
}

fn run_with<R: WatchdogRunner>(runner: &mut R, max_age_hours: u64, dry_run: bool) -> Result<()> {
    let servers = runner.list_servers()?;
    if servers.is_empty() {
        runner.line("No goblinmode servers found.".to_string());
        return Ok(());
    }

    let now = runner.now();
    let cutoff = now - Duration::hours(max_age_hours as i64);

    let mut to_pause: Vec<ServerInfo> = Vec::new();
    for s in servers {
        let created = match OffsetDateTime::parse(&s.created, &Rfc3339) {
            Ok(t) => t,
            Err(e) => {
                warn!(server_id = s.id, created = %s.created, error = %e, "invalid_created_timestamp");
                runner.line(format!(
                    "Skipping {} (id: {}): unparseable created timestamp '{}'",
                    s.name, s.id, s.created
                ));
                continue;
            }
        };
        let age_hours = (now - created).whole_hours();
        let eligible = s.status == "running" && created <= cutoff;
        let verb = if eligible { "Pausing" } else { "Keeping" };
        runner.line(format!(
            "{} {} (id: {}, status: {}, age: {}h)",
            verb, s.name, s.id, s.status, age_hours
        ));
        if eligible {
            to_pause.push(s);
        }
    }

    if to_pause.is_empty() {
        runner.line(format!(
            "No servers older than {}h to pause.",
            max_age_hours
        ));
        return Ok(());
    }

    if dry_run {
        runner.line(format!(
            "Dry-run: {} server(s) would be paused.",
            to_pause.len()
        ));
        return Ok(());
    }

    for s in &to_pause {
        runner.line(format!("Pausing {} (id: {})...", s.name, s.id));
        match runner.pause_server(s) {
            Ok(image_id) => {
                info!(
                    server_id = s.id,
                    image_id = image_id,
                    "watchdog_paused_server"
                );
            }
            Err(e) => {
                runner.line(format!("  failed: {}", e));
                continue;
            }
        }
        match runner.delete_tailscale_device(&s.name) {
            Ok(true) => runner.line(format!("  Tailscale device '{}' deleted.", s.name)),
            Ok(false) => runner.line(format!(
                "  Tailscale device '{}' not found (already removed?).",
                s.name
            )),
            Err(e) => runner.line(format!(
                "  warning: failed to remove Tailscale device '{}': {}",
                s.name, e
            )),
        }
        for host in [&s.name, &s.ipv4] {
            runner.remove_known_host(host);
        }
    }

    runner.line("Watchdog complete.".to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct MockRunner {
        now: OffsetDateTime,
        servers: Vec<ServerInfo>,
        pause_results: RefCell<Vec<Result<u64>>>,
        tailscale_results: RefCell<Vec<Result<bool>>>,
        calls: RefCell<Vec<String>>,
        lines: RefCell<Vec<String>>,
    }

    impl MockRunner {
        fn new(now: OffsetDateTime, servers: Vec<ServerInfo>) -> Self {
            Self {
                now,
                servers,
                pause_results: RefCell::new(Vec::new()),
                tailscale_results: RefCell::new(Vec::new()),
                calls: RefCell::new(Vec::new()),
                lines: RefCell::new(Vec::new()),
            }
        }
    }

    impl WatchdogRunner for MockRunner {
        fn now(&self) -> OffsetDateTime {
            self.now
        }

        fn list_servers(&mut self) -> Result<Vec<ServerInfo>> {
            Ok(std::mem::take(&mut self.servers))
        }

        fn pause_server(&mut self, server: &ServerInfo) -> Result<u64> {
            self.calls.borrow_mut().push(format!("pause:{}", server.id));
            let mut results = self.pause_results.borrow_mut();
            if results.is_empty() {
                Ok(42)
            } else {
                results.remove(0)
            }
        }

        fn delete_tailscale_device(&mut self, hostname: &str) -> Result<bool> {
            self.calls.borrow_mut().push(format!("ts:{}", hostname));
            let mut results = self.tailscale_results.borrow_mut();
            if results.is_empty() {
                Ok(true)
            } else {
                results.remove(0)
            }
        }

        fn remove_known_host(&mut self, host: &str) {
            self.calls.borrow_mut().push(format!("known_host:{}", host));
        }

        fn line(&mut self, message: String) {
            self.lines.borrow_mut().push(message);
        }
    }

    fn server(id: u64, name: &str, status: &str, created: &str) -> ServerInfo {
        ServerInfo {
            id,
            name: name.to_string(),
            status: status.to_string(),
            ipv4: "1.2.3.4".to_string(),
            created: created.to_string(),
        }
    }

    fn now() -> OffsetDateTime {
        OffsetDateTime::parse("2026-05-11T12:00:00+00:00", &Rfc3339).unwrap()
    }

    #[test]
    fn snapshot_description_matches_pause_format() {
        assert_eq!(snapshot_description("gob-myproj"), "gob-pause-myproj");
        assert_eq!(snapshot_description("custom"), "gob-pause-custom");
    }

    #[test]
    fn run_with_empty_list_reports_no_servers() {
        let mut runner = MockRunner::new(now(), vec![]);
        run_with(&mut runner, 8, false).unwrap();
        assert!(runner
            .lines
            .borrow()
            .contains(&"No goblinmode servers found.".to_string()));
        assert!(runner.calls.borrow().is_empty());
    }

    #[test]
    fn run_with_keeps_servers_younger_than_max_age() {
        // Created 1 hour ago, max-age is 8 hours → keep
        let s = server(1, "gob-a", "running", "2026-05-11T11:00:00+00:00");
        let mut runner = MockRunner::new(now(), vec![s]);
        run_with(&mut runner, 8, false).unwrap();
        assert!(runner.calls.borrow().is_empty());
        let lines = runner.lines.borrow();
        assert!(lines.iter().any(|l| l.starts_with("Keeping gob-a")));
        assert!(lines.iter().any(|l| l.contains("No servers older than 8h")));
    }

    #[test]
    fn run_with_pauses_running_servers_older_than_max_age() {
        // Created 10 hours ago, max-age 8 → pause
        let s = server(42, "gob-old", "running", "2026-05-11T02:00:00+00:00");
        let mut runner = MockRunner::new(now(), vec![s]);
        run_with(&mut runner, 8, false).unwrap();
        let calls = runner.calls.borrow();
        assert_eq!(
            calls.as_slice(),
            &[
                "pause:42",
                "ts:gob-old",
                "known_host:gob-old",
                "known_host:1.2.3.4",
            ]
        );
        assert!(runner
            .lines
            .borrow()
            .contains(&"Watchdog complete.".to_string()));
    }

    #[test]
    fn run_with_skips_non_running_servers_even_when_old() {
        let s = server(42, "gob-old", "off", "2026-05-11T02:00:00+00:00");
        let mut runner = MockRunner::new(now(), vec![s]);
        run_with(&mut runner, 8, false).unwrap();
        assert!(runner.calls.borrow().is_empty());
        assert!(runner
            .lines
            .borrow()
            .iter()
            .any(|l| l.starts_with("Keeping gob-old")));
    }

    #[test]
    fn run_with_dry_run_lists_but_does_not_pause() {
        let s = server(42, "gob-old", "running", "2026-05-11T02:00:00+00:00");
        let mut runner = MockRunner::new(now(), vec![s]);
        run_with(&mut runner, 8, true).unwrap();
        assert!(runner.calls.borrow().is_empty());
        let lines = runner.lines.borrow();
        assert!(lines.iter().any(|l| l.starts_with("Pausing gob-old")));
        assert!(lines
            .iter()
            .any(|l| l.contains("Dry-run: 1 server(s) would be paused")));
    }

    #[test]
    fn run_with_handles_invalid_created_timestamp() {
        let s = server(7, "gob-bad", "running", "not-a-date");
        let mut runner = MockRunner::new(now(), vec![s]);
        run_with(&mut runner, 8, false).unwrap();
        assert!(runner.calls.borrow().is_empty());
        let lines = runner.lines.borrow();
        assert!(lines
            .iter()
            .any(|l| l.contains("unparseable created timestamp")));
    }

    #[test]
    fn run_with_continues_after_pause_failure() {
        let s1 = server(1, "gob-a", "running", "2026-05-11T02:00:00+00:00");
        let s2 = server(2, "gob-b", "running", "2026-05-11T02:00:00+00:00");
        let mut runner = MockRunner::new(now(), vec![s1, s2]);
        runner
            .pause_results
            .borrow_mut()
            .push(Err(anyhow::anyhow!("boom")));
        runner.pause_results.borrow_mut().push(Ok(99));
        run_with(&mut runner, 8, false).unwrap();
        let calls = runner.calls.borrow();
        // First server: pause fails, no follow-up tailscale call.
        // Second server: pause succeeds, tailscale + known_host cleanup runs.
        assert_eq!(
            calls.as_slice(),
            &[
                "pause:1",
                "pause:2",
                "ts:gob-b",
                "known_host:gob-b",
                "known_host:1.2.3.4",
            ]
        );
        assert!(runner
            .lines
            .borrow()
            .iter()
            .any(|l| l.contains("failed: boom")));
    }

    #[test]
    fn run_with_tailscale_failure_is_non_fatal() {
        let s = server(1, "gob-a", "running", "2026-05-11T02:00:00+00:00");
        let mut runner = MockRunner::new(now(), vec![s]);
        runner
            .tailscale_results
            .borrow_mut()
            .push(Err(anyhow::anyhow!("ts down")));
        run_with(&mut runner, 8, false).unwrap();
        let lines = runner.lines.borrow();
        assert!(lines
            .iter()
            .any(|l| l.contains("warning: failed to remove Tailscale")));
        assert!(lines.contains(&"Watchdog complete.".to_string()));
    }

    #[test]
    fn run_with_tailscale_device_not_found_is_logged() {
        let s = server(1, "gob-a", "running", "2026-05-11T02:00:00+00:00");
        let mut runner = MockRunner::new(now(), vec![s]);
        runner.tailscale_results.borrow_mut().push(Ok(false));
        run_with(&mut runner, 8, false).unwrap();
        let lines = runner.lines.borrow();
        assert!(lines
            .iter()
            .any(|l| l.contains("not found (already removed?)")));
    }
}
