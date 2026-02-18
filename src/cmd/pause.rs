use anyhow::{bail, Result};
use std::io::{self, Write};
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

    if existing.server_id == 0 {
        bail!("No running server for this project. Nothing to pause.");
    }

    // 3. Load config
    let cfg = config::load_config()?;
    let hetzner = HetznerClient::new(cfg.hetzner_api_token);

    // 4. Shutdown server
    print!("Shutting down server (id: {})... ", existing.server_id);
    io::stdout().flush()?;
    hetzner.shutdown_server(existing.server_id)?;
    hetzner.wait_for_server_off(existing.server_id)?;
    println!("off");

    // 5. Create snapshot
    let description = format!("gob-pause-{}", project.name);
    print!("Creating snapshot... ");
    io::stdout().flush()?;
    let image_id = hetzner.create_image(existing.server_id, &description)?;
    hetzner.wait_for_image(image_id)?;
    println!("done (image: {})", image_id);

    // 6. Delete server
    print!("Deleting server... ");
    io::stdout().flush()?;
    hetzner.delete_server(existing.server_id)?;
    println!("done");

    // 7. Clean up Tailscale device
    let hostname = if existing.hostname.is_empty() {
        format!("gob-{}", project.name)
    } else {
        existing.hostname.clone()
    };
    let ts_client = TailscaleClient::new(cfg.tailscale_api_key);
    if let Err(e) = ts_client.delete_device_by_hostname(&hostname) {
        eprintln!("Warning: failed to remove Tailscale device: {}", e);
    }

    // 8. Remove from known_hosts
    for host in [&hostname, &existing.ipv4] {
        let _ = Command::new("ssh-keygen").args(["-R", host]).output();
    }

    // 9. Update state: clear server info, store snapshot_id, keep hostname
    let paused_state = state::ProjectState {
        server_id: 0,
        ipv4: String::new(),
        username: existing.username,
        hostname: existing.hostname,
        snapshot_id: Some(image_id),
    };
    state::save_state(&project.id, &paused_state)?;

    println!("Server paused. Run `gob up` to restore from snapshot.");
    Ok(())
}
