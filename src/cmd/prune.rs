use anyhow::Result;
use std::io::{self, Write};

use crate::config;
use crate::hetzner::HetznerClient;
use crate::tailscale::TailscaleClient;

pub fn run() -> Result<()> {
    let cfg = config::load_config()?;
    let hetzner = HetznerClient::new(cfg.hetzner_api_token);
    let tailscale = TailscaleClient::new(cfg.tailscale_api_key);

    let servers = hetzner.list_goblinmode_servers()?;
    let snapshots = hetzner.list_goblinmode_snapshots()?;
    let devices = tailscale.list_gob_devices()?;

    if servers.is_empty() && snapshots.is_empty() && devices.is_empty() {
        println!("No goblinmode resources found.");
        return Ok(());
    }

    if !servers.is_empty() {
        println!("Found {} server(s):", servers.len());
        for s in &servers {
            println!(
                "  {} (id: {}, status: {}, ip: {})",
                s.name, s.id, s.status, s.ipv4
            );
        }
    }

    if !snapshots.is_empty() {
        println!("Found {} snapshot(s):", snapshots.len());
        for s in &snapshots {
            println!("  {} (id: {}, created: {})", s.description, s.id, s.created);
        }
    }

    if !devices.is_empty() {
        println!("Found {} Tailscale device(s):", devices.len());
        for d in &devices {
            println!("  {}", d.hostname);
        }
    }

    print!("\nDelete all of them? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;

    if !answer.trim().eq_ignore_ascii_case("y") {
        println!("Aborted.");
        return Ok(());
    }

    for s in &servers {
        print!("Deleting server {} (id: {})... ", s.name, s.id);
        io::stdout().flush()?;
        match hetzner.delete_server(s.id) {
            Ok(()) => println!("done"),
            Err(e) => eprintln!("failed: {}", e),
        }
    }

    for s in &snapshots {
        print!("Deleting snapshot {} (id: {})... ", s.description, s.id);
        io::stdout().flush()?;
        match hetzner.delete_image(s.id) {
            Ok(()) => println!("done"),
            Err(e) => eprintln!("failed: {}", e),
        }
    }

    for d in &devices {
        print!("Deleting Tailscale device {}... ", d.hostname);
        io::stdout().flush()?;
        match tailscale.delete_device_by_id(&d.id) {
            Ok(()) => println!("done"),
            Err(e) => eprintln!("failed: {}", e),
        }
    }

    println!("Prune complete.");
    Ok(())
}
