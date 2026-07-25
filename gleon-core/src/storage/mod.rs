//! Storage module backed by `object_store` for cloud and local baseline synchronization.

pub mod adapter;
pub mod merge;
pub mod sync;

pub use adapter::{ManifestPointer, ObjectStoreAdapter, StorageConfig};
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

    /// Downloaded blob contents did not match the requested SHA-256 hash.
    #[error("Downloaded blob SHA-256 mismatch: expected {expected}, got {actual}")]
    BlobHashMismatch {
        /// SHA-256 hash requested from remote storage.
        expected: String,
        /// SHA-256 hash calculated from the downloaded bytes.
        actual: String,
    },

    /// A conditional manifest write could not be applied because the remote object changed.
    #[error("Conditional manifest write conflicted for remote object: {path}")]
    Conflict {
        /// Remote object path whose version or existence precondition failed.
        path: String,
    },

    /// The remote backend does not support conditional manifest writes.
    #[error("Remote storage does not support conditional manifest writes")]
    ConditionalWriteNotSupported,
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
