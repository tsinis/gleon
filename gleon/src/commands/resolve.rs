//! Implementation of the `gleon resolve` subcommand for interactive conflict resolution.

use dialoguer::Select;
use gleon_core::context::ResolvedContext;
use gleon_core::ops::{apply_resolution, scan_conflicts};
use gleon_core::storage::{ObjectStoreAdapter, StorageConfig};
use std::io::IsTerminal;
use tracing::{error, info, warn};

/// Runs interactive resolution for conflicted manifest files.
pub async fn run_resolve(
    ctx: &ResolvedContext,
    test_path_filter: Option<&str>,
    fetch: bool,
    storage_config: Option<StorageConfig>,
) -> anyhow::Result<i32> {
    info!("Scanning for conflicted manifest files...");

    let mut conflicts = match scan_conflicts(&ctx.base_dir, None) {
        Ok(c) => c,
        Err(e) => {
            error!("Error scanning for conflicts: {e}");
            return Ok(1);
        }
    };

    if let Some(filter) = test_path_filter {
        conflicts.retain(|item| item.test_path.contains(filter));
    }

    if conflicts.is_empty() {
        info!("No conflicted manifest files found.");
        return Ok(0);
    }

    info!("Found {} conflicted manifest file(s).", conflicts.len());

    let adapter = if fetch {
        if let Some(cfg) = storage_config {
            match ObjectStoreAdapter::from_config(&cfg) {
                Ok(a) => Some(a),
                Err(e) => {
                    warn!("Storage configured but failed to initialize adapter: {e}");
                    None
                }
            }
        } else {
            info!("Local mode active (no cloud storage configured). Skipping blob fetch.");
            None
        }
    } else {
        None
    };

    if !std::io::stdin().is_terminal() {
        error!("Terminal is non-interactive (not a TTY). Cannot prompt for resolution.");
        error!("Run 'gleon resolve' in an interactive terminal environment.");
        return Ok(1);
    }

    let mut resolved_count = 0;

    for item in conflicts {
        info!("Conflict in '{}' ({})", item.test_path, item.platform);
        info!(
            "  Ours   (HEAD):     hash={}, phash={}, {}x{}",
            item.conflict.ours.hash,
            item.conflict.ours.phash,
            item.conflict.ours.width,
            item.conflict.ours.height
        );
        info!(
            "  Theirs (Incoming): hash={}, phash={}, {}x{}",
            item.conflict.theirs.hash,
            item.conflict.theirs.phash,
            item.conflict.theirs.width,
            item.conflict.theirs.height
        );

        if let Some(ref adapter) = adapter {
            for manifest in [&item.conflict.ours, &item.conflict.theirs] {
                let local_blob = ctx
                    .base_dir
                    .join(".gleon")
                    .join("blobs")
                    .join("sha256")
                    .join(manifest.hash.value());
                if !local_blob.exists() {
                    info!(
                        "Fetching missing blob {} from storage...",
                        manifest.hash.value()
                    );
                    if let Err(e) = adapter
                        .download_blob(manifest.hash.value(), &local_blob)
                        .await
                    {
                        warn!("Failed to fetch blob {}: {e}", manifest.hash.value());
                    }
                }
            }
        }

        let choices = vec![
            format!(
                "Ours (HEAD: {}, {}x{})",
                item.conflict.ours.hash, item.conflict.ours.width, item.conflict.ours.height
            ),
            format!(
                "Theirs (Incoming: {}, {}x{})",
                item.conflict.theirs.hash, item.conflict.theirs.width, item.conflict.theirs.height
            ),
        ];

        let selection = Select::new()
            .with_prompt(format!("Choose baseline to keep for '{}'", item.test_path))
            .items(&choices)
            .default(0)
            .interact()?;

        let chosen = if selection == 0 {
            &item.conflict.ours
        } else {
            &item.conflict.theirs
        };

        if let Err(e) = apply_resolution(&item.manifest_file_path, chosen) {
            error!(
                "Error applying resolution to {}: {e}",
                item.manifest_file_path.display()
            );
            return Ok(1);
        }

        info!("Resolved '{}' to {}", item.test_path, chosen.hash);
        resolved_count += 1;
    }

    info!(
        "Successfully resolved {} manifest conflict(s).",
        resolved_count
    );
    Ok(0)
}
