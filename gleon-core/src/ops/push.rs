//! Push operation for uploading baseline blobs to remote storage.

use futures::{StreamExt as _, TryStreamExt as _};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::info;

use crate::context::{ContextError, ResolvedContext};
use crate::manifest::{ManifestError, WorkspaceIndex};
use crate::platform::validate_segment;
use crate::storage::{ObjectStoreAdapter, StorageConfig, StorageError};

/// Errors that can occur during a push operation.
#[derive(Debug, Error)]
pub enum PushError {
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

    /// Missing local blob for a manifest hash.
    #[error(
        "Missing local blob for hash '{hash}' referenced in manifest at platform '{platform}'. Please run 'gleon stage' first."
    )]
    MissingLocalBlob {
        /// The string representation of the missing hash.
        hash: String,
        /// The platform directory key where the reference was found.
        platform: String,
    },

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Summary of push operation results.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PushResult {
    /// Total unique baseline blobs referenced across scanned platforms.
    pub total_manifest_blobs: usize,
    /// Number of blobs uploaded to remote storage.
    pub uploaded_blobs: usize,
    /// Number of blobs already present on remote storage.
    pub skipped_blobs: usize,
    /// Indicates whether gleon executed in Local Flat Mode (no storage configured).
    pub local_mode: bool,
}

/// Helper function to discover valid platform directories under `.gleon/manifests/`.
pub(crate) fn list_platform_dirs(
    manifests_root: &Path,
) -> Result<Vec<(String, PathBuf)>, std::io::Error> {
    let entries = match std::fs::read_dir(manifests_root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    let mut platforms = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let is_dir = path.is_dir();
        let valid_name = entry
            .file_name()
            .to_str()
            .filter(|_| is_dir)
            .and_then(|n| crate::platform::validate_segment(n).ok())
            .map(|n| n.into_owned());

        if let Some(name) = valid_name {
            platforms.push((name, path));
        }
    }
    platforms.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(platforms)
}

