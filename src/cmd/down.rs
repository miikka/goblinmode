use anyhow::{bail, Result};

use crate::config;
use crate::hetzner::HetznerClient;
use crate::project;
use crate::state;

pub fn run() -> Result<()> {
    // 1. Detect project root
    let project = project::detect_project()?;
    println!("Project: {} ({})", project.name, project.root.display());

    // 2. Load existing state
    let existing = match state::load_state(&project.id)? {
        Some(s) => s,
        None => bail!("No server found for this project. Nothing to do."),
    };

    // 3. Load config and delete the server
    let cfg = config::load_config()?;
    let client = HetznerClient::new(cfg.hetzner_api_token);

    println!(
        "Deleting server {} (id: {})...",
        existing.ipv4, existing.server_id
    );
    match client.delete_server(existing.server_id) {
        Ok(()) => println!("Server deleted."),
        Err(e) => {
            eprintln!("Warning: failed to delete server: {}", e);
            eprintln!("Clearing local state anyway.");
        }
    }

    // 4. Remove state
    state::delete_state(&project.id)?;
    println!("Done.");
    Ok(())
}
