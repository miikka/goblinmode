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
