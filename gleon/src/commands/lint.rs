//! Implementation of the `gleon lint-manifests` subcommand.

use gleon_core::context::ResolvedContext;
use gleon_core::ops::lint_workspace_manifests;
use tracing::{error, info};

/// Runs manifest linting across the workspace.
///
/// Returns exit code `0` if all manifests are valid and unconflicted,
/// or `1` if any file contains conflict markers or schema corruption.
pub fn run_lint(ctx: &ResolvedContext, platform_filter: Option<&str>) -> anyhow::Result<i32> {
    info!("Running manifest linting...");

    let report = match lint_workspace_manifests(ctx, &ctx.base_dir, platform_filter) {
        Ok(rep) => rep,
        Err(e) => {
            error!("Error during manifest linting: {e}");
            return Ok(1);
        }
    };

    info!(
        "Inspected {} manifest file(s): {} valid.",
        report.total_files, report.valid_files
    );

    if !report.conflicted_files.is_empty() {
        error!("Git merge conflict markers (<<<<<<<) found in:");
        for (path, msg) in &report.conflicted_files {
            error!("  - {}: {}", path.display(), msg);
        }
    }

    if !report.corrupted_files.is_empty() {
        error!("Schema or syntax errors found in:");
        for (path, msg) in &report.corrupted_files {
            error!("  - {}: {}", path.display(), msg);
        }
    }

    if report.passed {
        info!("All manifest files passed linting.");
        Ok(0)
    } else {
        error!("Lint check failed! Run 'gleon resolve' to resolve Git conflicts.");
        Ok(1)
    }
}
