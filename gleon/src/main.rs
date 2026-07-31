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

    // Load environment configuration from .gleon/.env and .gleon/.env.local
    let dotenv = gleon_core::env::load_dotenv(&current_dir);
    if !dotenv.is_empty() {
        tracing::debug!(
            "Loaded {} environment variable(s) from .env files",
            dotenv.len()
        );
    }
    let env = MergedEnv { dotenv };

    let exit_code = run(&cli, &current_dir, &env).await?;
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

/// Merges .env file values with the OS process environment.
/// Process env always wins — dotenv values are fallback defaults.
struct MergedEnv {
    dotenv: std::collections::HashMap<String, String>,
}

impl gleon_core::git::EnvProvider for MergedEnv {
    fn get_var(&self, key: &str) -> Option<String> {
        std::env::var(key)
            .ok()
            .or_else(|| self.dotenv.get(key).cloned())
    }
}

fn get_storage_config(
    env: &dyn gleon_core::git::EnvProvider,
) -> Option<gleon_core::storage::StorageConfig> {
    gleon_core::storage::StorageConfig::from_env(env)
}

mod commands;

async fn run(
    cli: &Cli,
    current_dir: &std::path::Path,
    env: &dyn gleon_core::git::EnvProvider,
) -> anyhow::Result<i32> {
    match &cli.command {
        Commands::Init => {
            let ctx =
                gleon_core::context::ResolvedContext::from_cli_with_env(cli, current_dir, env)
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
            let ctx =
                gleon_core::context::ResolvedContext::from_cli_with_env(cli, current_dir, env)
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
            let ctx =
                gleon_core::context::ResolvedContext::from_cli_with_env(cli, current_dir, env)
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
            let ctx =
                gleon_core::context::ResolvedContext::from_cli_with_env(cli, current_dir, env)
                    .map_err(|e| anyhow::anyhow!(e))?;

            if *resolve {
                let storage_cfg = get_storage_config(env);
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
            let ctx =
                gleon_core::context::ResolvedContext::from_cli_with_env(cli, current_dir, env)
                    .map_err(|e| anyhow::anyhow!(e))?;
            return commands::lint::run_lint(&ctx, platform.as_deref());
        }
        Commands::Resolve { test_path, fetch } => {
            let ctx =
                gleon_core::context::ResolvedContext::from_cli_with_env(cli, current_dir, env)
                    .map_err(|e| anyhow::anyhow!(e))?;
            let storage_cfg = get_storage_config(env);
            return commands::resolve::run_resolve(&ctx, test_path.as_deref(), *fetch, storage_cfg)
                .await;
        }
        Commands::Test => {
            info!("Subcommand test is not fully implemented yet");
        }
        Commands::Pull {
            all_platforms,
            platform,
        } => {
            let ctx =
                gleon_core::context::ResolvedContext::from_cli_with_env(cli, current_dir, env)
                    .map_err(|e| anyhow::anyhow!(e))?;
            let storage_cfg = get_storage_config(env);
            return commands::pull::run_pull(
                &ctx,
                storage_cfg.as_ref(),
                *all_platforms,
                platform.as_deref(),
            )
            .await;
        }
        Commands::Push {
            all_platforms,
            platform,
        } => {
            let ctx =
                gleon_core::context::ResolvedContext::from_cli_with_env(cli, current_dir, env)
                    .map_err(|e| anyhow::anyhow!(e))?;
            let storage_cfg = get_storage_config(env);
            return commands::push::run_push(
                &ctx,
                storage_cfg.as_ref(),
                *all_platforms,
                platform.as_deref(),
            )
            .await;
        }
        Commands::Gc => {
            info!("Subcommand gc is not fully implemented yet");
        }
    }
    Ok(0)
}
