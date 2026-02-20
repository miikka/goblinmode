use anyhow::Result;

use crate::config;
use crate::hetzner::HetznerClient;
use crate::project;
use crate::state;

trait StatusDeps {
    fn detect_project(&self) -> Result<project::Project>;
    fn load_state(&self, project_id: &str) -> Result<Option<state::ProjectState>>;
    fn load_config(&self) -> Result<config::Config>;
    fn get_server_status(
        &self,
        hetzner_token: &str,
        server_id: u64,
    ) -> Result<Option<(String, String)>>;
}

trait StatusOutput {
    fn line(&mut self, message: String);
}

struct RealStatusDeps;
struct RealStatusOutput;

impl StatusDeps for RealStatusDeps {
    fn detect_project(&self) -> Result<project::Project> {
        project::detect_project()
    }

    fn load_state(&self, project_id: &str) -> Result<Option<state::ProjectState>> {
        state::load_state(project_id)
    }

    fn load_config(&self) -> Result<config::Config> {
        config::load_config()
    }

    fn get_server_status(
        &self,
        hetzner_token: &str,
        server_id: u64,
    ) -> Result<Option<(String, String)>> {
        let hetzner = HetznerClient::new(hetzner_token.to_string());
        hetzner.get_server_status(server_id)
    }
}

impl StatusOutput for RealStatusOutput {
    fn line(&mut self, message: String) {
        println!("{}", message);
    }
}

pub fn run() -> Result<()> {
    let deps = RealStatusDeps;
    let mut out = RealStatusOutput;
    run_with(&deps, &mut out)
}

fn run_with<D: StatusDeps, O: StatusOutput>(deps: &D, out: &mut O) -> Result<()> {
    let project = deps.detect_project()?;

    let existing = match deps.load_state(&project.id)? {
        Some(s) => s,
        None => {
            out.line(format!("No VM for project '{}'.", project.name));
            return Ok(());
        }
    };

    let hostname = if existing.hostname.is_empty() {
        format!("gob-{}", project.name)
    } else {
        existing.hostname.clone()
    };

    let cfg = deps.load_config()?;
    match deps.get_server_status(&cfg.hetzner_api_token, existing.server_id)? {
        Some((status, ip)) => {
            out.line(format!("Project:  {}", project.name));
            out.line(format!("Status:   {}", status));
            out.line(format!("Hostname: {}", hostname));
            out.line(format!("IP:       {}", ip));
            out.line(format!("User:     {}", existing.username));
        }
        None => {
            out.line(format!(
                "VM for project '{}' no longer exists (stale state).",
                project.name
            ));
            out.line("Run `gob down` to clean up local state.".to_string());
        }
    }

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
        status: Option<(String, String)>,
    }

    impl StatusDeps for MockDeps {
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

        fn get_server_status(
            &self,
            _hetzner_token: &str,
            _server_id: u64,
        ) -> Result<Option<(String, String)>> {
            Ok(self.status.clone())
        }
    }

    struct VecOutput {
        lines: RefCell<Vec<String>>,
    }

    impl VecOutput {
        fn new() -> Self {
            Self {
                lines: RefCell::new(Vec::new()),
            }
        }
    }

    impl StatusOutput for VecOutput {
        fn line(&mut self, message: String) {
            self.lines.borrow_mut().push(message);
        }
    }

    fn project() -> project::Project {
        project::Project {
            root: PathBuf::from("/tmp/p"),
            name: "proj".to_string(),
            id: "proj-1".to_string(),
        }
    }

    #[test]
    fn run_with_reports_no_vm_when_state_missing() {
        let deps = MockDeps {
            project: project(),
            state: None,
            status: None,
        };
        let mut out = VecOutput::new();
        run_with(&deps, &mut out).unwrap();
        assert_eq!(out.lines.borrow().as_slice(), &["No VM for project 'proj'."]);
    }

    #[test]
    fn run_with_reports_running_server() {
        let deps = MockDeps {
            project: project(),
            state: Some(state::ProjectState {
                server_id: 12,
                ipv4: "1.2.3.4".to_string(),
                username: "alice".to_string(),
                hostname: "".to_string(),
                snapshot_id: None,
            }),
            status: Some(("running".to_string(), "5.6.7.8".to_string())),
        };
        let mut out = VecOutput::new();
        run_with(&deps, &mut out).unwrap();
        assert_eq!(
            out.lines.borrow().as_slice(),
            &[
                "Project:  proj",
                "Status:   running",
                "Hostname: gob-proj",
                "IP:       5.6.7.8",
                "User:     alice",
            ]
        );
    }

    #[test]
    fn run_with_reports_stale_state_when_server_missing() {
        let deps = MockDeps {
            project: project(),
            state: Some(state::ProjectState {
                server_id: 12,
                ipv4: "1.2.3.4".to_string(),
                username: "alice".to_string(),
                hostname: "gob-custom".to_string(),
                snapshot_id: None,
            }),
            status: None,
        };
        let mut out = VecOutput::new();
        run_with(&deps, &mut out).unwrap();
        assert_eq!(
            out.lines.borrow().as_slice(),
            &[
                "VM for project 'proj' no longer exists (stale state).",
                "Run `gob down` to clean up local state.",
            ]
        );
    }
}
