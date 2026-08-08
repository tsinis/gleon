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

use super::{StorageError, blob_key};

/// Configuration for storage initialization and authentication credentials.
#[derive(Clone, PartialEq, Eq)]
pub struct StorageConfig {
    /// Remote storage URL (e.g., `s3://my-bucket/gleon`, `file:///path/to/store`, `memory://`).
    pub url: String,

    /// AWS or S3-compatible Access Key ID.
    pub aws_access_key_id: Option<String>,

    /// AWS or S3-compatible Secret Access Key.
    pub aws_secret_access_key: Option<String>,

    /// Google Cloud Storage JSON service account key.
    pub gcp_service_account_key: Option<String>,

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
            gcp_service_account_key: None,
            aws_region: None,
            aws_endpoint: None,
            r2_account_id: None,
            concurrency: 8,
        }
    }

    /// Constructs a `StorageConfig` from an environment provider.
    ///
    /// `GLEON_*` prefixed variables override standard `AWS_*` / `R2_*` variables.
    /// Returns `None` if `GLEON_STORAGE_URL` is missing or empty.
    #[must_use]
    pub fn from_env(env: &dyn crate::git::EnvProvider) -> Option<Self> {
        let url_val = env.get_var("GLEON_STORAGE_URL")?;
        let url = url_val.trim();
        if url.is_empty() {
            return None;
        }

        let get_var = |gleon_key: &str, std_key: &str| -> Option<String> {
            env.get_var(gleon_key)
                .filter(|v| !v.trim().is_empty())
                .or_else(|| env.get_var(std_key).filter(|v| !v.trim().is_empty()))
        };

        let concurrency = env
            .get_var("GLEON_CONCURRENCY")
            .and_then(|v| v.parse().ok())
            .unwrap_or(8);

        Some(Self {
            url: url.to_string(),
            aws_access_key_id: get_var("GLEON_AWS_ACCESS_KEY_ID", "AWS_ACCESS_KEY_ID"),
            aws_secret_access_key: get_var("GLEON_AWS_SECRET_ACCESS_KEY", "AWS_SECRET_ACCESS_KEY"),
            gcp_service_account_key: get_var(
                "GLEON_GOOGLE_SERVICE_ACCOUNT_KEY",
                "GOOGLE_SERVICE_ACCOUNT_KEY",
            ),
            aws_region: get_var("GLEON_AWS_REGION", "AWS_REGION"),
            aws_endpoint: get_var("GLEON_AWS_ENDPOINT_URL", "AWS_ENDPOINT_URL"),
            r2_account_id: get_var("GLEON_R2_ACCOUNT_ID", "R2_ACCOUNT_ID"),
            concurrency,
        })
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
            .field(
                "gcp_service_account_key",
                &self.gcp_service_account_key.as_ref().map(|_| "[REDACTED]"),
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
    signer: Option<Arc<dyn object_store::signer::Signer>>,
    prefix: object_store::path::Path,
    concurrency: usize,
}

impl ObjectStoreAdapter {
    /// Instantiates an `ObjectStoreAdapter` from a `StorageConfig`.
    ///
    /// # Errors
    /// Returns [`StorageError::InvalidUrl`] if the URL or parameters cannot be parsed by `object_store`.
    #[instrument(skip(config), level = "debug")]
    pub fn from_config(config: &StorageConfig) -> Result<Self, StorageError> {
        let parsed_url = url::Url::parse(&config.url).map_err(|e| StorageError::InvalidUrl {
            url: config.url.clone(),
            reason: e.to_string(),
        })?;

        let url_path = parsed_url.path().trim_start_matches('/');
        let prefix = if url_path.is_empty() {
            object_store::path::Path::default()
        } else {
            object_store::path::Path::parse(url_path).map_err(|e| StorageError::InvalidUrl {
                url: config.url.clone(),
                reason: e.to_string(),
            })?
        };

        let (store, signer): (
            Arc<dyn ObjectStore>,
            Option<Arc<dyn object_store::signer::Signer>>,
        ) = match parsed_url.scheme() {
            "s3" | "r2" => {
                let mut builder = object_store::aws::AmazonS3Builder::from_env();
                if parsed_url.scheme() == "r2" {
                    let r2_as_s3 = config.url.replace("r2://", "s3://");
                    builder = builder.with_url(&r2_as_s3);
                } else {
                    builder = builder.with_url(&config.url);
                }

                if let Some(key_id) = &config.aws_access_key_id {
                    builder = builder.with_access_key_id(key_id);
                }
                if let Some(secret) = &config.aws_secret_access_key {
                    builder = builder.with_secret_access_key(secret);
                }
                if let Some(region) = &config.aws_region {
                    builder = builder.with_region(region);
                } else if config.r2_account_id.is_some() {
                    builder = builder.with_region("auto");
                }
                if let Some(endpoint) = &config.aws_endpoint {
                    builder = builder.with_endpoint(endpoint);
                } else if let Some(account_id) = &config.r2_account_id {
                    let r2_endpoint = format!("https://{account_id}.r2.cloudflarestorage.com");
                    builder = builder.with_endpoint(r2_endpoint);
                }

                let s3 = builder.build().map_err(|e| StorageError::InvalidUrl {
                    url: config.url.clone(),
                    reason: e.to_string(),
                })?;
                let s3_arc = Arc::new(s3);
                (s3_arc.clone(), Some(s3_arc))
            }
            "gs" => {
                let mut builder = object_store::gcp::GoogleCloudStorageBuilder::from_env();
                builder = builder.with_url(&config.url);
                if let Some(sec) = &config.gcp_service_account_key {
                    builder = builder.with_service_account_key(sec);
                }
                let gcs = builder.build().map_err(|e| StorageError::InvalidUrl {
                    url: config.url.clone(),
                    reason: e.to_string(),
                })?;
                let gcs_arc = Arc::new(gcs);
                (gcs_arc.clone(), Some(gcs_arc))
            }
            _ => {
                let mut opts = BTreeMap::new();
                if let Some(key_id) = &config.aws_access_key_id {
                    let _ = opts.insert("aws_access_key_id".to_string(), key_id.clone());
                }
                if let Some(secret) = &config.aws_secret_access_key {
                    let _ = opts.insert("aws_secret_access_key".to_string(), secret.clone());
                }
                if let Some(region) = &config.aws_region {
                    let _ = opts.insert("aws_region".to_string(), region.clone());
                }
                if let Some(endpoint) = &config.aws_endpoint {
                    let _ = opts.insert("aws_endpoint".to_string(), endpoint.clone());
                }

                let (raw_store, path) =
                    parse_url_opts(&parsed_url, opts).map_err(|e| StorageError::InvalidUrl {
                        url: config.url.clone(),
                        reason: e.to_string(),
                    })?;

                let store: Arc<dyn ObjectStore> = if path.as_ref().is_empty() {
                    Arc::from(raw_store)
                } else {
                    Arc::new(object_store::prefix::PrefixStore::new(raw_store, path))
                };

                return Ok(Self {
                    store,
                    signer: None,
                    prefix: object_store::path::Path::from(""), // Handled internally by PrefixStore
                    concurrency: std::cmp::max(1, config.concurrency),
                });
            }
        };

        // Note: AmazonS3Builder and GoogleCloudStorageBuilder already configure the path prefix
        // internally when constructed via with_url. Therefore, we do NOT wrap store in PrefixStore
        // here to prevent double-prefixing on S3/GCS operations.
        Ok(Self {
            store,
            signer,
            prefix,
            concurrency: std::cmp::max(1, config.concurrency),
        })
    }

    /// Generates a pre-signed URL for a given remote blob path if supported by the storage backend.
    #[instrument(skip(self), level = "debug")]
    pub async fn sign_blob_url(
        &self,
        relative_path: &str,
        expires_in: std::time::Duration,
    ) -> Option<String> {
        if let Some(signer) = &self.signer {
            let mut path_str = relative_path.to_string();
            if !self.prefix.as_ref().is_empty() {
                path_str = format!("{}/{}", self.prefix.as_ref(), relative_path);
            }
            let path = object_store::path::Path::from(path_str);
            match signer
                .signed_url(http::Method::GET, &path, expires_in)
                .await
            {
                Ok(url) => Some(url.to_string()),
                Err(e) => {
                    tracing::warn!(
                        path = %relative_path,
                        error = %e,
                        "Signing URL failed for backend, falling back"
                    );
                    None
                }
            }
        } else {
            None
        }
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
    pub async fn upload_blob(
        &self,
        hash: &crate::manifest::ImageHash,
        src_path: &Path,
    ) -> Result<(), StorageError> {
        let key = blob_key(hash);
        let mut options = std::fs::OpenOptions::new();
        options.read(true);

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
        }

        let std_file = options
            .open(src_path)
            .map_err(|source| StorageError::Io { source })?;

        // On Windows, opening a reparse point with FILE_FLAG_OPEN_REPARSE_POINT succeeds.
        // We must inspect the metadata of the opened handle to reject symlinks.
        // On Unix, O_NOFOLLOW fails to open symlinks with ELOOP, but this check is a harmless safety net.
        let metadata = std_file
            .metadata()
            .map_err(|source| StorageError::Io { source })?;

        if metadata.is_symlink() || !metadata.is_file() {
            return Err(StorageError::Io {
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Symlink or non-regular file blobs are not allowed for security reasons",
                ),
            });
        }

        let mut file = tokio::fs::File::from_std(std_file);
        let mut bytes = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut file, &mut bytes)
            .await
            .map_err(|source| StorageError::Io { source })?;
        self.store
            .put(&key, object_store::PutPayload::from(bytes))
            .await
            .map_err(|source| StorageError::Store { source })?;
        debug!(hash = %hash.value(), "Successfully uploaded blob to remote storage");
        Ok(())
    }

    /// Checks if a blob exists on remote storage without downloading it.
    pub async fn blob_exists(
        &self,
        hash: &crate::manifest::ImageHash,
    ) -> Result<bool, StorageError> {
        let key = blob_key(hash);
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
    pub async fn download_blob(
        &self,
        hash: &crate::manifest::ImageHash,
        dest_path: &Path,
    ) -> Result<(), StorageError> {
        let key = blob_key(hash);

        let get_result = self.store.get(&key).await;
        let get_output = match get_result {
            Ok(output) => output,
            Err(object_store::Error::NotFound { .. }) => {
                return Err(StorageError::BlobNotFound(hash.value().to_string()));
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

        debug!(hash = %hash.value(), path = %dest_path.display(), "Successfully downloaded blob from remote storage");
        Ok(())
    }

    /// Lists all SHA256 blob hashes existing under the remote `blobs/sha256/` prefix.
    ///
    /// # Errors
    /// Returns [`StorageError`] if remote object listing fails.
    #[instrument(skip(self), level = "debug")]
    pub async fn list_blobs(&self, scheme: &str) -> Result<Vec<String>, StorageError> {
        let prefix = ObjPath::from(format!("blobs/{}", scheme));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MapEnv(HashMap<String, String>);
    impl crate::git::EnvProvider for MapEnv {
        fn get_var(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
    }

    #[test]
    fn test_storage_config_from_env_map_missing_or_empty_url() {
        let vars = HashMap::new();
        assert!(StorageConfig::from_env(&MapEnv(vars)).is_none());

        let mut vars_empty = HashMap::new();
        vars_empty.insert("GLEON_STORAGE_URL".to_string(), "  ".to_string());
        assert!(StorageConfig::from_env(&MapEnv(vars_empty)).is_none());
    }

    #[test]
    fn test_storage_config_from_env_map_priorities() {
        let mut vars = HashMap::new();
        vars.insert(
            "GLEON_STORAGE_URL".to_string(),
            "s3://my-bucket/gleon".to_string(),
        );

        // Standard AWS vars
        vars.insert("AWS_ACCESS_KEY_ID".to_string(), "aws_key".to_string());
        vars.insert("AWS_SECRET_ACCESS_KEY".to_string(), "aws_sec".to_string());
        vars.insert("AWS_REGION".to_string(), "us-east-1".to_string());
        vars.insert(
            "AWS_ENDPOINT_URL".to_string(),
            "https://aws.endpoint".to_string(),
        );
        vars.insert("R2_ACCOUNT_ID".to_string(), "r2_acc".to_string());

        let cfg = StorageConfig::from_env(&MapEnv(vars.clone())).unwrap();
        assert_eq!(cfg.url, "s3://my-bucket/gleon");
        assert_eq!(cfg.aws_access_key_id.as_deref(), Some("aws_key"));
        assert_eq!(cfg.aws_secret_access_key.as_deref(), Some("aws_sec"));
        assert_eq!(cfg.aws_region.as_deref(), Some("us-east-1"));
        assert_eq!(cfg.aws_endpoint.as_deref(), Some("https://aws.endpoint"));
        assert_eq!(cfg.r2_account_id.as_deref(), Some("r2_acc"));
        assert_eq!(cfg.concurrency, 8);

        // Override with GLEON_ prefixed vars
        vars.insert(
            "GLEON_AWS_ACCESS_KEY_ID".to_string(),
            "gleon_key".to_string(),
        );
        vars.insert(
            "GLEON_AWS_SECRET_ACCESS_KEY".to_string(),
            "gleon_sec".to_string(),
        );
        vars.insert("GLEON_AWS_REGION".to_string(), "gleon-region".to_string());
        vars.insert(
            "GLEON_AWS_ENDPOINT_URL".to_string(),
            "https://gleon.endpoint".to_string(),
        );
        vars.insert("GLEON_R2_ACCOUNT_ID".to_string(), "gleon_r2".to_string());
        vars.insert("GLEON_CONCURRENCY".to_string(), "16".to_string());

        let cfg_override = StorageConfig::from_env(&MapEnv(vars)).unwrap();
        assert_eq!(cfg_override.aws_access_key_id.as_deref(), Some("gleon_key"));
        assert_eq!(
            cfg_override.aws_secret_access_key.as_deref(),
            Some("gleon_sec")
        );
        assert_eq!(cfg_override.aws_region.as_deref(), Some("gleon-region"));
        assert_eq!(
            cfg_override.aws_endpoint.as_deref(),
            Some("https://gleon.endpoint")
        );
        assert_eq!(cfg_override.r2_account_id.as_deref(), Some("gleon_r2"));
        assert_eq!(cfg_override.concurrency, 16);
    }

    #[tokio::test]
    #[cfg(not(miri))]
    async fn test_sign_blob_url_memory_store_returns_none() {
        let cfg = StorageConfig::new("memory://");
        let adapter = ObjectStoreAdapter::from_config(&cfg).unwrap();
        let res = adapter
            .sign_blob_url("blobs/sha256/1234", std::time::Duration::from_secs(60))
            .await;
        assert!(res.is_none());
    }

    #[tokio::test]
    #[cfg(not(miri))]
    async fn test_sign_blob_url_s3_store() {
        // Use from_env with empty env to ensure hermetic execution
        let mut cfg = StorageConfig::from_env(&MapEnv(HashMap::new()))
            .unwrap_or_else(|| StorageConfig::new("s3://mybucket"));
        cfg.aws_access_key_id = Some("testkey".to_string());
        cfg.aws_secret_access_key = Some("testsecret".to_string());
        cfg.aws_region = Some("us-east-1".to_string());
        let adapter = ObjectStoreAdapter::from_config(&cfg).unwrap();
        let url = adapter
            .sign_blob_url("blobs/sha256/1234", std::time::Duration::from_secs(60))
            .await
            .expect("Expected Some URL for S3 signing");
        assert!(url.contains("mybucket"));
        assert!(url.contains("X-Amz-Signature"));
    }

    #[tokio::test]
    #[cfg(not(miri))]
    async fn test_sign_blob_url_s3_store_with_prefix() {
        let mut cfg = StorageConfig::from_env(&MapEnv(HashMap::new()))
            .unwrap_or_else(|| StorageConfig::new("s3://mybucket/subfolder/prefix"));
        cfg.aws_access_key_id = Some("testkey".to_string());
        cfg.aws_secret_access_key = Some("testsecret".to_string());
        cfg.aws_region = Some("us-east-1".to_string());
        let adapter = ObjectStoreAdapter::from_config(&cfg).unwrap();
        let url = adapter
            .sign_blob_url("blobs/sha256/1234", std::time::Duration::from_secs(60))
            .await
            .expect("Expected Some URL for S3 signing");
        assert!(url.contains("mybucket"));
        assert!(url.contains("subfolder/prefix/blobs/sha256/1234"));
        assert!(url.contains("X-Amz-Signature"));
    }

    #[tokio::test]
    #[cfg(not(miri))]
    async fn test_sign_blob_url_gcs_store() {
        let cfg = StorageConfig::from_env(&MapEnv(HashMap::new()))
            .unwrap_or_else(|| StorageConfig::new("gs://mybucket"));
        let adapter = ObjectStoreAdapter::from_config(&cfg).unwrap();
        let res = adapter
            .sign_blob_url("blobs/sha256/1234", std::time::Duration::from_secs(60))
            .await;
        // Unauthenticated / metadata-less GCS safely falls back to None instead of failing
        assert!(res.is_none());
    }

    #[test]
    fn test_storage_config_invalid_path_syntax() {
        let cfg = StorageConfig::new("s3://mybucket//invalid//path");
        let res = ObjectStoreAdapter::from_config(&cfg);
        assert!(matches!(
            res,
            Err(StorageError::InvalidUrl { ref url, ref reason })
            if url == "s3://mybucket//invalid//path" && !reason.is_empty()
        ));
    }

    #[test]
    fn test_storage_config_invalid_s3_builder_error() {
        let cfg = StorageConfig::new("s3://");
        let res = ObjectStoreAdapter::from_config(&cfg);
        assert!(matches!(res, Err(StorageError::InvalidUrl { .. })));
    }

    #[test]
    fn test_storage_config_invalid_gcs_builder_error() {
        let mut cfg = StorageConfig::new("gs://mybucket");
        cfg.gcp_service_account_key = Some("not valid json".to_string());
        let res = ObjectStoreAdapter::from_config(&cfg);
        assert!(matches!(res, Err(StorageError::InvalidUrl { .. })));
    }

    #[tokio::test]
    #[cfg(all(unix, not(miri)))]
    async fn test_upload_blob_rejects_symlink() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target.png");
        std::fs::write(&target, b"fake png").unwrap();
        let link = temp.path().join("link.png");
        symlink(&target, &link).unwrap();

        let cfg = StorageConfig::new(format!("file://{}", temp.path().display()));
        let adapter = ObjectStoreAdapter::from_config(&cfg).unwrap();
        let hash = crate::manifest::ImageHash::new(
            "sha256",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap();

        let result = adapter.upload_blob(&hash, &link).await;
        assert!(matches!(result, Err(StorageError::Io { .. })));
    }
}
