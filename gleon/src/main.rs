//! gleon CLI wrapper binary.

use clap::Parser;
use cli::{Cli, Commands};
use exit_code::ExitCode;
use tracing::info;

mod cli;
mod exit_code;

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
        .map_err(|e| anyhow::anyhow!("Failed to determine current directory: {e}"))?;

    // Load environment configuration from .gleon/.env and .gleon/.env.local
    let dotenv = gleon_core::env::load_dotenv(&current_dir);
    if !dotenv.is_empty() {
        tracing::debug!(
            "Loaded {} environment variable(s) from .env files",
            dotenv.len()
        );
    }
    let env = MergedEnv { dotenv };

    // Run License/Compliance Check
    let license_status = gleon_core::license::LicenseGate::verify(&env);
    let decision = gleon_core::license::enforce_policy(license_status, cli.strict, &env);
    for line in &decision.message {
        eprintln!("{line}");
    }
    if let Some(annotation) = &decision.gha_annotation {
        eprintln!("{annotation}");
    }
    if decision.action == gleon_core::license::EnforcementAction::Block {
        std::process::exit(42);
    }

    // Any error still bubbling here means a command couldn't even start (e.g. context
    // resolution failed) — every command that *did* start reports its own failures via
    // `commands::report_failure`, so this is the one remaining spot that needs to log+exit
    // consistently with those.
    let exit_code = match run(&cli, &current_dir, &env).await {
        Ok(code) => code,
        Err(e) => {
            tracing::error!("{e:#}");
            i32::from(ExitCode::Failure)
        }
    };
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

impl gleon_core::env::EnvProvider for MergedEnv {
    fn get_var(&self, key: &str) -> Option<String> {
        std::env::var(key)
            .ok()
            .or_else(|| self.dotenv.get(key).cloned())
    }
}

fn get_storage_config(
    env: &dyn gleon_core::env::EnvProvider,
) -> Option<gleon_core::storage::StorageConfig> {
    gleon_core::storage::StorageConfig::from_env(env)
}

mod commands;

/// Resolves the active [`gleon_core::context::ResolvedContext`] for the current invocation —
/// every subcommand needing workspace/platform/branch context builds it the same way, so this
/// is the one place that does.
fn resolve_context(
    cli: &Cli,
    current_dir: &std::path::Path,
    env: &dyn gleon_core::env::EnvProvider,
) -> anyhow::Result<gleon_core::context::ResolvedContext> {
    gleon_core::context::ResolvedContext::resolve(
        &gleon_core::context::ContextOptions::from(cli),
        current_dir,
        env,
    )
    .map_err(|e| anyhow::anyhow!(e))
}

/// Runs `gleon init`.
fn run_init(ctx: &gleon_core::context::ResolvedContext) -> anyhow::Result<()> {
    let res = gleon_core::ops::init_workspace(ctx).map_err(|e| anyhow::anyhow!(e))?;
    info!("Initialized gleon workspace at {}", res.gleon_dir.display());
    if let Some(ref config_path) = res.config_created {
        info!(
            "Created default configuration file at {}",
            config_path.display()
        );
    }
    Ok(())
}

/// Runs `gleon status`.
fn run_status(ctx: &gleon_core::context::ResolvedContext, json: bool) -> anyhow::Result<()> {
    let report = gleon_core::ops::check_status(ctx).map_err(|e| anyhow::anyhow!(e))?;
    if json {
        println!("{}", report.format_json().map_err(|e| anyhow::anyhow!(e))?);
    } else {
        print!("{}", report.format_text());
    }
    Ok(())
}

/// Runs `gleon stage`.
fn run_stage(
    ctx: &gleon_core::context::ResolvedContext,
    paths: &[std::path::PathBuf],
) -> anyhow::Result<()> {
    let filter = if paths.is_empty() { None } else { Some(paths) };
    let res = gleon_core::ops::stage_workspace(ctx, filter).map_err(|e| anyhow::anyhow!(e))?;
    if res.total_screenshots_staged == 0 {
        info!("Already up to date.");
    } else {
        info!(
            "Staged {} screenshot(s) across {} test case(s).",
            res.total_screenshots_staged,
            res.staged_test_cases.len()
        );
    }
    Ok(())
}

