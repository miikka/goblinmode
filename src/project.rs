use anyhow::{bail, Result};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Project {
    pub root: PathBuf,
    pub name: String,
    pub id: String,
}

pub fn detect_project() -> Result<Project> {
    detect_project_from(std::env::current_dir()?.as_path())
}

pub(crate) fn detect_project_from(start: &Path) -> Result<Project> {
    let mut dir = start.to_path_buf();
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
    fn detect_project_from_finds_git_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let project = detect_project_from(dir.path()).unwrap();
        assert_eq!(project.root, dir.path());
    }

    #[test]
    fn detect_project_from_walks_up_to_find_git() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let subdir = dir.path().join("a").join("b");
        std::fs::create_dir_all(&subdir).unwrap();
        let project = detect_project_from(&subdir).unwrap();
        assert_eq!(project.root, dir.path());
    }

    #[test]
    fn detect_project_from_errors_outside_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        let err = detect_project_from(dir.path()).unwrap_err();
        assert!(format!("{err}").contains("Not in a git repository"));
    }

    #[test]
    fn detect_project_from_uses_dir_name_as_project_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let project = detect_project_from(dir.path()).unwrap();
        let expected_name = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(project.name, expected_name);
    }

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
