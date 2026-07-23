//! Storage module backed by `object_store` for cloud and local baseline synchronization.

pub mod adapter;
pub mod merge;
pub mod sync;

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

    /// Specified blob hash was not found on remote.
    #[error("Blob not found on remote storage: sha256:{0}")]
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
pub fn blob_key(sha256: &str) -> ObjPath {
    ObjPath::from(format!("blobs/sha256/{sha256}"))
}

/// Helper function constructing the remote object path for a branch/platform manifest index.
#[must_use]
pub fn manifest_key(branch: &str, platform: &str) -> ObjPath {
    ObjPath::from(format!("branches/{branch}/{platform}/manifest_index.json"))
}
