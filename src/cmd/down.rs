use anyhow::{bail, Result};
use std::process::Command;

use crate::config;
use crate::hetzner::HetznerClient;
use crate::project;
use crate::state;
use crate::tailscale::TailscaleClient;

pub fn run() -> Result<()> {
    // 1. Detect project root
    let project = project::detect_project()?;
    println!("Project: {} ({})", project.name, project.root.display());

    // 2. Load existing state
    let existing = match state::load_state(&project.id)? {
        Some(s) => s,
        None => bail!("No server found for this project. Nothing to do."),
    };

    // 3. Load config
    let cfg = config::load_config()?;

    teardown(&project, &existing, &cfg)
}

/// Destroy all resources for a project. Does not bail if the server is already gone.
pub fn teardown(
    project: &project::Project,
    existing: &state::ProjectState,
    cfg: &config::Config,
) -> Result<()> {
    let hostname = if existing.hostname.is_empty() {
        format!("gob-{}", project.name)
    } else {
        existing.hostname.clone()
    };

    // Remove from Tailscale
    let ts_client = TailscaleClient::new(cfg.tailscale_api_key.clone());
    if let Err(e) = ts_client.delete_device_by_hostname(&hostname) {
        eprintln!("Warning: failed to remove Tailscale device: {}", e);
    }

    // Delete Hetzner server
    let hetzner_client = HetznerClient::new(cfg.hetzner_api_token.clone());
    if existing.server_id != 0 {
        println!(
            "Deleting server {} (id: {})...",
            existing.ipv4, existing.server_id
        );
        match hetzner_client.delete_server(existing.server_id) {
            Ok(()) => println!("Server deleted."),
            Err(e) => {
                eprintln!("Warning: failed to delete server: {}", e);
                eprintln!("Clearing local state anyway.");
            }
        }
    }

    // Delete snapshot if present
    if let Some(snapshot_id) = existing.snapshot_id {
        print!("Deleting snapshot (image: {})... ", snapshot_id);
        match hetzner_client.delete_image(snapshot_id) {
            Ok(()) => println!("done"),
            Err(e) => eprintln!("Warning: failed to delete snapshot: {}", e),
        }
    }

    // Remove from known_hosts
    for host in [&hostname, &existing.ipv4] {
        let _ = Command::new("ssh-keygen").args(["-R", host]).output();
    }

    // Remove git remote
    let _ = Command::new("git")
        .args(["remote", "remove", "gob"])
        .current_dir(&project.root)
        .output();

    // Remove state
    state::delete_state(&project.id)?;
    println!("Done.");
    Ok(())
}
