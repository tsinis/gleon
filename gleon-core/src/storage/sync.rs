use futures::StreamExt;
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, error, info};

use crate::manifest::{
    ImageHash, Manifest, ManifestIndexPointer, ManifestIndexRevision,
    SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION, TestManifestState,
};
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

    /// Pull the remote manifest-index pointer and its immutable revision.
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

        let remote_pointer = match self.adapter.download_manifest(branch, platform).await {
            Ok(bytes) => Self::parse_pointer(&bytes)?,
            Err(StorageError::BlobNotFound(_)) => {
                info!("Remote manifest pointer not found. Nothing to pull.");
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let remote_revision = self
            .resolve_remote_revision(&remote_pointer, options)
            .await?;

        let (final_pointer, final_revision, merged) =
            match self.load_local_revision(branch, platform)? {
                Some((local_pointer, local_revision))
                    if local_pointer.revision_hash != remote_pointer.revision_hash =>
                {
                    if self
                        .is_revision_ancestor(
                            &local_pointer.revision_hash,
                            &remote_pointer.revision_hash,
                            &remote_revision,
                            options,
                        )
                        .await?
                    {
                        (remote_pointer, remote_revision, false)
                    } else if self
                        .is_revision_ancestor(
                            &remote_pointer.revision_hash,
                            &local_pointer.revision_hash,
                            &local_revision,
                            options,
                        )
                        .await?
                    {
                        (local_pointer, local_revision, false)
                    } else {
                        let revision = self
                            .merge_revisions_and_manifests(
                                &remote_revision,
                                &local_revision,
                                &remote_pointer.revision_hash,
                                &local_pointer.revision_hash,
                                options,
                            )
                            .await?;
                        let pointer = self.persist_revision(&revision)?;
                        (pointer, revision, true)
                    }
                }
                _ => (remote_pointer, remote_revision, false),
            };

        self.download_revision_tree(&final_revision, options)
            .await?;
        if merged {
            self.upload_blob_if_missing(
                final_pointer.revision_hash.value(),
                &self.blob_path(&final_pointer.revision_hash),
                options,
            )
            .await?;
        }
        self.save_local_pointer(branch, platform, &final_pointer)?;

        info!("Pull completed successfully.");
        Ok(())
    }

    /// Push local immutable revisions and conditionally advance the remote pointer.
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

        let Some((local_pointer, local_revision)) = self.load_local_revision(branch, platform)?
        else {
            info!("No local manifest pointer found. Nothing to push.");
            return Ok(());
        };

        for attempt in 0..=options.retries {
            let remote = match self
                .adapter
                .download_manifest_pointer(branch, platform)
                .await
            {
                Ok(pointer) => Some((Self::parse_pointer(&pointer.bytes)?, pointer.version)),
                Err(StorageError::BlobNotFound(_)) => None,
                Err(error) => return Err(error),
            };

            if remote
                .as_ref()
                .is_some_and(|(pointer, _)| pointer.revision_hash == local_pointer.revision_hash)
            {
                info!("Remote manifest pointer already matches local head. Nothing to push.");
                return Ok(());
            }

            let (final_pointer, final_revision) = match &remote {
                Some((remote_pointer, _))
                    if remote_pointer.revision_hash != local_pointer.revision_hash =>
                {
                    let remote_revision = self
                        .resolve_remote_revision(remote_pointer, options)
                        .await?;
                    if self
                        .is_revision_ancestor(
                            &local_pointer.revision_hash,
                            &remote_pointer.revision_hash,
                            &remote_revision,
                            options,
                        )
                        .await?
                    {
                        // The remote head already contains the local head. Do not rewrite its
                        // pointer; materialize it locally instead.
                        self.download_revision_tree(&remote_revision, options)
                            .await?;
                        self.save_local_pointer(branch, platform, remote_pointer)?;
                        info!("Push fast-forwarded local pointer to remote head.");
                        return Ok(());
                    }
                    if self
                        .is_revision_ancestor(
                            &remote_pointer.revision_hash,
                            &local_pointer.revision_hash,
                            &local_revision,
                            options,
                        )
                        .await?
                    {
                        (local_pointer.clone(), local_revision.clone())
                    } else {
                        let revision = self
                            .merge_revisions_and_manifests(
                                &remote_revision,
                                &local_revision,
                                &remote_pointer.revision_hash,
                                &local_pointer.revision_hash,
                                options,
                            )
                            .await?;
                        let pointer = self.persist_revision(&revision)?;
                        (pointer, revision)
                    }
                }
                Some((remote_pointer, _)) => (remote_pointer.clone(), local_revision.clone()),
                None => (local_pointer.clone(), local_revision.clone()),
            };

            self.upload_revision_tree(&final_pointer.revision_hash, &final_revision, options)
                .await?;
            let pointer_bytes = Self::serialize_pointer(&final_pointer)?;
            let write_result = match remote {
                Some((_, version)) => {
                    self.adapter
                        .update_manifest(branch, platform, &pointer_bytes, version)
                        .await
                }
                None => {
                    self.adapter
                        .create_manifest(branch, platform, &pointer_bytes)
                        .await
                }
            };

            match write_result {
                Ok(_) => {
                    self.save_local_pointer(branch, platform, &final_pointer)?;
                    info!("Push completed successfully.");
                    return Ok(());
                }
                Err(StorageError::Conflict { .. }) if attempt < options.retries => {
                    debug!(
                        "Manifest pointer changed while pushing; retrying conditional update (attempt {})",
                        attempt + 1
                    );
                }
                Err(error) => return Err(error),
            }
        }

        unreachable!("the bounded OCC loop always returns on its final attempt")
    }

    fn pointer_path(&self, branch: &str, platform: &str) -> PathBuf {
        self.workspace_root
            .join(".gleon/branches")
            .join(branch)
            .join(platform)
            .join("manifest_index.json")
    }

    fn blob_path(&self, hash: &ImageHash) -> PathBuf {
        self.workspace_root
            .join(".gleon/blobs")
            .join(hash.scheme())
            .join(hash.value())
    }

    fn load_local_revision(
        &self,
        branch: &str,
        platform: &str,
    ) -> Result<Option<(ManifestIndexPointer, ManifestIndexRevision)>, StorageError> {
        let pointer_path = self.pointer_path(branch, platform);
        if !pointer_path.exists() {
            return Ok(None);
        }
        let pointer = ManifestIndexPointer::load(&pointer_path).map_err(Self::manifest_error)?;
        let revision = ManifestIndexRevision::load(self.blob_path(&pointer.revision_hash))
            .map_err(Self::manifest_error)?;
        Ok(Some((pointer, revision)))
    }

    fn save_local_pointer(
        &self,
        branch: &str,
        platform: &str,
        pointer: &ManifestIndexPointer,
    ) -> Result<(), StorageError> {
        pointer
            .save(self.pointer_path(branch, platform))
            .map_err(Self::manifest_error)
    }

    fn parse_pointer(bytes: &[u8]) -> Result<ManifestIndexPointer, StorageError> {
        let pointer: ManifestIndexPointer =
            serde_json::from_slice(bytes).map_err(|error| StorageError::Io {
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, error),
            })?;
        pointer.validate().map_err(Self::manifest_error)?;
        Ok(pointer)
    }

    fn serialize_pointer(pointer: &ManifestIndexPointer) -> Result<Vec<u8>, StorageError> {
        pointer.validate().map_err(Self::manifest_error)?;
        serde_json::to_vec_pretty(pointer).map_err(|error| StorageError::Io {
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, error),
        })
    }

    fn manifest_error(error: crate::manifest::ManifestError) -> StorageError {
        StorageError::Io {
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, error),
        }
    }

    fn persist_revision(
        &self,
        revision: &ManifestIndexRevision,
    ) -> Result<ManifestIndexPointer, StorageError> {
        revision.validate().map_err(Self::manifest_error)?;
        let revision_bytes =
            serde_json::to_vec_pretty(revision).map_err(|error| StorageError::Io {
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, error),
            })?;
        use sha2::Digest;
        let hash = hex::encode(sha2::Sha256::digest(&revision_bytes));
        let revision_hash = ImageHash::new("sha256", hash).map_err(Self::manifest_error)?;
        let revision_path = self.blob_path(&revision_hash);
        if !revision_path.exists() {
            crate::io::save_file_atomically(&revision_path, &revision_bytes).map_err(|error| {
                StorageError::Io {
                    source: std::io::Error::other(error.to_string()),
                }
            })?;
        }
        Ok(ManifestIndexPointer {
            schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
            revision_hash,
        })
    }

    async fn resolve_remote_revision(
        &self,
        pointer: &ManifestIndexPointer,
        options: &SyncOptions,
    ) -> Result<ManifestIndexRevision, StorageError> {
        let revision_path = self.blob_path(&pointer.revision_hash);
        if !revision_path.exists() {
            retry_with_backoff(
                "download_manifest_revision",
                pointer.revision_hash.value(),
                options,
                || {
                    self.adapter
                        .download_blob(pointer.revision_hash.value(), &revision_path)
                },
            )
            .await?;
        }
        ManifestIndexRevision::load(revision_path).map_err(Self::manifest_error)
    }

    async fn is_revision_ancestor(
        &self,
        ancestor_hash: &ImageHash,
        descendant_hash: &ImageHash,
        descendant_revision: &ManifestIndexRevision,
        options: &SyncOptions,
    ) -> Result<bool, StorageError> {
        if ancestor_hash == descendant_hash {
            return Ok(true);
        }

        let mut pending = vec![(descendant_hash.clone(), descendant_revision.clone())];
        let mut visited = BTreeSet::new();
        while let Some((revision_hash, revision)) = pending.pop() {
            if !visited.insert(revision_hash) {
                continue;
            }
            for parent_hash in revision.parent_hashes {
                if &parent_hash == ancestor_hash {
                    return Ok(true);
                }
                self.ensure_blob_downloaded(&parent_hash, options).await?;
                let parent_revision = ManifestIndexRevision::load(self.blob_path(&parent_hash))
                    .map_err(Self::manifest_error)?;
                pending.push((parent_hash, parent_revision));
            }
        }
        Ok(false)
    }

    async fn merge_revisions_and_manifests(
        &self,
        remote_revision: &ManifestIndexRevision,
        local_revision: &ManifestIndexRevision,
        remote_revision_hash: &ImageHash,
        local_revision_hash: &ImageHash,
        options: &SyncOptions,
    ) -> Result<ManifestIndexRevision, StorageError> {
        let index_base = self
            .resolve_revision_lca(
                remote_revision_hash,
                remote_revision,
                local_revision_hash,
                local_revision,
                options,
            )
            .await?;
        let mut merged = if let Some(base) = index_base {
            let mut merged = remote_revision.clone();
            let test_names = remote_revision
                .test_manifests
                .keys()
                .chain(local_revision.test_manifests.keys())
                .chain(base.test_manifests.keys())
                .collect::<BTreeSet<_>>();
            for test_name in test_names {
                let local_state = local_revision.test_manifests.get(test_name);
                let selected = if local_state == base.test_manifests.get(test_name) {
                    remote_revision.test_manifests.get(test_name)
                } else {
                    local_state
                };
                match selected {
                    Some(state) => {
                        merged
                            .test_manifests
                            .insert(test_name.clone(), state.clone());
                    }
                    None => {
                        merged.test_manifests.remove(test_name);
                    }
                }
            }
            merged
        } else {
            ManifestMerger::merge_index_revisions(remote_revision, local_revision)
        };
        let mut parent_hashes = BTreeSet::new();
        parent_hashes.insert(remote_revision_hash.clone());
        parent_hashes.insert(local_revision_hash.clone());
        merged.parent_hashes = parent_hashes.into_iter().collect();

        for (test_name, local_state) in &local_revision.test_manifests {
            let (
                TestManifestState::Present(local_hash),
                Some(TestManifestState::Present(remote_hash)),
            ) = (local_state, remote_revision.test_manifests.get(test_name))
            else {
                continue;
            };
            if local_hash == remote_hash {
                continue;
            }

            let local_manifest = self.load_manifest(local_hash, test_name)?;
            self.ensure_blob_downloaded(remote_hash, options).await?;
            let remote_manifest = self.load_manifest(remote_hash, test_name)?;
            let base_manifest = self
                .resolve_manifest_lca(remote_hash, local_hash, test_name, options)
                .await?;
            let mut merged_manifest = match base_manifest {
                Some(base) => ManifestMerger::merge_manifests_three_way(
                    &remote_manifest,
                    &local_manifest,
                    &base,
                ),
                None => ManifestMerger::merge_manifests(&remote_manifest, &local_manifest),
            }
            .map_err(|error| StorageError::Io {
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, error),
            })?;
            let mut manifest_parent_hashes = BTreeSet::new();
            manifest_parent_hashes.insert(remote_hash.clone());
            manifest_parent_hashes.insert(local_hash.clone());
            merged_manifest.parent_hashes = manifest_parent_hashes.into_iter().collect();
            let merged_hash = self.persist_manifest(&merged_manifest)?;
            self.upload_blob_if_missing(
                merged_hash.value(),
                &self.blob_path(&merged_hash),
                options,
            )
            .await?;
            merged
                .test_manifests
                .insert(test_name.clone(), TestManifestState::Present(merged_hash));
        }

        Ok(merged)
    }

    /// Finds a common index revision ancestor by walking both CAS parent DAGs.
    async fn resolve_revision_lca(
        &self,
        remote_hash: &ImageHash,
        remote_revision: &ManifestIndexRevision,
        local_hash: &ImageHash,
        local_revision: &ManifestIndexRevision,
        options: &SyncOptions,
    ) -> Result<Option<ManifestIndexRevision>, StorageError> {
        let mut local_ancestors = BTreeSet::new();
        let mut pending = VecDeque::from([(local_hash.clone(), local_revision.clone())]);
        while let Some((hash, revision)) = pending.pop_front() {
            if !local_ancestors.insert(hash.clone()) {
                continue;
            }
            for parent_hash in revision.parent_hashes {
                self.ensure_blob_downloaded(&parent_hash, options).await?;
                let parent = ManifestIndexRevision::load(self.blob_path(&parent_hash))
                    .map_err(Self::manifest_error)?;
                pending.push_back((parent_hash, parent));
            }
        }

        let mut pending = VecDeque::from([(remote_hash.clone(), remote_revision.clone())]);
        let mut visited = BTreeSet::new();
        while let Some((hash, revision)) = pending.pop_front() {
            if !visited.insert(hash.clone()) {
                continue;
            }
            if local_ancestors.contains(&hash) {
                return Ok(Some(revision));
            }
            for parent_hash in revision.parent_hashes {
                self.ensure_blob_downloaded(&parent_hash, options).await?;
                let parent = ManifestIndexRevision::load(self.blob_path(&parent_hash))
                    .map_err(Self::manifest_error)?;
                pending.push_back((parent_hash, parent));
            }
        }
        Ok(None)
    }

    /// Finds a common manifest ancestor by walking both CAS parent DAGs.
    async fn resolve_manifest_lca(
        &self,
        remote_hash: &ImageHash,
        local_hash: &ImageHash,
        test_name: &str,
        options: &SyncOptions,
    ) -> Result<Option<Manifest>, StorageError> {
        let mut local_ancestors = BTreeSet::new();
        let mut pending = VecDeque::from([local_hash.clone()]);
        while let Some(hash) = pending.pop_front() {
            if !local_ancestors.insert(hash.clone()) {
                continue;
            }
            self.ensure_blob_downloaded(&hash, options).await?;
            let manifest = self.load_manifest(&hash, test_name)?;
            pending.extend(manifest.parent_hashes);
        }

        let mut pending = VecDeque::from([remote_hash.clone()]);
        let mut visited = BTreeSet::new();
        while let Some(hash) = pending.pop_front() {
            if !visited.insert(hash.clone()) {
                continue;
            }
            self.ensure_blob_downloaded(&hash, options).await?;
            let manifest = self.load_manifest(&hash, test_name)?;
            if local_ancestors.contains(&hash) {
                return Ok(Some(manifest));
            }
            pending.extend(manifest.parent_hashes);
        }

        Ok(None)
    }

    fn load_manifest(&self, hash: &ImageHash, test_name: &str) -> Result<Manifest, StorageError> {
        Manifest::load(self.blob_path(hash)).map_err(|error| StorageError::Io {
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Failed to load manifest for {test_name}: {error}"),
            ),
        })
    }

    fn persist_manifest(&self, manifest: &Manifest) -> Result<ImageHash, StorageError> {
        manifest.validate().map_err(Self::manifest_error)?;
        let bytes = serde_json::to_vec_pretty(manifest).map_err(|error| StorageError::Io {
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, error),
        })?;
        use sha2::Digest;
        let hash = ImageHash::new("sha256", hex::encode(sha2::Sha256::digest(&bytes)))
            .map_err(Self::manifest_error)?;
        let path = self.blob_path(&hash);
        if !path.exists() {
            crate::io::save_file_atomically(path, &bytes).map_err(|error| StorageError::Io {
                source: std::io::Error::other(error.to_string()),
            })?;
        }
        Ok(hash)
    }

    async fn ensure_blob_downloaded(
        &self,
        hash: &ImageHash,
        options: &SyncOptions,
    ) -> Result<(), StorageError> {
        let path = self.blob_path(hash);
        if path.exists() {
            return Ok(());
        }
        retry_with_backoff("download", hash.value(), options, || {
            self.adapter.download_blob(hash.value(), &path)
        })
        .await
    }

    async fn download_revision_tree(
        &self,
        revision: &ManifestIndexRevision,
        options: &SyncOptions,
    ) -> Result<(), StorageError> {
        let manifests: Vec<_> = revision
            .test_manifests
            .values()
            .filter_map(|state| match state {
                TestManifestState::Present(hash) => Some(hash.clone()),
                TestManifestState::Deleted => None,
            })
            .collect();
        let missing_manifests: Vec<_> = manifests
            .iter()
            .filter(|hash| !self.blob_path(hash).exists())
            .map(|hash| hash.value().to_string())
            .collect();
        self.download_blobs_concurrently(&missing_manifests, options)
            .await?;

        let mut missing_images = BTreeSet::new();
        for hash in &manifests {
            let manifest = self.load_manifest(hash, "remote")?;
            missing_images.extend(
                manifest
                    .entries
                    .values()
                    .map(|entry| entry.hash.value().to_string())
                    .filter(|hash| {
                        !self
                            .workspace_root
                            .join(".gleon/blobs/sha256")
                            .join(hash)
                            .exists()
                    }),
            );
        }
        self.download_blobs_concurrently(&missing_images.into_iter().collect::<Vec<_>>(), options)
            .await?;
        Ok(())
    }

    async fn upload_revision_tree(
        &self,
        revision_hash: &ImageHash,
        revision: &ManifestIndexRevision,
        options: &SyncOptions,
    ) -> Result<(), StorageError> {
        let mut blobs = BTreeSet::new();
        let mut revisions = VecDeque::from([(revision_hash.clone(), revision.clone())]);
        let mut visited_revisions = BTreeSet::new();
        let mut manifests = VecDeque::new();
        let mut visited_manifests = BTreeSet::new();

        while let Some((hash, revision)) = revisions.pop_front() {
            if !visited_revisions.insert(hash.clone()) {
                continue;
            }
            blobs.insert(hash.value().to_string());
            for parent_hash in revision.parent_hashes {
                self.ensure_blob_downloaded(&parent_hash, options).await?;
                let parent = ManifestIndexRevision::load(self.blob_path(&parent_hash))
                    .map_err(Self::manifest_error)?;
                revisions.push_back((parent_hash, parent));
            }
            manifests.extend(
                revision
                    .test_manifests
                    .values()
                    .filter_map(|state| match state {
                        TestManifestState::Present(hash) => Some(hash.clone()),
                        TestManifestState::Deleted => None,
                    }),
            );
        }

        while let Some(hash) = manifests.pop_front() {
            if !visited_manifests.insert(hash.clone()) {
                continue;
            }
            blobs.insert(hash.value().to_string());
            self.ensure_blob_downloaded(&hash, options).await?;
            let manifest = self.load_manifest(&hash, "local")?;
            manifests.extend(manifest.parent_hashes);
            blobs.extend(
                manifest
                    .entries
                    .values()
                    .map(|entry| entry.hash.value().to_string()),
            );
        }

        self.upload_blobs_concurrently(&blobs.into_iter().collect::<Vec<_>>(), options)
            .await
    }

    async fn upload_blob_if_missing(
        &self,
        hash: &str,
        path: &Path,
        options: &SyncOptions,
    ) -> Result<(), StorageError> {
        retry_upload_with_backoff("upload", hash, options, || async {
            if self.adapter.blob_exists(hash).await? {
                Ok(())
            } else {
                self.adapter.upload_blob(hash, path).await
            }
        })
        .await
    }
}

