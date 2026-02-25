use anyhow::{bail, Result};
use std::io::{self, Write};
use std::process::Command;

use crate::config;
use crate::hetzner::HetznerClient;
use crate::project;
use crate::state;
use crate::tailscale::TailscaleClient;

trait PauseDeps {
    fn detect_project(&self) -> Result<project::Project>;
    fn load_state(&self, project_id: &str) -> Result<Option<state::ProjectState>>;
    fn load_config(&self) -> Result<config::Config>;
}

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

struct RealPauseDeps;

impl PauseDeps for RealPauseDeps {
    fn detect_project(&self) -> Result<project::Project> {
        project::detect_project()
    }

    fn load_state(&self, project_id: &str) -> Result<Option<state::ProjectState>> {
        state::load_state(project_id)
    }

    fn load_config(&self) -> Result<config::Config> {
        config::load_config()
    }
}

struct RealPauseActions {
    hetzner: HetznerClient,
    tailscale: TailscaleClient,
}

impl RealPauseActions {
    fn new(cfg: &config::Config) -> Self {
        Self {
            hetzner: HetznerClient::new(cfg.hetzner_api_token.clone()),
            tailscale: TailscaleClient::new(cfg.tailscale_api_key.clone()),
        }
    }
}

impl PauseActions for RealPauseActions {
    fn shutdown_server(&mut self, server_id: u64) -> Result<()> {
        self.hetzner.shutdown_server(server_id)
    }

    fn wait_for_server_off(&mut self, server_id: u64) -> Result<()> {
        self.hetzner.wait_for_server_off(server_id)
    }

    fn create_image(&mut self, server_id: u64, description: &str) -> Result<u64> {
        self.hetzner.create_image(server_id, description)
    }

    fn wait_for_image(&mut self, image_id: u64) -> Result<()> {
        self.hetzner.wait_for_image(image_id)
    }

    fn delete_server(&mut self, server_id: u64) -> Result<()> {
        self.hetzner.delete_server(server_id)
    }

    fn delete_tailscale_device(&mut self, hostname: &str) -> Result<()> {
        self.tailscale.delete_device_by_hostname(hostname)
    }

    fn remove_known_host(&mut self, host: &str) {
        let _ = Command::new("ssh-keygen").args(["-R", host]).output();
    }

    fn save_state(&mut self, project_id: &str, state: &state::ProjectState) -> Result<()> {
        state::save_state(project_id, state)
    }
}

pub fn run() -> Result<()> {
    let deps = RealPauseDeps;
    run_with(&deps)
}

/// Pause a running server: snapshot it, destroy the server, update state.
/// Exposed for use by `gob down` (which pauses by default).
pub fn pause(
    project: &project::Project,
    existing: state::ProjectState,
    cfg: &config::Config,
) -> Result<()> {
    let mut actions = RealPauseActions::new(cfg);
    pause_with(&mut actions, project, existing)
}

