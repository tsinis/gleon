//! Handler for `gleon clean` subcommand.

use gleon_core::context::ResolvedContext;
use gleon_core::ops::clean::{CleanOptions, clean_workspace};
use tracing::info;

/// Runs the `clean` command.
pub fn run_clean(
    ctx: &ResolvedContext,
    dry_run: bool,
    skip_gitignore: bool,
    keep_runs: bool,
) -> anyhow::Result<i32> {
    let options = CleanOptions {
        dry_run,
        skip_gitignore,
        keep_runs,
    };

    let res = clean_workspace(ctx, &ctx.base_dir, &options)?;

    if dry_run {
        info!(
            "[dry-run] Found {} screenshot(s) to remove:",
            res.deleted_files.len()
        );
        for f in &res.deleted_files {
            info!("  - {}", f.display());
        }
        if !res.gitignore_entries_added.is_empty() {
            info!("[dry-run] Would add to .gitignore:");
            for entry in &res.gitignore_entries_added {
                info!("  + {}", entry);
            }
        }
        if res.cache_cleaned {
            info!("[dry-run] Would remove .gleon/runs and .gleon/diffs directories.");
        }
    } else {
        info!(
            "Removed {} screenshot(s) ({} untracked from Git index).",
            res.deleted_files.len(),
            res.untracked_files.len()
        );
        if !res.gitignore_entries_added.is_empty() {
            info!(
                "Added {} entry/entries to .gitignore.",
                res.gitignore_entries_added.len()
            );
        }
        if res.cache_cleaned {
            info!("Cleaned .gleon/runs and .gleon/diffs cache.");
        }
    }

    Ok(0)
}
