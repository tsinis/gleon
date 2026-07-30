//! gleon CLI wrapper binary.

use clap::Parser;
use gleon_core::cli::{Cli, Commands};
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    let exit_code = run(&cli, &current_dir).await?;
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

#[allow(dead_code)]
fn get_storage_config() -> Option<gleon_core::storage::StorageConfig> {
    let url = std::env::var("GLEON_STORAGE_URL").ok()?;
    if url.is_empty() {
        return None;
    }
    let mut storage_cfg = gleon_core::storage::StorageConfig::new(url);

    // Read standard AWS vars
    storage_cfg.aws_access_key_id = std::env::var("AWS_ACCESS_KEY_ID").ok();
    storage_cfg.aws_secret_access_key = std::env::var("AWS_SECRET_ACCESS_KEY").ok();
    storage_cfg.aws_region = std::env::var("AWS_REGION").ok();
    storage_cfg.aws_endpoint = std::env::var("AWS_ENDPOINT_URL").ok();
    storage_cfg.r2_account_id = std::env::var("R2_ACCOUNT_ID").ok();

    // Allow GLEON_ overrides
    if let Ok(v) = std::env::var("GLEON_AWS_ACCESS_KEY_ID") {
        storage_cfg.aws_access_key_id = Some(v);
    }
    if let Ok(v) = std::env::var("GLEON_AWS_SECRET_ACCESS_KEY") {
        storage_cfg.aws_secret_access_key = Some(v);
    }
    if let Ok(v) = std::env::var("GLEON_AWS_REGION") {
        storage_cfg.aws_region = Some(v);
    }
    if let Ok(v) = std::env::var("GLEON_AWS_ENDPOINT_URL") {
        storage_cfg.aws_endpoint = Some(v);
    }
    if let Ok(v) = std::env::var("GLEON_R2_ACCOUNT_ID") {
        storage_cfg.r2_account_id = Some(v);
    }

    if let Some(c) = std::env::var("GLEON_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        storage_cfg.concurrency = c;
    }

    Some(storage_cfg)
}

mod commands;

async fn run(cli: &Cli, current_dir: &std::path::Path) -> anyhow::Result<i32> {
    match &cli.command {
        Commands::Init => {
            let ctx = gleon_core::context::ResolvedContext::from_cli(cli, current_dir)
                .map_err(|e| anyhow::anyhow!(e))?;
            let res = gleon_core::ops::init_workspace(&ctx, &ctx.base_dir)
                .map_err(|e| anyhow::anyhow!(e))?;
            info!("Initialized gleon workspace at {}", res.gleon_dir.display());
            if let Some(ref config_path) = res.config_created {
                info!(
                    "Created default configuration file at {}",
                    config_path.display()
                );
            }
        }
        Commands::Status { json } => {
            let ctx = gleon_core::context::ResolvedContext::from_cli(cli, current_dir)
                .map_err(|e| anyhow::anyhow!(e))?;
            let report = gleon_core::ops::check_status(&ctx, &ctx.base_dir)
                .map_err(|e| anyhow::anyhow!(e))?;
            if *json {
                println!("{}", report.format_json().map_err(|e| anyhow::anyhow!(e))?);
            } else {
                print!("{}", report.format_text());
            }
        }
        Commands::Stage { paths } => {
            let ctx = gleon_core::context::ResolvedContext::from_cli(cli, current_dir)
                .map_err(|e| anyhow::anyhow!(e))?;
            let filter = if paths.is_empty() {
                None
            } else {
                Some(paths.as_slice())
            };
            let res = gleon_core::ops::stage_workspace(&ctx, &ctx.base_dir, filter)
                .map_err(|e| anyhow::anyhow!(e))?;
            if res.total_screenshots_staged == 0 {
                info!("Already up to date.");
            } else {
                info!(
                    "Staged {} screenshot(s) across {} test case(s).",
                    res.total_screenshots_staged,
                    res.staged_test_cases.len()
                );
            }
        }
        Commands::Diff {
            auto_pull: _,
            resolve,
        } => {
            let ctx = gleon_core::context::ResolvedContext::from_cli(cli, current_dir)
                .map_err(|e| anyhow::anyhow!(e))?;

            if *resolve {
                let storage_cfg = get_storage_config();
                return commands::resolve::run_resolve(&ctx, None, false, storage_cfg).await;
            }

            let report =
                gleon_core::ops::run_diff(&ctx, &ctx.base_dir).map_err(|e| anyhow::anyhow!(e))?;
            info!(
                "Ran {} test(s). Passed: {}, Failed: {}.",
                report.total_tests,
                report.total_tests.saturating_sub(report.failed_tests),
                report.failed_tests
            );
            info!("Report generated at {}", report.runs_dir.display());
            if !report.passed {
                return Ok(1);
            }
        }
        Commands::LintManifests { platform } => {
            let ctx = gleon_core::context::ResolvedContext::from_cli(cli, current_dir)
                .map_err(|e| anyhow::anyhow!(e))?;
            return commands::lint::run_lint(&ctx, platform.as_deref());
        }
        Commands::Resolve { test_path, fetch } => {
            let ctx = gleon_core::context::ResolvedContext::from_cli(cli, current_dir)
                .map_err(|e| anyhow::anyhow!(e))?;
            let storage_cfg = get_storage_config();
            return commands::resolve::run_resolve(&ctx, test_path.as_deref(), *fetch, storage_cfg)
                .await;
        }
        Commands::Test => {
            info!("Subcommand test is not fully implemented yet");
        }
        Commands::Pull => {
            info!("Blob pull will be updated in Phase 3.5.");
        }
        Commands::Push => {
            info!("Blob push will be updated in Phase 3.5.");
        }
        Commands::Merge { target_branch } => {
            info!(
                "Subcommand merge for branch '{}' is not fully implemented yet",
                target_branch
            );
        }
        Commands::Gc => {
            info!("Subcommand gc is not fully implemented yet");
        }
    }
    Ok(0)
}
