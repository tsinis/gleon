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

    // Initialize tracing subscriber for logging, directing log output to stderr
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(log_level)
        .init();

    info!("Gleon CLI starting up...");
    match cli.command {
        Commands::Status => {
            let current_dir =
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let branch = match gleon_core::git::GitResolver::resolve_branch_impl(
                cli.branch.as_deref(),
                &current_dir,
                &gleon_core::git::OsEnv,
            ) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("Error resolving branch: {}", e);
                    std::process::exit(1);
                }
            };

            match gleon_core::context::ResolvedContext::from_cli(&cli, &current_dir) {
                Ok(ctx) => {
                    let info = ctx.platform;
                    info!("Platform resolved successfully");
                    let key = info
                        .to_key()
                        .expect("PlatformInfo is guaranteed to produce a key");
                    println!("Branch: {}", branch);
                    println!("Key: {}", key);
                    println!("OS: {}", info.os);
                    if let Some(ref arch) = info.arch {
                        println!("Architecture: {}", arch);
                    }
                    if let Some(ref r) = info.renderer {
                        println!("Renderer: {}", r);
                    }
                    if !info.labels.is_empty() {
                        println!("Labels:");
                        for (k, v) in info.labels {
                            println!("  {} = {}", k, v);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error resolving platform: {}", e);
                    std::process::exit(1);
                }
            }
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
