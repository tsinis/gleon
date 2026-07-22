//! gleon CLI wrapper binary.

use clap::Parser;
use gleon_core::cli::{Cli, Commands};
use tracing::info;

fn main() -> anyhow::Result<()> {
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

    info!("gleon CLI starting up...");

    let current_dir = std::env::current_dir()
        .map_err(|e| anyhow::anyhow!("Failed to determine current directory: {}", e))?;

    let exit_code = run(&cli, &current_dir)?;
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

fn run(cli: &Cli, current_dir: &std::path::Path) -> anyhow::Result<i32> {
    match &cli.command {
        Commands::Init => {
            let ctx = gleon_core::context::ResolvedContext::from_cli(cli, current_dir)?;
            let res = gleon_core::ops::init_workspace(&ctx, &ctx.base_dir)?;
            println!("Initialized gleon workspace at {}", res.gleon_dir.display());
            if let Some(ref config_path) = res.config_created {
                println!(
                    "Created default configuration file at {}",
                    config_path.display()
                );
            }
        }
        Commands::Status { json } => {
            let ctx = gleon_core::context::ResolvedContext::from_cli(cli, current_dir)?;
            let report = gleon_core::ops::check_status(&ctx, &ctx.base_dir)?;
            if *json {
                println!("{}", report.format_json()?);
            } else {
                print!("{}", report.format_text());
            }
        }
        Commands::Stage { paths } => {
            let ctx = gleon_core::context::ResolvedContext::from_cli(cli, current_dir)?;
            let filter = if paths.is_empty() {
                None
            } else {
                Some(paths.as_slice())
            };
            let res = gleon_core::ops::stage_workspace(&ctx, &ctx.base_dir, filter)?;
            if res.total_screenshots_staged == 0 {
                println!("Already up to date.");
            } else {
                println!(
                    "Staged {} screenshot(s) across {} test case(s).",
                    res.total_screenshots_staged,
                    res.staged_test_cases.len()
                );
            }
        }
        Commands::Diff => {
            let ctx = gleon_core::context::ResolvedContext::from_cli(cli, current_dir)?;
            let report = gleon_core::ops::run_diff(&ctx, &ctx.base_dir)?;
            println!(
                "Ran {} test(s). Passed: {}, Failed: {}.",
                report.total_tests,
                report.total_tests.saturating_sub(report.failed_tests),
                report.failed_tests
            );
            println!("Report generated at {}", report.runs_dir.display());
            if !report.passed {
                return Ok(1);
            }
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
    Ok(0)
}
