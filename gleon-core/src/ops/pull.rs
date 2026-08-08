//! Pull operation for downloading missing baseline blobs from remote storage.

use futures::{StreamExt as _, TryStreamExt as _};
use std::path::Path;
use thiserror::Error;
use tracing::info;

use crate::context::{ContextError, ResolvedContext};
use crate::manifest::{ManifestError, WorkspaceIndex};
use crate::ops::push::list_platform_dirs;
use crate::platform::validate_segment;
use crate::storage::{ObjectStoreAdapter, StorageConfig, StorageError};

/// Errors that can occur during a pull operation.
#[derive(Debug, Error)]
pub enum PullError {
    /// Workspace has not been initialized (`.gleon` missing).
    #[error("gleon workspace is not initialized. Please run 'gleon init' first.")]
    NotInitialized,

    /// Error resolving context.
    #[error("Context resolution error: {0}")]
    Context(#[from] ContextError),

    /// Error scanning or reading manifests.
    #[error("Manifest error: {0}")]
    Manifest(#[from] ManifestError),

    /// Storage adapter error.
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),

    /// Missing remote blob on Object Store.
    #[error(
        "Missing remote blob for hash '{hash}' referenced in manifest at platform '{platform}'. Blob was not found in remote storage."
    )]
    MissingRemoteBlob {
        /// The string representation of the missing hash.
        hash: String,
        /// The platform directory key where the reference was found.
        platform: String,
    },

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Summary of pull operation results.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PullResult {
    /// Total unique baseline blobs referenced across scanned platforms.
    pub total_manifest_blobs: usize,
    /// Number of blobs downloaded from remote storage.
    pub downloaded_blobs: usize,
    /// Number of blobs already present in local storage.
    pub skipped_blobs: usize,
    /// Indicates whether gleon executed in Local Flat Mode (no storage configured).
    pub local_mode: bool,
}

