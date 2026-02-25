use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;

use crate::packages::PackageSpec;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct AppliedRuntimeConfig {
    #[serde(default)]
    pub packages: Vec<PackageSpec>,
    /// Legacy field kept for reading old state files.  New state files omit it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vm_packages: Vec<String>,
    /// Legacy field kept for reading old state files.  New state files omit it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coding_agents: Vec<String>,
    #[serde(default)]
    pub serve_ports: Vec<u16>,
}

impl AppliedRuntimeConfig {
    /// Migrate old `vm_packages`/`coding_agents` fields into the unified
    /// `packages` field.  A no-op when `packages` is already populated.
    pub fn migrate(self) -> Self {
        if !self.packages.is_empty() {
            return self;
        }
        if self.vm_packages.is_empty() && self.coding_agents.is_empty() {
            return self;
        }
        let packages = self
            .vm_packages
            .iter()
            .map(|n| PackageSpec::Apt { name: n.clone() })
            .chain(
                self.coding_agents
                    .iter()
                    .filter_map(|n| crate::packages::resolve_coding_agent(n)),
            )
            .collect();
        Self {
            packages,
            vm_packages: Vec::new(),
            coding_agents: Vec::new(),
            serve_ports: self.serve_ports,
        }
    }
}

/// A language toolchain to install on the VM during provisioning.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Toolchain {
    Rust,
    Python,
}

