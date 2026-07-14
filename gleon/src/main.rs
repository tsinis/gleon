//! Gleon CLI wrapper binary.

use clap::Parser;
use gleon_core::cli::{Cli, Commands};
use tracing::info;

fn main() {
    // Initialize tracing subscriber for logging
    tracing_subscriber::fmt::init();
    info!("Gleon CLI starting up...");

    let cli = Cli::parse();
    match cli.command {
        Commands::Status => {
            println!("Subcommand status is not fully implemented yet");
        }
        Commands::Stage => {
            println!("Subcommand stage is not fully implemented yet");
        }
        Commands::Diff => {
            println!("Subcommand diff is not fully implemented yet");
        }
        Commands::Test => {
            println!("Subcommand test is not fully implemented yet");
        }
        Commands::Pull => {
            println!("Subcommand pull is not fully implemented yet");
        }
        Commands::Push => {
            println!("Subcommand push is not fully implemented yet");
        }
        Commands::Merge { branch } => {
            println!(
                "Subcommand merge for branch '{}' is not fully implemented yet",
                branch
            );
        }
        Commands::Gc => {
            println!("Subcommand gc is not fully implemented yet");
        }
    }
}
