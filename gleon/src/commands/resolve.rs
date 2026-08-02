//! Implementation of the `gleon resolve` subcommand for interactive conflict resolution.

use dialoguer::Select;
use gleon_core::context::ResolvedContext;
use gleon_core::ops::resolve::ConflictedManifestItem;
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

    let resolved_count =
        resolve_conflicts_with_selector(ctx, conflicts, adapter.as_ref(), |item| {
            let choices = format_conflict_choices(item);

            Select::new()
                .with_prompt(format!("Choose baseline to keep for '{}'", item.test_path))
                .items(&choices)
                .default(0)
                .interact()
                .map_err(Into::into)
        })
        .await?;

    info!(
        "Successfully resolved {} manifest conflict(s).",
        resolved_count
    );

    Ok(0)
}

/// Formats selectable prompt choice descriptions for a conflicted manifest item.
#[must_use]
pub fn format_conflict_choices(item: &ConflictedManifestItem) -> Vec<String> {
    vec![
        format!(
            "Ours (HEAD: {}, {}x{})",
            item.conflict.ours.hash, item.conflict.ours.width, item.conflict.ours.height
        ),
        format!(
            "Theirs (Incoming: {}, {}x{})",
            item.conflict.theirs.hash, item.conflict.theirs.width, item.conflict.theirs.height
        ),
    ]
}

