use anyhow::Result;
use std::process::Command;

use super::up;

pub fn run() -> Result<()> {
    let env = up::ensure_running()?;

    let target = format!("{}@{}", env.username, env.hostname);
    println!("Connecting with mosh to {}...", target);

    let status = Command::new("mosh").arg(&target).status()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}
