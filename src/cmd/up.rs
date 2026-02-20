use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{self, Write};
use std::process::Command;
use std::time::Duration;

use crate::cmd::down;
use crate::config;
use crate::hetzner::HetznerClient;
use crate::project;
use crate::project_config;
use crate::state;

/// Connection info for a running environment.
pub struct Env {
    pub username: String,
    pub hostname: String,
    pub project_name: String,
}

/// Ensure the dev environment is running, provisioning if needed.
/// Returns connection info.
pub fn ensure_running(reset: bool) -> Result<Env> {
    // 1. Detect project root
    let project = project::detect_project()?;
    println!("Project: {} ({})", project.name, project.root.display());

    // 2. Load config
    let cfg = config::load_config()?;
    let client = HetznerClient::new(cfg.hetzner_api_token.clone());

    // 3. Load project config
    let project_config = project_config::load_project_config(&project.root)?;

    // 4. Check existing state
    if let Some(existing) = state::load_state(&project.id)? {
        if reset {
            println!("--reset: destroying existing VM...");
            down::teardown(&project, &existing, &cfg)?;
        } else {
            // 4a. Check for snapshot restore
            if let Some(snapshot_id) = existing.snapshot_id {
                return restore_from_snapshot(
                    &project, &cfg, &client, snapshot_id, &existing, &project_config,
                );
            }

            if existing.server_id != 0 {
                print!(
                    "Existing server found (id: {}), checking status... ",
                    existing.server_id
                );
                io::stdout().flush()?;

                match client.get_server_status(existing.server_id)? {
                    Some((status, ip)) if status == "running" => {
                        println!("running");
                        wait_for_ssh(&existing.username, &ip)?;
                        setup_tailscale_serve(&existing.username, &ip, &project_config.serve_ports);
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
        }
    }

    // 5. Read SSH public key
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

    // 5b. Ensure goblinmode SSH key exists and is uploaded to Hetzner
    let goblin_pubkey = ensure_goblin_ssh_key()?;
    let hetzner_key_id = client.ensure_ssh_key("goblinmode", &goblin_pubkey)?;

    // 6. Create server with cloud-init
    let username = whoami();
    let is_rust = project.root.join("Cargo.toml").exists();
    let user_data = build_cloud_init(
        &username,
        ssh_pubkey.trim(),
        &cfg.tailscale_auth_key,
        is_rust,
        &cfg.vm_packages,
        &cfg.coding_agents,
    );
    let server_name = format!("gob-{}", project.name);
    println!("Creating server '{}' (type: {})...", server_name, project_config.server_type);
    let (server_id, initial_ip) = client.create_server(
        &server_name,
        &project_config.server_type,
        "debian-13",
        "hel1",
        Some(&user_data),
        Some(vec![hetzner_key_id]),
    )?;
    println!(
        "  Server created (id: {}), waiting for it to start...",
        server_id
    );

    // 6. Save state immediately so Ctrl-C doesn't orphan the server
    let project_state = state::ProjectState {
        server_id,
        ipv4: initial_ip,
        username: username.clone(),
        hostname: server_name.clone(),
        snapshot_id: None,
    };
    state::save_state(&project.id, &project_state)?;

    // 7. Poll until running
    let ip = client.wait_for_server(server_id)?;
    println!("  Server running at {}", ip);

    // Update state with final IP
    let project_state = state::ProjectState {
        server_id,
        ipv4: ip.clone(),
        username: username.clone(),
        hostname: server_name.clone(),
        snapshot_id: None,
    };
    state::save_state(&project.id, &project_state)?;

    // 8. Wait for SSH
    wait_for_ssh(&username, &ip)?;

    // 9. Wait for cloud-init to finish (packages, tailscale, etc.)
    wait_for_cloud_init(&username, &ip)?;

    // 10. Configure tailscale serve ports
    setup_tailscale_serve(&username, &ip, &project_config.serve_ports);

    // 11. Init git repo and push project to VM
    init_vm_repo(&username, &ip, &project.name)?;
    push_to_vm(&project.root, &username, &server_name, &ip, &project.name)?;

    // 12. Setup VM origin and SSH key
    setup_vm_origin(&username, &ip, &project.root, &project.name);
    setup_vm_ssh_key(&username, &ip);

    // 13. Setup dotfiles
    if let Some(ref repo) = cfg.dotfiles_repo {
        let install_cmd = cfg.dotfiles_install.as_deref().unwrap_or("./install.sh");
        setup_dotfiles(&username, &ip, repo, install_cmd);
    }

    Ok(Env {
        username,
        hostname: server_name,
        project_name: project.name,
    })
}

pub fn run(reset: bool) -> Result<()> {
    let env = ensure_running(reset)?;
    println!("SSH ready: ssh {}@{}", env.username, env.hostname);
    Ok(())
}

fn restore_from_snapshot(
    project: &crate::project::Project,
    cfg: &config::Config,
    client: &HetznerClient,
    snapshot_id: u64,
    existing: &state::ProjectState,
    project_config: &project_config::ProjectConfig,
) -> Result<Env> {
    println!("Restoring from snapshot (image: {})...", snapshot_id);

    let server_name = if existing.hostname.is_empty() {
        format!("gob-{}", project.name)
    } else {
        existing.hostname.clone()
    };
    let username = if existing.username.is_empty() {
        whoami()
    } else {
        existing.username.clone()
    };

    // Ensure goblinmode SSH key exists and is uploaded to Hetzner
    let goblin_pubkey = ensure_goblin_ssh_key()?;
    let hetzner_key_id = client.ensure_ssh_key("goblinmode", &goblin_pubkey)?;

    // Create server from snapshot (no cloud-init needed)
    println!("  Server type: {}", project_config.server_type);
    let (server_id, initial_ip) = client.create_server(
        &server_name,
        &project_config.server_type,
        &snapshot_id.to_string(),
        "hel1",
        None,
        Some(vec![hetzner_key_id]),
    )?;
    println!("  Server created (id: {}), waiting for it to start...", server_id);

    // Save state immediately so Ctrl-C doesn't orphan the server
    let project_state = state::ProjectState {
        server_id,
        ipv4: initial_ip,
        username: username.clone(),
        hostname: server_name.clone(),
        snapshot_id: None,
    };
    state::save_state(&project.id, &project_state)?;

    let ip = client.wait_for_server(server_id)?;
    println!("  Server running at {}", ip);

    // Update state with final IP
    let project_state = state::ProjectState {
        server_id,
        ipv4: ip.clone(),
        username: username.clone(),
        hostname: server_name.clone(),
        snapshot_id: None,
    };
    state::save_state(&project.id, &project_state)?;

    // Wait for SSH
    wait_for_ssh(&username, &ip)?;

    // Re-authenticate tailscale
    print!("Re-authenticating Tailscale... ");
    io::stdout().flush()?;
    let ts_result = Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=accept-new",
            &format!("{}@{}", username, ip),
            &format!(
                "sudo tailscale up --auth-key={} --ssh",
                cfg.tailscale_auth_key
            ),
        ])
        .output();
    match ts_result {
        Ok(output) if output.status.success() => println!("ok"),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("Warning: tailscale re-auth may have failed: {}", stderr.trim());
        }
        Err(e) => eprintln!("Warning: tailscale re-auth failed: {}", e),
    }

    // Configure tailscale serve ports
    setup_tailscale_serve(&username, &ip, &project_config.serve_ports);

    // Push project and re-copy SSH key (repo and origin are in the snapshot)
    push_to_vm(&project.root, &username, &server_name, &ip, &project.name)?;
    setup_vm_ssh_key(&username, &ip);

    // Delete old snapshot
    print!("Cleaning up snapshot... ");
    io::stdout().flush()?;
    match client.delete_image(snapshot_id) {
        Ok(()) => println!("done"),
        Err(e) => eprintln!("Warning: failed to delete snapshot: {}", e),
    }

    Ok(Env {
        username,
        hostname: server_name,
        project_name: project.name.clone(),
    })
}