/// Helper to process interactive or automated conflict selections.
pub async fn resolve_conflicts_with_selector<F>(
    ctx: &ResolvedContext,
    conflicts: Vec<ConflictedManifestItem>,
    adapter: Option<&ObjectStoreAdapter>,
    mut selector: F,
) -> Result<usize, anyhow::Error>
where
    F: FnMut(&ConflictedManifestItem) -> Result<usize, anyhow::Error>,
{
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

        if let Some(adapter) = adapter {
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

        let selection = selector(&item)?;

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
            return Err(e.into());
        }

        info!("Resolved '{}' to {}", item.test_path, chosen.hash);
        resolved_count += 1;
    }

    Ok(resolved_count)
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use gleon_core::cli::{Cli, Commands};
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_run_resolve_missing_manifest_dir() {
        let temp = tempdir().unwrap();
        let cli = Cli::for_test(Commands::Init);
        let ctx = ResolvedContext::from_cli(&cli, temp.path()).unwrap();

        // Missing manifest directory causes scan_conflicts to fail -> return Ok(1)
        let res = run_resolve(&ctx, None, false, None).await.unwrap();
        assert_eq!(res, 1);
    }

    #[tokio::test]
    async fn test_run_resolve_filter_and_fetch_options() {
        let temp = tempdir().unwrap();
        let base_dir = temp.path();
        let manifests_dir = base_dir
            .join(".gleon")
            .join("manifests")
            .join("macos-aarch64")
            .join("auth");
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let conflicted = include_str!("../../../gleon-core/tests/fixtures/conflict_2way.json");
        std::fs::write(manifests_dir.join("login.json"), conflicted).unwrap();

        let cli = Cli::for_test(Commands::Init);
        let ctx = ResolvedContext::from_cli(&cli, base_dir).unwrap();

        // 1. Filter out all test paths
        let res_filtered = run_resolve(&ctx, Some("nonexistent_filter"), false, None)
            .await
            .unwrap();
        assert_eq!(res_filtered, 0);

        // 2. Matching filter in non-interactive mode
        let res_matching = run_resolve(&ctx, Some("login"), false, None).await.unwrap();
        assert_eq!(res_matching, 1);

        // 3. Fetch mode without storage config
        let res_fetch_local = run_resolve(&ctx, Some("login"), true, None).await.unwrap();
        assert_eq!(res_fetch_local, 1);

        // 4. Fetch mode with invalid storage config
        let invalid_storage = StorageConfig::new("invalid_scheme://bucket".to_string());
        let res_fetch_invalid = run_resolve(&ctx, Some("login"), true, Some(invalid_storage))
            .await
            .unwrap();
        assert_eq!(res_fetch_invalid, 1);

        // 5. Fetch mode with valid memory storage config (hits Ok(a) => Some(a))
        let valid_storage = StorageConfig::new("memory://");
        let res_fetch_valid = run_resolve(&ctx, Some("login"), true, Some(valid_storage))
            .await
            .unwrap();
        assert_eq!(res_fetch_valid, 1);
    }

    #[tokio::test]
    async fn test_format_conflict_choices() {
        let temp = tempdir().unwrap();
        let base_dir = temp.path();
        let manifests_dir = base_dir
            .join(".gleon")
            .join("manifests")
            .join("macos-aarch64")
            .join("auth");
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let conflicted = include_str!("../../../gleon-core/tests/fixtures/conflict_2way.json");
        std::fs::write(manifests_dir.join("login.json"), conflicted).unwrap();

        let conflicts = scan_conflicts(base_dir, None).unwrap();
        assert_eq!(conflicts.len(), 1);

        let choices = format_conflict_choices(&conflicts[0]);
        assert_eq!(choices.len(), 2);
        assert!(choices[0].contains("Ours (HEAD:"));
        assert!(choices[1].contains("Theirs (Incoming:"));
    }

    #[tokio::test]
    async fn test_resolve_conflicts_with_selector_ours_and_theirs() {
        let temp = tempdir().unwrap();
        let base_dir = temp.path();
        let manifests_dir = base_dir
            .join(".gleon")
            .join("manifests")
            .join("macos-aarch64")
            .join("auth");
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let conflicted = include_str!("../../../gleon-core/tests/fixtures/conflict_2way.json");
        let login_path = manifests_dir.join("login.json");
        std::fs::write(&login_path, conflicted).unwrap();

        let cli = Cli::for_test(Commands::Init);
        let ctx = ResolvedContext::from_cli(&cli, base_dir).unwrap();

        let conflicts = scan_conflicts(base_dir, None).unwrap();
        assert_eq!(conflicts.len(), 1);

        // Test resolving selecting 'ours' (index 0)
        let count_ours = resolve_conflicts_with_selector(&ctx, conflicts.clone(), None, |_| Ok(0))
            .await
            .unwrap();
        assert_eq!(count_ours, 1);

        let content_ours = std::fs::read_to_string(&login_path).unwrap();
        assert!(!content_ours.contains("<<<<<<<"));
        assert!(
            content_ours.contains(
                "sha256:1111111111111111111111111111111111111111111111111111111111111111"
            )
        );

        // Restore conflicted file
        std::fs::write(&login_path, conflicted).unwrap();
        let conflicts_theirs = scan_conflicts(base_dir, None).unwrap();

        // Test resolving selecting 'theirs' (index 1)
        let count_theirs = resolve_conflicts_with_selector(&ctx, conflicts_theirs, None, |_| Ok(1))
            .await
            .unwrap();
        assert_eq!(count_theirs, 1);

        let content_theirs = std::fs::read_to_string(&login_path).unwrap();
        assert!(!content_theirs.contains("<<<<<<<"));
        assert!(
            content_theirs.contains(
                "sha256:2222222222222222222222222222222222222222222222222222222222222222"
            )
        );
    }

    #[tokio::test]
    async fn test_resolve_conflicts_with_selector_adapter_blob_fetch() {
        let temp = tempdir().unwrap();
        let base_dir = temp.path();
        let manifests_dir = base_dir
            .join(".gleon")
            .join("manifests")
            .join("macos-aarch64")
            .join("auth");
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let conflicted = include_str!("../../../gleon-core/tests/fixtures/conflict_2way.json");
        let login_path = manifests_dir.join("login.json");
        std::fs::write(&login_path, conflicted).unwrap();

        let cli = Cli::for_test(Commands::Init);
        let ctx = ResolvedContext::from_cli(&cli, base_dir).unwrap();

        let conflicts = scan_conflicts(base_dir, None).unwrap();
        let config = StorageConfig::new("memory://");
        let adapter = ObjectStoreAdapter::from_config(&config).unwrap();

        // Upload a dummy blob for ours hash into memory adapter so download_blob succeeds
        adapter
            .upload_blob(
                "1111111111111111111111111111111111111111111111111111111111111111",
                &login_path,
            )
            .await
            .unwrap();

        // This will attempt to download missing blob for ours and theirs
        let count = resolve_conflicts_with_selector(&ctx, conflicts, Some(&adapter), |_| Ok(0))
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_resolve_conflicts_with_selector_apply_resolution_failure() {
        let temp = tempdir().unwrap();
        let base_dir = temp.path();
        let manifests_dir = base_dir
            .join(".gleon")
            .join("manifests")
            .join("macos-aarch64")
            .join("auth");
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let conflicted = include_str!("../../../gleon-core/tests/fixtures/conflict_2way.json");
        let login_path = manifests_dir.join("login.json");
        std::fs::write(&login_path, conflicted).unwrap();

        let cli = Cli::for_test(Commands::Init);
        let ctx = ResolvedContext::from_cli(&cli, base_dir).unwrap();

        let conflicts = scan_conflicts(base_dir, None).unwrap();
        let mut invalid_conflicts = conflicts;
        // Setting manifest_file_path to a directory path will cause apply_resolution to fail
        invalid_conflicts[0].manifest_file_path = manifests_dir.clone();

        let res = resolve_conflicts_with_selector(&ctx, invalid_conflicts, None, |_| Ok(0)).await;
        assert!(res.is_err());
    }
}
