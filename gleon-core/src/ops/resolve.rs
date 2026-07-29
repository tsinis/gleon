//! Resolution operation for handling Git merge conflicts in manifest JSON files.

use crate::io::{IoError, save_json_atomically};
use crate::manifest::{
    ConflictManifest, ConflictParseError, SingleTestManifest, parse_conflict_manifest,
};
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during conflict resolution scan or application.
#[derive(Debug, Error)]
pub enum ResolveError {
    /// Target manifest directory was not found.
    #[error("Manifest directory does not exist at {0}")]
    ManifestDirNotFound(PathBuf),

    /// Standard I/O error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to parse conflict markers in a manifest file.
    #[error("Failed to parse conflict markers in {0}: {1}")]
    ConflictParse(PathBuf, #[source] ConflictParseError),

    /// Failed to save resolved manifest file.
    #[error("Failed to save resolved manifest to {0}: {1}")]
    Save(PathBuf, #[source] IoError),
}

/// Represents a conflicted manifest file discovered during scanning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictedManifestItem {
    /// Relative test path (e.g. "auth/login_screen").
    pub test_path: String,
    /// Platform component (e.g. "macos-aarch64").
    pub platform: String,
    /// Absolute path to the conflicted manifest file.
    pub manifest_file_path: PathBuf,
    /// Parsed conflict details (`ours` and `theirs`).
    pub conflict: ConflictManifest,
}

/// Scans `.gleon/manifests/` for files containing Git conflict markers (`<<<<<<<`).
///
/// # Errors
/// Returns [`ResolveError`] if directory walking fails or if a conflict marker cannot be parsed.
pub fn scan_conflicts(
    base_dir: &Path,
    platform_filter: Option<&str>,
) -> Result<Vec<ConflictedManifestItem>, ResolveError> {
    let manifests_root = base_dir.join(".gleon").join("manifests");

    let search_dir = match platform_filter {
        Some(p) => manifests_root.join(p),
        None => manifests_root.clone(),
    };

    let mut items = Vec::new();

    for entry_res in WalkBuilder::new(&search_dir)
        .standard_filters(false)
        .build()
    {
        let entry = match entry_res {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!("Skipping unreadable manifest entry: {}", err);
                continue;
            }
        };

        let path = entry.path();
        if entry.file_type().is_some_and(|ft| ft.is_file())
            && path.extension().is_some_and(|ext| ext == "json")
        {
            let content = std::fs::read_to_string(path)?;
            if content.contains("<<<<<<<") {
                let conflict = parse_conflict_manifest(&content)
                    .map_err(|e| ResolveError::ConflictParse(path.to_path_buf(), e))?;

                let rel = path.strip_prefix(&manifests_root).unwrap_or(path);
                let mut components = rel.components().filter_map(|c| match c {
                    std::path::Component::Normal(s) => s.to_str(),
                    _ => None,
                });

                let platform = components.next().unwrap_or_default().to_string();

                let mut test_path = String::new();
                for (i, segment) in components.enumerate() {
                    if i > 0 {
                        test_path.push('/');
                    }
                    test_path.push_str(segment);
                }
                let test_path_clean = test_path
                    .strip_suffix(".json")
                    .unwrap_or(&test_path)
                    .to_string();

                items.push(ConflictedManifestItem {
                    test_path: test_path_clean,
                    platform,
                    manifest_file_path: path.to_path_buf(),
                    conflict,
                });
            }
        }
    }

    Ok(items)
}

/// Applies a resolution choice to a conflicted manifest file, writing the chosen manifest atomically.
///
/// # Errors
/// Returns [`ResolveError`] if atomic serialization/saving fails.
pub fn apply_resolution(
    manifest_file_path: &Path,
    chosen: &SingleTestManifest,
) -> Result<(), ResolveError> {
    save_json_atomically(manifest_file_path, chosen)
        .map_err(|e| ResolveError::Save(manifest_file_path.to_path_buf(), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_scan_and_apply_resolution() {
        let temp = tempdir().unwrap();
        let manifests_dir = temp
            .path()
            .join(".gleon")
            .join("manifests")
            .join("macos-aarch64")
            .join("auth");
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let conflicted = include_str!("../../tests/fixtures/conflict_2way.json");

        let login_path = manifests_dir.join("login.json");
        std::fs::write(&login_path, conflicted).unwrap();

        let conflicts = scan_conflicts(temp.path(), None).expect("Scan failed");
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].test_path, "auth/login");
        assert_eq!(conflicts[0].platform, "macos-aarch64");

        // Apply resolution choosing 'theirs'
        apply_resolution(&login_path, &conflicts[0].conflict.theirs).expect("Apply failed");

        let updated = std::fs::read_to_string(&login_path).unwrap();
        assert!(!updated.contains("<<<<<<<"));
        assert!(
            updated.contains(
                "sha256:2222222222222222222222222222222222222222222222222222222222222222"
            )
        );
    }
}