fn wait_for_ssh(username: &str, ip: &str) -> Result<()> {
    print!("Waiting for SSH... ");
    io::stdout().flush()?;

    let max_attempts = 60; // 2 minutes max
    let target = format!("{}@{}", username, ip);

    for _ in 0..max_attempts {
        let result = Command::new("ssh")
            .args([
                "-o",
                "StrictHostKeyChecking=accept-new",
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
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }

    anyhow::bail!("Timed out waiting for SSH on {}", ip);
}

fn init_vm_repo(username: &str, ip: &str, project_name: &str) -> Result<()> {
    println!("Initializing git repo on VM...");
    let remote_cmd = format!(
        "mkdir -p ~/{project_name} && cd ~/{project_name} && git init && git config receive.denyCurrentBranch updateInstead"
    );
    let output = Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=accept-new",
            &format!("{}@{}", username, ip),
            &remote_cmd,
        ])
        .output()
        .context("Failed to run ssh")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to init git repo on VM: {}", stderr.trim());
    }
    println!("  Git repo initialized at ~/{}/", project_name);
    Ok(())
}

fn push_to_vm(
    project_root: &std::path::Path,
    username: &str,
    hostname: &str,
    ip: &str,
    project_name: &str,
) -> Result<()> {
    println!("Pushing project to VM...");

    // Pre-check: ensure there are commits to push
    let head_check = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(project_root)
        .output()
        .context("Failed to run git")?;
    if !head_check.status.success() {
        bail!("No commits in this repository. Make an initial commit before running `gob up`.");
    }

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
    if !status.success() {
        bail!("Failed to add git remote 'gob'");
    }

    // Detect current branch
    let branch_output = Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(project_root)
        .output()
        .context("Failed to run git")?;

    let ssh_cmd = "ssh -o StrictHostKeyChecking=accept-new";

    let branch = if branch_output.status.success() {
        String::from_utf8_lossy(&branch_output.stdout).trim().to_string()
    } else {
        "main".to_string()
    };

    let push_status = if branch_output.status.success() {
        Command::new("git")
            .args(["push", "gob", &branch])
            .env("GIT_SSH_COMMAND", ssh_cmd)
            .current_dir(project_root)
            .status()
            .context("Failed to run git push")?
    } else {
        // Detached HEAD — push as main
        Command::new("git")
            .args(["push", "gob", "HEAD:refs/heads/main"])
            .env("GIT_SSH_COMMAND", ssh_cmd)
            .current_dir(project_root)
            .status()
            .context("Failed to run git push")?
    };

    if !push_status.success() {
        bail!("git push to VM failed");
    }

    // Checkout the pushed branch on the VM
    let checkout_output = Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=accept-new",
            &format!("{}@{}", username, ip),
            &format!("cd ~/{} && git checkout {}", project_name, branch),
        ])
        .output()
        .context("Failed to checkout branch on VM")?;
    if !checkout_output.status.success() {
        let stderr = String::from_utf8_lossy(&checkout_output.stderr);
        bail!("Failed to checkout branch '{}' on VM: {}", branch, stderr.trim());
    }

    println!("  Project pushed to {}", remote_url);
    Ok(())
}

