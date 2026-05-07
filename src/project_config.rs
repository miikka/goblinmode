use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

fn default_server_type() -> String {
    "cx23".to_string()
}

#[derive(Debug, Deserialize)]
pub struct ProjectConfig {
    #[serde(default)]
    pub serve_ports: Vec<u16>,
    #[serde(default = "default_server_type")]
    pub server_type: String,
    /// Extra APT packages installed on this project's VM, in addition to
    /// the packages listed in the user config.
    #[serde(default)]
    pub packages: Vec<String>,
    /// Extra cargo-binstall packages installed on this project's VM.
    #[serde(default)]
    pub binstall_packages: Vec<String>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        ProjectConfig {
            serve_ports: Vec::new(),
            server_type: default_server_type(),
            packages: Vec::new(),
            binstall_packages: Vec::new(),
        }
    }
}

pub fn load_project_config(project_root: &Path) -> Result<ProjectConfig> {
    let config_path = project_root.join(".config/goblinmode.toml");
    if !config_path.exists() {
        return Ok(ProjectConfig::default());
    }
    let content = std::fs::read_to_string(&config_path)?;
    let config: ProjectConfig = toml::from_str(&content)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config = ProjectConfig::default();
        assert_eq!(config.server_type, "cx23");
        assert!(config.serve_ports.is_empty());
        assert!(config.packages.is_empty());
        assert!(config.binstall_packages.is_empty());
    }

    #[test]
    fn missing_config_file_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_project_config(dir.path()).unwrap();
        assert_eq!(config.server_type, "cx23");
        assert!(config.serve_ports.is_empty());
        assert!(config.packages.is_empty());
        assert!(config.binstall_packages.is_empty());
    }

    #[test]
    fn partial_config_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("goblinmode.toml"),
            "server_type = \"cx42\"\n",
        )
        .unwrap();
        let config = load_project_config(dir.path()).unwrap();
        assert_eq!(config.server_type, "cx42");
        assert!(config.serve_ports.is_empty());
    }

    #[test]
    fn serve_ports_parse() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("goblinmode.toml"),
            "serve_ports = [3000, 8080]\n",
        )
        .unwrap();
        let config = load_project_config(dir.path()).unwrap();
        assert_eq!(config.serve_ports, vec![3000, 8080]);
    }

    #[test]
    fn packages_parse() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("goblinmode.toml"),
            "packages = [\"nodejs\", \"python3\"]\n",
        )
        .unwrap();
        let config = load_project_config(dir.path()).unwrap();
        assert_eq!(config.packages, vec!["nodejs", "python3"]);
    }

    #[test]
    fn binstall_packages_parse() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("goblinmode.toml"),
            "binstall_packages = [\"jj-cli\"]\n",
        )
        .unwrap();
        let config = load_project_config(dir.path()).unwrap();
        assert_eq!(config.binstall_packages, vec!["jj-cli"]);
    }

    #[test]
    fn empty_config_file_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("goblinmode.toml"), "").unwrap();
        let config = load_project_config(dir.path()).unwrap();
        assert_eq!(config.server_type, "cx23");
        assert!(config.serve_ports.is_empty());
    }
}