/// Executes push pipeline, uploading baseline blobs to remote storage.
///
/// # Errors
/// Returns [`PushError`] if the workspace is not initialized, local blobs are missing,
/// or remote storage operations fail.
pub async fn push_blobs(
    context: &ResolvedContext,
    base_dir: &Path,
    storage_config: Option<&StorageConfig>,
    all_platforms: bool,
    platform_override: Option<&str>,
) -> Result<PushResult, PushError> {
    let gleon_dir = base_dir.join(".gleon");
    match std::fs::metadata(&gleon_dir) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(PushError::NotInitialized);
        }
        Err(e) => return Err(PushError::Io(e)),
    }

    let storage_cfg = match storage_config {
        Some(cfg) if !cfg.url.trim().is_empty() => cfg,
        _ => {
            info!("Operating in local mode. Cloud sync disabled. Please configure storage.");
            return Ok(PushResult {
                total_manifest_blobs: 0,
                uploaded_blobs: 0,
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
            Err(e) => return Err(PushError::Io(e)),
        }
    } else if let Some(p) = platform_override {
        let valid_key = validate_segment(p)
            .map_err(|e| PushError::Context(ContextError::Platform(e)))?
            .into_owned();
        vec![(valid_key.clone(), manifests_root.join(valid_key))]
    } else {
        let platform_key = match context.platform.to_key() {
            Ok(key) => key,
            Err(e) => return Err(PushError::Context(ContextError::Platform(e))),
        };
        vec![(platform_key.clone(), manifests_root.join(platform_key))]
    };

    // Collect all referenced unique sha256 blob hashes and their platform (for error reporting)
    let mut hash_to_platform = BTreeMap::new();

    for (plat_key, plat_dir) in &platform_dirs {
        match std::fs::metadata(plat_dir) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(PushError::Io(e)),
        }
        let index = match WorkspaceIndex::load(plat_dir) {
            Ok(idx) => idx,
            Err(e) => return Err(PushError::Manifest(e)),
        };
        for manifest in index.entries().values() {
            let hash = &manifest.hash;
            hash_to_platform
                .entry(hash.clone())
                .or_insert_with(|| plat_key.clone());
        }
    }

    let total_manifest_blobs = hash_to_platform.len();
    if total_manifest_blobs == 0 {
        return Ok(PushResult {
            total_manifest_blobs: 0,
            uploaded_blobs: 0,
            skipped_blobs: 0,
            local_mode: false,
        });
    }

    let adapter = ObjectStoreAdapter::from_config(storage_cfg)?;

    // Query remote storage in batch per unique scheme using adapter.list_blobs()
    let mut unique_schemes = HashSet::new();
    for hash in hash_to_platform.keys() {
        unique_schemes.insert(hash.scheme());
    }

    let mut existing_remote_hashes = HashSet::new();
    for scheme in unique_schemes {
        let remote_hashes = adapter
            .list_blobs(scheme)
            .await
            .map_err(PushError::Storage)?;
        for val in remote_hashes {
            if let Ok(h) = crate::manifest::ImageHash::new(scheme, val) {
                existing_remote_hashes.insert(h);
            }
        }
    }

    let mut missing_blobs = Vec::new();
    let mut skipped_blobs = 0;

    for (hash, platform) in hash_to_platform {
        if existing_remote_hashes.contains(&hash) {
            skipped_blobs += 1;
        } else {
            let local_blob_path = crate::storage::local_blob_path(&blobs_root, &hash);
            if !crate::storage::is_usable_blob(&local_blob_path) {
                return Err(PushError::MissingLocalBlob {
                    hash: hash.value().to_string(),
                    platform,
                });
            }
            missing_blobs.push(hash);
        }
    }

    let missing_count = missing_blobs.len();
    let progress_bar = crate::ui::create_progress_bar(missing_count as u64);

    // Upload missing blobs in parallel with Fail-Fast short-circuiting
    let mut upload_stream = futures::stream::iter(missing_blobs.into_iter().map(|hash| {
        let adapter = adapter.clone();
        let src_path = blobs_root.join(hash.scheme()).join(hash.value());
        let pb = progress_bar.clone();
        async move {
            pb.set_message(format!(
                "Uploading {}",
                &hash.value()[..8.min(hash.value().len())]
            ));
            let res = adapter
                .upload_blob(&hash, &src_path)
                .await
                .map_err(PushError::Storage);
            pb.inc(1);
            res
        }
    }))
    .buffer_unordered(adapter.concurrency());

    let upload_res = async {
        while let Some(()) = upload_stream.try_next().await? {}
        Ok::<(), PushError>(())
    }
    .await;
    progress_bar.finish_and_clear();
    upload_res?;

    Ok(PushResult {
        total_manifest_blobs,
        uploaded_blobs: missing_count,
        skipped_blobs,
        local_mode: false,
    })
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::platform::PlatformError;

    #[test]
    fn test_push_error_display() {
        let err1 = PushError::NotInitialized;
        assert!(err1.to_string().contains("not initialized"));

        let err2 = PushError::MissingLocalBlob {
            hash: "xyz".to_string(),
            platform: "linux-x86_64".to_string(),
        };
        assert!(err2.to_string().contains("Missing local blob"));
        assert!(err2.to_string().contains("xyz"));

        let err3 = PushError::Io(std::io::Error::other("io test"));
        assert!(err3.to_string().contains("IO error"));

        let err4 = PushError::Context(ContextError::Platform(PlatformError::InvalidSegment(
            "bad".to_string(),
        )));
        assert!(err4.to_string().contains("Context resolution error"));

        let err5 = PushError::Storage(StorageError::BlobNotFound("hash".to_string()));
        assert!(err5.to_string().contains("Storage error"));
    }

    #[test]
    fn test_push_result_derived() {
        let res = PushResult {
            total_manifest_blobs: 5,
            uploaded_blobs: 2,
            skipped_blobs: 3,
            local_mode: false,
        };
        assert_eq!(res.clone(), res);
        assert!(!format!("{:?}", res).is_empty());
        let default_res = PushResult::default();
        assert_eq!(default_res.total_manifest_blobs, 0);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_push_platform_override_validation() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        let ctx = ResolvedContext::default();
        let cfg = StorageConfig::new("memory://");

        // Invalid platform override segment
        let err = push_blobs(&ctx, temp.path(), Some(&cfg), false, Some("../invalid")).await;
        assert!(matches!(
            err,
            Err(PushError::Context(ContextError::Platform(_)))
        ));

        // Empty manifest directory for valid platform override -> 0 blobs
        let res = push_blobs(&ctx, temp.path(), Some(&cfg), false, Some("macos-aarch64"))
            .await
            .unwrap();
        assert_eq!(res.total_manifest_blobs, 0);
    }

    #[test]
    fn test_list_platform_dirs_filters_invalid_entries() {
        let temp = tempfile::tempdir().unwrap();
        let manifests = temp.path().join("manifests");
        std::fs::create_dir_all(manifests.join("valid-platform")).unwrap();
        std::fs::create_dir_all(manifests.join("invalid platform space")).unwrap();
        std::fs::write(manifests.join("some_file.txt"), "hello").unwrap();

        let res = list_platform_dirs(&manifests).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].0, "valid-platform");
    }

    #[test]
    fn test_list_platform_dirs_missing_root() {
        let temp = tempfile::tempdir().unwrap();
        let manifests = temp.path().join("does_not_exist");
        let res = list_platform_dirs(&manifests).unwrap();
        assert!(res.is_empty());
    }

    #[tokio::test]
    #[cfg(unix)]
    #[cfg_attr(miri, ignore)]
    async fn test_push_unreadable_manifests_root() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let manifests = temp.path().join(".gleon").join("manifests");
        std::fs::create_dir_all(&manifests).unwrap();

        let mut perms = std::fs::metadata(&manifests).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&manifests, perms.clone()).unwrap();

        let can_read = std::fs::read_dir(&manifests).is_ok();

        let ctx = ResolvedContext::default();
        let cfg = StorageConfig::new("memory://");
        let res = push_blobs(&ctx, temp.path(), Some(&cfg), true, None).await;

        perms.set_mode(0o755);
        std::fs::set_permissions(&manifests, perms).unwrap();

        if !can_read {
            assert!(matches!(res, Err(PushError::Io(_))));
        }
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_push_invalid_platform_key() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = ResolvedContext::default();
        ctx.platform.os = "invalid/os".to_string(); // Will fail to_key()

        std::fs::create_dir_all(temp.path().join(".gleon").join("manifests")).unwrap();

        let cfg = StorageConfig::new("memory://");
        let res = push_blobs(&ctx, temp.path(), Some(&cfg), false, None).await;
        assert!(matches!(
            res,
            Err(PushError::Context(ContextError::Platform(_)))
        ));
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_push_invalid_manifest_load() {
        let temp = tempfile::tempdir().unwrap();

        let mut ctx = ResolvedContext::default();
        ctx.platform.os = "linux".to_string();
        let key = ctx.platform.to_key().unwrap();

        let plat_dir = temp.path().join(".gleon").join("manifests").join(&key);
        std::fs::create_dir_all(&plat_dir).unwrap();
        std::fs::write(plat_dir.join("bad.json"), "not json").unwrap();

        let cfg = StorageConfig::new("memory://");
        let res = push_blobs(&ctx, temp.path(), Some(&cfg), false, None).await;
        assert!(matches!(res, Err(PushError::Manifest(_))));
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_push_success_with_referenced_blob() {
        let temp = tempfile::tempdir().unwrap();

        let mut ctx = ResolvedContext::default();
        ctx.platform.os = "linux".to_string();
        let key = ctx.platform.to_key().unwrap();

        let plat_dir = temp.path().join(".gleon").join("manifests").join(&key);
        std::fs::create_dir_all(&plat_dir).unwrap();

        let hash = "1111111111111111111111111111111111111111111111111111111111111111";
        let manifest = crate::manifest::SingleTestManifest::new(
            crate::manifest::ImageHash::new("sha256", hash).unwrap(),
            crate::manifest::ImageHash::new("dhash", "0000000000000000").unwrap(),
            1,
            1,
        )
        .unwrap();
        manifest.save(plat_dir.join("test.json")).unwrap();
        manifest.save(plat_dir.join("test2.json")).unwrap();

        // Create the local blob so validation passes
        let blobs_root = temp.path().join(".gleon").join("blobs");
        std::fs::create_dir_all(blobs_root.join("sha256")).unwrap();
        std::fs::write(blobs_root.join("sha256").join(hash), b"data").unwrap();

        let cfg = StorageConfig::new("memory://");
        let res = push_blobs(&ctx, temp.path(), Some(&cfg), false, None)
            .await
            .unwrap();

        assert_eq!(res.total_manifest_blobs, 1);
        assert_eq!(res.uploaded_blobs, 1);
        assert_eq!(res.skipped_blobs, 0);
    }

    #[tokio::test]
    #[cfg(unix)]
    #[cfg_attr(miri, ignore)]
    async fn test_push_rejects_symlink_blob() {
        let temp = tempfile::tempdir().unwrap();

        let mut ctx = ResolvedContext::default();
        ctx.platform.os = "linux".to_string();
        let key = ctx.platform.to_key().unwrap();

        let plat_dir = temp.path().join(".gleon").join("manifests").join(&key);
        std::fs::create_dir_all(&plat_dir).unwrap();

        let hash = "2222222222222222222222222222222222222222222222222222222222222222";
        let manifest = crate::manifest::SingleTestManifest::new(
            crate::manifest::ImageHash::new("sha256", hash).unwrap(),
            crate::manifest::ImageHash::new("dhash", "0000000000000000").unwrap(),
            1,
            1,
        )
        .unwrap();
        manifest.save(plat_dir.join("symlink_test.json")).unwrap();

        let blobs_root = temp.path().join(".gleon").join("blobs");
        std::fs::create_dir_all(blobs_root.join("sha256")).unwrap();
        let target_file = temp.path().join("target.txt");
        std::fs::write(&target_file, b"secret data").unwrap();
        std::os::unix::fs::symlink(&target_file, blobs_root.join("sha256").join(hash)).unwrap();

        let cfg = StorageConfig::new("memory://");
        let res = push_blobs(&ctx, temp.path(), Some(&cfg), false, None).await;
        assert!(matches!(res, Err(PushError::MissingLocalBlob { .. })));
    }

    #[tokio::test]
    #[cfg(all(unix, not(miri)))]
    async fn test_push_metadata_io_error_propagation() {
        let temp = tempfile::tempdir().unwrap();

        let mut ctx = ResolvedContext::default();
        ctx.platform.os = "linux".to_string();
        let key = ctx.platform.to_key().unwrap();

        let manifests_dir = temp.path().join(".gleon").join("manifests");
        let plat_dir = manifests_dir.join(&key);
        std::fs::create_dir_all(&plat_dir).unwrap();

        // Set parent directory permissions to 000 so stat on plat_dir fails with PermissionDenied
        use std::os::unix::fs::PermissionsExt;
        let original_perms = std::fs::metadata(&manifests_dir).unwrap().permissions();
        std::fs::set_permissions(&manifests_dir, std::fs::Permissions::from_mode(0o000)).unwrap();

        let cfg = StorageConfig::new("memory://");
        let res = push_blobs(&ctx, temp.path(), Some(&cfg), false, None).await;

        let was_permission_denied = std::fs::metadata(&plat_dir).is_err();
        let _ = std::fs::set_permissions(&manifests_dir, original_perms);

        if was_permission_denied {
            assert!(matches!(res, Err(PushError::Io(_))));
        } else {
            // Superuser/root runners bypass 000 directory permissions
            assert!(res.is_ok());
        }
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_push_skips_when_blob_already_in_remote_even_if_missing_locally() {
        use crate::storage::ObjectStoreAdapter;

        let temp = tempfile::tempdir().unwrap();

        let mut ctx = ResolvedContext::default();
        ctx.platform.os = "linux".to_string();
        let key = ctx.platform.to_key().unwrap();

        let plat_dir = temp.path().join(".gleon").join("manifests").join(&key);
        std::fs::create_dir_all(&plat_dir).unwrap();

        let hash_str = "3333333333333333333333333333333333333333333333333333333333333333";
        let image_hash = crate::manifest::ImageHash::new("sha256", hash_str).unwrap();
        let manifest = crate::manifest::SingleTestManifest::new(
            image_hash.clone(),
            crate::manifest::ImageHash::new("dhash", "0000000000000000").unwrap(),
            1,
            1,
        )
        .unwrap();
        manifest.save(plat_dir.join("test_remote.json")).unwrap();

        let remote_dir = temp.path().join("remote_storage");
        std::fs::create_dir_all(&remote_dir).unwrap();
        let cfg = StorageConfig::new(format!("file://{}", remote_dir.display()));
        let adapter = ObjectStoreAdapter::from_config(&cfg).unwrap();

        // Pre-populate remote storage with the blob directly
        let tmp_file = temp.path().join("tmp_blob.png");
        std::fs::write(&tmp_file, b"remote blob data").unwrap();
        adapter.upload_blob(&image_hash, &tmp_file).await.unwrap();

        // Local blob directory does NOT have the blob
        let res = push_blobs(&ctx, temp.path(), Some(&cfg), false, None)
            .await
            .unwrap();

        assert_eq!(res.total_manifest_blobs, 1);
        assert_eq!(res.uploaded_blobs, 0);
        assert_eq!(res.skipped_blobs, 1);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_push_fails_fast_when_blob_missing_both_remotely_and_locally() {
        let temp = tempfile::tempdir().unwrap();

        let mut ctx = ResolvedContext::default();
        ctx.platform.os = "linux".to_string();
        let key = ctx.platform.to_key().unwrap();

        let plat_dir = temp.path().join(".gleon").join("manifests").join(&key);
        std::fs::create_dir_all(&plat_dir).unwrap();

        let hash_str = "4444444444444444444444444444444444444444444444444444444444444444";
        let image_hash = crate::manifest::ImageHash::new("sha256", hash_str).unwrap();
        let manifest = crate::manifest::SingleTestManifest::new(
            image_hash,
            crate::manifest::ImageHash::new("dhash", "0000000000000000").unwrap(),
            1,
            1,
        )
        .unwrap();
        manifest.save(plat_dir.join("test_missing.json")).unwrap();

        let cfg = StorageConfig::new("memory://");
        let res = push_blobs(&ctx, temp.path(), Some(&cfg), false, None).await;

        assert!(matches!(res, Err(PushError::MissingLocalBlob { .. })));
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn test_push_batch_partial_remote_and_local_upload() {
        use crate::storage::ObjectStoreAdapter;

        let temp = tempfile::tempdir().unwrap();

        let mut ctx = ResolvedContext::default();
        ctx.platform.os = "linux".to_string();
        let key = ctx.platform.to_key().unwrap();

        let plat_dir = temp.path().join(".gleon").join("manifests").join(&key);
        std::fs::create_dir_all(&plat_dir).unwrap();

        // 1. Blob that exists remotely (should be skipped)
        let hash_remote_str = "1111111111111111111111111111111111111111111111111111111111111111";
        let image_hash_remote = crate::manifest::ImageHash::new("sha256", hash_remote_str).unwrap();
        let manifest_remote = crate::manifest::SingleTestManifest::new(
            image_hash_remote.clone(),
            crate::manifest::ImageHash::new("dhash", "0000000000000000").unwrap(),
            1,
            1,
        )
        .unwrap();
        manifest_remote
            .save(plat_dir.join("test_remote.json"))
            .unwrap();

        // 2. Blob that is missing remotely but exists locally (should be uploaded)
        let hash_local_str = "2222222222222222222222222222222222222222222222222222222222222222";
        let image_hash_local = crate::manifest::ImageHash::new("sha256", hash_local_str).unwrap();
        let manifest_local = crate::manifest::SingleTestManifest::new(
            image_hash_local.clone(),
            crate::manifest::ImageHash::new("dhash", "0000000000000000").unwrap(),
            1,
            1,
        )
        .unwrap();
        manifest_local
            .save(plat_dir.join("test_local.json"))
            .unwrap();

        let blobs_dir = temp.path().join(".gleon").join("blobs").join("sha256");
        std::fs::create_dir_all(&blobs_dir).unwrap();
        std::fs::write(blobs_dir.join(hash_local_str), b"local blob data").unwrap();

        let remote_dir = temp.path().join("remote_storage");
        std::fs::create_dir_all(&remote_dir).unwrap();
        let cfg = StorageConfig::new(format!("file://{}", remote_dir.display()));
        let adapter = ObjectStoreAdapter::from_config(&cfg).unwrap();

        // Pre-populate remote storage with the remote blob only
        let tmp_file = temp.path().join("tmp_blob.png");
        std::fs::write(&tmp_file, b"remote blob data").unwrap();
        adapter
            .upload_blob(&image_hash_remote, &tmp_file)
            .await
            .unwrap();

        let res = push_blobs(&ctx, temp.path(), Some(&cfg), false, None)
            .await
            .unwrap();

        assert_eq!(res.total_manifest_blobs, 2);
        assert_eq!(res.uploaded_blobs, 1);
        assert_eq!(res.skipped_blobs, 1);
        assert!(!res.local_mode);
    }
}
