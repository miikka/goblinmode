use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, Default)]
pub struct ProjectConfig {
    #[serde(default)]
    pub serve_ports: Vec<u16>,
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