/// Runs `gleon diff`, or delegates to `gleon resolve` first if `--resolve` was passed.
async fn run_diff_command(
    ctx: &gleon_core::context::ResolvedContext,
    resolve: bool,
    storage_cfg: Option<gleon_core::storage::StorageConfig>,
) -> anyhow::Result<i32> {
    if resolve {
        let code = commands::resolve::run_resolve(ctx, None, false, storage_cfg).await;
        return Ok(code.into());
    }

    let report = gleon_core::ops::run_diff(ctx).map_err(|e| anyhow::anyhow!(e))?;
    info!(
        "Ran {} test(s). Passed: {}, Failed: {}.",
        report.total_tests,
        report.total_tests.saturating_sub(report.failed_tests),
        report.failed_tests
    );
    info!("Report generated at {}", report.runs_dir.display());
    let code = if report.passed {
        ExitCode::Success
    } else {
        ExitCode::Failure
    };
    Ok(code.into())
}

async fn run(
    cli: &Cli,
    current_dir: &std::path::Path,
    env: &dyn gleon_core::env::EnvProvider,
) -> anyhow::Result<i32> {
    match &cli.command {
        Commands::Init => run_init(&resolve_context(cli, current_dir, env)?)?,
        Commands::Status { json } => run_status(&resolve_context(cli, current_dir, env)?, *json)?,
        Commands::Stage { paths } => run_stage(&resolve_context(cli, current_dir, env)?, paths)?,
        Commands::Diff {
            auto_pull: _,
            resolve,
        } => {
            let ctx = resolve_context(cli, current_dir, env)?;
            let storage_cfg = get_storage_config(env);
            return run_diff_command(&ctx, *resolve, storage_cfg).await;
        }
        Commands::LintManifests { platform } => {
            let ctx = resolve_context(cli, current_dir, env)?;
            return Ok(commands::lint::run_lint(&ctx, platform.as_deref()).into());
        }
        Commands::Resolve { test_path, fetch } => {
            let ctx = resolve_context(cli, current_dir, env)?;
            let storage_cfg = get_storage_config(env);
            let code =
                commands::resolve::run_resolve(&ctx, test_path.as_deref(), *fetch, storage_cfg)
                    .await;
            return Ok(code.into());
        }
        Commands::Test => info!("Subcommand test is not fully implemented yet"),
        Commands::Pull {
            all_platforms,
            platform,
        } => {
            let ctx = resolve_context(cli, current_dir, env)?;
            let storage_cfg = get_storage_config(env);
            let code = commands::pull::run_pull(
                &ctx,
                storage_cfg.as_ref(),
                *all_platforms,
                platform.as_deref(),
            )
            .await;
            return Ok(code.into());
        }
        Commands::Push {
            all_platforms,
            platform,
        } => {
            let ctx = resolve_context(cli, current_dir, env)?;
            let storage_cfg = get_storage_config(env);
            let code = commands::push::run_push(
                &ctx,
                storage_cfg.as_ref(),
                *all_platforms,
                platform.as_deref(),
            )
            .await;
            return Ok(code.into());
        }
        Commands::Gc => info!("Subcommand gc is not fully implemented yet"),
        Commands::Report {
            format,
            report,
            pr_number,
            out,
        } => {
            let storage_cfg = get_storage_config(env);
            let code = commands::report::run_report(
                env,
                storage_cfg,
                format,
                report,
                *pr_number,
                out.as_deref(),
            )
            .await;
            return Ok(code.into());
        }
        Commands::Approve { paths, from } => {
            let ctx = resolve_context(cli, current_dir, env)?;
            return Ok(commands::approve::run_approve(&ctx, paths, from.as_ref()).into());
        }
        Commands::Clean {
            dry_run,
            skip_gitignore,
            keep_runs,
        } => {
            let ctx = resolve_context(cli, current_dir, env)?;
            let code = commands::clean::run_clean(&ctx, *dry_run, *skip_gitignore, *keep_runs);
            return Ok(code.into());
        }
    }
    Ok(ExitCode::Success.into())
}
