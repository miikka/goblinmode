use anyhow::{bail, Result};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub struct Project {
    pub root: PathBuf,
    pub name: String,
    pub id: String,
}

pub fn detect_project() -> Result<Project> {
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join(".git").exists() {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unnamed".to_string());
            let id = make_project_id(&name, &dir);
            return Ok(Project {
                root: dir,
                name,
                id,
            });
        }
        if !dir.pop() {
            bail!("Not in a git repository (no .git found in any parent directory)");
        }
    }
}

fn make_project_id(name: &str, path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    let hash = hasher.finish();
    format!("{}-{:x}", name, hash & 0xFFFF)
}