/// Retries a reachable upload without allowing `fail_fast = false` to hide a
/// failure: advancing a pointer after such a failure would publish a broken tree.
async fn retry_upload_with_backoff<F, Fut>(
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
            Ok(()) => return Ok(()),
            Err(error) => {
                if matches!(
                    error,
                    StorageError::BlobNotFound(_) | StorageError::InvalidUrl { .. }
                ) || retries >= options.retries
                {
                    return Err(error);
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
                // Permanent errors should not be retried
                if matches!(
                    e,
                    StorageError::BlobNotFound(_) | StorageError::InvalidUrl { .. }
                ) {
                    return Err(e);
                }

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
            result?;
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
            self.upload_blob_if_missing(hash, &src_path, options).await
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

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::storage::StorageError;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_retry_with_backoff_permanent_error() {
        let options = SyncOptions {
            concurrency: 1,
            retries: 3,
            fail_fast: true,
            on_progress: None,
        };
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();

        let result = retry_with_backoff("test_action", "target", &options, || async {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            Err(StorageError::BlobNotFound("hash".to_string()))
        })
        .await;

        assert!(matches!(result, Err(StorageError::BlobNotFound(_))));
        // Should fail immediately on the first attempt without retrying
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_with_backoff_transient_error_success() {
        let options = SyncOptions {
            concurrency: 1,
            retries: 3,
            fail_fast: true,
            on_progress: None,
        };
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_clone = attempts.clone();

        let result = retry_with_backoff("test_action", "target", &options, || async {
            let count = attempts_clone.fetch_add(1, Ordering::SeqCst);
            if count < 2 {
                Err(StorageError::Io {
                    source: std::io::Error::new(std::io::ErrorKind::ConnectionReset, "transient"),
                })
            } else {
                Ok(())
            }
        })
        .await;

        assert!(result.is_ok());
        // Succeeded on the 3rd attempt (index 2)
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_with_backoff_fail_fast_false() {
        let options = SyncOptions {
            concurrency: 1,
            retries: 1,
            fail_fast: false,
            on_progress: None,
        };

        let result = retry_with_backoff("test_action", "target", &options, || async {
            Err(StorageError::Io {
                source: std::io::Error::new(std::io::ErrorKind::ConnectionReset, "transient"),
            })
        })
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_download_and_upload_blobs_progress_and_empty() {
        let options = SyncOptions {
            concurrency: 2,
            retries: 1,
            fail_fast: true,
            on_progress: Some(Arc::new(|| {})),
        };

        let dir = tempfile::tempdir().unwrap();
        let adapter = Arc::new(
            ObjectStoreAdapter::from_config(&crate::storage::StorageConfig::new("memory://"))
                .unwrap(),
        );
        let orchestrator = SyncOrchestrator::new(adapter, dir.path().to_path_buf());

        assert!(
            orchestrator
                .download_blobs_concurrently(&[], &options)
                .await
                .is_ok()
        );
        assert!(
            orchestrator
                .upload_blobs_concurrently(&[], &options)
                .await
                .is_ok()
        );
    }
}
