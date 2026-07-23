use futures::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, error, info};

use crate::manifest::{Manifest, ManifestIndex};
use crate::storage::StorageError;
use crate::storage::adapter::ObjectStoreAdapter;
use crate::storage::merge::ManifestMerger;

#[derive(Clone)]
pub struct SyncOptions {
    pub concurrency: usize,
    pub retries: usize,
    pub fail_fast: bool,
    pub on_progress: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for SyncOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncOptions")
            .field("concurrency", &self.concurrency)
            .field("retries", &self.retries)
            .field("fail_fast", &self.fail_fast)
            .field("on_progress", &self.on_progress.is_some())
            .finish()
    }
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            concurrency: 10,
            retries: 3,
            fail_fast: true,
            on_progress: None,
        }
    }
}

pub struct SyncOrchestrator {
    pub adapter: Arc<ObjectStoreAdapter>,
    pub workspace_root: PathBuf,
}

impl SyncOrchestrator {
    pub fn new(adapter: Arc<ObjectStoreAdapter>, workspace_root: PathBuf) -> Self {
        Self {
            adapter,
            workspace_root,
        }
    }

    /// Pull remote manifest index, compute delta, and download missing blobs.
    pub async fn pull(
        &self,
        branch: &str,
        platform: &str,
        options: &SyncOptions,
    ) -> Result<(), StorageError> {
        info!(
            "Pulling manifest for branch {} / platform {}",
            branch, platform
        );

        let remote_manifest_bytes = match self.adapter.download_manifest(branch, platform).await {
            Ok(bytes) => bytes,
            Err(StorageError::BlobNotFound(_)) => {
                info!("Remote manifest not found. Nothing to pull.");
                return Ok(());
            }
            Err(e) => return Err(e),
        };

        let remote_index: ManifestIndex =
            serde_json::from_slice(&remote_manifest_bytes).map_err(|e| StorageError::Io {
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
            })?;

        // 1. Collect missing Manifest blobs
        let mut missing_manifest_blobs = Vec::new();
        for hash in remote_index.test_manifests.values() {
            let blob_hash = hash.value().to_string();
            let dest_path = self
                .workspace_root
                .join(".gleon/blobs/sha256")
                .join(&blob_hash);
            if !dest_path.exists() {
                missing_manifest_blobs.push(blob_hash);
            }
        }
        missing_manifest_blobs.sort();
        missing_manifest_blobs.dedup();

        // 2. Download missing Manifest blobs
        self.download_blobs_concurrently(&missing_manifest_blobs, options)
            .await?;

        // 3. Parse the downloaded Manifests to find missing PNG blobs
        let mut missing_png_blobs = Vec::new();
        for hash in remote_index.test_manifests.values() {
            let blob_hash = hash.value();
            let manifest_path = self
                .workspace_root
                .join(".gleon/blobs/sha256")
                .join(blob_hash);
            let manifest = Manifest::load(&manifest_path).map_err(|e| StorageError::Io {
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "Downloaded manifest blob missing or corrupt for hash {blob_hash}: {e}"
                    ),
                ),
            })?;
            for entry in manifest.entries.values() {
                let png_hash = entry.hash.value();
                let png_path = self
                    .workspace_root
                    .join(".gleon/blobs/sha256")
                    .join(png_hash);
                if !png_path.exists() {
                    missing_png_blobs.push(png_hash.to_string());
                }
            }
        }

        missing_png_blobs.sort();
        missing_png_blobs.dedup();

        // 4. Download missing PNG blobs
        self.download_blobs_concurrently(&missing_png_blobs, options)
            .await?;

        // 5. Update local manifest index
        let local_index_path = self
            .workspace_root
            .join(".gleon/branches")
            .join(branch)
            .join(platform)
            .join("manifest_index.json");

        let final_local_index = match ManifestIndex::load(&local_index_path) {
            Ok(local_index) => ManifestMerger::merge_indexes(&remote_index, &local_index),
            Err(_) => remote_index.clone(),
        };

        final_local_index
            .save(&local_index_path)
            .map_err(|e| StorageError::Io {
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
            })?;

        info!("Pull completed successfully.");
        Ok(())
    }

    /// Push local manifest index, merge with remote if present, and upload missing blobs.
    pub async fn push(
        &self,
        branch: &str,
        platform: &str,
        options: &SyncOptions,
    ) -> Result<(), StorageError> {
        info!(
            "Pushing manifest for branch {} / platform {}",
            branch, platform
        );

        let local_index_path = self
            .workspace_root
            .join(".gleon/branches")
            .join(branch)
            .join(platform)
            .join("manifest_index.json");

        let local_index = match ManifestIndex::load(&local_index_path) {
            Ok(index) => index,
            Err(crate::manifest::ManifestError::Io(crate::io::IoError::Io(e)))
                if e.kind() == std::io::ErrorKind::NotFound =>
            {
                info!("No local manifest index found. Nothing to push.");
                return Ok(());
            }
            Err(e) => {
                return Err(StorageError::Io {
                    source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                });
            }
        };

        // 1. Identify all local blobs referenced by the index and its manifests
        let mut blobs_to_upload = Vec::new();

        for hash in local_index.test_manifests.values() {
            let blob_hash = hash.value().to_string();
            blobs_to_upload.push(blob_hash.clone());

            let manifest_path = self
                .workspace_root
                .join(".gleon/blobs/sha256")
                .join(&blob_hash);
            let manifest = Manifest::load(&manifest_path).map_err(|e| StorageError::Io {
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Local manifest missing or corrupt for hash {blob_hash}: {e}"),
                ),
            })?;
            for entry in manifest.entries.values() {
                blobs_to_upload.push(entry.hash.value().to_string());
            }
        }

        blobs_to_upload.sort();
        blobs_to_upload.dedup();

        // Upload blobs concurrently. Skip blobs that already exist on remote (via list_blobs bulk check).
        self.upload_blobs_concurrently(&blobs_to_upload, options)
            .await?;

        // 2. Fetch remote index and merge
        let remote_manifest_bytes_res = self.adapter.download_manifest(branch, platform).await;

        let final_index = match remote_manifest_bytes_res {
            Ok(bytes) => {
                let remote_index: ManifestIndex =
                    serde_json::from_slice(&bytes).map_err(|e| StorageError::Io {
                        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
                    })?;
                ManifestMerger::merge_indexes(&remote_index, &local_index)
            }
            Err(StorageError::BlobNotFound(_)) => local_index,
            Err(e) => return Err(e),
        };

        // 3. Upload the final manifest
        let final_index_bytes =
            serde_json::to_vec_pretty(&final_index).map_err(|e| StorageError::Io {
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
            })?;
        self.adapter
            .upload_manifest(branch, platform, &final_index_bytes)
            .await?;

        info!("Push completed successfully.");
        Ok(())
    }
}

