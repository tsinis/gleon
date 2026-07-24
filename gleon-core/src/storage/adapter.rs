//! Object store storage adapter implementing baseline and blob synchronization.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;

use futures::StreamExt as _;
use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, ObjectStoreExt, parse_url_opts};
use tempfile::NamedTempFile;
use tracing::{debug, instrument};

use super::{StorageError, blob_key, manifest_key};

/// Configuration for storage initialization and authentication credentials.
#[derive(Clone, PartialEq, Eq)]
pub struct StorageConfig {
    /// Remote storage URL (e.g., `s3://my-bucket/gleon`, `file:///path/to/store`, `memory://`).
    pub url: String,

    /// AWS or S3-compatible Access Key ID.
    pub aws_access_key_id: Option<String>,

    /// AWS or S3-compatible Secret Access Key.
    pub aws_secret_access_key: Option<String>,

    /// AWS region (defaults to `auto` for Cloudflare R2).
    pub aws_region: Option<String>,

    /// Custom AWS / S3 endpoint URL.
    pub aws_endpoint: Option<String>,

    /// Cloudflare R2 Account ID (used to construct R2 endpoint if endpoint is not set).
    pub r2_account_id: Option<String>,

    /// Concurrency limit for parallel transfer operations.
    pub concurrency: usize,
}

impl StorageConfig {
    /// Constructs a `StorageConfig` with standard defaults.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            aws_access_key_id: None,
            aws_secret_access_key: None,
            aws_region: None,
            aws_endpoint: None,
            r2_account_id: None,
            concurrency: 8,
        }
    }
}

impl fmt::Debug for StorageConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let masked_url = match url::Url::parse(&self.url) {
            Ok(mut parsed) => {
                if parsed.password().is_some() {
                    let _ = parsed.set_password(Some("[REDACTED]"));
                }
                parsed.to_string()
            }
            Err(_) => self.url.clone(),
        };

        f.debug_struct("StorageConfig")
            .field("url", &masked_url)
            .field(
                "aws_access_key_id",
                &self.aws_access_key_id.as_ref().map(|_| "[PRESENT]"),
            )
            .field(
                "aws_secret_access_key",
                &self.aws_secret_access_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("aws_region", &self.aws_region)
            .field("aws_endpoint", &self.aws_endpoint)
            .field("r2_account_id", &self.r2_account_id)
            .field("concurrency", &self.concurrency)
            .finish()
    }
}

/// Unified storage adapter backing baseline and blob operations via `object_store`.
#[derive(Clone)]
pub struct ObjectStoreAdapter {
    store: Arc<dyn ObjectStore>,
    concurrency: usize,
}

impl ObjectStoreAdapter {
    /// Instantiates an `ObjectStoreAdapter` from a `StorageConfig`.
    ///
    /// # Errors
    /// Returns [`StorageError::InvalidUrl`] if the URL or parameters cannot be parsed by `object_store`.
    #[instrument(skip(config), level = "debug")]
    pub fn from_config(config: &StorageConfig) -> Result<Self, StorageError> {
        let mut opts = BTreeMap::new();

        if let Some(key_id) = &config.aws_access_key_id {
            let _ = opts.insert("aws_access_key_id".to_string(), key_id.clone());
        }
        if let Some(secret) = &config.aws_secret_access_key {
            let _ = opts.insert("aws_secret_access_key".to_string(), secret.clone());
        }

        if let Some(region) = &config.aws_region {
            let _ = opts.insert("aws_region".to_string(), region.clone());
        } else if config.r2_account_id.is_some() {
            let _ = opts.insert("aws_region".to_string(), "auto".to_string());
        }

        if let Some(endpoint) = &config.aws_endpoint {
            let _ = opts.insert("aws_endpoint".to_string(), endpoint.clone());
        } else if let Some(account_id) = &config.r2_account_id {
            let r2_endpoint = format!("https://{account_id}.r2.cloudflarestorage.com");
            let _ = opts.insert("aws_endpoint".to_string(), r2_endpoint);
        }

        let url = url::Url::parse(&config.url).map_err(|e| StorageError::InvalidUrl {
            url: config.url.clone(),
            reason: e.to_string(),
        })?;

        let (store, path) = parse_url_opts(&url, opts).map_err(|e| StorageError::InvalidUrl {
            url: config.url.clone(),
            reason: e.to_string(),
        })?;

        let store: Arc<dyn ObjectStore> = if path.as_ref().is_empty() {
            Arc::from(store)
        } else {
            Arc::new(object_store::prefix::PrefixStore::new(store, path))
        };

        Ok(Self {
            store,
            concurrency: std::cmp::max(1, config.concurrency),
        })
    }

    /// Returns the concurrency limit configured for this adapter.
    #[must_use]
    pub const fn concurrency(&self) -> usize {
        self.concurrency
    }

    /// Uploads a single blob from disk to remote storage at `blobs/sha256/<hash>`.
    ///
    /// # Errors
    /// Returns [`StorageError`] if the local file cannot be read or remote upload fails.
    #[instrument(skip(self, src_path), level = "debug")]
    pub async fn upload_blob(&self, sha256: &str, src_path: &Path) -> Result<(), StorageError> {
        let key = blob_key(sha256);
        let bytes = tokio::fs::read(src_path)
            .await
            .map_err(|source| StorageError::Io { source })?;
        self.store
            .put(&key, object_store::PutPayload::from(bytes))
            .await
            .map_err(|source| StorageError::Store { source })?;
        debug!(sha256 = %sha256, "Successfully uploaded blob to remote storage");
        Ok(())
    }