impl Toolchain {
    /// All known toolchains, in detection order.
    pub fn all() -> &'static [Toolchain] {
        &[Toolchain::Rust, Toolchain::Python]
    }

    /// Returns true if this toolchain is detected in the given project root.
    pub fn detect(&self, project_root: &std::path::Path) -> bool {
        match self {
            Toolchain::Rust => project_root.join("Cargo.toml").exists(),
            Toolchain::Python => project_root.join("pyproject.toml").exists(),
        }
    }

    /// APT packages that must be installed for this toolchain.
    pub fn apt_packages(&self) -> &'static [&'static str] {
        match self {
            Toolchain::Rust => &["build-essential", "rustup"],
            Toolchain::Python => &[],
        }
    }

    /// cloud-init `runcmd` entries that set up this toolchain.
    pub fn runcmds(&self, username: &str) -> Vec<String> {
        match self {
            Toolchain::Rust => vec![format!("su - {username} -c 'rustup default stable'")],
            Toolchain::Python => vec![format!(
                "su - {username} -c 'curl -LsSf https://astral.sh/uv/install.sh | sh'"
            )],
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct AppliedProvisioningConfig {
    #[serde(default)]
    pub server_type: String,
    /// Toolchains detected and installed for this project.
    #[serde(default)]
    pub toolchains: Vec<Toolchain>,
    /// Legacy field kept for reading old state files. New state files omit it.
    #[serde(default, skip_serializing)]
    pub is_rust: bool,
    #[serde(default)]
    pub dotfiles_repo: Option<String>,
    #[serde(default)]
    pub dotfiles_install: Option<String>,
}

impl AppliedProvisioningConfig {
    /// Migrate old `is_rust` field into the unified `toolchains` field.
    /// A no-op when `toolchains` is already populated or `is_rust` is false.
    pub fn migrate(self) -> Self {
        if !self.is_rust || self.toolchains.iter().any(|t| matches!(t, Toolchain::Rust)) {
            return self;
        }
        let mut toolchains = self.toolchains.clone();
        toolchains.push(Toolchain::Rust);
        Self {
            toolchains,
            is_rust: false,
            ..self
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectState {
    #[serde(default)]
    pub server_id: u64,
    #[serde(default)]
    pub ipv4: String,
    #[serde(default = "default_username")]
    pub username: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub snapshot_id: Option<u64>,
    #[serde(default)]
    pub applied_runtime: Option<AppliedRuntimeConfig>,
    #[serde(default)]
    pub applied_provisioning: Option<AppliedProvisioningConfig>,
}

fn default_username() -> String {
    "root".to_string()
}

fn state_path(project_id: &str) -> Result<std::path::PathBuf> {
    let data_dir = dirs::data_dir().context("Could not determine data directory")?;
    Ok(data_dir
        .join("goblinmode")
        .join(project_id)
        .join("state.json"))
}

pub fn load_state(project_id: &str) -> Result<Option<ProjectState>> {
    let path = state_path(project_id)?;
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read state from {}", path.display()))?;
    let mut state: ProjectState = serde_json::from_str(&contents)
        .with_context(|| format!("Failed to parse state from {}", path.display()))?;
    state.applied_runtime = state.applied_runtime.map(|r| r.migrate());
    state.applied_provisioning = state.applied_provisioning.map(|p| p.migrate());
    Ok(Some(state))
}

pub fn save_state(project_id: &str, state: &ProjectState) -> Result<()> {
    let path = state_path(project_id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    let contents = serde_json::to_string_pretty(state)?;
    fs::write(&path, contents)
        .with_context(|| format!("Failed to write state to {}", path.display()))?;
    Ok(())
}

pub fn delete_state(project_id: &str) -> Result<()> {
    let path = state_path(project_id)?;
    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("Failed to delete state at {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trip() {
        let state = ProjectState {
            server_id: 12345,
            ipv4: "1.2.3.4".to_string(),
            username: "testuser".to_string(),
            hostname: "gob-test".to_string(),
            snapshot_id: None,
            applied_runtime: None,
            applied_provisioning: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        let deserialized: ProjectState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.server_id, 12345);
        assert_eq!(deserialized.ipv4, "1.2.3.4");
        assert_eq!(deserialized.username, "testuser");
        assert_eq!(deserialized.hostname, "gob-test");
        assert!(deserialized.snapshot_id.is_none());
    }

    #[test]
    fn missing_username_defaults_to_root() {
        let json = r#"{"server_id": 1, "ipv4": "1.2.3.4", "hostname": "gob-x"}"#;
        let state: ProjectState = serde_json::from_str(json).unwrap();
        assert_eq!(state.username, "root");
    }

    #[test]
    fn snapshot_id_round_trips() {
        let state = ProjectState {
            server_id: 1,
            ipv4: "1.2.3.4".to_string(),
            username: "u".to_string(),
            hostname: "h".to_string(),
            snapshot_id: Some(99999),
            applied_runtime: None,
            applied_provisioning: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        let deserialized: ProjectState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.snapshot_id, Some(99999));
    }

    #[test]
    fn missing_applied_configs_default_to_none() {
        let json = r#"{"server_id": 1, "ipv4": "1.2.3.4", "hostname": "gob-x"}"#;
        let state: ProjectState = serde_json::from_str(json).unwrap();
        assert!(state.applied_runtime.is_none());
        assert!(state.applied_provisioning.is_none());
    }

    #[test]
    fn applied_config_structs_default_empty() {
        let runtime = AppliedRuntimeConfig::default();
        assert!(runtime.packages.is_empty());
        assert!(runtime.serve_ports.is_empty());

        let provisioning = AppliedProvisioningConfig::default();
        assert!(provisioning.server_type.is_empty());
        assert!(provisioning.toolchains.is_empty());
        assert!(provisioning.dotfiles_repo.is_none());
        assert!(provisioning.dotfiles_install.is_none());
    }

    #[test]
    fn migrate_converts_old_vm_packages_and_agents() {
        let old = AppliedRuntimeConfig {
            packages: vec![],
            vm_packages: vec!["vim".to_string(), "tmux".to_string()],
            coding_agents: vec!["claude-code".to_string()],
            serve_ports: vec![3000],
        };
        let migrated = old.migrate();
        assert!(migrated.vm_packages.is_empty());
        assert!(migrated.coding_agents.is_empty());
        assert_eq!(migrated.serve_ports, vec![3000]);
        assert_eq!(
            migrated.packages,
            vec![
                PackageSpec::Apt {
                    name: "vim".to_string()
                },
                PackageSpec::Apt {
                    name: "tmux".to_string()
                },
                PackageSpec::CurlInstaller {
                    name: "claude-code".to_string(),
                    url: "https://claude.ai/install.sh".to_string(),
                },
            ]
        );
    }

    #[test]
    fn migrate_is_noop_when_packages_already_set() {
        let existing = vec![PackageSpec::Apt {
            name: "vim".to_string(),
        }];
        let config = AppliedRuntimeConfig {
            packages: existing.clone(),
            vm_packages: vec!["old".to_string()],
            coding_agents: vec![],
            serve_ports: vec![],
        };
        let migrated = config.migrate();
        assert_eq!(migrated.packages, existing);
    }

    #[test]
    fn migrate_skips_unknown_coding_agents() {
        let old = AppliedRuntimeConfig {
            packages: vec![],
            vm_packages: vec![],
            coding_agents: vec!["unknown-agent".to_string()],
            serve_ports: vec![],
        };
        let migrated = old.migrate();
        // unknown agents are silently dropped during migration
        assert!(migrated.packages.is_empty());
    }

    #[test]
    fn old_state_json_deserializes_and_migrates() {
        // Simulates a state file written before the PackageSpec migration.
        let json = r#"{
            "server_id": 1,
            "ipv4": "1.2.3.4",
            "hostname": "gob-x",
            "applied_runtime": {
                "vm_packages": ["vim"],
                "coding_agents": ["opencode"],
                "serve_ports": []
            }
        }"#;
        let mut state: ProjectState = serde_json::from_str(json).unwrap();
        state.applied_runtime = state.applied_runtime.map(|r| r.migrate());
        let runtime = state.applied_runtime.unwrap();
        assert_eq!(
            runtime.packages,
            vec![
                PackageSpec::Apt {
                    name: "vim".to_string()
                },
                PackageSpec::CurlInstaller {
                    name: "opencode".to_string(),
                    url: "https://opencode.ai/install".to_string(),
                },
            ]
        );
    }

    #[test]
    fn migrate_provisioning_converts_is_rust_to_toolchain() {
        let json = r#"{
            "server_id": 1,
            "ipv4": "1.2.3.4",
            "hostname": "gob-x",
            "applied_provisioning": {
                "server_type": "cx23",
                "is_rust": true
            }
        }"#;
        let mut state: ProjectState = serde_json::from_str(json).unwrap();
        state.applied_provisioning = state.applied_provisioning.map(|p| p.migrate());
        let provisioning = state.applied_provisioning.unwrap();
        assert_eq!(provisioning.toolchains, vec![Toolchain::Rust]);
    }
}
