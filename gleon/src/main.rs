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

fn create_spinner(
    msg: &str,
    concurrency: usize,
) -> (
    indicatif::ProgressBar,
    gleon_core::storage::sync::SyncOptions,
) {
    let spinner = indicatif::ProgressBar::new_spinner();
    spinner.set_style(
        indicatif::ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .expect("Valid spinner template"),
    );
    spinner.set_message(msg.to_string());
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    let mut options = gleon_core::storage::sync::SyncOptions {
        concurrency,
        ..Default::default()
    };
    let sp = spinner.clone();
    options.on_progress = Some(std::sync::Arc::new(move || {
        sp.tick();
    }));

    (spinner, options)
}

async fn run(cli: &Cli, current_dir: &std::path::Path) -> anyhow::Result<i32> {
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
        Commands::Diff { auto_pull } => {
            let ctx = gleon_core::context::ResolvedContext::from_cli(cli, current_dir)?;

            if *auto_pull {
                if let Some(storage_cfg) = get_storage_config() {
                    let adapter = std::sync::Arc::new(
                        gleon_core::storage::adapter::ObjectStoreAdapter::from_config(
                            &storage_cfg,
                        )?,
                    );
                    let concurrency = adapter.concurrency();
                    let orchestrator = gleon_core::storage::sync::SyncOrchestrator::new(
                        adapter,
                        ctx.base_dir.clone(),
                    );

                    let platform_key = ctx.platform.to_key()?;
                    let (spinner, options) =
                        create_spinner("Auto-pulling latest baselines...", concurrency);
                    orchestrator
                        .pull(&ctx.branch, &platform_key, &options)
                        .await?;
                    spinner.finish_with_message("Auto-pull complete.");
                } else {
                    println!("No storage configured via GLEON_STORAGE_URL. Skipping auto-pull.");
                }
            }

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
            let ctx = gleon_core::context::ResolvedContext::from_cli(cli, current_dir)?;
            if let Some(storage_cfg) = get_storage_config() {
                let adapter = std::sync::Arc::new(
                    gleon_core::storage::adapter::ObjectStoreAdapter::from_config(&storage_cfg)?,
                );
                let concurrency = adapter.concurrency();
                let orchestrator =
                    gleon_core::storage::sync::SyncOrchestrator::new(adapter, ctx.base_dir.clone());

                let platform_key = ctx.platform.to_key()?;
                let (spinner, options) = create_spinner("Pulling latest baselines...", concurrency);
                orchestrator
                    .pull(&ctx.branch, &platform_key, &options)
                    .await?;
                spinner.finish_with_message("Pull complete.");
            } else {
                println!("No storage configured via GLEON_STORAGE_URL. Nothing to pull.");
            }
        }
        Commands::Push => {
            let ctx = gleon_core::context::ResolvedContext::from_cli(cli, current_dir)?;
            if let Some(storage_cfg) = get_storage_config() {
                let adapter = std::sync::Arc::new(
                    gleon_core::storage::adapter::ObjectStoreAdapter::from_config(&storage_cfg)?,
                );
                let concurrency = adapter.concurrency();
                let orchestrator =
                    gleon_core::storage::sync::SyncOrchestrator::new(adapter, ctx.base_dir.clone());

                let platform_key = ctx.platform.to_key()?;
                let (spinner, options) = create_spinner("Pushing baselines...", concurrency);
                orchestrator
                    .push(&ctx.branch, &platform_key, &options)
                    .await?;
                spinner.finish_with_message("Push complete.");
            } else {
                println!("No storage configured via GLEON_STORAGE_URL. Nothing to push.");
            }
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
