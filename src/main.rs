mod cmd;
mod config;
mod hetzner;
mod project;
mod state;

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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Up => cmd::up::run(),
    }
}
