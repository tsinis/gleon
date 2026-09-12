//! Implementation of the `gleon stage` subcommand.

use gleon_core::context::ResolvedContext;
use std::path::PathBuf;
use tracing::info;

use crate::commands::report_failure;
use crate::exit_code::ExitCode;

/// Runs the `gleon stage` subcommand, optionally restricted to `paths`.
pub fn run_stage(ctx: &ResolvedContext, paths: &[PathBuf]) -> ExitCode {
    let filter = if paths.is_empty() { None } else { Some(paths) };

    let res = match gleon_core::ops::stage_workspace(ctx, filter) {
        Ok(res) => res,
        Err(e) => return report_failure("Error staging screenshots", &e),
    };

    if res.total_screenshots_staged == 0 {
        info!("Already up to date.");
    } else {
        info!(
            "Staged {} screenshot(s) across {} test case(s).",
            res.total_screenshots_staged,
            res.staged_test_cases.len()
        );
    }
    ExitCode::Success
}
