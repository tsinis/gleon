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

/// Returns `true` if `hash`'s blob exists under `blobs_root` and is usable.
///
/// A convenience for callers that only need the yes/no answer, not the path itself (which
/// they'd otherwise compute via [`local_blob_path`] just to immediately discard).
#[must_use]
pub fn has_usable_local_blob(
    blobs_root: &std::path::Path,
    hash: &crate::manifest::ImageHash,
) -> bool {
    is_usable_blob(&local_blob_path(blobs_root, hash))
}

/// Returns `true` only if `path` is an existing regular file and not a symlink.
#[must_use]
pub fn is_usable_blob(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| !meta.is_symlink() && meta.is_file())
}

#[cfg(all(test, not(miri)))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::pedantic,
    clippy::nursery
)]
mod tests {
    use super::*;
    use crate::manifest::ImageHash;

    fn sha256(value: &str) -> ImageHash {
        ImageHash::new("sha256", value).unwrap()
    }

    #[test]
    fn test_has_usable_local_blob_requires_a_real_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let hash = sha256(&"a".repeat(64));

        assert!(
            !has_usable_local_blob(root, &hash),
            "missing blob must not be reported as usable"
        );

        let path = local_blob_path(root, &hash);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"png bytes").unwrap();
        assert!(has_usable_local_blob(root, &hash));

        // A directory sitting where the blob should be is not a usable blob either.
        let dir_hash = sha256(&"b".repeat(64));
        std::fs::create_dir_all(local_blob_path(root, &dir_hash)).unwrap();
        assert!(!has_usable_local_blob(root, &dir_hash));
    }

    #[test]
    #[cfg(unix)]
    fn test_has_usable_local_blob_rejects_symlinks() {
        // Symlinked blobs are refused on purpose: a crafted manifest must not be able to make
        // gleon upload an arbitrary file from outside the blob store.
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let hash = sha256(&"c".repeat(64));

        let target = temp.path().join("outside-secret.txt");
        std::fs::write(&target, b"secret").unwrap();
        let link = local_blob_path(root, &hash);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(link.exists(), "symlink target resolves");
        assert!(!has_usable_local_blob(root, &hash));
    }
}
