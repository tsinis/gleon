//! Implementation of the `gleon dashboard` subcommand.

use std::num::NonZeroUsize;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use gleon_core::context::ResolvedContext;
use gleon_core::dashboard::{DashboardCompiler, DashboardOptions};
use gleon_core::ops::common::ensure_initialized;
use gleon_core::storage::StorageConfig;

use crate::commands::report_failure;
use crate::exit_code::ExitCode;

/// Runs the `gleon dashboard` subcommand.
///
/// Compiles `dashboard.html` from `history.json` and optionally pushes both to remote storage.
/// Returns [`ExitCode::Success`] on success or [`ExitCode::Failure`] on error.
pub async fn run_dashboard(
    ctx: &ResolvedContext,
    storage_cfg: Option<StorageConfig>,
    report: Option<&Path>,
    out: Option<&Path>,
    truncate_history: Option<NonZeroUsize>,
    push: bool,
) -> ExitCode {
    match run_dashboard_inner(ctx, storage_cfg, report, out, truncate_history, push).await {
        Ok(()) => ExitCode::Success,
        Err(e) => report_failure("Error compiling dashboard", &*e),
    }
}

async fn run_dashboard_inner(
    ctx: &ResolvedContext,
    storage_cfg: Option<StorageConfig>,
    report: Option<&Path>,
    out: Option<&Path>,
    truncate_history: Option<NonZeroUsize>,
    push: bool,
) -> Result<()> {
    tracing::info!("Compiling visual regression dashboard...");

    let paths = ensure_initialized(&ctx.base_dir)
        .context("Workspace not initialized. Run `gleon init` first.")?;

    let effective_report = if let Some(p) = report {
        let candidate = if p.is_file() {
            p.to_path_buf()
        } else {
            ctx.base_dir.join(p)
        };
        if !candidate.is_file() {
            return Err(anyhow!("Report file not found at '{}'", p.display()));
        }
        candidate
    } else {
        let default_report = paths.report_file();
        if !default_report.is_file() {
            return Err(anyhow!(
                "No test report found at '{}'. Run tests first with `gleon diff` or provide `--report <PATH>`.",
                default_report.display()
            ));
        }
        default_report
    };

    let options = DashboardOptions {
        out_html: out,
        truncate_limit: truncate_history,
        push_to_storage: push,
    };

    let result = DashboardCompiler::execute(
        &paths,
        ctx,
        &effective_report,
        &options,
        storage_cfg.as_ref(),
    )
    .await
    .context("Failed to compile history dashboard")?;

    // Output primary machine-readable artifact path to stdout
    println!("{}", result.html_path.display());

    tracing::info!(
        "Dashboard compiled successfully at {} (total runs in history: {})",
        result.html_path.display(),
        result.total_runs
    );

    if result.pushed {
        tracing::info!("Successfully uploaded history.json and dashboard.html to remote storage.");
    }

    Ok(())
}

#[cfg(all(test, not(miri)))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::pedantic,
    clippy::nursery
)]
mod tests {
    use super::*;
    use gleon_core::context::ContextOptions;

    #[tokio::test]
    async fn test_run_dashboard_uninitialized() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = ResolvedContext::from_options(&ContextOptions::default(), temp.path()).unwrap();
        let report_path = temp.path().join("report.json");

        let exit_code = run_dashboard(&ctx, None, Some(&report_path), None, None, false).await;
        assert_eq!(exit_code, ExitCode::Failure);
    }

    #[tokio::test]
    async fn test_run_dashboard_success() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = ResolvedContext::from_options(&ContextOptions::default(), temp.path()).unwrap();
        gleon_core::ops::init_workspace(&ctx).unwrap();

        let report_path = temp.path().join("report.json");
        std::fs::write(&report_path, "[]").unwrap();

        let exit_code = run_dashboard(&ctx, None, Some(&report_path), None, None, false).await;
        assert_eq!(exit_code, ExitCode::Success);
    }

    #[tokio::test]
    async fn test_run_dashboard_resolves_default_report() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = ResolvedContext::from_options(&ContextOptions::default(), temp.path()).unwrap();
        gleon_core::ops::init_workspace(&ctx).unwrap();

        // Place report in default location (.gleon/runs/latest/gleon-report.json)
        let paths = gleon_core::paths::GleonPaths::new(temp.path());
        std::fs::create_dir_all(paths.runs_latest()).unwrap();
        std::fs::write(paths.report_file(), "[]").unwrap();

        // When report is None, default report_file() is picked up
        let exit_code = run_dashboard(&ctx, None, None, None, None, false).await;
        assert_eq!(exit_code, ExitCode::Success);
    }

    #[tokio::test]
    async fn test_run_dashboard_missing_specified_report_fails_fast() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = ResolvedContext::from_options(&ContextOptions::default(), temp.path()).unwrap();
        gleon_core::ops::init_workspace(&ctx).unwrap();

        let non_existent = temp.path().join("does_not_exist.json");
        let exit_code = run_dashboard(&ctx, None, Some(&non_existent), None, None, false).await;
        assert_eq!(exit_code, ExitCode::Failure);
    }

    #[tokio::test]
    async fn test_run_dashboard_missing_default_report_fails_fast() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = ResolvedContext::from_options(&ContextOptions::default(), temp.path()).unwrap();
        gleon_core::ops::init_workspace(&ctx).unwrap();

        // No report at paths.report_file() -> should fail with friendly message
        let exit_code = run_dashboard(&ctx, None, None, None, None, false).await;
        assert_eq!(exit_code, ExitCode::Failure);
    }
}
