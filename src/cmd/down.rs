// SPDX-FileCopyrightText: 2026 Miikka Koskinen
//
// SPDX-License-Identifier: MIT

use anyhow::{bail, Result};
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use crate::config;
use crate::hetzner::HetznerClient;
use crate::project;
use crate::state;
use crate::tailscale::TailscaleClient;

trait PauseActions {
    fn shutdown_server(&mut self, server_id: u64) -> Result<()>;
    fn wait_for_server_off(&mut self, server_id: u64) -> Result<()>;
    fn create_image(&mut self, server_id: u64, description: &str) -> Result<u64>;
    fn wait_for_image(&mut self, image_id: u64) -> Result<()>;
    fn delete_server(&mut self, server_id: u64) -> Result<()>;
    fn delete_tailscale_device(&mut self, hostname: &str) -> Result<()>;
    fn remove_known_host(&mut self, host: &str);
    fn save_state(&mut self, project_id: &str, state: &state::ProjectState) -> Result<()>;
}

trait DownActions {
    fn delete_tailscale_device(&mut self, hostname: &str) -> Result<()>;
    fn delete_server(&mut self, server_id: u64) -> Result<()>;
    fn delete_image(&mut self, image_id: u64) -> Result<()>;
    fn remove_known_host(&mut self, host: &str);
    fn remove_git_remote(&mut self, project_root: &Path);
    fn delete_state(&mut self, project_id: &str) -> Result<()>;
}

struct RealDownActions {
    ts_client: TailscaleClient,
    hetzner_client: HetznerClient,
}

impl RealDownActions {
    fn new(cfg: &config::Config) -> Self {
        Self {
            ts_client: TailscaleClient::new(cfg.tailscale_api_key.clone()),
            hetzner_client: HetznerClient::new(cfg.hetzner_api_token.clone()),
        }
    }
}

impl PauseActions for RealDownActions {
    fn shutdown_server(&mut self, server_id: u64) -> Result<()> {
        self.hetzner_client.shutdown_server(server_id)
    }

    fn wait_for_server_off(&mut self, server_id: u64) -> Result<()> {
        self.hetzner_client.wait_for_server_off(server_id)
    }

    fn create_image(&mut self, server_id: u64, description: &str) -> Result<u64> {
        self.hetzner_client.create_image(server_id, description)
    }

    fn wait_for_image(&mut self, image_id: u64) -> Result<()> {
        self.hetzner_client.wait_for_image(image_id)
    }

    fn delete_server(&mut self, server_id: u64) -> Result<()> {
        self.hetzner_client.delete_server(server_id)
    }

    fn delete_tailscale_device(&mut self, hostname: &str) -> Result<()> {
        if self.ts_client.delete_device_by_hostname(hostname)? {
            println!("Tailscale device '{}' deleted.", hostname);
        } else {
            println!(
                "Tailscale device '{}' not found (already removed?).",
                hostname
            );
        }
        Ok(())
    }

    fn remove_known_host(&mut self, host: &str) {
        let _ = Command::new("ssh-keygen").args(["-R", host]).output();
    }

    fn save_state(&mut self, project_id: &str, s: &state::ProjectState) -> Result<()> {
        state::save_state(project_id, s)
    }
}

impl DownActions for RealDownActions {
    fn delete_tailscale_device(&mut self, hostname: &str) -> Result<()> {
        if self.ts_client.delete_device_by_hostname(hostname)? {
            println!("Tailscale device '{}' deleted.", hostname);
        } else {
            println!(
                "Tailscale device '{}' not found (already removed?).",
                hostname
            );
        }
        Ok(())
    }

    fn delete_server(&mut self, server_id: u64) -> Result<()> {
        self.hetzner_client.delete_server(server_id)
    }

    fn delete_image(&mut self, image_id: u64) -> Result<()> {
        self.hetzner_client.delete_image(image_id)
    }

