use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{self, Write};
use std::net::TcpStream;
use std::process::Command;
use std::time::Duration;

use crate::config;
use crate::hetzner::HetznerClient;
use crate::project;
use crate::state;

/// Connection info for a running environment.
pub struct Env {
    pub username: String,
    pub hostname: String,
    pub project_name: String,
}

/// Ensure the dev environment is running, provisioning if needed.
/// Returns connection info.
pub fn ensure_running() -> Result<Env> {
    // 1. Detect project root
    let project = project::detect_project()?;
    println!("Project: {} ({})", project.name, project.root.display());

    // 2. Load config
    let cfg = config::load_config()?;
    let client = HetznerClient::new(cfg.hetzner_api_token);

    // 3. Check existing state
    if let Some(existing) = state::load_state(&project.id)? {
        print!(
            "Existing server found (id: {}), checking status... ",
            existing.server_id
        );
        io::stdout().flush()?;

        match client.get_server_status(existing.server_id)? {
            Some((status, ip)) if status == "running" => {
                println!("running");
                wait_for_ssh(&ip)?;
                let hostname = if existing.hostname.is_empty() {
                    format!("gob-{}", project.name)
                } else {
                    existing.hostname
                };
                return Ok(Env {
                    username: existing.username,
                    hostname,
                    project_name: project.name,
                });
            }
            Some((status, _)) => {
                println!("{}", status);
                println!(
                    "Server is not running (status: {}). Creating a new one.",
                    status
                );
            }
            None => {
                println!("not found");
                println!("Server no longer exists. Creating a new one.");
            }
        }
    }

    // 4. Read SSH public key
    let ssh_key_path = dirs::home_dir()
        .context("Could not determine home directory")?
        .join(".ssh")
        .join("id_ed25519.pub");
    let ssh_pubkey = fs::read_to_string(&ssh_key_path).with_context(|| {
        format!(
            "Failed to read SSH public key from {}",
            ssh_key_path.display()
        )
    })?;

    // 5. Create server with cloud-init
    let username = whoami();
    let user_data = build_cloud_init(&username, ssh_pubkey.trim(), &cfg.tailscale_auth_key);
    let server_name = format!("gob-{}", project.name);
    println!("Creating server '{}'...", server_name);
    let (server_id, _ip) =
        client.create_server(&server_name, "cx23", "debian-13", "hel1", Some(&user_data))?;
    println!(
        "  Server created (id: {}), waiting for it to start...",
        server_id
    );

    // 6. Poll until running
    let ip = client.wait_for_server(server_id)?;
    println!("  Server running at {}", ip);

    // 7. Save state
    let project_state = state::ProjectState {
        server_id,
        ipv4: ip.clone(),
        username: username.clone(),
        hostname: server_name.clone(),
    };
    state::save_state(&project.id, &project_state)?;

    // 8. Wait for SSH
    wait_for_ssh(&ip)?;

    // 9. Push project to VM
    sync_project(&project.root, &project.name, &username, &ip)?;

    // 10. Add git remote
    add_git_remote(&project.root, &username, &server_name, &project.name)?;

    Ok(Env {
        username,
        hostname: server_name,
        project_name: project.name,
    })
}

pub fn run() -> Result<()> {
    let env = ensure_running()?;
    println!("SSH ready: ssh {}@{}", env.username, env.hostname);
    Ok(())
}

fn wait_for_ssh(ip: &str) -> Result<()> {
    print!("Waiting for SSH... ");
    io::stdout().flush()?;

    let addr = format!("{}:22", ip);
    let timeout = Duration::from_secs(2);
    let max_attempts = 60; // 2 minutes max

    for _ in 0..max_attempts {
        if TcpStream::connect_timeout(&addr.parse().context("Invalid IP address")?, timeout).is_ok()
        {
            println!("ok");
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
    }

    anyhow::bail!("Timed out waiting for SSH on {}", addr);
}

fn add_git_remote(
    project_root: &std::path::Path,
    username: &str,
    hostname: &str,
    project_name: &str,
) -> Result<()> {
    let remote_url = format!("{}@{}:~/{}/", username, hostname, project_name);

    // Remove existing remote if present, ignore errors
    let _ = Command::new("git")
        .args(["remote", "remove", "gob"])
        .current_dir(project_root)
        .output();

    let status = Command::new("git")
        .args(["remote", "add", "gob", &remote_url])
        .current_dir(project_root)
        .status()
        .context("Failed to run git")?;

    if status.success() {
        println!("  Git remote 'gob' added: {}", remote_url);
    } else {
        eprintln!("Warning: failed to add git remote 'gob'");
    }

    Ok(())
}

fn sync_project(
    project_root: &std::path::Path,
    project_name: &str,
    username: &str,
    ip: &str,
) -> Result<()> {
    println!("Syncing project to VM...");

    // Ensure source path ends with / so rsync copies contents, not the directory itself
    let mut src = project_root.to_string_lossy().to_string();
    if !src.ends_with('/') {
        src.push('/');
    }

    let dest = format!("{}@{}:~/{}/", username, ip, project_name);
    let status = Command::new("rsync")
        .args([
            "-az",
            "--filter=:- .gitignore",
            "-e",
            "ssh -o StrictHostKeyChecking=accept-new",
            &src,
            &dest,
        ])
        .status()
        .context("Failed to run rsync")?;

    if !status.success() {
        bail!(
            "rsync failed with exit code {}",
            status.code().unwrap_or(-1)
        );
    }

    println!("  Project synced to ~/{}/", project_name);
    Ok(())
}

fn build_cloud_init(username: &str, ssh_pubkey: &str, tailscale_auth_key: &str) -> String {
    format!(
        r#"#cloud-config
users:
  - name: {username}
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - {ssh_pubkey}

ssh_pwauth: false

package_update: true
packages:
  - tmux
  - mosh
  - atuin

runcmd:
  - sed -i 's/^PermitRootLogin .*/PermitRootLogin no/' /etc/ssh/sshd_config
  - systemctl restart sshd
  - curl -fsSL https://tailscale.com/install.sh | sh
  - tailscale up --auth-key={tailscale_auth_key} --ssh
"#
    )
}

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}