fn run_with<D: PauseDeps>(deps: &D) -> Result<()> {
    let project = deps.detect_project()?;
    println!("Project: {} ({})", project.name, project.root.display());

    let existing = match deps.load_state(&project.id)? {
        Some(s) => s,
        None => bail!("No server found for this project. Nothing to do."),
    };

    if existing.server_id == 0 {
        bail!("No running server for this project. Nothing to pause.");
    }

    let cfg = deps.load_config()?;
    let mut actions = RealPauseActions::new(&cfg);
    pause_with(&mut actions, &project, existing)
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

    let hostname = if existing.hostname.is_empty() {
        format!("gob-{}", project.name)
    } else {
        existing.hostname.clone()
    };
    if let Err(e) = actions.delete_tailscale_device(&hostname) {
        eprintln!("Warning: failed to remove Tailscale device: {}", e);
    }

    for host in [&hostname, &existing.ipv4] {
        actions.remove_known_host(host);
    }

    let paused_state = state::ProjectState {
        server_id: 0,
        ipv4: String::new(),
        username: existing.username,
        hostname: existing.hostname,
        snapshot_id: Some(image_id),
        applied_runtime: existing.applied_runtime,
        applied_provisioning: existing.applied_provisioning,
    };
    actions.save_state(&project.id, &paused_state)?;

    println!("Server paused. Run `gob up` to restore from snapshot.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    struct MockDeps {
        project: project::Project,
        state: Option<state::ProjectState>,
    }

    impl PauseDeps for MockDeps {
        fn detect_project(&self) -> Result<project::Project> {
            Ok(project::Project {
                root: self.project.root.clone(),
                name: self.project.name.clone(),
                id: self.project.id.clone(),
            })
        }

        fn load_state(&self, _project_id: &str) -> Result<Option<state::ProjectState>> {
            Ok(self.state.as_ref().map(|s| state::ProjectState {
                server_id: s.server_id,
                ipv4: s.ipv4.clone(),
                username: s.username.clone(),
                hostname: s.hostname.clone(),
                snapshot_id: s.snapshot_id,
                applied_runtime: s.applied_runtime.clone(),
                applied_provisioning: s.applied_provisioning.clone(),
            }))
        }

        fn load_config(&self) -> Result<config::Config> {
            Ok(config::Config {
                hetzner_api_token: "h".to_string(),
                tailscale_auth_key: None,
                tailscale_api_key: "t".to_string(),
                tailscale_tags: vec![],
                dotfiles_repo: None,
                dotfiles_install: None,
                vm_packages: vec![],
                coding_agents: vec![],
            })
        }
    }

    #[derive(Default)]
    struct MockActions {
        calls: RefCell<Vec<String>>,
        saved_state: RefCell<Option<state::ProjectState>>,
        tailscale_fail: bool,
    }

    impl PauseActions for MockActions {
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
                server_id: s.server_id,
                ipv4: s.ipv4.clone(),
                username: s.username.clone(),
                hostname: s.hostname.clone(),
                snapshot_id: s.snapshot_id,
                applied_runtime: s.applied_runtime.clone(),
                applied_provisioning: s.applied_provisioning.clone(),
            }));
            Ok(())
        }
    }

    fn project() -> project::Project {
        project::Project {
            root: PathBuf::from("/tmp/proj"),
            name: "proj".to_string(),
            id: "proj-1".to_string(),
        }
    }

    #[test]
    fn run_with_errors_when_state_missing() {
        let deps = MockDeps {
            project: project(),
            state: None,
        };
        let err = run_with(&deps).unwrap_err();
        assert!(format!("{err:#}").contains("No server found"));
    }

    #[test]
    fn run_with_errors_when_server_not_running() {
        let deps = MockDeps {
            project: project(),
            state: Some(state::ProjectState {
                server_id: 0,
                ipv4: "".to_string(),
                username: "u".to_string(),
                hostname: "".to_string(),
                snapshot_id: None,
                applied_runtime: None,
                applied_provisioning: None,
            }),
        };
        let err = run_with(&deps).unwrap_err();
        assert!(format!("{err:#}").contains("No running server"));
    }

    #[test]
    fn pause_with_runs_expected_sequence_and_saves_snapshot_state() {
        let mut actions = MockActions::default();
        let existing = state::ProjectState {
            server_id: 42,
            ipv4: "1.2.3.4".to_string(),
            username: "alice".to_string(),
            hostname: "".to_string(),
            snapshot_id: None,
            applied_runtime: None,
            applied_provisioning: None,
        };
        pause_with(&mut actions, &project(), existing).unwrap();

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
        let mut actions = MockActions {
            tailscale_fail: true,
            ..Default::default()
        };
        let existing = state::ProjectState {
            server_id: 42,
            ipv4: "1.2.3.4".to_string(),
            username: "alice".to_string(),
            hostname: "gob-custom".to_string(),
            snapshot_id: None,
            applied_runtime: None,
            applied_provisioning: None,
        };
        pause_with(&mut actions, &project(), existing).unwrap();
        assert!(actions.calls.borrow().contains(&"save_state".to_string()));
    }
}
