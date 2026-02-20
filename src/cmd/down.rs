use anyhow::{bail, Result};
use std::path::Path;
use std::process::Command;

use crate::config;
use crate::hetzner::HetznerClient;
use crate::project;
use crate::state;
use crate::tailscale::TailscaleClient;

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

impl DownActions for RealDownActions {
    fn delete_tailscale_device(&mut self, hostname: &str) -> Result<()> {
        self.ts_client.delete_device_by_hostname(hostname)
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

pub fn run() -> Result<()> {
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

    teardown(&project, &existing, &cfg)
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

fn teardown_with<A: DownActions>(
    actions: &mut A,
    project: &project::Project,
    existing: &state::ProjectState,
) -> Result<()> {
    let hostname = if existing.hostname.is_empty() {
        format!("gob-{}", project.name)
    } else {
        existing.hostname.clone()
    };

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

    fn project() -> project::Project {
        project::Project {
            root: PathBuf::from("/tmp/proj"),
            name: "myproj".to_string(),
            id: "myproj-1234".to_string(),
        }
    }

    #[test]
    fn teardown_calls_expected_actions_with_server_and_snapshot() {
        let mut actions = MockActions::default();
        let existing = state::ProjectState {
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
