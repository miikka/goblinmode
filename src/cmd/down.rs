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

    // 4. Remove from Tailscale
    let hostname = if existing.hostname.is_empty() {
        format!("gob-{}", project.name)
    } else {
        existing.hostname
    };
    let ts_client = TailscaleClient::new(cfg.tailscale_api_key);
    if let Err(e) = ts_client.delete_device_by_hostname(&hostname) {
        eprintln!("Warning: failed to remove Tailscale device: {}", e);
    }

    // 5. Delete Hetzner server
    let hetzner_client = HetznerClient::new(cfg.hetzner_api_token);
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

    // 6. Remove from known_hosts
    for host in [&hostname, &existing.ipv4] {
        let _ = Command::new("ssh-keygen").args(["-R", host]).output();
    }

    // 7. Remove git remote
    let _ = Command::new("git")
        .args(["remote", "remove", "gob"])
        .current_dir(&project.root)
        .output();

    // 8. Remove state
    state::delete_state(&project.id)?;
    println!("Done.");
    Ok(())
}
