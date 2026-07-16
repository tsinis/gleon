//! Gleon CLI wrapper binary.

use clap::Parser;
use gleon_core::cli::{Cli, Commands};
use tracing::info;

fn main() {
    let cli = Cli::parse();

    // Determine the log level based on CLI flags
    let log_level = if cli.quiet {
        tracing::Level::WARN
    } else if cli.verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    // Initialize tracing subscriber for logging
    tracing_subscriber::fmt().with_max_level(log_level).init();

    info!("Gleon CLI starting up...");
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
        Commands::Merge { target_branch } => {
            println!(
                "Subcommand merge for branch '{}' is not fully implemented yet",
                target_branch
            );
        }
        Commands::Gc => {
            println!("Subcommand gc is not fully implemented yet");
        }
    }
}
