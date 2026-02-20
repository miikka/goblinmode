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
}

impl Default for ProjectConfig {
    fn default() -> Self {
        ProjectConfig {
            serve_ports: Vec::new(),
            server_type: default_server_type(),
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
    }

    #[test]
    fn missing_config_file_returns_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_project_config(dir.path()).unwrap();
        assert_eq!(config.server_type, "cx23");
        assert!(config.serve_ports.is_empty());
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
