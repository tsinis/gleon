//! Pull operation for downloading missing baseline blobs from remote storage.

use futures::StreamExt as _;
use std::collections::BTreeSet;
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

    /// Missing remote blob on Object Store.
    #[error(
        "Missing remote blob for hash '{sha256}' referenced in manifest at platform '{platform}'. Blob was not found in remote storage."
    )]
    MissingRemoteBlob {
        /// The SHA256 hex string of the missing blob.
        sha256: String,
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
    /// Indicates whether Gleon executed in Local Flat Mode (no storage configured).
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
    let blobs_dir = gleon_dir.join("blobs").join("sha256");

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
        vec![(platform_key.clone(), manifests_root.join(platform_key))]
    };

    // Collect all referenced unique sha256 blob hashes and identify missing local ones
    let mut referenced_hashes = BTreeSet::new();
    let mut missing_local = Vec::new();
    let mut skipped_blobs = 0;

    for (plat_key, plat_dir) in &platform_dirs {
        if !plat_dir.exists() {
            continue;
        }
        let index = match WorkspaceIndex::load(plat_dir) {
            Ok(idx) => idx,
            Err(e) => return Err(PullError::Manifest(e)),
        };
        for manifest in index.entries().values() {
            let hash_str = manifest.hash.value();
            if !referenced_hashes.contains(hash_str) {
                referenced_hashes.insert(hash_str.to_string());
                let local_blob_path = blobs_dir.join(hash_str);
                if local_blob_path.is_file() {
                    skipped_blobs += 1;
                } else {
                    missing_local.push((hash_str.to_string(), plat_key.clone()));
                }
            }
        }
    }

    let total_manifest_blobs = referenced_hashes.len();
    if missing_local.is_empty() {
        return Ok(PullResult {
            total_manifest_blobs,
            downloaded_blobs: 0,
            skipped_blobs,
            local_mode: false,
        });
    }

    let adapter = ObjectStoreAdapter::from_config(storage_cfg)?;

    let missing_count = missing_local.len();

    // Download missing blobs in parallel (Fail Fast on BlobNotFound or Store Error)
    let download_stream = futures::stream::iter(missing_local.into_iter().map(|(hash, plat)| {
        let adapter = adapter.clone();
        let dest_path = blobs_dir.join(&hash);
        async move {
            match adapter.download_blob(&hash, &dest_path).await {
                Ok(()) => Ok(()),
                Err(StorageError::BlobNotFound(_)) => Err(PullError::MissingRemoteBlob {
                    sha256: hash,
                    platform: plat,
                }),
                Err(e) => Err(PullError::Storage(e)),
            }
        }
    }))
    .buffer_unordered(adapter.concurrency());

    let results: Vec<Result<(), PullError>> = download_stream.collect().await;
    for res in results {
        res?;
    }

    Ok(PullResult {
        total_manifest_blobs,
        downloaded_blobs: missing_count,
        skipped_blobs,
        local_mode: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::PlatformError;

    #[test]
    fn test_pull_error_display() {
        let err1 = PullError::NotInitialized;
        assert!(err1.to_string().contains("not initialized"));

        let err2 = PullError::MissingRemoteBlob {
            sha256: "xyz".to_string(),
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
}
