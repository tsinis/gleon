//! Implementation of the `gleon push` subcommand.

use gleon_core::context::ResolvedContext;
use gleon_core::ops::push_blobs;
use gleon_core::storage::StorageConfig;
use tracing::{error, info};

/// Runs the `gleon push` subcommand.
///
/// Returns exit code `0` on success or local mode, or `1` on error.
pub async fn run_push(
    ctx: &ResolvedContext,
    storage_cfg: Option<&StorageConfig>,
    all_platforms: bool,
    platform_override: Option<&str>,
) -> anyhow::Result<i32> {
    info!("Running blob push...");

    match push_blobs(
        ctx,
        &ctx.base_dir,
        storage_cfg,
        all_platforms,
        platform_override,
    )
    .await
    {
        Ok(res) => {
            if res.local_mode {
                info!("Operating in local mode. Cloud sync disabled. Please configure storage.");
            } else if res.total_manifest_blobs == 0 {
                info!("No baseline blobs found to push.");
            } else if res.uploaded_blobs == 0 {
                info!(
                    "All {} baseline blob(s) are already present in remote storage.",
                    res.total_manifest_blobs
                );
            } else {
                info!(
                    "Uploaded {} missing baseline blob(s) to storage ({} skipped, total: {}).",
                    res.uploaded_blobs, res.skipped_blobs, res.total_manifest_blobs
                );
            }
            Ok(0)
        }
        Err(e) => {
            error!("Error pushing baseline blobs: {e}");
            Ok(1)
        }
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use gleon_core::cli::{Cli, Commands};
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_run_push_uninitialized_and_local_mode() {
        let temp = tempdir().unwrap();
        let cli = Cli::for_test(Commands::Init);
        let ctx = ResolvedContext::from_cli(&cli, temp.path()).unwrap();

        // 1. Uninitialized -> exit code 1
        let exit_code_uninit = run_push(&ctx, None, false, None).await.unwrap();
        assert_eq!(exit_code_uninit, 1);

        // 2. Initialized + Local Mode -> exit code 0
        gleon_core::ops::init_workspace(&ctx, temp.path()).unwrap();
        let exit_code_local = run_push(&ctx, None, false, None).await.unwrap();
        assert_eq!(exit_code_local, 0);
    }
}