    /// Checks if a blob exists on remote storage without downloading it.
    pub async fn blob_exists(&self, sha256: &str) -> Result<bool, StorageError> {
        let key = blob_key(sha256);
        match self.store.head(&key).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(StorageError::Store { source: e }),
        }
    }

    /// Downloads a single blob from remote storage at `blobs/sha256/<hash>` to `dest_path` atomically.
    ///
    /// # Errors
    /// Returns [`StorageError::BlobNotFound`] if the hash does not exist on remote storage,
    /// or [`StorageError::Io`] / [`StorageError::PersistFailed`] if atomic write fails.
    #[instrument(skip(self, dest_path), level = "debug")]
    pub async fn download_blob(&self, sha256: &str, dest_path: &Path) -> Result<(), StorageError> {
        let key = blob_key(sha256);

        let get_result = self.store.get(&key).await;
        let get_output = match get_result {
            Ok(output) => output,
            Err(object_store::Error::NotFound { .. }) => {
                return Err(StorageError::BlobNotFound(sha256.to_string()));
            }
            Err(err) => return Err(StorageError::Store { source: err }),
        };

        let bytes = get_output
            .bytes()
            .await
            .map_err(|source| StorageError::Store { source })?;

        let dest_path_buf = dest_path.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<(), StorageError> {
            let parent_dir = dest_path_buf.parent().unwrap_or_else(|| Path::new("."));

            std::fs::create_dir_all(parent_dir).map_err(|source| StorageError::Io { source })?;

            let mut temp_file =
                NamedTempFile::new_in(parent_dir).map_err(|source| StorageError::Io { source })?;
            temp_file
                .write_all(&bytes)
                .map_err(|source| StorageError::Io { source })?;
            temp_file
                .as_file()
                .sync_all()
                .map_err(|source| StorageError::Io { source })?;
            temp_file
                .persist(&dest_path_buf)
                .map_err(|e| StorageError::PersistFailed {
                    path: dest_path_buf.display().to_string(),
                    source: e,
                })?;

            if let Ok(dir_file) = std::fs::File::open(parent_dir) {
                let _ = dir_file.sync_all();
            }
            Ok(())
        })
        .await
        .map_err(|e| StorageError::Io {
            source: std::io::Error::other(e),
        })??;

        debug!(sha256 = %sha256, path = %dest_path.display(), "Successfully downloaded blob from remote storage");
        Ok(())
    }

    /// Uploads a manifest index JSON buffer to `branches/<branch>/<platform>/manifest_index.json`.
    ///
    /// # Errors
    /// Returns [`StorageError`] if upload to remote storage fails.
    #[instrument(skip(self, bytes), level = "debug")]
    pub async fn upload_manifest(
        &self,
        branch: &str,
        platform: &str,
        bytes: &[u8],
    ) -> Result<(), StorageError> {
        let key = manifest_key(branch, platform);
        self.store
            .put(&key, object_store::PutPayload::from(bytes.to_vec()))
            .await
            .map_err(|source| StorageError::Store { source })?;
        debug!(branch = %branch, platform = %platform, "Successfully uploaded manifest to remote storage");
        Ok(())
    }

    /// Downloads a manifest index JSON buffer from `branches/<branch>/<platform>/manifest_index.json`.
    ///
    /// # Errors
    /// Returns [`StorageError::BlobNotFound`] if manifest does not exist on remote storage,
    /// or [`StorageError::Store`] on download failure.
    #[instrument(skip(self), level = "debug")]
    pub async fn download_manifest(
        &self,
        branch: &str,
        platform: &str,
    ) -> Result<Vec<u8>, StorageError> {
        let key = manifest_key(branch, platform);
        let get_result = self.store.get(&key).await;
        let get_output = match get_result {
            Ok(output) => output,
            Err(object_store::Error::NotFound { .. }) => {
                return Err(StorageError::BlobNotFound(format!(
                    "{branch}/{platform}/manifest_index.json"
                )));
            }
            Err(err) => return Err(StorageError::Store { source: err }),
        };

        let bytes = get_output
            .bytes()
            .await
            .map_err(|source| StorageError::Store { source })?;
        debug!(branch = %branch, platform = %platform, "Successfully downloaded manifest from remote storage");
        Ok(bytes.to_vec())
    }

    /// Lists all SHA256 blob hashes existing under the remote `blobs/sha256/` prefix.
    ///
    /// # Errors
    /// Returns [`StorageError`] if remote object listing fails.
    #[instrument(skip(self), level = "debug")]
    pub async fn list_blobs(&self) -> Result<Vec<String>, StorageError> {
        let prefix = ObjPath::from("blobs/sha256");
        let mut list_stream = self.store.list(Some(&prefix));

        let mut hashes = Vec::new();
        while let Some(meta_res) = list_stream.next().await {
            let meta = meta_res.map_err(|source| StorageError::Store { source })?;
            if let Some(filename) = meta.location.filename() {
                hashes.push(filename.to_string());
            }
        }

        Ok(hashes)
    }
}
