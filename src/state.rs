use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;

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