fn setup_vm_origin(
    username: &str,
    ip: &str,
    project_root: &std::path::Path,
    project_name: &str,
) {
    // Get local origin URL
    let origin_output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(project_root)
        .output();

    let origin_url = match origin_output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => {
            eprintln!("Warning: no 'origin' remote found locally, skipping VM origin setup");
            return;
        }
    };

    let remote_cmd = format!(
        "cd ~/{project_name} && git remote remove origin 2>/dev/null; git remote add origin {origin_url}"
    );
    let result = Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=accept-new",
            &format!("{}@{}", username, ip),
            &remote_cmd,
        ])
        .status();

    match result {
        Ok(s) if s.success() => println!("  VM origin set to {}", origin_url),
        Ok(_) => eprintln!("Warning: failed to set origin remote on VM"),
        Err(e) => eprintln!("Warning: failed to set origin remote on VM: {}", e),
    }
}

fn setup_vm_ssh_key(username: &str, ip: &str) {
    println!("Setting up SSH key on VM...");

    let data_dir = match dirs::data_dir() {
        Some(d) => d,
        None => {
            eprintln!("Warning: could not determine data directory, skipping VM SSH key setup");
            return;
        }
    };

    let key_dir = data_dir.join("goblinmode");
    if let Err(e) = fs::create_dir_all(&key_dir) {
        eprintln!("Warning: failed to create key directory: {}", e);
        return;
    }

    let private_key_path = key_dir.join("vm_id_ed25519");
    let public_key_path = key_dir.join("vm_id_ed25519.pub");

    // Generate key pair if it doesn't exist
    if !private_key_path.exists() {
        println!("  Generating VM SSH key...");
        let status = Command::new("ssh-keygen")
            .args([
                "-t", "ed25519",
                "-f", &private_key_path.to_string_lossy(),
                "-N", "",
                "-C", "goblinmode-vm",
            ])
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(_) => {
                eprintln!("Warning: ssh-keygen failed, skipping VM SSH key setup");
                return;
            }
            Err(e) => {
                eprintln!("Warning: failed to run ssh-keygen: {}", e);
                return;
            }
        }
    }

    let target = format!("{}@{}", username, ip);

    // SCP private and public key to VM
    let scp_result = Command::new("scp")
        .args([
            "-o", "StrictHostKeyChecking=accept-new",
            &private_key_path.to_string_lossy(),
            &format!("{}:~/.ssh/id_ed25519", target),
        ])
        .status();
    if !matches!(scp_result, Ok(s) if s.success()) {
        eprintln!("Warning: failed to copy SSH private key to VM");
        return;
    }

    let scp_result = Command::new("scp")
        .args([
            "-o", "StrictHostKeyChecking=accept-new",
            &public_key_path.to_string_lossy(),
            &format!("{}:~/.ssh/id_ed25519.pub", target),
        ])
        .status();
    if !matches!(scp_result, Ok(s) if s.success()) {
        eprintln!("Warning: failed to copy SSH public key to VM");
        return;
    }

    // Fix permissions and configure SSH on VM
    let ssh_config = r#"Host github.com gitlab.com
    StrictHostKeyChecking accept-new"#;
    let remote_cmd = format!(
        "chmod 600 ~/.ssh/id_ed25519 && echo '{}' >> ~/.ssh/config && chmod 600 ~/.ssh/config",
        ssh_config
    );
    let _ = Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=accept-new",
            &target,
            &remote_cmd,
        ])
        .status();

    // Print public key for user
    if let Ok(pubkey) = fs::read_to_string(&public_key_path) {
        println!("  VM SSH public key (add to GitHub/GitLab if not already done):");
        println!("  {}", pubkey.trim());
    }
}

