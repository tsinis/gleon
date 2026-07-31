//! Push operation for uploading baseline blobs to remote storage.

use futures::{StreamExt as _, TryStreamExt as _};
use std::collections::BTreeSet;
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
    #[error("Gleon workspace is not initialized. Please run 'gleon init' first.")]
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
        "Missing local blob for hash '{sha256}' referenced in manifest at platform '{platform}'. Please run 'gleon stage' first."
    )]
    MissingLocalBlob {
        /// The SHA256 hex string of the missing blob.
        sha256: String,
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
    /// Indicates whether Gleon executed in Local Flat Mode (no storage configured).
    pub local_mode: bool,
}

/// Helper function to discover valid platform directories under `.gleon/manifests/`.
pub(crate) fn list_platform_dirs(
    manifests_root: &Path,
) -> Result<Vec<(String, PathBuf)>, std::io::Error> {
    if !manifests_root.exists() {
        return Ok(Vec::new());
    }

    let mut platforms = Vec::new();
    for entry in std::fs::read_dir(manifests_root)? {
        let entry = entry?;
        let path = entry.path();
        let is_dir = path.is_dir();
        let valid_name = entry
            .file_name()
            .to_str()
            .filter(|n| is_dir && validate_segment(n).is_ok())
            .map(|n| n.to_string());

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
    if !gleon_dir.exists() {
        return Err(PushError::NotInitialized);
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
    let blobs_dir = gleon_dir.join("blobs").join("sha256");

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

    // Collect all referenced unique sha256 blob hashes and verify local existence (Fail Fast)
    let mut referenced_hashes = BTreeSet::new();

    for (plat_key, plat_dir) in &platform_dirs {
        if !plat_dir.exists() {
            continue;
        }
        let index = match WorkspaceIndex::load(plat_dir) {
            Ok(idx) => idx,
            Err(e) => return Err(PushError::Manifest(e)),
        };
        for manifest in index.entries().values() {
            let hash_str = manifest.hash.value();
            if !referenced_hashes.contains(hash_str) {
                let local_blob_path = blobs_dir.join(hash_str);
                if !local_blob_path.is_file() {
                    return Err(PushError::MissingLocalBlob {
                        sha256: hash_str.to_string(),
                        platform: plat_key.clone(),
                    });
                }
                referenced_hashes.insert(hash_str.to_string());
            }
        }
    }

    let total_manifest_blobs = referenced_hashes.len();
    if total_manifest_blobs == 0 {
        return Ok(PushResult {
            total_manifest_blobs: 0,
            uploaded_blobs: 0,
            skipped_blobs: 0,
            local_mode: false,
        });
    }

    let adapter = ObjectStoreAdapter::from_config(storage_cfg)?;

    let mut missing_blobs = Vec::new();
    let mut skipped_blobs = 0;

    let mut check_stream = futures::stream::iter(referenced_hashes.into_iter().map(|hash| {
        let adapter = adapter.clone();
        async move {
            let exists = adapter
                .blob_exists(&hash)
                .await
                .map_err(PushError::Storage)?;
            Ok::<(String, bool), PushError>((hash, exists))
        }
    }))
    .buffer_unordered(adapter.concurrency());

    while let Some((hash, exists)) = check_stream.try_next().await? {
        if exists {
            skipped_blobs += 1;
        } else {
            missing_blobs.push(hash);
        }
    }

    let missing_count = missing_blobs.len();
    let progress_bar = crate::ui::create_progress_bar(missing_count as u64);

    // Upload missing blobs in parallel with Fail-Fast short-circuiting
    let mut upload_stream = futures::stream::iter(missing_blobs.into_iter().map(|hash| {
        let adapter = adapter.clone();
        let src_path = blobs_dir.join(&hash);
        let pb = progress_bar.clone();
        async move {
            pb.set_message(format!("Uploading {}", &hash[..8.min(hash.len())]));
            let res = adapter
                .upload_blob(&hash, &src_path)
                .await
                .map_err(PushError::Storage);
            pb.inc(1);
            res
        }
    }))
    .buffer_unordered(adapter.concurrency());

    while let Some(()) = upload_stream.try_next().await? {}
    progress_bar.finish_and_clear();

    Ok(PushResult {
        total_manifest_blobs,
        uploaded_blobs: missing_count,
        skipped_blobs,
        local_mode: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::PlatformError;

    #[test]
    fn test_push_error_display() {
        let err1 = PushError::NotInitialized;
        assert!(err1.to_string().contains("not initialized"));

        let err2 = PushError::MissingLocalBlob {
            sha256: "abc".to_string(),
            platform: "macos-aarch64".to_string(),
        };
        assert!(err2.to_string().contains("Missing local blob"));
        assert!(err2.to_string().contains("abc"));

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

        let dirs = list_platform_dirs(&manifests).unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].0, "valid-platform");
    }
}
