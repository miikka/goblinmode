use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{self, Write};
use std::process::Command;
use tracing::{info, instrument, warn};

use crate::cloud_init;
use crate::cmd::down;
use crate::config;
use crate::hetzner::HetznerClient;
use crate::packages::PackageSpec;
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
    let current_runtime = current_runtime_config(&cfg, &project_config);
    let current_provisioning = current_provisioning_config(&project, &cfg, &project_config);

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
                        info!(server_id = existing.server_id, ip = %ip, "existing_server_running");
                        println!("running");
                        let username = existing.username.clone();
                        let sess = SshSession::new(&username, &ip);
                        ssh::wait_for_ssh(&sess)?;
                        reconcile_runtime_config(
                            &sess,
                            existing.applied_runtime.as_ref(),
                            &current_runtime,
                        );
                        warn_for_provisioning_changes(
                            existing.applied_provisioning.as_ref(),
                            &current_provisioning,
                        );
                        let hostname = existing.hostname_or_default(&project.name);
                        state::save_state(
                            &project.id,
                            &state::ProjectState::new(
                                existing.server_id,
                                ip,
                                username.clone(),
                                hostname.clone(),
                                existing.snapshot_id,
                                Some(current_runtime.clone()),
                                Some(current_provisioning.clone()),
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
    let user_data = cloud_init::build_cloud_init(
        &username,
        ssh_pubkey.trim(),
        &tailscale_auth_key,
        &current_provisioning.toolchains,
        &current_runtime.packages,
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
        Some(current_runtime.clone()),
        Some(current_provisioning.clone()),
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
        Some(current_runtime.clone()),
        Some(current_provisioning.clone()),
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

fn current_runtime_config(
    cfg: &config::Config,
    project_config: &project_config::ProjectConfig,
) -> state::AppliedRuntimeConfig {
    let project_specs: Vec<PackageSpec> = project_config
        .packages
        .iter()
        .map(|n| PackageSpec::Apt { name: n.clone() })
        .chain(
            project_config
                .cargo_packages
                .iter()
                .map(|n| PackageSpec::CargoBinstall { name: n.clone() }),
        )
        .collect();
    state::AppliedRuntimeConfig {
        packages: merge_package_specs(&cfg.vm_packages, &project_specs),
        serve_ports: project_config.serve_ports.clone(),
        ..Default::default()
    }
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

fn current_provisioning_config(
    project: &project::Project,
    cfg: &config::Config,
    project_config: &project_config::ProjectConfig,
) -> state::AppliedProvisioningConfig {
    let toolchains = state::Toolchain::all()
        .iter()
        .filter(|t| t.detect(&project.root))
        .cloned()
        .collect();
    state::AppliedProvisioningConfig {
        server_type: project_config.server_type.clone(),
        toolchains,
        dotfiles_repo: cfg.dotfiles_repo.clone(),
        dotfiles_install: cfg.dotfiles_install.clone(),
        ..Default::default()
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
    if previous.toolchains != current.toolchains {
        changed.push(format!(
            "toolchains: {:?} -> {:?}",
            previous.toolchains, current.toolchains
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
    added: Vec<PackageSpec>,
    removed: Vec<PackageSpec>,
}

fn runtime_config_delta(
    previous: Option<&state::AppliedRuntimeConfig>,
    current: &state::AppliedRuntimeConfig,
) -> RuntimeConfigDelta {
    let previous_packages = previous.map(|c| c.packages.as_slice()).unwrap_or(&[]);

    let removed = previous_packages
        .iter()
        .filter(|p| !current.packages.iter().any(|c| c.name() == p.name()))
        .cloned()
        .collect();
    let added = current
        .packages
        .iter()
        .filter(|p| !previous_packages.iter().any(|prev| prev.name() == p.name()))
        .cloned()
        .collect();

    RuntimeConfigDelta { added, removed }
}

fn reconcile_runtime_config(
    sess: &SshSession,
    previous: Option<&state::AppliedRuntimeConfig>,
    current: &state::AppliedRuntimeConfig,
) {
    reconcile_runtime_config_with(sess.username(), previous, current, |remote_cmd| {
        ssh::run_remote_status(sess, remote_cmd)
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

    let mut non_reconcilable: Vec<&PackageSpec> = Vec::new();
    for spec in &delta.added {
        match spec.runtime_install_cmd(username) {
            Some(cmd) => {
                println!("Installing newly configured package '{}'...", spec.name());
                if !run_remote(&cmd) {
                    eprintln!("Warning: failed to install package '{}'", spec.name());
                }
            }
            None => non_reconcilable.push(spec),
        }
    }
    if !non_reconcilable.is_empty() {
        eprintln!(
            "Warning: the following packages require `gob up --reset` to install (cargo-binstall): {}",
            non_reconcilable
                .iter()
                .map(|s| s.name())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    if !delta.removed.is_empty() {
        eprintln!(
            "Warning: removed packages are not auto-uninstalled: {}",
            delta
                .removed
                .iter()
                .map(|s| s.name())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    reconcile_tailscale_serve_with(&current.serve_ports, &mut run_remote);
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
    let current_runtime = current_runtime_config(cfg, project_config);
    let current_provisioning = current_provisioning_config(project, cfg, project_config);

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

    // Save state immediately so Ctrl-C doesn't orphan the server
    let project_state = state::ProjectState::new(
        server_id,
        initial_ip,
        username.clone(),
        server_name.clone(),
        None,
        Some(current_runtime.clone()),
        Some(current_provisioning.clone()),
    );
    state::save_state(&project.id, &project_state)?;

    let ip = client.wait_for_server(server_id)?;
    println!("  Server running at {}", ip);
    info!(server_id = server_id, ip = %ip, "snapshot_server_running");

    // Update state with final IP
    let project_state = state::ProjectState::new(
        server_id,
        ip.clone(),
        username.clone(),
        server_name.clone(),
        None,
        Some(current_runtime.clone()),
        Some(current_provisioning.clone()),
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
    reconcile_runtime_config(&sess, existing.applied_runtime.as_ref(), &current_runtime);
    warn_for_provisioning_changes(
        existing.applied_provisioning.as_ref(),
        &current_provisioning,
    );

    // Push project and re-copy SSH key (repo and origin are in the snapshot)
    push_to_vm(&project.root, &sess, &server_name, &project.name)?;
    setup_vm_ssh_key(&sess);

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
    use std::path::PathBuf;

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
            vm_packages: vec![
                PackageSpec::Apt {
                    name: "jq".to_string(),
                },
                PackageSpec::Apt {
                    name: "ripgrep".to_string(),
                },
                PackageSpec::CurlInstaller {
                    name: "claude-code".to_string(),
                    url: "https://claude.ai/install.sh".to_string(),
                },
            ],
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
            packages: vec![],
            cargo_packages: vec![],
        };
        let runtime = current_runtime_config(&cfg, &project_cfg);
        assert_eq!(
            runtime.packages,
            vec![
                PackageSpec::Apt {
                    name: "jq".to_string()
                },
                PackageSpec::Apt {
                    name: "ripgrep".to_string()
                },
                PackageSpec::CurlInstaller {
                    name: "claude-code".to_string(),
                    url: "https://claude.ai/install.sh".to_string(),
                },
            ]
        );
        assert_eq!(runtime.serve_ports, vec![3000, 8080]);
    }

    #[test]
    fn current_runtime_config_merges_project_packages() {
        let cfg = test_config(); // vm_packages = [Apt("jq"), Apt("ripgrep"), CurlInstaller("claude-code")]
        let project_cfg = project_config::ProjectConfig {
            serve_ports: vec![],
            server_type: "cx23".to_string(),
            packages: vec!["nodejs".to_string(), "jq".to_string()], // jq is a duplicate
            cargo_packages: vec![],
        };
        let runtime = current_runtime_config(&cfg, &project_cfg);
        // jq appears in both lists but must not be duplicated
        assert_eq!(
            runtime.packages,
            vec![
                PackageSpec::Apt {
                    name: "jq".to_string()
                },
                PackageSpec::Apt {
                    name: "ripgrep".to_string()
                },
                PackageSpec::CurlInstaller {
                    name: "claude-code".to_string(),
                    url: "https://claude.ai/install.sh".to_string(),
                },
                PackageSpec::Apt {
                    name: "nodejs".to_string()
                },
            ]
        );
    }

    #[test]
    fn current_provisioning_config_detects_rust_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let cfg = test_config();
        let project_cfg = project_config::ProjectConfig {
            serve_ports: vec![],
            server_type: "cx42".to_string(),
            packages: vec![],
            cargo_packages: vec![],
        };
        let provisioning = current_provisioning_config(
            &test_project(dir.path().to_path_buf()),
            &cfg,
            &project_cfg,
        );
        assert_eq!(provisioning.server_type, "cx42");
        assert!(provisioning.toolchains.contains(&state::Toolchain::Rust));
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
    fn current_provisioning_config_detects_python_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pyproject.toml"), "[project]\nname=\"x\"\n").unwrap();
        let cfg = test_config();
        let project_cfg = project_config::ProjectConfig::default();
        let provisioning = current_provisioning_config(
            &test_project(dir.path().to_path_buf()),
            &cfg,
            &project_cfg,
        );
        assert!(provisioning.toolchains.contains(&state::Toolchain::Python));
        assert!(!provisioning.toolchains.contains(&state::Toolchain::Rust));
    }

    #[test]
    fn provisioning_change_messages_reports_all_differences() {
        let previous = state::AppliedProvisioningConfig {
            server_type: "cx23".to_string(),
            toolchains: vec![],
            dotfiles_repo: None,
            dotfiles_install: None,
            ..Default::default()
        };
        let current = state::AppliedProvisioningConfig {
            server_type: "cx42".to_string(),
            toolchains: vec![state::Toolchain::Rust],
            dotfiles_repo: Some("git@example.com:dotfiles.git".to_string()),
            dotfiles_install: Some("./install.sh".to_string()),
            ..Default::default()
        };
        let changes = provisioning_change_messages(Some(&previous), &current);
        assert_eq!(changes.len(), 4);
        assert!(changes.iter().any(|c| c.contains("server_type")));
        assert!(changes.iter().any(|c| c.contains("toolchains")));
        assert!(changes.iter().any(|c| c.contains("dotfiles.repo")));
        assert!(changes.iter().any(|c| c.contains("dotfiles.install")));
        assert!(provisioning_change_messages(None, &current).is_empty());
    }

    #[test]
    fn runtime_config_delta_tracks_added_and_removed_entries() {
        let previous = state::AppliedRuntimeConfig {
            packages: vec![
                PackageSpec::Apt {
                    name: "git".to_string(),
                },
                PackageSpec::Apt {
                    name: "jq".to_string(),
                },
                PackageSpec::CurlInstaller {
                    name: "claude-code".to_string(),
                    url: "https://claude.ai/install.sh".to_string(),
                },
            ],
            serve_ports: vec![3000],
            ..Default::default()
        };
        let current = state::AppliedRuntimeConfig {
            packages: vec![
                PackageSpec::Apt {
                    name: "jq".to_string(),
                },
                PackageSpec::Apt {
                    name: "ripgrep".to_string(),
                },
                PackageSpec::CurlInstaller {
                    name: "opencode".to_string(),
                    url: "https://opencode.ai/install".to_string(),
                },
            ],
            serve_ports: vec![8080],
            ..Default::default()
        };
        let delta = runtime_config_delta(Some(&previous), &current);
        assert_eq!(delta.added.len(), 2);
        assert!(delta.added.iter().any(|s| s.name() == "ripgrep"));
        assert!(delta.added.iter().any(|s| s.name() == "opencode"));
        assert_eq!(delta.removed.len(), 2);
        assert!(delta.removed.iter().any(|s| s.name() == "git"));
        assert!(delta.removed.iter().any(|s| s.name() == "claude-code"));
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
            packages: vec![PackageSpec::Apt {
                name: "git".to_string(),
            }],
            serve_ports: vec![1234],
            ..Default::default()
        };
        let current = state::AppliedRuntimeConfig {
            packages: vec![
                PackageSpec::Apt {
                    name: "git".to_string(),
                },
                PackageSpec::Apt {
                    name: "jq".to_string(),
                },
                PackageSpec::CurlInstaller {
                    name: "claude-code".to_string(),
                    url: "https://claude.ai/install.sh".to_string(),
                },
            ],
            serve_ports: vec![3000],
            ..Default::default()
        };
        let calls = RefCell::new(Vec::<String>::new());
        reconcile_runtime_config_with("alice", Some(&previous), &current, |cmd| {
            calls.borrow_mut().push(cmd.to_string());
            true
        });
        let calls = calls.into_inner();
        assert!(calls[0].contains("apt-get install -y jq"));
        assert!(calls[1].contains("claude.ai/install.sh"));
        assert_eq!(calls[2], "sudo tailscale serve reset");
        assert_eq!(calls[3], "sudo tailscale serve --bg 3000");
        assert_eq!(calls.len(), 4);
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
}
