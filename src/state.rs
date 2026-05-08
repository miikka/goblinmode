use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;

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

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectState {
    #[serde(default)]
    pub version: u32,
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
}

impl ProjectState {
    /// Create a new state.
    pub fn new(
        server_id: u64,
        ipv4: String,
        username: String,
        hostname: String,
        snapshot_id: Option<u64>,
    ) -> Self {
        Self {
            version: 0,
            server_id,
            ipv4,
            username,
            hostname,
            snapshot_id,
        }
    }

    /// Returns the stored hostname, or falls back to `gob-{project_name}`.
    pub fn hostname_or_default(&self, project_name: &str) -> String {
        if self.hostname.is_empty() {
            format!("gob-{}", project_name)
        } else {
            self.hostname.clone()
        }
    }
}

fn default_username() -> String {
    "root".to_string()
}

fn state_path(project_id: &str) -> Result<std::path::PathBuf> {
    let data_dir = if let Ok(dir) = std::env::var("GOBLINMODE_DATA_DIR") {
        std::path::PathBuf::from(dir)
    } else {
        dirs::data_dir().context("Could not determine data directory")?
    };
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
    let state: ProjectState = serde_json::from_str(&contents)
        .with_context(|| format!("Failed to parse state from {}", path.display()))?;
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
    use std::path::PathBuf;

    #[test]
    fn toolchain_all_returns_both_variants() {
        let all = Toolchain::all();
        assert!(all.contains(&Toolchain::Rust));
        assert!(all.contains(&Toolchain::Python));
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn toolchain_detect_rust_true_when_cargo_toml_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        assert!(Toolchain::Rust.detect(dir.path()));
    }

    #[test]
    fn toolchain_detect_rust_false_when_no_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!Toolchain::Rust.detect(dir.path()));
    }

    #[test]
    fn toolchain_detect_python_true_when_pyproject_toml_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pyproject.toml"), "[project]").unwrap();
        assert!(Toolchain::Python.detect(dir.path()));
    }

    #[test]
    fn toolchain_detect_python_false_when_no_pyproject_toml() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!Toolchain::Python.detect(dir.path()));
    }

    #[test]
    fn toolchain_apt_packages_rust_includes_rustup() {
        let pkgs = Toolchain::Rust.apt_packages();
        assert!(pkgs.contains(&"rustup"));
        assert!(pkgs.contains(&"build-essential"));
    }

    #[test]
    fn toolchain_apt_packages_python_is_empty() {
        assert!(Toolchain::Python.apt_packages().is_empty());
    }

    #[test]
    fn toolchain_runcmds_rust_includes_rustup_default() {
        let cmds = Toolchain::Rust.runcmds("alice");
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].contains("rustup default stable"));
        assert!(cmds[0].contains("alice"));
    }

    #[test]
    fn toolchain_runcmds_python_includes_uv_installer() {
        let cmds = Toolchain::Python.runcmds("alice");
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].contains("uv"));
        assert!(cmds[0].contains("alice"));
    }

    fn with_temp_data_dir<F: FnOnce(&PathBuf)>(f: F) {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("GOBLINMODE_DATA_DIR", dir.path());
        f(&dir.path().to_path_buf());
        std::env::remove_var("GOBLINMODE_DATA_DIR");
    }

    #[test]
    fn save_and_load_state_round_trip() {
        with_temp_data_dir(|_| {
            let state = ProjectState::new(
                42,
                "1.2.3.4".to_string(),
                "alice".to_string(),
                "gob-proj".to_string(),
                None,
            );
            save_state("test-proj-save", &state).unwrap();
            let loaded = load_state("test-proj-save").unwrap().unwrap();
            assert_eq!(loaded.server_id, 42);
            assert_eq!(loaded.ipv4, "1.2.3.4");
            assert_eq!(loaded.username, "alice");
            assert_eq!(loaded.hostname, "gob-proj");
        });
    }

    #[test]
    fn load_state_returns_none_when_not_found() {
        with_temp_data_dir(|_| {
            let result = load_state("nonexistent-project-id").unwrap();
            assert!(result.is_none());
        });
    }

    #[test]
    fn delete_state_removes_the_file() {
        with_temp_data_dir(|_| {
            let state = ProjectState::new(
                1,
                "1.2.3.4".to_string(),
                "u".to_string(),
                "h".to_string(),
                Some(99),
            );
            save_state("test-proj-delete", &state).unwrap();
            assert!(load_state("test-proj-delete").unwrap().is_some());
            delete_state("test-proj-delete").unwrap();
            assert!(load_state("test-proj-delete").unwrap().is_none());
        });
    }

    #[test]
    fn delete_state_is_noop_when_not_found() {
        with_temp_data_dir(|_| {
            delete_state("never-existed").unwrap();
        });
    }

    #[test]
    fn state_round_trip() {
        let state = ProjectState {
            version: 0,
            server_id: 12345,
            ipv4: "1.2.3.4".to_string(),
            username: "testuser".to_string(),
            hostname: "gob-test".to_string(),
            snapshot_id: None,
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
            version: 0,
            server_id: 1,
            ipv4: "1.2.3.4".to_string(),
            username: "u".to_string(),
            hostname: "h".to_string(),
            snapshot_id: Some(99999),
        };
        let json = serde_json::to_string(&state).unwrap();
        let deserialized: ProjectState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.snapshot_id, Some(99999));
    }

    #[test]
    fn old_state_with_applied_config_fields_still_deserializes() {
        // Old state files may have applied_runtime/applied_provisioning — they should be ignored.
        let json = r#"{
            "server_id": 1,
            "ipv4": "1.2.3.4",
            "hostname": "gob-x",
            "applied_runtime": {"packages": [], "serve_ports": []},
            "applied_provisioning": {"server_type": "cx23", "toolchains": []}
        }"#;
        let state: ProjectState = serde_json::from_str(json).unwrap();
        assert_eq!(state.server_id, 1);
        assert_eq!(state.hostname, "gob-x");
    }
}