/// Executes pull pipeline, downloading missing baseline blobs from remote storage.
///
/// # Errors
/// Returns [`PullError`] if the workspace is not initialized, remote blobs are missing,
/// or remote storage operations fail.
pub async fn pull_blobs(
    context: &ResolvedContext,
    base_dir: &Path,
    storage_config: Option<&StorageConfig>,
    all_platforms: bool,
    platform_override: Option<&str>,
) -> Result<PullResult, PullError> {
    let gleon_dir = base_dir.join(".gleon");
    if !gleon_dir.exists() {
        return Err(PullError::NotInitialized);
    }

    let storage_cfg = match storage_config {
        Some(cfg) if !cfg.url.trim().is_empty() => cfg,
        _ => {
            info!("Operating in local mode. Cloud sync disabled. Please configure storage.");
            return Ok(PullResult {
                total_manifest_blobs: 0,
                downloaded_blobs: 0,
                skipped_blobs: 0,
                local_mode: true,
            });
        }
    };

    let manifests_root = gleon_dir.join("manifests");
    let blobs_root = gleon_dir.join("blobs");

    let platform_dirs = if all_platforms {
        match list_platform_dirs(&manifests_root) {
            Ok(dirs) => dirs,
            Err(e) => return Err(PullError::Io(e)),
        }
    } else if let Some(p) = platform_override {
        let valid_key = validate_segment(p)
            .map_err(|e| PullError::Context(ContextError::Platform(e)))?
            .into_owned();
        vec![(valid_key.clone(), manifests_root.join(valid_key))]
    } else {
        let platform_key = match context.platform.to_key() {
            Ok(key) => key,
            Err(e) => return Err(PullError::Context(ContextError::Platform(e))),
        };
        let plat_dir = manifests_root.join(&platform_key);
        let plat_idx = WorkspaceIndex::load(&plat_dir).map_err(PullError::Manifest)?;
        let has_manifests = !plat_idx.is_empty();

        if !has_manifests {
            if let Some(ref fallback_key) = context.fallback_platform_key {
                let fb_dir = manifests_root.join(fallback_key);
                let fb_idx = WorkspaceIndex::load(&fb_dir).map_err(PullError::Manifest)?;
                if !fb_idx.is_empty() {
                    tracing::warn!(
                        "No manifests found for platform '{}'. Falling back to pulling blobs for fallback platform '{}'.",
                        platform_key,
                        fallback_key
                    );
                    vec![(fallback_key.clone(), fb_dir)]
                } else {
                    vec![(platform_key.clone(), plat_dir)]
                }
            } else {
                vec![(platform_key.clone(), plat_dir)]
            }
        } else {
            vec![(platform_key.clone(), plat_dir)]
        }
    };

    let mut referenced_hashes = std::collections::BTreeSet::new();
    let mut missing_blobs = Vec::new();
    let mut skipped_blobs = 0;

    for (target_platform_key, target_dir) in platform_dirs {
        let index = WorkspaceIndex::load(&target_dir).map_err(PullError::Manifest)?;

        for manifest in index.entries().values() {
            let hash = &manifest.hash;
            if !referenced_hashes.contains(hash) {
                referenced_hashes.insert(hash.clone());
                let local_blob_path = blobs_root.join(hash.scheme()).join(hash.value());
                if local_blob_path.is_file() {
                    skipped_blobs += 1;
                } else {
                    missing_blobs.push((hash.clone(), target_platform_key.clone()));
                }
            }
        }
    }

    let total_manifest_blobs = referenced_hashes.len();

    if missing_blobs.is_empty() {
        return Ok(PullResult {
            total_manifest_blobs,
            downloaded_blobs: 0,
            skipped_blobs,
            local_mode: false,
        });
    }

    let adapter = ObjectStoreAdapter::from_config(storage_cfg)?;

    let missing_count = missing_blobs.len();
    let progress_bar = crate::ui::create_progress_bar(missing_count as u64);

    let mut download_stream =
        futures::stream::iter(missing_blobs.into_iter().map(|(hash, _plat)| {
            let adapter = adapter.clone();
            let dest_path = blobs_root.join(hash.scheme()).join(hash.value());
            let pb = progress_bar.clone();
            async move {
                pb.set_message(format!(
                    "Downloading {}",
                    &hash.value()[..8.min(hash.value().len())]
                ));
                let res = match adapter.download_blob(&hash, &dest_path).await {
                    Ok(_) => Ok(()),
                    Err(StorageError::BlobNotFound(_)) => Err(PullError::MissingRemoteBlob {
                        hash: hash.value().to_string(),
                        platform: _plat,
                    }),
                    Err(e) => Err(PullError::Storage(e)),
                };
                pb.inc(1);
                res
            }
        }))
        .buffer_unordered(adapter.concurrency());

    let download_res = async {
        while let Some(()) = download_stream.try_next().await? {}
        Ok::<(), PullError>(())
    }
    .await;
    progress_bar.finish_and_clear();
    download_res?;

    Ok(PullResult {
        total_manifest_blobs,
        downloaded_blobs: missing_count,
        skipped_blobs,
        local_mode: false,
    })
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::platform::PlatformError;

    #[test]
    fn test_pull_error_display() {
        let err1 = PullError::NotInitialized;
        assert!(err1.to_string().contains("not initialized"));

        let err2 = PullError::MissingRemoteBlob {
            hash: "xyz".to_string(),
            platform: "linux-x86_64".to_string(),
        };
        assert!(err2.to_string().contains("Missing remote blob"));
        assert!(err2.to_string().contains("xyz"));

        let err3 = PullError::Io(std::io::Error::other("io test"));
        assert!(err3.to_string().contains("IO error"));

        let err4 = PullError::Context(ContextError::Platform(PlatformError::InvalidSegment(
            "bad".to_string(),
        )));
        assert!(err4.to_string().contains("Context resolution error"));

        let err5 = PullError::Storage(StorageError::BlobNotFound("hash".to_string()));
        assert!(err5.to_string().contains("Storage error"));
    }

    #[test]
    fn test_pull_result_derived() {
        let res = PullResult {
            total_manifest_blobs: 10,
            downloaded_blobs: 4,
            skipped_blobs: 6,
            local_mode: false,
        };
        assert_eq!(res.clone(), res);
        assert!(!format!("{:?}", res).is_empty());
        let default_res = PullResult::default();
        assert_eq!(default_res.total_manifest_blobs, 0);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_pull_platform_override_validation() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        let ctx = ResolvedContext::default();
        let cfg = StorageConfig::new("memory://");

        // Invalid platform override segment
        let res = pull_blobs(&ctx, temp.path(), Some(&cfg), false, Some("../invalid")).await;
        assert!(matches!(
            res,
            Err(PullError::Context(ContextError::Platform(_)))
        ));

        // Empty manifest directory for valid platform override -> 0 blobs
        let res = pull_blobs(&ctx, temp.path(), Some(&cfg), false, Some("macos-aarch64"))
            .await
            .unwrap();
        assert_eq!(res.total_manifest_blobs, 0);
    }

    #[cfg(all(unix, not(miri)))]
    #[tokio::test]
    async fn test_pull_manifests_root_unreadable() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        let manifests_dir = gleon_dir.join("manifests");
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let mut perms = std::fs::metadata(&manifests_dir).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&manifests_dir, perms.clone()).unwrap();

        let ctx = ResolvedContext::default();
        let cfg = StorageConfig::new("memory://");
        let res = pull_blobs(&ctx, temp.path(), Some(&cfg), true, None).await;
        assert!(matches!(res, Err(PullError::Io(_))));

        perms.set_mode(0o755);
        std::fs::set_permissions(&manifests_dir, perms).unwrap();
    }

    #[tokio::test]
    async fn test_pull_invalid_platform_key() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();
        let mut ctx = ResolvedContext::default();
        ctx.platform.os = "invalid/os".to_string();

        let cfg = StorageConfig::new("memory://");
        let res = pull_blobs(&ctx, temp.path(), Some(&cfg), false, None).await;
        assert!(matches!(
            res,
            Err(PullError::Context(ContextError::Platform(_)))
        ));
    }

    #[tokio::test]
    async fn test_pull_empty_fallback_platform() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");

        let plat_key = "5:linux-6:x86_64";
        let fb_key = "5:macos-7:aarch64";
        std::fs::create_dir_all(gleon_dir.join("manifests").join(plat_key)).unwrap();
        std::fs::create_dir_all(gleon_dir.join("manifests").join(fb_key)).unwrap();

        let mut ctx = ResolvedContext::default();
        // Since we created both platform dir and fallback dir, setting platform key via os segment string won't work perfectly since plat_key contains colon.
        // Wait, how do I mock ctx.platform.to_key() to return "5:linux-6:x86_64"?
        ctx.platform.os = "linux".to_string();
        ctx.platform.arch = Some("x86_64".to_string());
        ctx.platform.renderer = None;
        ctx.fallback_platform_key = Some(fb_key.to_string());

        let cfg = StorageConfig::new("memory://");
        let res = pull_blobs(&ctx, temp.path(), Some(&cfg), false, None).await;
        assert!(res.is_ok());
        assert_eq!(res.unwrap().total_manifest_blobs, 0);
    }

    #[cfg(all(unix, not(miri)))]
    #[tokio::test]
    async fn test_pull_storage_io_error() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(gleon_dir.join("manifests")).unwrap();

        let plat_key = "5:linux-6:x86_64";
        let manifests_dir = gleon_dir.join("manifests").join(plat_key);
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let hash = "1111111111111111111111111111111111111111111111111111111111111111";
        let manifest = crate::manifest::SingleTestManifest::new(
            crate::manifest::ImageHash::new("sha256", hash).unwrap(),
            crate::manifest::ImageHash::new("dhash", "0000000000000000").unwrap(),
            1,
            1,
        )
        .unwrap();
        manifest.save(manifests_dir.join("test.json")).unwrap();

        let mut ctx = ResolvedContext::default();
        ctx.platform.os = "linux".to_string();
        ctx.platform.arch = Some("x86_64".to_string());

        let remote_dir = temp.path().join("remote_blobs");
        std::fs::create_dir_all(&remote_dir).unwrap();
        // Use a valid file storage URL
        let cfg = StorageConfig::new(format!("file://{}", remote_dir.display()));
        let adapter = crate::storage::ObjectStoreAdapter::from_config(&cfg).unwrap();
        // Create an empty file to upload
        let empty_file = temp.path().join("empty");
        std::fs::write(&empty_file, "").unwrap();
        // Upload the dummy blob so it exists remotely
        adapter
            .upload_blob(
                &crate::manifest::ImageHash::new("sha256", hash).unwrap(),
                &empty_file,
            )
            .await
            .ok();

        // Make the local blobs directory unreadable to trigger an IO error during download
        let local_blobs = temp.path().join(".gleon").join("blobs");
        std::fs::create_dir_all(&local_blobs).unwrap();

        let mut perms = std::fs::metadata(&local_blobs).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&local_blobs, perms.clone()).unwrap();

        let res = pull_blobs(&ctx, temp.path(), Some(&cfg), false, None).await;
        assert!(matches!(res, Err(PullError::Storage(_))));

        perms.set_mode(0o755);
        std::fs::set_permissions(&local_blobs, perms).unwrap();
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_pull_fallback_platform_and_duplicate_hashes() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        let ctx = ResolvedContext {
            fallback_platform_key: Some("fallback-platform".to_string()),
            ..Default::default()
        };

        let cfg = StorageConfig::new("memory://");
        let hash = crate::manifest::ImageHash::new(
            "sha256",
            "1111111111111111111111111111111111111111111111111111111111111111",
        )
        .unwrap();
        let phash = crate::manifest::ImageHash::new("dhash", "0000000000000000").unwrap();

        // 1. Create manifests in fallback-platform directory
        let fallback_manifests_dir = gleon_dir.join("manifests").join("fallback-platform");
        std::fs::create_dir_all(&fallback_manifests_dir).unwrap();

        // Write two manifests with identical hash to test deduplication (`referenced_hashes.insert(...) == false`)
        let m1 =
            crate::manifest::SingleTestManifest::new(hash.clone(), phash.clone(), 10, 10).unwrap();
        let m2 = crate::manifest::SingleTestManifest::new(hash, phash, 10, 10).unwrap();
        m1.save(fallback_manifests_dir.join("test1.json")).unwrap();
        m2.save(fallback_manifests_dir.join("test2.json")).unwrap();

        // Pre-create the blob file locally so it counts as skipped
        let blob_path = gleon_dir
            .join("blobs")
            .join("sha256")
            .join("1111111111111111111111111111111111111111111111111111111111111111");
        std::fs::create_dir_all(blob_path.parent().unwrap()).unwrap();
        std::fs::write(&blob_path, "blob content").unwrap();

        let res = pull_blobs(&ctx, temp.path(), Some(&cfg), false, None)
            .await
            .unwrap();

        assert_eq!(res.total_manifest_blobs, 1);
        assert_eq!(res.skipped_blobs, 1);
    }
}
