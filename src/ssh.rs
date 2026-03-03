use anyhow::{Context, Result};
use std::io::{self, Write};
use std::process::Command;
use std::time::Duration;
use tracing::{info, instrument, warn};

/// Common SSH options used across all SSH commands.
const SSH_OPTS: [&str; 2] = ["-o", "StrictHostKeyChecking=accept-new"];

/// Wait for SSH to become available on a remote host.
#[instrument(level = "info", skip_all, fields(username = username, ip = ip))]
pub fn wait_for_ssh(username: &str, ip: &str) -> Result<()> {
    print!("Waiting for SSH... ");
    io::stdout().flush()?;

    let max_attempts = 60; // 2 minutes max
    let target = format!("{}@{}", username, ip);

    for attempt in 1..=max_attempts {
        let result = Command::new("ssh")
            .args(SSH_OPTS)
            .args([
                "-o",
                "ConnectTimeout=2",
                "-o",
                "BatchMode=yes",
                &target,
                "true",
            ])
            .output();

        if let Ok(output) = result {
            if output.status.success() {
                println!("ok");
                info!(attempt = attempt, "wait_for_ssh_success");
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }

    warn!(ip = ip, max_attempts = max_attempts, "wait_for_ssh_timeout");
    anyhow::bail!("Timed out waiting for SSH on {}", ip);
}

/// Run a remote command via SSH and return its exit status.
#[instrument(level = "debug", skip_all, fields(target = target, remote_cmd = remote_cmd))]
pub fn run_remote_status(target: &str, remote_cmd: &str) -> io::Result<std::process::ExitStatus> {
    let status = Command::new("ssh")
        .args(SSH_OPTS)
        .args([target, remote_cmd])
        .status();
    match &status {
        Ok(exit) => info!(
            target = target,
            remote_cmd = remote_cmd,
            code = ?exit.code(),
            "run_remote_status"
        ),
        Err(err) => warn!(
            target = target,
            remote_cmd = remote_cmd,
            error = %err,
            "run_remote_status_failed"
        ),
    }
    status
}

/// Run a remote command via SSH, returning its full output.
pub fn run_ssh_cmd(username: &str, ip: &str, remote_cmd: &str) -> Result<std::process::Output> {
    Command::new("ssh")
        .args(SSH_OPTS)
        .args([&format!("{}@{}", username, ip), remote_cmd])
        .output()
        .context("Failed to run ssh")
}

/// Copy a file to the remote host via SCP.
pub fn scp_file(local_path: &str, remote_dest: &str) -> Result<bool> {
    let result = Command::new("scp")
        .args(SSH_OPTS)
        .args([local_path, remote_dest])
        .status();
    Ok(matches!(result, Ok(s) if s.success()))
}

/// Wait for cloud-init to finish on the remote VM.
#[instrument(level = "info", skip_all, fields(username = username, ip = ip))]
pub fn wait_for_cloud_init(username: &str, ip: &str) -> Result<()> {
    print!("Waiting for cloud-init... ");
    io::stdout().flush()?;

    let output = run_ssh_cmd(username, ip, "cloud-init status --wait")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() || stdout.contains("status: done") {
        println!("done");
        info!("wait_for_cloud_init_done");
    } else {
        let msg = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        anyhow::bail!("cloud-init failed: {}", msg);
    }

    Ok(())
}