fn wait_for_cloud_init(username: &str, ip: &str) -> Result<()> {
    print!("Waiting for cloud-init... ");
    io::stdout().flush()?;

    let output = Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=accept-new",
            &format!("{}@{}", username, ip),
            "cloud-init status --wait",
        ])
        .output()
        .context("Failed to run ssh")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() || stdout.contains("status: done") {
        println!("done");
    } else {
        let msg = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        bail!("cloud-init failed: {}", msg);
    }

    Ok(())
}

fn setup_dotfiles(username: &str, ip: &str, repo: &str, install_cmd: &str) {
    println!("Setting up dotfiles...");

    let remote_cmd = format!(
        "git clone {} ~/dotfiles && cd ~/dotfiles && {}",
        repo, install_cmd
    );

    let result = Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=accept-new",
            &format!("{}@{}", username, ip),
            &remote_cmd,
        ])
        .status();

    match result {
        Ok(status) if status.success() => println!("  Dotfiles installed"),
        Ok(status) => eprintln!(
            "Warning: dotfiles setup failed (exit code {})",
            status.code().unwrap_or(-1)
        ),
        Err(e) => eprintln!("Warning: dotfiles setup failed: {}", e),
    }
}

fn setup_tailscale_serve(username: &str, ip: &str, ports: &[u16]) {
    for port in ports {
        println!("Setting up tailscale serve for port {}...", port);
        let result = Command::new("ssh")
            .args([
                "-o",
                "StrictHostKeyChecking=accept-new",
                &format!("{}@{}", username, ip),
                &format!("sudo tailscale serve --bg {}", port),
            ])
            .status();
        match result {
            Ok(s) if s.success() => {}
            Ok(_) => eprintln!(
                "Warning: failed to configure tailscale serve for port {}",
                port
            ),
            Err(e) => eprintln!(
                "Warning: failed to configure tailscale serve for port {}: {}",
                port, e
            ),
        }
    }
}

fn detect_timezone() -> Option<String> {
    // 1. Honour explicit TZ env var
    if let Ok(tz) = std::env::var("TZ") {
        if !tz.is_empty() {
            return Some(tz);
        }
    }

    // 2. Resolve /etc/localtime symlink and strip the zoneinfo prefix.
    // Works on Linux (/usr/share/zoneinfo/…) and macOS (/var/db/timezone/zoneinfo/…).
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let s = target.to_string_lossy();
        if let Some(pos) = s.find("/zoneinfo/") {
            let tz = &s[pos + "/zoneinfo/".len()..];
            if !tz.is_empty() {
                return Some(tz.to_string());
            }
        }
    }

    // 3. Linux fallback: /etc/timezone plain-text file (e.g. "Europe/Helsinki\n")
    // (Not present on macOS, but harmless to try.)
    if let Ok(contents) = std::fs::read_to_string("/etc/timezone") {
        let tz = contents.trim().to_string();
        if !tz.is_empty() {
            return Some(tz);
        }
    }

    None
}

