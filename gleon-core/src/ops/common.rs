//! Shared error type and helper functions used across `ops::*` modules.
//!
//! Each `ops` module keeps its own operation-specific error variants but delegates the
//! handful of concerns every operation shares (workspace initialization checks, platform key
//! resolution, manifest index loading with fallback, config-driven scanning, blob hashing, and
//! `.gitignore`/scaffold file management) to the helpers here, wrapping [`CoreError`] via
//! `#[error(transparent)] Core(#[from] CoreError)`.

use crate::config::ConfigError;
use crate::context::{ContextError, ResolvedContext};
use crate::engine::phash::compute_phash;
use crate::manifest::{ImageHash, ManifestError, SingleTestManifest, WorkspaceIndex};
use crate::paths::GleonPaths;
use crate::scanner::{FileScanner, ScannerError, TestCase};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Errors shared across `ops::*` operations.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Workspace has not been initialized (`.gleon` missing).
    #[error("gleon workspace is not initialized. Please run 'gleon init' first.")]
    NotInitialized,

    /// Error resolving context.
    #[error("Context resolution error: {0}")]
    Context(#[from] ContextError),

    /// Error loading configuration.
    #[error("Config error: {0}")]
    Config(#[from] ConfigError),

    /// Error scanning files.
    #[error("Scanner error: {0}")]
    Scanner(#[from] ScannerError),

    /// Error loading or saving a manifest.
    #[error("Manifest error: {0}")]
    Manifest(#[from] ManifestError),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<crate::io::IoError> for CoreError {
    fn from(err: crate::io::IoError) -> Self {
        match err {
            crate::io::IoError::Io(e) => Self::Io(e),
            crate::io::IoError::JsonParse(e) => Self::Io(std::io::Error::other(e)),
        }
    }
}

/// Ensures the workspace rooted at `base_dir` has been initialized (`.gleon/` exists),
/// returning its resolved paths.
///
/// # Errors
/// Returns [`CoreError::NotInitialized`] if `.gleon/` does not exist, or [`CoreError::Io`] if
/// its metadata cannot be queried for any other reason (e.g. a permission error).
pub fn ensure_initialized(base_dir: &Path) -> Result<GleonPaths, CoreError> {
    let paths = GleonPaths::new(base_dir);
    match std::fs::metadata(paths.gleon_dir()) {
        Ok(_) => Ok(paths),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CoreError::NotInitialized),
        Err(e) => Err(CoreError::Io(e)),
    }
}

/// Resolves the active platform key from a [`ResolvedContext`].
///
/// # Errors
/// Returns [`CoreError::Context`] if the platform identity fails segment validation.
pub fn platform_key(context: &ResolvedContext) -> Result<String, CoreError> {
    context
        .platform
        .to_key()
        .map_err(|e| CoreError::Context(ContextError::Platform(e)))
}

/// Loads the `WorkspaceIndex` for `platform_key`, and separately the fallback platform's index
/// (if `fallback_platform_key` is set and differs from `platform_key`).
///
/// Callers decide how to use the fallback index: some merge it into the primary index for
/// missing entries, others compare against it without merging.
///
/// # Errors
/// Returns [`CoreError::Manifest`] if either index fails to load.
pub fn load_index_with_fallback(
    paths: &GleonPaths,
    platform_key: &str,
    fallback_platform_key: Option<&str>,
) -> Result<(WorkspaceIndex, Option<WorkspaceIndex>), CoreError> {
    let manifests_dir = paths.manifests_dir(platform_key);
    let workspace_index = WorkspaceIndex::load(&manifests_dir).map_err(CoreError::Manifest)?;

    let fallback_index = fallback_platform_key
        .filter(|&k| k != platform_key)
        .map(|fb_key| {
            WorkspaceIndex::load(paths.manifests_dir(fb_key)).map_err(CoreError::Manifest)
        })
        .transpose()?;

    Ok((workspace_index, fallback_index))
}

/// Loads the effective `GleonConfig` from `context` (or its default) and scans the workspace
/// rooted at `context.base_dir` for matching test cases.
///
/// # Errors
/// Returns [`CoreError::Scanner`] if any include/exclude glob fails to compile or a derived
/// test name fails validation.
pub fn load_config_and_scan(context: &ResolvedContext) -> Result<Vec<TestCase>, CoreError> {
    let config = context.config.clone().unwrap_or_default();
    FileScanner::scan_workspace(&config, &context.base_dir).map_err(CoreError::Scanner)
}

/// Decodes a PNG's raw bytes and computes its SHA-256 digest, perceptual hash, and dimensions.
///
/// Uses [`SingleTestManifest::load_image_from_bytes`] (not a bare decode) so callers get the
/// same dimension/format validation regardless of call site.
///
/// # Errors
/// Returns the underlying [`ManifestError`] if the bytes fail to decode or validate as a
/// supported image.
pub fn hash_and_measure(png_bytes: &[u8]) -> Result<(String, String, u32, u32), ManifestError> {
    let dynamic_img = SingleTestManifest::load_image_from_bytes(png_bytes)?;
    let width = dynamic_img.width();
    let height = dynamic_img.height();
    let rgba_img = dynamic_img.to_rgba8();

    let phash_str = compute_phash(&rgba_img);
    let sha256_hex = hex::encode(Sha256::digest(png_bytes));

    Ok((sha256_hex, phash_str, width, height))
}

/// Builds a [`SingleTestManifest`] from a sha256 hex digest, dhash string, and dimensions.
///
/// # Errors
/// Returns [`CoreError::Manifest`] if any component fails validation.
pub fn build_manifest(
    sha256_hex: &str,
    phash_str: &str,
    width: u32,
    height: u32,
) -> Result<SingleTestManifest, CoreError> {
    let hash = ImageHash::new("sha256", sha256_hex).map_err(CoreError::Manifest)?;
    let phash = phash_str
        .parse::<ImageHash>()
        .map_err(CoreError::Manifest)?;
    SingleTestManifest::new(hash, phash, width, height).map_err(CoreError::Manifest)
}

/// Appends `entries` missing (as a trimmed line) from the gitignore-style file at `path`.
///
/// Creates the file if missing, writes atomically, and returns the entries that were actually
/// appended, in order (empty if all were already present, in which case no write occurs).
///
/// # Errors
/// Returns [`CoreError::Io`] if the existing file (when present) fails to read for a reason
/// other than not existing, or if the atomic write fails.
pub fn append_missing_gitignore_lines(
    path: &Path,
    entries: &[String],
) -> Result<Vec<String>, CoreError> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let existing_lines: std::collections::HashSet<&str> = existing.lines().map(str::trim).collect();

    let mut added = Vec::new();
    let mut to_append = String::new();
    for entry in entries {
        if !existing_lines.contains(entry.as_str()) {
            use std::fmt::Write as _;
            // Writing to a `String` via `fmt::Write` never fails.
            #[allow(clippy::expect_used)]
            writeln!(to_append, "{entry}").expect("write! to a String cannot fail");
            added.push(entry.clone());
        }
    }

    if added.is_empty() {
        return Ok(added);
    }

    let mut buffer = existing;
    if !buffer.is_empty() && !buffer.ends_with('\n') {
        buffer.push('\n');
    }
    buffer.push_str(&to_append);

    crate::io::write_file_atomically(path, |writer| {
        use std::io::Write as _;
        writer
            .write_all(buffer.as_bytes())
            .map_err(crate::io::IoError::Io)
    })?;

    Ok(added)
}

/// Creates the file at `path` with `content` if it does not already exist; a no-op otherwise.
///
/// Uses `create_new` for idempotency and durably syncs the file and `dir_to_sync` (its parent
/// directory) to disk.
///
/// # Errors
/// Returns [`CoreError::Io`] if file creation, writing, or syncing fails for a reason other
/// than the file already existing.
#[cfg_attr(windows, allow(unused_variables))]
pub fn create_new_file_with_content(
    path: &Path,
    content: &[u8],
    dir_to_sync: &Path,
) -> Result<bool, CoreError> {
    use std::io::Write as _;

    let create_res = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path);
    match create_res {
        Ok(mut f) => {
            if let Err(e) = f.write_all(content).and_then(|()| f.sync_all()) {
                let _ = std::fs::remove_file(path);
                return Err(CoreError::Io(e));
            }
            #[cfg(not(windows))]
            if let Ok(d) = std::fs::File::open(dir_to_sync) {
                let _ = d.sync_all();
            }
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(CoreError::Io(e)),
    }
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
    use tempfile::tempdir;

    #[test]
    fn test_ensure_initialized_missing_and_present() {
        let temp = tempdir().unwrap();
        assert!(matches!(
            ensure_initialized(temp.path()),
            Err(CoreError::NotInitialized)
        ));

        std::fs::create_dir_all(temp.path().join(".gleon")).unwrap();
        let paths = ensure_initialized(temp.path()).unwrap();
        assert_eq!(paths.gleon_dir(), temp.path().join(".gleon"));
    }

    #[test]
    fn test_platform_key_resolves_and_rejects_invalid() {
        let mut ctx = ResolvedContext::default();
        ctx.platform.os = "linux".to_string();
        assert_eq!(platform_key(&ctx).unwrap(), "5:linux");

        ctx.platform.os = "in/valid".to_string();
        assert!(matches!(platform_key(&ctx), Err(CoreError::Context(_))));
    }

    #[test]
    fn test_load_index_with_fallback_merges_only_when_requested() {
        let temp = tempdir().unwrap();
        let paths = GleonPaths::new(temp.path());
        std::fs::create_dir_all(paths.manifests_dir("linux")).unwrap();
        std::fs::create_dir_all(paths.manifests_dir("macos")).unwrap();

        let (primary, fallback) = load_index_with_fallback(&paths, "linux", Some("macos")).unwrap();
        assert!(primary.is_empty());
        assert!(fallback.is_some());

        let (_, no_fallback) = load_index_with_fallback(&paths, "linux", None).unwrap();
        assert!(no_fallback.is_none());

        // Same platform as fallback: no fallback index loaded.
        let (_, self_fallback) = load_index_with_fallback(&paths, "linux", Some("linux")).unwrap();
        assert!(self_fallback.is_none());
    }

    #[test]
    fn test_hash_and_measure_and_build_manifest_roundtrip() {
        let png_bytes = include_bytes!("../../tests/fixtures/baseline_100x100.png");
        let (sha256_hex, phash_str, width, height) = hash_and_measure(png_bytes).unwrap();
        assert_eq!(width, 100);
        assert_eq!(height, 100);

        let manifest = build_manifest(&sha256_hex, &phash_str, width, height).unwrap();
        assert_eq!(manifest.hash.value(), sha256_hex);
    }

    #[test]
    fn test_hash_and_measure_rejects_corrupt_bytes() {
        assert!(hash_and_measure(b"not a png").is_err());
    }

    #[test]
    fn test_append_missing_gitignore_lines_idempotent() {
        let temp = tempdir().unwrap();
        let path = temp.path().join(".gitignore");

        let added =
            append_missing_gitignore_lines(&path, &["blobs/".to_string(), "runs/".to_string()])
                .unwrap();
        assert_eq!(added, vec!["blobs/".to_string(), "runs/".to_string()]);

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("blobs/"));
        assert!(content.contains("runs/"));

        // Second call with an overlapping + new entry only appends the new one.
        let added_again = append_missing_gitignore_lines(
            &path,
            &["blobs/".to_string(), "credentials".to_string()],
        )
        .unwrap();
        assert_eq!(added_again, vec!["credentials".to_string()]);
    }

    #[test]
    fn test_create_new_file_with_content_idempotent() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("gleon.yaml");

        assert!(create_new_file_with_content(&path, b"hello", temp.path()).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");

        // Already exists: no-op, content untouched.
        assert!(!create_new_file_with_content(&path, b"world", temp.path()).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }
}
