mod cmd;
mod config;
mod hetzner;
mod project;
mod project_config;
mod state;
mod tailscale;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "gob", about = "Goblin Mode dev environments")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a development VM for the current project
    Up,
    /// Destroy the development VM for the current project
    Down,
    /// Connect to the development VM with mosh
    Mosh,
    /// Open the remote project in Zed
    Zed,
    /// Snapshot the VM and destroy it (resume with `gob up`)
    Pause,
    /// Show the status of the development VM
    #[command(alias = "ps")]
    Status,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Up => cmd::up::run(),
        Commands::Down => cmd::down::run(),
        Commands::Pause => cmd::pause::run(),
        Commands::Mosh => cmd::mosh::run(),
        Commands::Zed => cmd::zed::run(),
        Commands::Status => cmd::status::run(),
    }
}
