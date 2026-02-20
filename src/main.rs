mod cmd;
mod config;
mod hetzner;
mod project;
mod project_config;
mod state;
mod tailscale;
mod tracing_setup;

use clap::{Parser, Subcommand};
use tracing::{error, info};

#[derive(Parser)]
#[command(name = "gob", about = "Goblin Mode dev environments")]
struct Cli {
    /// Write a structured trace file (JSONL) for debugging.
    /// Optionally pass a file path.
    #[arg(long, global = true, value_name = "PATH", num_args = 0..=1, default_missing_value = "")]
    trace: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Clone)]
enum Commands {
    /// Start a development VM for the current project
    Up {
        /// Destroy the existing VM and recreate it from scratch
        #[arg(long)]
        reset: bool,
    },
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
    /// Delete all goblinmode servers on Hetzner
    Prune,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let (trace_path, _trace_guard) = tracing_setup::init(cli.trace.as_deref())?;

    if let Some(path) = trace_path.as_ref() {
        eprintln!("Trace enabled: {}", path.display());
    }

    info!(
        command = command_name(&cli.command),
        trace_enabled = trace_path.is_some(),
        "command_start"
    );

    let result = match cli.command {
        Commands::Up { reset } => cmd::up::run(reset),
        Commands::Down => cmd::down::run(),
        Commands::Pause => cmd::pause::run(),
        Commands::Mosh => cmd::mosh::run(),
        Commands::Zed => cmd::zed::run(),
        Commands::Status => cmd::status::run(),
        Commands::Prune => cmd::prune::run(),
    };

    match &result {
        Ok(()) => info!(ok = true, "command_finish"),
        Err(err) => error!(ok = false, error = %err, "command_finish"),
    }

    result
}

fn command_name(command: &Commands) -> &'static str {
    match command {
        Commands::Up { .. } => "up",
        Commands::Down => "down",
        Commands::Pause => "pause",
        Commands::Mosh => "mosh",
        Commands::Zed => "zed",
        Commands::Status => "status",
        Commands::Prune => "prune",
    }
}