pub(crate) fn build_cloud_init(
    username: &str,
    ssh_pubkey: &str,
    tailscale_auth_key: &str,
    is_rust: bool,
    vm_packages: &[String],
    coding_agents: &[String],
) -> String {
    let extra_packages = if is_rust {
        "\n  - build-essential\n  - rustup"
    } else {
        ""
    };
    let rust_cmds = if is_rust {
        format!("\n  - su - {username} -c 'rustup default stable'")
    } else {
        String::new()
    };
    let timezone_line = match detect_timezone() {
        Some(tz) => format!("\ntimezone: {tz}"),
        None => String::new(),
    };
    let configurable_packages: String = vm_packages
        .iter()
        .map(|p| format!("\n  - {p}"))
        .collect();

    // Accumulate runcmd entries for each coding agent (run as the provisioned user)
    let mut agent_cmds = String::new();

    for agent in coding_agents {
        match agent.as_str() {
            "claude-code" => {
                agent_cmds.push_str(&format!(
                    "\n  - su - {username} -c 'curl -fsSL https://claude.ai/install.sh | bash'"
                ));
            }
            "opencode" => {
                agent_cmds.push_str(&format!(
                    "\n  - su - {username} -c 'curl -fsSL https://opencode.ai/install | sh'"
                ));
            }
            other => {
                eprintln!("Warning: unknown coding_agent '{}', skipping", other);
            }
        }
    }

    format!(
        r#"#cloud-config{timezone_line}
users:
  - name: {username}
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/zsh
    ssh_authorized_keys:
      - {ssh_pubkey}

ssh_pwauth: false

package_update: true
packages:
  - git
  - stow
  - zsh
  - tmux
  - mosh
  - just{configurable_packages}{extra_packages}

runcmd:
  - sed -i 's/^PermitRootLogin .*/PermitRootLogin no/' /etc/ssh/sshd_config
  - systemctl restart sshd
  - curl -fsSL https://tailscale.com/install.sh | sh
  - tailscale up --auth-key={tailscale_auth_key} --ssh
  - su - {username} -c "curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash"
  - su - {username} -c "/home/{username}/.cargo/bin/cargo-binstall --no-confirm --strategies crate-meta-data jj-cli"{rust_cmds}{agent_cmds}
"#
    )
}

pub(crate) fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Ensure a goblinmode SSH key pair exists in the data directory.
/// Returns the public key contents.
fn ensure_goblin_ssh_key() -> Result<String> {
    let data_dir = dirs::data_dir().context("Could not determine data directory")?;
    let key_dir = data_dir.join("goblinmode");
    fs::create_dir_all(&key_dir)?;

    let private_key_path = key_dir.join("id_ed25519");
    let public_key_path = key_dir.join("id_ed25519.pub");

    if !private_key_path.exists() {
        println!("Generating goblinmode SSH key...");
        let status = Command::new("ssh-keygen")
            .args([
                "-t", "ed25519",
                "-f", &private_key_path.to_string_lossy(),
                "-N", "",
                "-C", "goblinmode",
            ])
            .status()
            .context("Failed to run ssh-keygen")?;
        if !status.success() {
            bail!("ssh-keygen failed");
        }
    }

    let pubkey = fs::read_to_string(&public_key_path).with_context(|| {
        format!("Failed to read {}", public_key_path.display())
    })?;
    Ok(pubkey.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cloud_init(is_rust: bool, packages: &[String], agents: &[String]) -> String {
        // Pin timezone so snapshots are deterministic across machines
        std::env::set_var("TZ", "UTC");
        build_cloud_init("testuser", "ssh-ed25519 AAAA", "tskey-auth-xxx", is_rust, packages, agents)
    }

    #[test]
    fn cloud_init_basic() {
        let output = test_cloud_init(false, &[], &[]);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn cloud_init_with_rust() {
        let output = test_cloud_init(true, &[], &[]);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn cloud_init_with_packages() {
        let packages = vec!["nodejs".to_string(), "python3".to_string()];
        let output = test_cloud_init(false, &packages, &[]);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn cloud_init_with_agents() {
        let agents = vec!["claude-code".to_string(), "opencode".to_string()];
        let output = test_cloud_init(false, &[], &agents);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn cloud_init_full() {
        let packages = vec!["nodejs".to_string()];
        let agents = vec!["claude-code".to_string()];
        let output = test_cloud_init(true, &packages, &agents);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn whoami_uses_user_env() {
        let key = "USER";
        let original = std::env::var(key).ok();
        std::env::set_var(key, "gobtest");
        let result = whoami();
        match original {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        assert_eq!(result, "gobtest");
    }
}
