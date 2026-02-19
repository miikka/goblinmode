use anyhow::Result;
use std::process::Command;

use super::up;

pub fn run() -> Result<()> {
    let env = up::ensure_running(false)?;

    let url = format!(
        "ssh://{}@{}/~/{}/",
        env.username, env.hostname, env.project_name
    );
    println!("Opening Zed: {}", url);

    let status = Command::new("zed").arg(&url).status()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}
