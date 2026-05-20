// SPDX-FileCopyrightText: 2026 Miikka Koskinen
//
// SPDX-License-Identifier: MIT

use anyhow::Result;
use std::process::Command;

use super::up;

trait MoshDeps {
    fn ensure_running(&self, reset: bool) -> Result<up::Env>;
    fn run_mosh(&self, target: &str) -> Result<i32>;
}

struct RealMoshDeps;

impl MoshDeps for RealMoshDeps {
    fn ensure_running(&self, reset: bool) -> Result<up::Env> {
        up::ensure_running(reset)
    }

    fn run_mosh(&self, target: &str) -> Result<i32> {
        let status = Command::new("mosh").arg(target).status()?;
        if status.success() {
            return Ok(0);
        }
        Ok(status.code().unwrap_or(1))
    }
}

pub fn run() -> Result<()> {
    let deps = RealMoshDeps;
    if let Some(code) = run_with(&deps)? {
        std::process::exit(code);
    }
    Ok(())
}

fn run_with<D: MoshDeps>(deps: &D) -> Result<Option<i32>> {
    let env = deps.ensure_running(false)?;

    let target = format!("{}@{}", env.username, env.hostname);
    println!("Connecting with mosh to {}...", target);

    let code = deps.run_mosh(&target)?;
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
        mosh_code: i32,
        targets: RefCell<Vec<String>>,
    }

    impl MoshDeps for MockDeps {
        fn ensure_running(&self, _reset: bool) -> Result<up::Env> {
            Ok(up::Env {
                username: self.env.username.clone(),
                hostname: self.env.hostname.clone(),
                project_name: self.env.project_name.clone(),
            })
        }

        fn run_mosh(&self, target: &str) -> Result<i32> {
            self.targets.borrow_mut().push(target.to_string());
            Ok(self.mosh_code)
        }
    }

    #[test]
    fn run_with_connects_to_expected_target() {
        let deps = MockDeps {
            env: up::Env {
                username: "alice".to_string(),
                hostname: "gob-proj".to_string(),
                project_name: "proj".to_string(),
            },
            mosh_code: 0,
            targets: RefCell::new(Vec::new()),
        };
        let code = run_with(&deps).unwrap();
        assert_eq!(code, None);
        assert_eq!(deps.targets.borrow().as_slice(), &["alice@gob-proj"]);
    }

    #[test]
    fn run_with_propagates_nonzero_exit_code() {
        let deps = MockDeps {
            env: up::Env {
                username: "alice".to_string(),
                hostname: "gob-proj".to_string(),
                project_name: "proj".to_string(),
            },
            mosh_code: 7,
            targets: RefCell::new(Vec::new()),
        };
        let code = run_with(&deps).unwrap();
        assert_eq!(code, Some(7));
    }
}
