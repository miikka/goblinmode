use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{self, Write};
use std::process::Command;
use tracing::{info, instrument, warn};

use crate::cloud_init;
use crate::cmd::down;
use crate::config;
use crate::hetzner::HetznerClient;
use crate::packages::{resolve_coding_agent, PackageSpec};
use crate::project;
use crate::project_config;
use crate::ssh::{self, SshSession};
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
#[instrument(level = "info", skip_all, fields(reset = reset))]
pub fn ensure_running(reset: bool) -> Result<Env> {
    // 1. Detect project root
    let project = project::detect_project()?;
    println!("Project: {} ({})", project.name, project.root.display());
    info!(project = %project.name, root = %project.root.display(), "project_detected");

    // 2. Load config
    let cfg = config::load_config()?;
    let client = HetznerClient::new(cfg.hetzner_api_token.clone());

    // 3. Load project config
    let project_config = project_config::load_project_config(&project.root)?;

    // 4. Check existing state
    if let Some(existing) = state::load_state(&project.id)? {
        info!(
            server_id = existing.server_id,
            snapshot_id = ?existing.snapshot_id,
            "existing_state_found"
        );
        if reset {
            println!("--reset: destroying existing VM...");
            info!("reset_requested_teardown_existing");
            down::teardown(&project, &existing, &cfg)?;
        } else {
            // 4a. Check for snapshot restore (only when no server exists yet)
            if let Some(snapshot_id) = existing.snapshot_id {
                if existing.server_id == 0 {
                    return restore_from_snapshot(
                        &project,
                        &cfg,
                        &client,
                        snapshot_id,
                        &existing,
                        &project_config,
                    );
                }
                // server_id != 0: server was already created from snapshot but we
                // crashed before deleting it. Fall through to check server status;
                // the snapshot will be cleaned up in the "running" branch below.
            }

            if existing.server_id != 0 {
                print!(
                    "Existing server found (id: {}), checking status... ",
                    existing.server_id
                );
                io::stdout().flush()?;

                match client.get_server_status(existing.server_id)? {
                    Some((status, ip)) if status == "running" => {
                        info!(server_id = existing.server_id, ip = %ip, "existing_server_running");
                        println!("running");
                        let username = existing.username.clone();
                        let sess = SshSession::new(&username, &ip);
                        ssh::wait_for_ssh(&sess)?;
                        let hostname = existing.hostname_or_default(&project.name);
                        // Clean up any snapshot left over from a crashed restore
                        let snap = existing.snapshot_id;
                        if let Some(snap_id) = snap {
                            print!("Cleaning up snapshot... ");
                            io::stdout().flush()?;
                            match client.delete_image(snap_id) {
                                Ok(()) => println!("done"),
                                Err(e) => {
                                    eprintln!("Warning: failed to delete snapshot: {}", e)
                                }
                            }
                        }
                        state::save_state(
                            &project.id,
                            &state::ProjectState::new(
                                existing.server_id,
                                ip,
                                username.clone(),
                                hostname.clone(),
                                None,
                            ),
                        )?;
                        return Ok(Env {
                            username,
                            hostname,
                            project_name: project.name,
                        });
                    }
                    Some((status, _)) => {
                        info!(server_id = existing.server_id, status = %status, "existing_server_not_running");
                        println!("{}", status);
                        println!(
                            "Server is not running (status: {}). Creating a new one.",
                            status
                        );
                    }
                    None => {
                        warn!(server_id = existing.server_id, "existing_server_missing");
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
    let toolchains: Vec<state::Toolchain> = state::Toolchain::all()
        .iter()
        .filter(|t| t.detect(&project.root))
        .cloned()
        .collect();
    let project_specs: Vec<PackageSpec> = project_config
        .packages
        .iter()
        .map(|n| PackageSpec::Apt { name: n.clone() })
        .chain(
            project_config
                .binstall_packages
                .iter()
                .map(|n| PackageSpec::CargoBinstall { name: n.clone() }),
        )
        .chain(project_config.coding_agents.iter().filter_map(|name| {
            let spec = resolve_coding_agent(name);
            if spec.is_none() {
                warn!(agent = %name, "unknown coding_agent in project config, skipping");
            }
            spec
        }))
        .collect();
    let packages = merge_package_specs(&cfg.vm_packages, &project_specs);
    let user_data = cloud_init::build_cloud_init(
        &username,
        ssh_pubkey.trim(),
        &tailscale_auth_key,
        &toolchains,
        &packages,
    );
    let server_name = format!("gob-{}", project.name);
    println!(
        "Creating server '{}' (type: {})...",
        server_name, project_config.server_type
    );
    info!(
        server_name = %server_name,
        server_type = %project_config.server_type,
        "creating_server"
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
    info!(server_id = server_id, ip = %initial_ip, "server_created");

    // 6. Save state immediately so Ctrl-C doesn't orphan the server
    let project_state = state::ProjectState::new(
        server_id,
        initial_ip,
        username.clone(),
        server_name.clone(),
        None,
    );
    state::save_state(&project.id, &project_state)?;

    // 7. Poll until running
    let ip = client.wait_for_server(server_id)?;
    println!("  Server running at {}", ip);
    info!(server_id = server_id, ip = %ip, "server_running");

    // Update state with final IP
    let project_state = state::ProjectState::new(
        server_id,
        ip.clone(),
        username.clone(),
        server_name.clone(),
        None,
    );
    state::save_state(&project.id, &project_state)?;

    // 8. Wait for SSH (also establishes the ControlMaster socket for subsequent operations)
    let sess = SshSession::new(&username, &ip);
    ssh::wait_for_ssh(&sess)?;

    // 9. Wait for cloud-init to finish (packages, tailscale, etc.)
    ssh::wait_for_cloud_init(&sess)?;

    // 10. Configure tailscale serve ports
    setup_tailscale_serve(&sess, &project_config.serve_ports);

    // 11. Init git repo and push project to VM
    init_vm_repo(&sess, &project.name)?;
    push_to_vm(&project.root, &sess, &server_name, &project.name)?;

    // 12. Setup VM origin and SSH key
    setup_vm_origin(&sess, &project.root, &project.name);
    setup_vm_ssh_key(&sess);

    // 13. Setup dotfiles
    if let Some(ref repo) = cfg.dotfiles_repo {
        let install_cmd = cfg.dotfiles_install.as_deref().unwrap_or("./install.sh");
        setup_dotfiles(&sess, repo, install_cmd);
    }

    Ok(Env {
        username,
        hostname: server_name,
        project_name: project.name,
    })
}

#[instrument(level = "info", skip_all, fields(reset = reset))]
pub fn run(reset: bool) -> Result<()> {
    let env = ensure_running(reset)?;
    println!("SSH ready: ssh {}@{}", env.username, env.hostname);
    info!(username = %env.username, hostname = %env.hostname, "up_command_ready");
    Ok(())
}

/// Combine user-level and project-level package specs.
/// Project specs whose name already appears in the user list are not duplicated.
fn merge_package_specs(user: &[PackageSpec], project: &[PackageSpec]) -> Vec<PackageSpec> {
    let mut result = user.to_vec();
    for spec in project {
        if !result.iter().any(|s| s.name() == spec.name()) {
            result.push(spec.clone());
        }
    }
    result
}

#[instrument(
    level = "info",
    skip_all,
    fields(snapshot_id = snapshot_id, project = %project.name)
)]
fn restore_from_snapshot(
    project: &crate::project::Project,
    cfg: &config::Config,
    client: &HetznerClient,
    snapshot_id: u64,
    existing: &state::ProjectState,
    project_config: &project_config::ProjectConfig,
) -> Result<Env> {
    println!("Restoring from snapshot (image: {})...", snapshot_id);

    let server_name = existing.hostname_or_default(&project.name);
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
    info!(server_id = server_id, "snapshot_server_created");

    // Save state immediately so Ctrl-C doesn't orphan the server.
    // Keep snapshot_id so a future `gob up` can clean it up if we crash before
    // deleting it.
    let project_state = state::ProjectState::new(
        server_id,
        initial_ip,
        username.clone(),
        server_name.clone(),
        Some(snapshot_id),
    );
    state::save_state(&project.id, &project_state)?;

    let ip = client.wait_for_server(server_id)?;
    println!("  Server running at {}", ip);
    info!(server_id = server_id, ip = %ip, "snapshot_server_running");

    // Update state with final IP, still tracking snapshot_id until deleted
    let project_state = state::ProjectState::new(
        server_id,
        ip.clone(),
        username.clone(),
        server_name.clone(),
        Some(snapshot_id),
    );
    state::save_state(&project.id, &project_state)?;

    // Wait for SSH (also establishes the ControlMaster socket for subsequent operations)
    let sess = SshSession::new(&username, &ip);
    ssh::wait_for_ssh(&sess)?;

    // Re-authenticate tailscale
    print!("Re-authenticating Tailscale... ");
    io::stdout().flush()?;
    let tailscale_auth_key = resolve_tailscale_auth_key(cfg)?;
    match ssh::run_ssh_cmd(
        &sess,
        &format!("sudo tailscale up --auth-key={} --ssh", tailscale_auth_key),
    ) {
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
    setup_tailscale_serve(&sess, &project_config.serve_ports);

    // Push project and re-copy SSH key (repo and origin are in the snapshot)
    push_to_vm(&project.root, &sess, &server_name, &project.name)?;
    setup_vm_ssh_key(&sess);

    // Delete old snapshot and clear it from state
    print!("Cleaning up snapshot... ");
    io::stdout().flush()?;
    match client.delete_image(snapshot_id) {
        Ok(()) => {
            println!("done");
            let final_state = state::ProjectState::new(
                server_id,
                ip.clone(),
                username.clone(),
                server_name.clone(),
                None,
            );
            if let Err(e) = state::save_state(&project.id, &final_state) {
                eprintln!(
                    "Warning: failed to update state after snapshot deletion: {}",
                    e
                );
            }
        }
        Err(e) => eprintln!("Warning: failed to delete snapshot: {}", e),
    }

    Ok(Env {
        username,
        hostname: server_name,
        project_name: project.name.clone(),
    })
}

#[instrument(level = "info", skip_all, fields(username = %sess.username(), ip = %sess.ip(), project_name = project_name))]
fn init_vm_repo(sess: &SshSession, project_name: &str) -> Result<()> {
    println!("Initializing git repo on VM...");
    let remote_cmd = format!(
        "mkdir -p ~/{project_name} && cd ~/{project_name} && git init && git config receive.denyCurrentBranch updateInstead"
    );
    let output = ssh::run_ssh_cmd(sess, &remote_cmd)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to init git repo on VM: {}", stderr.trim());
    }
    println!("  Git repo initialized at ~/{}/", project_name);
    Ok(())
}

#[instrument(
    level = "info",
    skip_all,
    fields(
        username = %sess.username(),
        hostname = hostname,
        ip = %sess.ip(),
        project_name = project_name
    )
)]
fn push_to_vm(
    project_root: &std::path::Path,
    sess: &SshSession,
    hostname: &str,
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

    // Use the server IP rather than the Tailscale hostname so that the push
    // works even when the VM has not yet joined the tailnet (e.g. cloud-init
    // just finished and Tailscale MagicDNS hasn't propagated yet).
    let remote_url = format!("{}@{}:~/{}/", sess.username(), sess.ip(), project_name);

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

    let ssh_cmd = sess.git_ssh_command();

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
            .env("GIT_SSH_COMMAND", &ssh_cmd)
            .current_dir(project_root)
            .status()
            .context("Failed to run git push")?
    } else {
        // Detached HEAD — push as main
        Command::new("git")
            .args(["push", "gob", "HEAD:refs/heads/main"])
            .env("GIT_SSH_COMMAND", &ssh_cmd)
            .current_dir(project_root)
            .status()
            .context("Failed to run git push")?
    };

    if !push_status.success() {
        bail!("git push to VM failed");
    }

    // Checkout the pushed branch on the VM
    let checkout_output = ssh::run_ssh_cmd(
        sess,
        &format!("cd ~/{} && git checkout {}", project_name, branch),
    )?;
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

#[instrument(level = "info", skip_all, fields(username = %sess.username(), ip = %sess.ip(), project_name = project_name))]
fn setup_vm_origin(sess: &SshSession, project_root: &std::path::Path, project_name: &str) {
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
        "cd ~/{project_name} && git remote remove origin 2>/dev/null; git remote add origin {origin_url} && git branch --set-upstream-to=origin/$(git rev-parse --abbrev-ref HEAD) $(git rev-parse --abbrev-ref HEAD) 2>/dev/null || true"
    );
    match ssh::run_ssh_cmd(sess, &remote_cmd) {
        Ok(output) if output.status.success() => println!("  VM origin set to {}", origin_url),
        Ok(_) => eprintln!("Warning: failed to set origin remote on VM"),
        Err(e) => eprintln!("Warning: failed to set origin remote on VM: {}", e),
    }
}

#[instrument(level = "info", skip_all, fields(username = %sess.username(), ip = %sess.ip()))]
fn setup_vm_ssh_key(sess: &SshSession) {
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

    // SCP private and public key to VM using the existing control socket.
    if !ssh::scp_file(
        sess,
        &private_key_path.to_string_lossy(),
        &format!("{}:~/.ssh/id_ed25519", sess.target()),
    )
    .unwrap_or(false)
    {
        eprintln!("Warning: failed to copy SSH private key to VM");
        return;
    }

    if !ssh::scp_file(
        sess,
        &public_key_path.to_string_lossy(),
        &format!("{}:~/.ssh/id_ed25519.pub", sess.target()),
    )
    .unwrap_or(false)
    {
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
    let _ = ssh::run_ssh_cmd(sess, &remote_cmd);

    // Print public key for user
    if let Ok(pubkey) = fs::read_to_string(&public_key_path) {
        println!("  VM SSH public key (add to GitHub/GitLab if not already done):");
        println!("  {}", pubkey.trim());
    }
}

#[instrument(level = "info", skip_all, fields(username = %sess.username(), ip = %sess.ip(), repo = repo))]
fn setup_dotfiles(sess: &SshSession, repo: &str, install_cmd: &str) {
    println!("Setting up dotfiles...");

    let remote_cmd = format!(
        "git clone {} ~/dotfiles && cd ~/dotfiles && {}",
        repo, install_cmd
    );

    match ssh::run_ssh_cmd(sess, &remote_cmd) {
        Ok(output) if output.status.success() => println!("  Dotfiles installed"),
        Ok(output) => eprintln!(
            "Warning: dotfiles setup failed (exit code {})",
            output.status.code().unwrap_or(-1)
        ),
        Err(e) => eprintln!("Warning: dotfiles setup failed: {}", e),
    }
}

#[instrument(level = "debug", skip_all, fields(ports = ?ports))]
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

#[instrument(level = "info", skip_all, fields(username = %sess.username(), ip = %sess.ip(), ports = ?ports))]
fn setup_tailscale_serve(sess: &SshSession, ports: &[u16]) {
    reconcile_tailscale_serve_with(ports, &mut |remote_cmd| {
        ssh::run_remote_status(sess, remote_cmd)
            .map(|s| s.success())
            .unwrap_or(false)
    });
}

/// Resolve the Tailscale auth key: use the configured key if set, otherwise
/// create a one-time preauthorized key via the Tailscale API.
#[instrument(level = "info", skip_all)]
fn resolve_tailscale_auth_key(cfg: &config::Config) -> Result<String> {
    if let Some(ref key) = cfg.tailscale_auth_key {
        return Ok(key.clone());
    }
    println!("No tailscale_auth_key configured — creating one-time key via API...");
    let ts = TailscaleClient::new(cfg.tailscale_api_key.clone());
    ts.create_auth_key(&cfg.tailscale_tags)
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
    fn merge_package_specs_deduplicates_and_preserves_order() {
        let user = vec![
            PackageSpec::Apt {
                name: "jq".to_string(),
            },
            PackageSpec::Apt {
                name: "ripgrep".to_string(),
            },
        ];
        let project = vec![
            PackageSpec::Apt {
                name: "nodejs".to_string(),
            },
            PackageSpec::Apt {
                name: "jq".to_string(),
            },
        ];
        let result = merge_package_specs(&user, &project);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].name(), "jq");
        assert_eq!(result[1].name(), "ripgrep");
        assert_eq!(result[2].name(), "nodejs");
    }

    #[test]
    fn merge_package_specs_empty_project_returns_user_list() {
        let user = vec![PackageSpec::Apt {
            name: "jq".to_string(),
        }];
        let result = merge_package_specs(&user, &[]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name(), "jq");
    }

    #[test]
    fn merge_package_specs_empty_user_returns_project_list() {
        let project = vec![PackageSpec::Apt {
            name: "nodejs".to_string(),
        }];
        let result = merge_package_specs(&[], &project);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name(), "nodejs");
    }

    #[test]
    fn reconcile_tailscale_serve_with_warns_on_reset_failure() {
        let calls = RefCell::new(Vec::<String>::new());
        reconcile_tailscale_serve_with(&[3000], &mut |cmd| {
            calls.borrow_mut().push(cmd.to_string());
            false
        });
        let calls = calls.into_inner();
        assert!(calls.contains(&"sudo tailscale serve reset".to_string()));
        assert!(calls.contains(&"sudo tailscale serve --bg 3000".to_string()));
    }

    #[test]
    fn reconcile_tailscale_serve_with_empty_ports_only_resets() {
        let calls = RefCell::new(Vec::<String>::new());
        reconcile_tailscale_serve_with(&[], &mut |cmd| {
            calls.borrow_mut().push(cmd.to_string());
            true
        });
        assert_eq!(calls.into_inner(), vec!["sudo tailscale serve reset"]);
    }

    #[test]
    fn resolve_tailscale_auth_key_returns_configured_key() {
        let cfg = crate::config::Config {
            hetzner_api_token: "h".to_string(),
            tailscale_auth_key: Some("ts-key-configured".to_string()),
            tailscale_api_key: "t".to_string(),
            tailscale_tags: vec![],
            dotfiles_repo: None,
            dotfiles_install: None,
            vm_packages: vec![],
        };
        let key = resolve_tailscale_auth_key(&cfg).unwrap();
        assert_eq!(key, "ts-key-configured");
    }
}
