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
        };
        let json = serde_json::to_string(&state).unwrap();
        let deserialized: ProjectState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.snapshot_id, Some(99999));
    }
}
