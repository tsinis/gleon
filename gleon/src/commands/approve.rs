//! Handler for `gleon approve` subcommand.

use gleon_core::context::ResolvedContext;
use std::path::PathBuf;
use tracing::info;

/// Runs the `approve` command.
pub fn run_approve(
    ctx: &ResolvedContext,
    paths: &[PathBuf],
    from: Option<&PathBuf>,
) -> anyhow::Result<i32> {
    let res =
        gleon_core::ops::approve_workspace(ctx, &ctx.base_dir, paths, from.map(|p| p.as_path()))?;

    if res.total_approved == 0 {
        info!("No screenshots approved.");
    } else {
        info!(
            "Approved {} screenshot(s) across {} test case(s).",
            res.total_approved,
            res.approved_test_cases.len()
        );
    }
    Ok(0)
}
