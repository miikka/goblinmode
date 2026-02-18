use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectState {
    pub server_id: u64,
    pub ipv4: String,
    pub ssh_key_id: u64,
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
