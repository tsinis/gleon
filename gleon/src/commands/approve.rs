//! Handler for `gleon approve` subcommand.

use gleon_core::context::ResolvedContext;
use std::path::PathBuf;
use tracing::info;

use crate::commands::report_failure;
use crate::exit_code::ExitCode;

/// Runs the `approve` command.
pub fn run_approve(ctx: &ResolvedContext, paths: &[PathBuf], from: Option<&PathBuf>) -> ExitCode {
    let res = match gleon_core::ops::approve_workspace(ctx, paths, from.map(PathBuf::as_path)) {
        Ok(res) => res,
        Err(e) => return report_failure("Error approving screenshots", e),
    };

    if res.total_approved == 0 {
        info!("No screenshots approved.");
    } else {
        info!(
            "Approved {} screenshot(s) across {} test case(s).",
            res.total_approved,
            res.approved_test_cases.len()
        );
    }
    ExitCode::Success
}
