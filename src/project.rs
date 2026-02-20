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

pub(crate) fn make_project_id(name: &str, path: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    let hash = hasher.finish();
    format!("{}-{:x}", name, hash & 0xFFFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn make_project_id_format() {
        let id = make_project_id("myproject", &PathBuf::from("/home/user/myproject"));
        // Should be "name-hexhash"
        let parts: Vec<&str> = id.splitn(2, '-').collect();
        assert_eq!(parts[0], "myproject");
        // Hash part should be valid hex
        assert!(u64::from_str_radix(parts[1], 16).is_ok());
    }

    #[test]
    fn make_project_id_deterministic() {
        let path = PathBuf::from("/home/user/project");
        let id1 = make_project_id("proj", &path);
        let id2 = make_project_id("proj", &path);
        assert_eq!(id1, id2);
    }

    #[test]
    fn make_project_id_different_paths_differ() {
        let id1 = make_project_id("proj", &PathBuf::from("/home/user/a"));
        let id2 = make_project_id("proj", &PathBuf::from("/home/user/b"));
        assert_ne!(id1, id2);
    }
}