    fn remove_known_host(&mut self, host: &str) {
        let _ = Command::new("ssh-keygen").args(["-R", host]).output();
    }

    fn remove_git_remote(&mut self, project_root: &Path) {
        let _ = Command::new("git")
            .args(["remote", "remove", "gob"])
            .current_dir(project_root)
            .output();
    }

    fn delete_state(&mut self, project_id: &str) -> Result<()> {
        state::delete_state(project_id)
    }
}

pub fn run(destroy: bool) -> Result<()> {
    // 1. Detect project root
    let project = project::detect_project()?;
    println!("Project: {} ({})", project.name, project.root.display());

    // 2. Load existing state
    let existing = match state::load_state(&project.id)? {
        Some(s) => s,
        None => bail!("No server found for this project. Nothing to do."),
    };

    // 3. Load config
    let cfg = config::load_config()?;

    // Default behaviour: pause (snapshot + destroy) to protect work.
    // Fall back to teardown when --destroy is given, or when the server is
    // already paused (server_id == 0) — there is nothing to snapshot in that
    // case, so we just clean up state and any leftover snapshot image.
    if !destroy && existing.server_id != 0 {
        println!("Pausing VM (snapshotting before shutdown)...");
        println!("Tip: use `gob down --destroy` to skip the snapshot.");
        let mut actions = RealDownActions::new(&cfg);
        pause_with(&mut actions, &project, existing)
    } else {
        teardown(&project, &existing, &cfg)
    }
}

/// Destroy all resources for a project. Does not bail if the server is already gone.
pub fn teardown(
    project: &project::Project,
    existing: &state::ProjectState,
    cfg: &config::Config,
) -> Result<()> {
    let mut actions = RealDownActions::new(cfg);
    teardown_with(&mut actions, project, existing)
}

fn pause_with<A: PauseActions>(
    actions: &mut A,
    project: &project::Project,
    existing: state::ProjectState,
) -> Result<()> {
    print!("Shutting down server (id: {})... ", existing.server_id);
    io::stdout().flush()?;
    actions.shutdown_server(existing.server_id)?;
    actions.wait_for_server_off(existing.server_id)?;
    println!("off");

    let description = format!("gob-pause-{}", project.name);
    print!("Creating snapshot... ");
    io::stdout().flush()?;
    let image_id = actions.create_image(existing.server_id, &description)?;
    actions.wait_for_image(image_id)?;
    println!("done (image: {})", image_id);

    print!("Deleting server... ");
    io::stdout().flush()?;
    actions.delete_server(existing.server_id)?;
    println!("done");

    let hostname = existing.hostname_or_default(&project.name);
    if let Err(e) = actions.delete_tailscale_device(&hostname) {
        eprintln!("Warning: failed to remove Tailscale device: {}", e);
    }

    for host in [&hostname, &existing.ipv4] {
        actions.remove_known_host(host);
    }

    let paused_state = state::ProjectState::new(
        0,
        String::new(),
        existing.username,
        existing.hostname,
        Some(image_id),
    );
    actions.save_state(&project.id, &paused_state)?;

    println!("Server paused. Run `gob up` to restore from snapshot.");
    Ok(())
}