async fn retry_with_backoff<F, Fut>(
    action_name: &str,
    target: &str,
    options: &SyncOptions,
    f: F,
) -> Result<(), StorageError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<(), StorageError>>,
{
    let mut retries = 0;
    loop {
        match f().await {
            Ok(_) => return Ok(()),
            Err(e) => {
                if retries >= options.retries {
                    if options.fail_fast {
                        return Err(e);
                    }
                    error!(
                        "Failed to {} {} after {} retries: {}",
                        action_name, target, retries, e
                    );
                    return Ok(());
                }
                retries += 1;
                debug!(
                    "Retrying {} for {} (attempt {})",
                    action_name, target, retries
                );
                let backoff_ms = 50 * (1 << (retries - 1).min(6));
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
            }
        }
    }
}

impl SyncOrchestrator {
    async fn download_blobs_concurrently(
        &self,
        blobs: &[String],
        options: &SyncOptions,
    ) -> Result<(), StorageError> {
        if blobs.is_empty() {
            return Ok(());
        }

        info!("Downloading {} missing blobs", blobs.len());

        let stream = futures::stream::iter(blobs).map(|hash| async move {
            let dest_path = self.workspace_root.join(".gleon/blobs/sha256").join(hash);
            retry_with_backoff("download", hash, options, || {
                self.adapter.download_blob(hash, &dest_path)
            })
            .await
        });

        let mut buffered = stream.buffer_unordered(options.concurrency);
        while let Some(result) = buffered.next().await {
            result?; // Return on first error if fail_fast
            if let Some(cb) = &options.on_progress {
                cb();
            }
        }

        Ok(())
    }

    async fn upload_blobs_concurrently(
        &self,
        blobs: &[String],
        options: &SyncOptions,
    ) -> Result<(), StorageError> {
        if blobs.is_empty() {
            return Ok(());
        }

        info!("Uploading {} blob(s)", blobs.len());

        let stream = futures::stream::iter(blobs).map(|hash| async move {
            let src_path = self.workspace_root.join(".gleon/blobs/sha256").join(hash);
            if !src_path.exists() {
                return Err(StorageError::Io {
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("Local blob missing for upload: {hash}"),
                    ),
                });
            }

            retry_with_backoff("upload", hash, options, || async {
                if self.adapter.blob_exists(hash).await? {
                    debug!("Blob {} already exists on remote, skipping upload.", hash);
                    Ok(())
                } else {
                    self.adapter.upload_blob(hash, &src_path).await
                }
            })
            .await
        });

        let mut buffered = stream.buffer_unordered(options.concurrency);
        while let Some(result) = buffered.next().await {
            result?;
            if let Some(cb) = &options.on_progress {
                cb();
            }
        }

        Ok(())
    }
}
