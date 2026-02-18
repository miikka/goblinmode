use anyhow::Result;

use crate::config;
use crate::hetzner::HetznerClient;
use crate::project;
use crate::state;

pub fn run() -> Result<()> {
    let project = project::detect_project()?;

    let existing = match state::load_state(&project.id)? {
        Some(s) => s,
        None => {
            println!("No VM for project '{}'.", project.name);
            return Ok(());
        }
    };

    let hostname = if existing.hostname.is_empty() {
        format!("gob-{}", project.name)
    } else {
        existing.hostname
    };

    // Check actual server status via Hetzner API
    let cfg = config::load_config()?;
    let hetzner = HetznerClient::new(cfg.hetzner_api_token);

    match hetzner.get_server_status(existing.server_id)? {
        Some((status, ip)) => {
            println!("Project:  {}", project.name);
            println!("Status:   {}", status);
            println!("Hostname: {}", hostname);
            println!("IP:       {}", ip);
            println!("User:     {}", existing.username);
        }
        None => {
            println!(
                "VM for project '{}' no longer exists (stale state).",
                project.name
            );
            println!("Run `gob down` to clean up local state.");
        }
    }

    Ok(())
}
