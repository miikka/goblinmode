use anyhow::Result;
use std::process::Command;

use super::up;

trait ZedDeps {
    fn ensure_running(&self, reset: bool) -> Result<up::Env>;
    fn run_zed(&self, url: &str) -> Result<i32>;
}

struct RealZedDeps;

impl ZedDeps for RealZedDeps {
    fn ensure_running(&self, reset: bool) -> Result<up::Env> {
        up::ensure_running(reset)
    }

    fn run_zed(&self, url: &str) -> Result<i32> {
        let status = Command::new("zed").arg(url).status()?;
        if status.success() {
            return Ok(0);
        }
        Ok(status.code().unwrap_or(1))
    }
}

pub fn run() -> Result<()> {
    let deps = RealZedDeps;
    if let Some(code) = run_with(&deps)? {
        std::process::exit(code);
    }
    Ok(())
}

fn run_with<D: ZedDeps>(deps: &D) -> Result<Option<i32>> {
    let env = deps.ensure_running(false)?;

    let url = format!(
        "ssh://{}@{}/~/{}/",
        env.username, env.hostname, env.project_name
    );
    println!("Opening Zed: {}", url);

    let code = deps.run_zed(&url)?;
    if code != 0 {
        return Ok(Some(code));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct MockDeps {
        env: up::Env,
        zed_code: i32,
        urls: RefCell<Vec<String>>,
    }

    impl ZedDeps for MockDeps {
        fn ensure_running(&self, _reset: bool) -> Result<up::Env> {
            Ok(up::Env {
                username: self.env.username.clone(),
                hostname: self.env.hostname.clone(),
                project_name: self.env.project_name.clone(),
            })
        }

        fn run_zed(&self, url: &str) -> Result<i32> {
            self.urls.borrow_mut().push(url.to_string());
            Ok(self.zed_code)
        }
    }

    #[test]
    fn run_with_builds_expected_url() {
        let deps = MockDeps {
            env: up::Env {
                username: "alice".to_string(),
                hostname: "gob-proj".to_string(),
                project_name: "proj".to_string(),
            },
            zed_code: 0,
            urls: RefCell::new(Vec::new()),
        };
        let code = run_with(&deps).unwrap();
        assert_eq!(code, None);
        assert_eq!(
            deps.urls.borrow().as_slice(),
            &["ssh://alice@gob-proj/~/proj/"]
        );
    }

    #[test]
    fn run_with_propagates_nonzero_exit_code() {
        let deps = MockDeps {
            env: up::Env {
                username: "alice".to_string(),
                hostname: "gob-proj".to_string(),
                project_name: "proj".to_string(),
            },
            zed_code: 3,
            urls: RefCell::new(Vec::new()),
        };
        let code = run_with(&deps).unwrap();
        assert_eq!(code, Some(3));
    }
}
