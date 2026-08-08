//! Storage module backed by `object_store` for cloud and local baseline synchronization.

pub mod adapter;

pub use adapter::{ObjectStoreAdapter, StorageConfig};
use object_store::path::Path as ObjPath;

/// Storage error types.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// Error originating from the underlying `object_store` crate.
    #[error("Object store operation failed: {source}")]
    Store {
        /// Source error from `object_store`.
        #[from]
        source: object_store::Error,
    },

    /// Standard I/O error.
    #[error("I/O operation failed: {source}")]
    Io {
        /// Inner I/O error.
        #[from]
        source: std::io::Error,
    },

    /// Invalid or unparseable storage URL.
    #[error("Invalid storage URL '{url}': {reason}")]
    InvalidUrl {
        /// The raw invalid URL string.
        url: String,
        /// Reason for failure.
        reason: String,
    },

    /// Specified blob hash or object key was not found on remote.
    #[error("Object or blob not found on remote storage: {0}")]
    BlobNotFound(String),

    /// Persist operation failed during atomic download.
    #[error("Atomic persist failed for target path '{path}': {source}")]
    PersistFailed {
        /// Target file path.
        path: String,
        /// Inner tempfile persist error.
        #[source]
        source: tempfile::PersistError,
    },
}

/// Helper function constructing the remote object path for a CAS blob hash.
#[must_use]
pub fn blob_key(hash: &crate::manifest::ImageHash) -> ObjPath {
    ObjPath::from(format!("blobs/{}/{}", hash.scheme(), hash.value()))
}

/// Returns the local file path for a CAS blob under `blobs_root`.
#[must_use]
pub fn local_blob_path(
    blobs_root: &std::path::Path,
    hash: &crate::manifest::ImageHash,
) -> std::path::PathBuf {
    blobs_root.join(hash.scheme()).join(hash.value())
}

/// Returns `true` only if `path` is an existing regular file and not a symlink.
#[must_use]
pub fn is_usable_blob(path: &std::path::Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => !meta.is_symlink() && meta.is_file(),
        Err(_) => false,
    }
}