fn teardown_with<A: DownActions>(
    actions: &mut A,
    project: &project::Project,
    existing: &state::ProjectState,
) -> Result<()> {
    let hostname = existing.hostname_or_default(&project.name);

    // Remove from Tailscale
    if let Err(e) = actions.delete_tailscale_device(&hostname) {
        eprintln!("Warning: failed to remove Tailscale device: {}", e);
    }

    // Delete Hetzner server
    if existing.server_id != 0 {
        println!(
            "Deleting server {} (id: {})...",
            existing.ipv4, existing.server_id
        );
        match actions.delete_server(existing.server_id) {
            Ok(()) => println!("Server deleted."),
            Err(e) => {
                eprintln!("Warning: failed to delete server: {}", e);
                eprintln!("Clearing local state anyway.");
            }
        }
    }

    // Delete snapshot if present
    if let Some(snapshot_id) = existing.snapshot_id {
        print!("Deleting snapshot (image: {})... ", snapshot_id);
        match actions.delete_image(snapshot_id) {
            Ok(()) => println!("done"),
            Err(e) => eprintln!("Warning: failed to delete snapshot: {}", e),
        }
    }

    // Remove from known_hosts
    for host in [&hostname, &existing.ipv4] {
        actions.remove_known_host(host);
    }

    // Remove git remote
    actions.remove_git_remote(&project.root);

    // Remove state
    actions.delete_state(&project.id)?;
    println!("Done.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[derive(Default)]
    struct MockActions {
        calls: Vec<String>,
        fail_tailscale: bool,
        fail_server: bool,
        fail_image: bool,
    }

    impl DownActions for MockActions {
        fn delete_tailscale_device(&mut self, hostname: &str) -> Result<()> {
            self.calls.push(format!("tailscale:{hostname}"));
            if self.fail_tailscale {
                anyhow::bail!("ts fail");
            }
            Ok(())
        }

        fn delete_server(&mut self, server_id: u64) -> Result<()> {
            self.calls.push(format!("server:{server_id}"));
            if self.fail_server {
                anyhow::bail!("server fail");
            }
            Ok(())
        }

        fn delete_image(&mut self, image_id: u64) -> Result<()> {
            self.calls.push(format!("image:{image_id}"));
            if self.fail_image {
                anyhow::bail!("image fail");
            }
            Ok(())
        }

        fn remove_known_host(&mut self, host: &str) {
            self.calls.push(format!("known_host:{host}"));
        }

        fn remove_git_remote(&mut self, _project_root: &Path) {
            self.calls.push("git_remote".to_string());
        }

        fn delete_state(&mut self, project_id: &str) -> Result<()> {
            self.calls.push(format!("state:{project_id}"));
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockPauseActions {
        calls: std::cell::RefCell<Vec<String>>,
        saved_state: std::cell::RefCell<Option<state::ProjectState>>,
        tailscale_fail: bool,
    }

    impl PauseActions for MockPauseActions {
        fn shutdown_server(&mut self, server_id: u64) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("shutdown:{server_id}"));
            Ok(())
        }

        fn wait_for_server_off(&mut self, server_id: u64) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("wait_off:{server_id}"));
            Ok(())
        }

        fn create_image(&mut self, server_id: u64, description: &str) -> Result<u64> {
            self.calls
                .borrow_mut()
                .push(format!("create_image:{server_id}:{description}"));
            Ok(77)
        }

        fn wait_for_image(&mut self, image_id: u64) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("wait_image:{image_id}"));
            Ok(())
        }

        fn delete_server(&mut self, server_id: u64) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("delete_server:{server_id}"));
            Ok(())
        }

        fn delete_tailscale_device(&mut self, hostname: &str) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("delete_tailscale:{hostname}"));
            if self.tailscale_fail {
                anyhow::bail!("ts fail");
            }
            Ok(())
        }

        fn remove_known_host(&mut self, host: &str) {
            self.calls.borrow_mut().push(format!("known_host:{host}"));
        }

        fn save_state(&mut self, _project_id: &str, s: &state::ProjectState) -> Result<()> {
            self.calls.borrow_mut().push("save_state".to_string());
            self.saved_state.replace(Some(state::ProjectState {
                version: s.version,
                server_id: s.server_id,
                ipv4: s.ipv4.clone(),
                username: s.username.clone(),
                hostname: s.hostname.clone(),
                snapshot_id: s.snapshot_id,
            }));
            Ok(())
        }
    }

    fn project() -> project::Project {
        project::Project {
            root: PathBuf::from("/tmp/proj"),
            name: "myproj".to_string(),
            id: "myproj-1234".to_string(),
        }
    }

    fn pause_project() -> project::Project {
        project::Project {
            root: PathBuf::from("/tmp/proj"),
            name: "proj".to_string(),
            id: "proj-1".to_string(),
        }
    }

    #[test]
    fn pause_with_runs_expected_sequence_and_saves_snapshot_state() {
        let mut actions = MockPauseActions::default();
        let existing = state::ProjectState {
            version: 0,
            server_id: 42,
            ipv4: "1.2.3.4".to_string(),
            username: "alice".to_string(),
            hostname: "".to_string(),
            snapshot_id: None,
        };
        pause_with(&mut actions, &pause_project(), existing).unwrap();

        assert_eq!(
            actions.calls.borrow().as_slice(),
            &[
                "shutdown:42",
                "wait_off:42",
                "create_image:42:gob-pause-proj",
                "wait_image:77",
                "delete_server:42",
                "delete_tailscale:gob-proj",
                "known_host:gob-proj",
                "known_host:1.2.3.4",
                "save_state",
            ]
        );
        let saved = actions.saved_state.borrow();
        let saved = saved.as_ref().unwrap();
        assert_eq!(saved.server_id, 0);
        assert_eq!(saved.snapshot_id, Some(77));
        assert_eq!(saved.username, "alice");
    }

    #[test]
    fn pause_with_continues_when_tailscale_cleanup_fails() {
        let mut actions = MockPauseActions {
            tailscale_fail: true,
            ..Default::default()
        };
        let existing = state::ProjectState {
            version: 0,
            server_id: 42,
            ipv4: "1.2.3.4".to_string(),
            username: "alice".to_string(),
            hostname: "gob-custom".to_string(),
            snapshot_id: None,
        };
        pause_with(&mut actions, &pause_project(), existing).unwrap();
        assert!(actions.calls.borrow().contains(&"save_state".to_string()));
    }

    #[test]
    fn teardown_calls_expected_actions_with_server_and_snapshot() {
        let mut actions = MockActions::default();
        let existing = state::ProjectState {
            version: 0,
            server_id: 42,
            ipv4: "1.2.3.4".to_string(),
            username: "u".to_string(),
            hostname: "".to_string(),
            snapshot_id: Some(99),
        };

        teardown_with(&mut actions, &project(), &existing).unwrap();

        assert_eq!(
            actions.calls,
            vec![
                "tailscale:gob-myproj",
                "server:42",
                "image:99",
                "known_host:gob-myproj",
                "known_host:1.2.3.4",
                "git_remote",
                "state:myproj-1234",
            ]
        );
    }

    #[test]
    fn teardown_skips_server_delete_when_server_id_is_zero() {
        let mut actions = MockActions::default();
        let existing = state::ProjectState {
            version: 0,
            server_id: 0,
            ipv4: "1.2.3.4".to_string(),
            username: "u".to_string(),
            hostname: "gob-custom".to_string(),
            snapshot_id: None,
        };

        teardown_with(&mut actions, &project(), &existing).unwrap();

        assert_eq!(
            actions.calls,
            vec![
                "tailscale:gob-custom",
                "known_host:gob-custom",
                "known_host:1.2.3.4",
                "git_remote",
                "state:myproj-1234",
            ]
        );
    }

    #[test]
    fn teardown_continues_despite_cleanup_errors() {
        let mut actions = MockActions {
            fail_tailscale: true,
            fail_server: true,
            fail_image: true,
            ..Default::default()
        };
        let existing = state::ProjectState {
            version: 0,
            server_id: 42,
            ipv4: "1.2.3.4".to_string(),
            username: "u".to_string(),
            hostname: "gob-custom".to_string(),
            snapshot_id: Some(99),
        };

        teardown_with(&mut actions, &project(), &existing).unwrap();

        assert!(actions.calls.contains(&"state:myproj-1234".to_string()));
    }
}
