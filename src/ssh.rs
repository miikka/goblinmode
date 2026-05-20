// SPDX-FileCopyrightText: 2026 Miikka Koskinen
//
// SPDX-License-Identifier: MIT

use anyhow::{Context, Result};
use std::env;
use std::io::{self, Write};
use std::process::Command;
use std::time::Duration;
use tracing::{info, instrument, warn};

/// Reuses a single SSH connection for all operations to a remote host,
/// eliminating repeated TCP + SSH handshake overhead via ControlMaster.
pub struct SshSession {
    username: String,
    ip: String,
    control_path: String,
}

impl SshSession {
    pub fn new(username: &str, ip: &str) -> Self {
        let sanitized_ip = ip.replace(['.', ':'], "_");
        let control_path = env::temp_dir().join(format!("gob-ssh-{}-{}", username, sanitized_ip));
        Self {
            username: username.to_string(),
            ip: ip.to_string(),
            control_path: control_path.to_str().unwrap().to_owned(),
        }
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn ip(&self) -> &str {
        &self.ip
    }

    pub fn target(&self) -> String {
        format!("{}@{}", self.username, self.ip)
    }

    /// Common SSH options: host-key trust + ControlMaster multiplexing.
    fn base_args(&self) -> Vec<String> {
        vec![
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
            "-o".to_string(),
            "ControlMaster=auto".to_string(),
            "-o".to_string(),
            format!("ControlPath={}", self.control_path),
            "-o".to_string(),
            "ControlPersist=300".to_string(),
        ]
    }

    /// Value for GIT_SSH_COMMAND that routes git through the same control socket.
    pub fn git_ssh_command(&self) -> String {
        format!(
            "ssh -o StrictHostKeyChecking=accept-new -o ControlMaster=auto -o ControlPath={} -o ControlPersist=300",
            self.control_path
        )
    }
}

/// Wait for SSH to become available on a remote host.
/// The first successful connection establishes the ControlMaster socket
/// for all subsequent operations.
#[instrument(level = "info", skip_all, fields(username = %ssh.username, ip = %ssh.ip))]
pub fn wait_for_ssh(ssh: &SshSession) -> Result<()> {
    print!("Waiting for SSH... ");
    io::stdout().flush()?;

    let max_attempts = 60; // 2 minutes max

    for attempt in 1..=max_attempts {
        let mut args = ssh.base_args();
        args.extend([
            "-o".to_string(),
            "ConnectTimeout=2".to_string(),
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            ssh.target(),
            "true".to_string(),
        ]);
        let result = Command::new("ssh").args(&args).output();

        if let Ok(output) = result {
            if output.status.success() {
                println!("ok");
                info!(attempt = attempt, "wait_for_ssh_success");
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }

    warn!(ip = %ssh.ip, max_attempts = max_attempts, "wait_for_ssh_timeout");
    anyhow::bail!("Timed out waiting for SSH on {}", ssh.ip);
}

/// Run a remote command via SSH and return its exit status.
#[instrument(level = "debug", skip_all, fields(username = %ssh.username, ip = %ssh.ip, remote_cmd = remote_cmd))]
pub fn run_remote_status(
    ssh: &SshSession,
    remote_cmd: &str,
) -> io::Result<std::process::ExitStatus> {
    let mut args = ssh.base_args();
    args.extend([ssh.target(), remote_cmd.to_string()]);
    let status = Command::new("ssh").args(&args).status();
    match &status {
        Ok(exit) => info!(
            remote_cmd = remote_cmd,
            code = ?exit.code(),
            "run_remote_status"
        ),
        Err(err) => warn!(
            remote_cmd = remote_cmd,
            error = %err,
            "run_remote_status_failed"
        ),
    }
    status
}

/// Run a remote command via SSH, returning its full output.
pub fn run_ssh_cmd(ssh: &SshSession, remote_cmd: &str) -> Result<std::process::Output> {
    let mut args = ssh.base_args();
    args.extend([ssh.target(), remote_cmd.to_string()]);
    Command::new("ssh")
        .args(&args)
        .output()
        .context("Failed to run ssh")
}

/// Copy a file to the remote host via SCP.
pub fn scp_file(ssh: &SshSession, local_path: &str, remote_dest: &str) -> Result<bool> {
    let mut args = ssh.base_args();
    args.extend([local_path.to_string(), remote_dest.to_string()]);
    let result = Command::new("scp").args(&args).status();
    Ok(matches!(result, Ok(s) if s.success()))
}

/// Wait for cloud-init to finish on the remote VM.
#[instrument(level = "info", skip_all, fields(username = %ssh.username, ip = %ssh.ip))]
pub fn wait_for_cloud_init(ssh: &SshSession) -> Result<()> {
    print!("Waiting for cloud-init... ");
    io::stdout().flush()?;

    let output = run_ssh_cmd(ssh, "cloud-init status --wait")?;

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
