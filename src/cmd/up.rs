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
use crate::tailscale::TailscaleClient;

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
    let current_runtime = current_runtime_config(&cfg, &project_config);
    let current_provisioning = current_provisioning_config(&project, &cfg, &project_config);

    // 4. Check existing state
    if let Some(existing) = state::load_state(&project.id)? {
        if reset {
            println!("--reset: destroying existing VM...");
            down::teardown(&project, &existing, &cfg)?;
        } else {
            // 4a. Check for snapshot restore
            if let Some(snapshot_id) = existing.snapshot_id {
                return restore_from_snapshot(
                    &project,
                    &cfg,
                    &client,
                    snapshot_id,
                    &existing,
                    &project_config,
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
                        let username = existing.username.clone();
                        wait_for_ssh(&username, &ip)?;
                        reconcile_runtime_config(
                            &username,
                            &ip,
                            existing.applied_runtime.as_ref(),
                            &current_runtime,
                        );
                        warn_for_provisioning_changes(
                            existing.applied_provisioning.as_ref(),
                            &current_provisioning,
                        );
                        let hostname = if existing.hostname.is_empty() {
                            format!("gob-{}", project.name)
                        } else {
                            existing.hostname.clone()
                        };
                        state::save_state(
                            &project.id,
                            &state::ProjectState {
                                server_id: existing.server_id,
                                ipv4: ip,
                                username: username.clone(),
                                hostname: hostname.clone(),
                                snapshot_id: existing.snapshot_id,
                                applied_runtime: Some(current_runtime.clone()),
                                applied_provisioning: Some(current_provisioning.clone()),
                            },
                        )?;
                        return Ok(Env {
                            username,
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
    let tailscale_auth_key = resolve_tailscale_auth_key(&cfg)?;
    let is_rust = current_provisioning.is_rust;
    let user_data = build_cloud_init(
        &username,
        ssh_pubkey.trim(),
        &tailscale_auth_key,
        is_rust,
        &cfg.vm_packages,
        &cfg.coding_agents,
    );
    let server_name = format!("gob-{}", project.name);
    println!(
        "Creating server '{}' (type: {})...",
        server_name, project_config.server_type
    );
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
        applied_runtime: Some(current_runtime.clone()),
        applied_provisioning: Some(current_provisioning.clone()),
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
        applied_runtime: Some(current_runtime.clone()),
        applied_provisioning: Some(current_provisioning.clone()),
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

fn current_runtime_config(
    cfg: &config::Config,
    project_config: &project_config::ProjectConfig,
) -> state::AppliedRuntimeConfig {
    state::AppliedRuntimeConfig {
        vm_packages: cfg.vm_packages.clone(),
        coding_agents: cfg.coding_agents.clone(),
        serve_ports: project_config.serve_ports.clone(),
    }
}

fn current_provisioning_config(
    project: &project::Project,
    cfg: &config::Config,
    project_config: &project_config::ProjectConfig,
) -> state::AppliedProvisioningConfig {
    state::AppliedProvisioningConfig {
        server_type: project_config.server_type.clone(),
        is_rust: project.root.join("Cargo.toml").exists(),
        dotfiles_repo: cfg.dotfiles_repo.clone(),
        dotfiles_install: cfg.dotfiles_install.clone(),
    }
}

fn warn_for_provisioning_changes(
    previous: Option<&state::AppliedProvisioningConfig>,
    current: &state::AppliedProvisioningConfig,
) {
    let changed = provisioning_change_messages(previous, current);
    if !changed.is_empty() {
        eprintln!("Warning: provisioning-time settings changed:");
        for change in changed {
            eprintln!("  - {}", change);
        }
        eprintln!("Run `gob up --reset` to apply these provisioning changes.");
    }
}

fn provisioning_change_messages(
    previous: Option<&state::AppliedProvisioningConfig>,
    current: &state::AppliedProvisioningConfig,
) -> Vec<String> {
    let Some(previous) = previous else {
        return Vec::new();
    };

    let mut changed = Vec::new();
    if previous.server_type != current.server_type {
        changed.push(format!(
            "server_type: '{}' -> '{}'",
            previous.server_type, current.server_type
        ));
    }
    if previous.is_rust != current.is_rust {
        changed.push(format!(
            "rust project mode: {} -> {}",
            previous.is_rust, current.is_rust
        ));
    }
    if previous.dotfiles_repo != current.dotfiles_repo {
        changed.push(format!(
            "dotfiles.repo: {:?} -> {:?}",
            previous.dotfiles_repo, current.dotfiles_repo
        ));
    }
    if previous.dotfiles_install != current.dotfiles_install {
        changed.push(format!(
            "dotfiles.install: {:?} -> {:?}",
            previous.dotfiles_install, current.dotfiles_install
        ));
    }
    changed
}

#[derive(Debug, PartialEq, Eq)]
struct RuntimeConfigDelta {
    added_packages: Vec<String>,
    removed_packages: Vec<String>,
    added_agents: Vec<String>,
    removed_agents: Vec<String>,
}

fn runtime_config_delta(
    previous: Option<&state::AppliedRuntimeConfig>,
    current: &state::AppliedRuntimeConfig,
) -> RuntimeConfigDelta {
    let previous_packages = previous.map(|c| c.vm_packages.as_slice()).unwrap_or(&[]);
    let previous_agents = previous.map(|c| c.coding_agents.as_slice()).unwrap_or(&[]);

    let removed_packages = previous_packages
        .iter()
        .filter(|p| !current.vm_packages.contains(*p))
        .cloned()
        .collect();
    let added_packages = current
        .vm_packages
        .iter()
        .filter(|p| !previous_packages.contains(*p))
        .cloned()
        .collect();
    let removed_agents = previous_agents
        .iter()
        .filter(|a| !current.coding_agents.contains(*a))
        .cloned()
        .collect();
    let added_agents = current
        .coding_agents
        .iter()
        .filter(|a| !previous_agents.contains(*a))
        .cloned()
        .collect();

    RuntimeConfigDelta {
        added_packages,
        removed_packages,
        added_agents,
        removed_agents,
    }
}

fn reconcile_runtime_config(
    username: &str,
    ip: &str,
    previous: Option<&state::AppliedRuntimeConfig>,
    current: &state::AppliedRuntimeConfig,
) {
    let target = format!("{}@{}", username, ip);
    reconcile_runtime_config_with(username, previous, current, |remote_cmd| {
        run_remote_status(&target, remote_cmd)
            .map(|s| s.success())
            .unwrap_or(false)
    });
}

fn reconcile_runtime_config_with<F>(
    username: &str,
    previous: Option<&state::AppliedRuntimeConfig>,
    current: &state::AppliedRuntimeConfig,
    mut run_remote: F,
) where
    F: FnMut(&str) -> bool,
{
    let delta = runtime_config_delta(previous, current);

    if !delta.added_packages.is_empty() {
        println!("Installing newly configured VM packages...");
        let install_cmd = format!(
            "sudo DEBIAN_FRONTEND=noninteractive apt-get update && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y {}",
            delta.added_packages.join(" ")
        );
        if !run_remote(&install_cmd) {
            eprintln!(
                "Warning: failed to install some configured VM packages: {}",
                delta.added_packages.join(", ")
            );
        }
    }
    if !delta.removed_packages.is_empty() {
        eprintln!(
            "Warning: removed vm.packages are not auto-uninstalled: {}",
            delta.removed_packages.join(", ")
        );
    }

    for agent in &delta.added_agents {
        if let Some(cmd) = coding_agent_install_cmd(agent, username) {
            println!("Installing newly configured coding agent '{}'...", agent);
            if !run_remote(&cmd) {
                eprintln!("Warning: failed to install coding agent '{}'", agent);
            }
        } else {
            eprintln!("Warning: unknown coding_agent '{}', skipping", agent);
        }
    }
    if !delta.removed_agents.is_empty() {
        eprintln!(
            "Warning: removed vm.coding_agents are not auto-uninstalled: {}",
            delta.removed_agents.join(", ")
        );
    }

    reconcile_tailscale_serve_with(&current.serve_ports, &mut run_remote);
}

fn restore_from_snapshot(
    project: &crate::project::Project,
    cfg: &config::Config,
    client: &HetznerClient,
    snapshot_id: u64,
    existing: &state::ProjectState,
    project_config: &project_config::ProjectConfig,
) -> Result<Env> {
    let current_runtime = current_runtime_config(cfg, project_config);
    let current_provisioning = current_provisioning_config(project, cfg, project_config);

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
    println!(
        "  Server created (id: {}), waiting for it to start...",
        server_id
    );

    // Save state immediately so Ctrl-C doesn't orphan the server
    let project_state = state::ProjectState {
        server_id,
        ipv4: initial_ip,
        username: username.clone(),
        hostname: server_name.clone(),
        snapshot_id: None,
        applied_runtime: Some(current_runtime.clone()),
        applied_provisioning: Some(current_provisioning.clone()),
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
        applied_runtime: Some(current_runtime.clone()),
        applied_provisioning: Some(current_provisioning.clone()),
    };
    state::save_state(&project.id, &project_state)?;

    // Wait for SSH
    wait_for_ssh(&username, &ip)?;

    // Re-authenticate tailscale
    print!("Re-authenticating Tailscale... ");
    io::stdout().flush()?;
    let tailscale_auth_key = resolve_tailscale_auth_key(cfg)?;
    let ts_result = Command::new("ssh")
        .args([
            "-o",
            "StrictHostKeyChecking=accept-new",
            &format!("{}@{}", username, ip),
            &format!("sudo tailscale up --auth-key={} --ssh", tailscale_auth_key),
        ])
        .output();
    match ts_result {
        Ok(output) if output.status.success() => println!("ok"),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!(
                "Warning: tailscale re-auth may have failed: {}",
                stderr.trim()
            );
        }
        Err(e) => eprintln!("Warning: tailscale re-auth failed: {}", e),
    }

    // Configure tailscale serve ports
    reconcile_runtime_config(
        &username,
        &ip,
        existing.applied_runtime.as_ref(),
        &current_runtime,
    );
    warn_for_provisioning_changes(
        existing.applied_provisioning.as_ref(),
        &current_provisioning,
    );

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
            "-o",
            "StrictHostKeyChecking=accept-new",
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
        String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string()
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
            "-o",
            "StrictHostKeyChecking=accept-new",
            &format!("{}@{}", username, ip),
            &format!("cd ~/{} && git checkout {}", project_name, branch),
        ])
        .output()
        .context("Failed to checkout branch on VM")?;
    if !checkout_output.status.success() {
        let stderr = String::from_utf8_lossy(&checkout_output.stderr);
        bail!(
            "Failed to checkout branch '{}' on VM: {}",
            branch,
            stderr.trim()
        );
    }

    println!("  Project pushed to {}", remote_url);
    Ok(())
}

fn setup_vm_origin(username: &str, ip: &str, project_root: &std::path::Path, project_name: &str) {
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
            "-o",
            "StrictHostKeyChecking=accept-new",
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
                "-t",
                "ed25519",
                "-f",
                &private_key_path.to_string_lossy(),
                "-N",
                "",
                "-C",
                "goblinmode-vm",
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
            "-o",
            "StrictHostKeyChecking=accept-new",
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
            "-o",
            "StrictHostKeyChecking=accept-new",
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
            "-o",
            "StrictHostKeyChecking=accept-new",
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

fn run_remote_status(target: &str, remote_cmd: &str) -> io::Result<std::process::ExitStatus> {
    Command::new("ssh")
        .args(["-o", "StrictHostKeyChecking=accept-new", target, remote_cmd])
        .status()
}

fn reconcile_tailscale_serve_with<F>(ports: &[u16], run_remote: &mut F)
where
    F: FnMut(&str) -> bool,
{
    println!("Resetting tailscale serve configuration...");
    if !run_remote("sudo tailscale serve reset") {
        eprintln!("Warning: failed to reset tailscale serve configuration");
    }

    for port in ports {
        println!("Setting up tailscale serve for port {}...", port);
        if !run_remote(&format!("sudo tailscale serve --bg {}", port)) {
            eprintln!(
                "Warning: failed to configure tailscale serve for port {}",
                port
            );
        }
    }
}

fn setup_tailscale_serve(username: &str, ip: &str, ports: &[u16]) {
    let target = format!("{}@{}", username, ip);
    reconcile_tailscale_serve_with(ports, &mut |remote_cmd| {
        run_remote_status(&target, remote_cmd)
            .map(|s| s.success())
            .unwrap_or(false)
    });
}

fn coding_agent_install_cmd(agent: &str, username: &str) -> Option<String> {
    match agent {
        "claude-code" => Some(format!(
            "su - {username} -c 'curl -fsSL https://claude.ai/install.sh | bash'"
        )),
        "opencode" => Some(format!(
            "su - {username} -c 'curl -fsSL https://opencode.ai/install | sh'"
        )),
        _ => None,
    }
}

/// Resolve the Tailscale auth key: use the configured key if set, otherwise
/// create a one-time preauthorized key via the Tailscale API.
fn resolve_tailscale_auth_key(cfg: &config::Config) -> Result<String> {
    if let Some(ref key) = cfg.tailscale_auth_key {
        return Ok(key.clone());
    }
    println!("No tailscale_auth_key configured — creating one-time key via API...");
    let ts = TailscaleClient::new(cfg.tailscale_api_key.clone());
    ts.create_auth_key(&cfg.tailscale_tags)
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
    let configurable_packages: String = vm_packages.iter().map(|p| format!("\n  - {p}")).collect();

    // Accumulate runcmd entries for each coding agent (run as the provisioned user)
    let mut agent_cmds = String::new();

    for agent in coding_agents {
        if let Some(cmd) = coding_agent_install_cmd(agent, username) {
            agent_cmds.push_str(&format!("\n  - {cmd}"));
        } else {
            eprintln!("Warning: unknown coding_agent '{}', skipping", agent);
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
                "-t",
                "ed25519",
                "-f",
                &private_key_path.to_string_lossy(),
                "-N",
                "",
                "-C",
                "goblinmode",
            ])
            .status()
            .context("Failed to run ssh-keygen")?;
        if !status.success() {
            bail!("ssh-keygen failed");
        }
    }

    let pubkey = fs::read_to_string(&public_key_path)
        .with_context(|| format!("Failed to read {}", public_key_path.display()))?;
    Ok(pubkey.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    fn test_cloud_init(is_rust: bool, packages: &[String], agents: &[String]) -> String {
        // Pin timezone so snapshots are deterministic across machines
        std::env::set_var("TZ", "UTC");
        build_cloud_init(
            "testuser",
            "ssh-ed25519 AAAA",
            "tskey-auth-xxx",
            is_rust,
            packages,
            agents,
        )
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

    fn test_config() -> config::Config {
        config::Config {
            hetzner_api_token: "h".to_string(),
            tailscale_auth_key: None,
            tailscale_api_key: "t".to_string(),
            tailscale_tags: vec![],
            dotfiles_repo: Some("git@example.com:dotfiles.git".to_string()),
            dotfiles_install: Some("./install.sh".to_string()),
            vm_packages: vec!["jq".to_string(), "ripgrep".to_string()],
            coding_agents: vec!["claude-code".to_string()],
        }
    }

    fn test_project(root: PathBuf) -> project::Project {
        project::Project {
            root,
            name: "proj".to_string(),
            id: "proj-1".to_string(),
        }
    }

    #[test]
    fn current_runtime_config_maps_values() {
        let cfg = test_config();
        let project_cfg = project_config::ProjectConfig {
            serve_ports: vec![3000, 8080],
            server_type: "cx42".to_string(),
        };
        let runtime = current_runtime_config(&cfg, &project_cfg);
        assert_eq!(runtime.vm_packages, vec!["jq", "ripgrep"]);
        assert_eq!(runtime.coding_agents, vec!["claude-code"]);
        assert_eq!(runtime.serve_ports, vec![3000, 8080]);
    }

    #[test]
    fn current_provisioning_config_detects_rust_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let cfg = test_config();
        let project_cfg = project_config::ProjectConfig {
            serve_ports: vec![],
            server_type: "cx42".to_string(),
        };
        let provisioning = current_provisioning_config(
            &test_project(dir.path().to_path_buf()),
            &cfg,
            &project_cfg,
        );
        assert_eq!(provisioning.server_type, "cx42");
        assert!(provisioning.is_rust);
        assert_eq!(
            provisioning.dotfiles_repo,
            Some("git@example.com:dotfiles.git".to_string())
        );
        assert_eq!(
            provisioning.dotfiles_install,
            Some("./install.sh".to_string())
        );
    }

    #[test]
    fn provisioning_change_messages_reports_all_differences() {
        let previous = state::AppliedProvisioningConfig {
            server_type: "cx23".to_string(),
            is_rust: false,
            dotfiles_repo: None,
            dotfiles_install: None,
        };
        let current = state::AppliedProvisioningConfig {
            server_type: "cx42".to_string(),
            is_rust: true,
            dotfiles_repo: Some("git@example.com:dotfiles.git".to_string()),
            dotfiles_install: Some("./install.sh".to_string()),
        };
        let changes = provisioning_change_messages(Some(&previous), &current);
        assert_eq!(changes.len(), 4);
        assert!(changes.iter().any(|c| c.contains("server_type")));
        assert!(changes.iter().any(|c| c.contains("rust project mode")));
        assert!(changes.iter().any(|c| c.contains("dotfiles.repo")));
        assert!(changes.iter().any(|c| c.contains("dotfiles.install")));
        assert!(provisioning_change_messages(None, &current).is_empty());
    }

    #[test]
    fn runtime_config_delta_tracks_added_and_removed_entries() {
        let previous = state::AppliedRuntimeConfig {
            vm_packages: vec!["git".to_string(), "jq".to_string()],
            coding_agents: vec!["claude-code".to_string()],
            serve_ports: vec![3000],
        };
        let current = state::AppliedRuntimeConfig {
            vm_packages: vec!["jq".to_string(), "ripgrep".to_string()],
            coding_agents: vec!["opencode".to_string()],
            serve_ports: vec![8080],
        };
        let delta = runtime_config_delta(Some(&previous), &current);
        assert_eq!(delta.added_packages, vec!["ripgrep"]);
        assert_eq!(delta.removed_packages, vec!["git"]);
        assert_eq!(delta.added_agents, vec!["opencode"]);
        assert_eq!(delta.removed_agents, vec!["claude-code"]);
    }

    #[test]
    fn reconcile_tailscale_serve_with_runs_reset_then_ports() {
        let calls = RefCell::new(Vec::<String>::new());
        reconcile_tailscale_serve_with(&[3000, 8080], &mut |cmd| {
            calls.borrow_mut().push(cmd.to_string());
            true
        });
        assert_eq!(
            calls.into_inner(),
            vec![
                "sudo tailscale serve reset".to_string(),
                "sudo tailscale serve --bg 3000".to_string(),
                "sudo tailscale serve --bg 8080".to_string(),
            ]
        );
    }

    #[test]
    fn reconcile_runtime_config_with_applies_additions_and_serve() {
        let previous = state::AppliedRuntimeConfig {
            vm_packages: vec!["git".to_string()],
            coding_agents: vec!["unknown-old".to_string()],
            serve_ports: vec![1234],
        };
        let current = state::AppliedRuntimeConfig {
            vm_packages: vec!["git".to_string(), "jq".to_string()],
            coding_agents: vec!["claude-code".to_string(), "unknown-new".to_string()],
            serve_ports: vec![3000],
        };
        let calls = RefCell::new(Vec::<String>::new());
        reconcile_runtime_config_with("alice", Some(&previous), &current, |cmd| {
            calls.borrow_mut().push(cmd.to_string());
            true
        });
        let calls = calls.into_inner();
        assert!(calls[0].contains("apt-get install -y jq"));
        assert!(calls[1].contains("curl -fsSL https://claude.ai/install.sh"));
        assert_eq!(calls[2], "sudo tailscale serve reset");
        assert_eq!(calls[3], "sudo tailscale serve --bg 3000");
        assert_eq!(calls.len(), 4);
    }

    #[test]
    fn coding_agent_install_cmd_supports_known_agents() {
        let claude = coding_agent_install_cmd("claude-code", "alice").unwrap();
        assert!(claude.contains("claude.ai/install.sh"));
        let opencode = coding_agent_install_cmd("opencode", "alice").unwrap();
        assert!(opencode.contains("opencode.ai/install"));
        assert!(coding_agent_install_cmd("unknown", "alice").is_none());
    }
}
