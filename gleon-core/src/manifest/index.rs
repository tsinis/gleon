//! In-memory workspace index built from per-test manifest files.

use ignore::WalkBuilder;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::manifest::ManifestError;
use crate::manifest::single::SingleTestManifest;

/// Normalizes path separators to forward slashes and lowercases test names without unnecessary allocations.
pub fn normalize_test_name(test_name: &str) -> Cow<'_, str> {
    if test_name.chars().any(|c| c.is_uppercase() || c == '\\') {
        let mut s = String::with_capacity(test_name.len());
        for c in test_name.chars() {
            if c == '\\' {
                s.push('/');
            } else {
                s.extend(c.to_lowercase());
            }
        }
        Cow::Owned(s)
    } else {
        Cow::Borrowed(test_name)
    }
}

/// Validates a relative test path (e.g. `auth/login_screen`).
/// Splits on both `/` and `\`, verifying that each segment contains only valid characters `[a-z0-9_.-]`.
pub fn validate_test_path(test_path: &str) -> Result<(), ManifestError> {
    if test_path.trim().is_empty() {
        return Err(ManifestError::Validation(
            "Test path cannot be empty".to_string(),
        ));
    }

    for segment in test_path.split(['/', '\\']) {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(ManifestError::Validation(format!(
                "Invalid test path segment '{}' in '{}'",
                segment, test_path
            )));
        }
        if !segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
        {
            return Err(ManifestError::Validation(format!(
                "Test path segment '{}' contains invalid characters",
                segment
            )));
        }
    }
    Ok(())
}

/// In-memory index mapping test case relative paths to their `SingleTestManifest`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceIndex {
    entries: BTreeMap<String, SingleTestManifest>,
    source_paths: BTreeMap<String, String>,
}

impl WorkspaceIndex {
    /// Creates a new empty `WorkspaceIndex`.
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            source_paths: BTreeMap::new(),
        }
    }

    /// Loads the `WorkspaceIndex` by scanning the given platform manifest directory.
    /// If the directory does not exist on disk, returns an empty index.
    pub fn load<P: AsRef<Path>>(manifest_dir: P) -> Result<Self, ManifestError> {
        let manifest_dir = manifest_dir.as_ref();

        let mut entries = BTreeMap::new();
        let mut source_paths = BTreeMap::new();
        let walker = WalkBuilder::new(manifest_dir)
            .standard_filters(false)
            .build();

        for entry_res in walker {
            let entry = match entry_res {
                Ok(e) => e,
                Err(err) => {
                    let err_msg = err.to_string();
                    let depth = err.depth();
                    if let Some(io_err) = err.into_io_error() {
                        if io_err.kind() == std::io::ErrorKind::NotFound && depth == Some(0) {
                            return Ok(Self::new());
                        }
                        return Err(ManifestError::StdIo(io_err));
                    }
                    return Err(ManifestError::Validation(format!(
                        "Manifest walker error: {}",
                        err_msg
                    )));
                }
            };
            let path = entry.path();
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }

            let rel_path = match path.strip_prefix(manifest_dir) {
                Ok(p) => p,
                Err(_) => continue,
            };

            // Remove .json extension
            let without_ext = rel_path.with_extension("");
            let rel_str = without_ext.to_str().ok_or_else(|| {
                ManifestError::Validation(format!(
                    "Non UTF-8 path encountered in manifest directory: {:?}",
                    without_ext
                ))
            })?;
            let normalized = normalize_test_name(rel_str);

            validate_test_path(normalized.as_ref())?;

            if entries.contains_key(normalized.as_ref()) {
                return Err(ManifestError::Validation(format!(
                    "Duplicate test case key collision in manifest index: '{}'",
                    normalized
                )));
            }

            let manifest = SingleTestManifest::load(path)?;
            source_paths.insert(normalized.to_string(), rel_str.to_string());
            entries.insert(normalized.into_owned(), manifest);
        }

        Ok(Self {
            entries,
            source_paths,
        })
    }

    /// Returns `true` if the index contains no test cases.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the number of test cases in the index.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns a reference to the inner entries map.
    pub fn entries(&self) -> &BTreeMap<String, SingleTestManifest> {
        &self.entries
    }

    /// Consumes the index and returns the inner entries map.
    pub fn into_entries(self) -> BTreeMap<String, SingleTestManifest> {
        self.entries
    }

    /// Gets a single test manifest by test case name.
    pub fn get(&self, test_name: &str) -> Option<&SingleTestManifest> {
        let normalized = normalize_test_name(test_name);
        self.entries.get(normalized.as_ref())
    }

    /// Inserts or updates a single test manifest in memory.
    pub fn insert(&mut self, test_name: String, manifest: SingleTestManifest) {
        let normalized = normalize_test_name(&test_name).into_owned();
        self.source_paths.insert(normalized.clone(), test_name);
        self.entries.insert(normalized, manifest);
    }

    /// Removes a test case entry from the in-memory map.
    pub fn remove(&mut self, test_name: &str) -> Option<SingleTestManifest> {
        let normalized = normalize_test_name(test_name);
        self.source_paths.remove(normalized.as_ref());
        self.entries.remove(normalized.as_ref())
    }

    /// Saves a single test manifest to disk under `manifest_dir` and updates memory.
    pub fn save_test<P: AsRef<Path>>(
        &mut self,
        manifest_dir: P,
        test_name: &str,
        manifest: &SingleTestManifest,
    ) -> Result<(), ManifestError> {
        let normalized = normalize_test_name(test_name);
        validate_test_path(normalized.as_ref())?;

        let manifest_dir = manifest_dir.as_ref();
        let canonical_key = normalized.as_ref();
        let target_path = manifest_dir.join(format!("{canonical_key}.json"));

        match manifest.save(&target_path) {
            Ok(()) => {}
            Err(e) => return Err(e),
        }

        // Remove legacy-cased manifest file on disk if it differs from canonical path
        if let Some(old_source) = self
            .source_paths
            .get(canonical_key)
            .filter(|s| *s != canonical_key)
        {
            let old_path = manifest_dir.join(format!("{old_source}.json"));
            let is_same_file = match (fs::canonicalize(&old_path), fs::canonicalize(&target_path)) {
                (Ok(p1), Ok(p2)) => p1 == p2,
                _ => false,
            };
            if !is_same_file {
                match fs::remove_file(&old_path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(ManifestError::StdIo(e)),
                }
            }
        }

        self.source_paths
            .insert(canonical_key.to_string(), canonical_key.to_string());
        self.entries
            .insert(normalized.into_owned(), manifest.clone());
        Ok(())
    }

    /// Removes a test case manifest file from disk and memory.
    pub fn remove_test<P: AsRef<Path>>(
        &mut self,
        manifest_dir: P,
        test_name: &str,
    ) -> Result<Option<SingleTestManifest>, ManifestError> {
        let normalized = normalize_test_name(test_name);
        validate_test_path(normalized.as_ref())?;

        let manifest_dir = manifest_dir.as_ref();
        let canonical_key = normalized.as_ref();

        if let Some(old_source) = self.source_paths.remove(canonical_key) {
            let old_path = manifest_dir.join(format!("{old_source}.json"));
            match fs::remove_file(&old_path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(ManifestError::StdIo(e)),
            }
        }

        let target_path = manifest_dir.join(format!("{canonical_key}.json"));
        match fs::remove_file(&target_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(ManifestError::StdIo(e)),
        }
        Ok(self.entries.remove(canonical_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ImageHash;
    use tempfile::tempdir;

    #[test]
    fn test_workspace_index_load_empty() {
        let temp = tempdir().unwrap();
        let non_existent = temp.path().join("does_not_exist");
        let index = WorkspaceIndex::load(&non_existent).unwrap();
        assert!(index.entries().is_empty());
    }

    #[test]
    fn test_workspace_index_save_and_load() {
        let temp = tempdir().unwrap();
        let manifest_dir = temp.path().join("macos-aarch64");

        let hash = ImageHash::new("sha256", "a".repeat(64)).unwrap();
        let phash = ImageHash::new("dhash", "0000000000000000").unwrap();
        let single = SingleTestManifest::new(hash, phash, 100, 200).unwrap();

        let mut index = WorkspaceIndex::new();
        index
            .save_test(&manifest_dir, "auth/login_screen", &single)
            .unwrap();

        assert_eq!(index.entries().len(), 1);
        assert_eq!(index.get("auth/login_screen"), Some(&single));

        let loaded = WorkspaceIndex::load(&manifest_dir).unwrap();
        assert_eq!(index, loaded);

        // Test remove_test
        let removed = index
            .remove_test(&manifest_dir, "auth/login_screen")
            .unwrap();
        assert_eq!(removed, Some(single));
        assert!(index.entries().is_empty());

        // Test remove_test on non-existent file (should be Ok(None))
        let removed_again = index
            .remove_test(&manifest_dir, "auth/login_screen")
            .unwrap();
        assert_eq!(removed_again, None);

        // Test remove_test with invalid path (parent traversal / absolute)
        assert!(index.remove_test(&manifest_dir, "../invalid").is_err());
    }

    #[test]
    fn test_workspace_index_load_filters_and_validation() {
        let temp = tempdir().unwrap();
        let manifest_dir = temp.path().join("macos-aarch64");
        std::fs::create_dir_all(&manifest_dir).unwrap();

        // 1. Subdirectory inside manifest_dir (should be skipped)
        std::fs::create_dir_all(manifest_dir.join("subfolder")).unwrap();

        // 2. Non-JSON file (should be skipped)
        std::fs::write(manifest_dir.join("notes.txt"), "hello").unwrap();

        // 3. Invalid test path file (e.g. contains invalid chars)
        std::fs::write(manifest_dir.join("bad!name.json"), "{}").unwrap();

        let index = WorkspaceIndex::load(&manifest_dir);
        assert!(index.is_err());
    }

    #[test]
    fn test_workspace_index_load_rejects_duplicates() {
        let temp = tempdir().unwrap();
        let manifest_dir = temp.path().join("macos-aarch64");
        std::fs::create_dir_all(manifest_dir.join("auth")).unwrap();

        let hash = ImageHash::new("sha256", "a".repeat(64)).unwrap();
        let phash = ImageHash::new("dhash", "0000000000000000").unwrap();
        let manifest = SingleTestManifest::new(hash, phash, 100, 100).unwrap();
        manifest.save(manifest_dir.join("auth/login.json")).unwrap();

        let loaded = WorkspaceIndex::load(&manifest_dir).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert!(loaded.get("auth/login").is_some());
    }

    #[test]
    fn test_workspace_index_into_entries_and_validation() {
        let hash = ImageHash::new("sha256", "a".repeat(64)).unwrap();
        let phash = ImageHash::new("dhash", "0000000000000000").unwrap();
        let single = SingleTestManifest::new(hash, phash, 100, 200).unwrap();

        let mut index = WorkspaceIndex::new();
        index.insert("billing/form".to_string(), single.clone());
        assert_eq!(index.remove("billing/form"), Some(single));

        assert!(validate_test_path("").is_err());
        assert!(validate_test_path("invalid/../path").is_err());
        assert!(validate_test_path("invalid/path!").is_err());

        let mut index2 = WorkspaceIndex::new();
        index2.insert(
            "test".to_string(),
            SingleTestManifest::new(
                ImageHash::new("sha256", "b".repeat(64)).unwrap(),
                ImageHash::new("dhash", "1111111111111111").unwrap(),
                10,
                10,
            )
            .unwrap(),
        );
        let map = index2.into_entries();
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn test_legacy_cased_manifest_migration() {
        let temp = tempdir().unwrap();
        let manifest_dir = temp.path().join("macos-aarch64");
        std::fs::create_dir_all(manifest_dir.join("Auth")).unwrap();

        let hash = ImageHash::new("sha256", "a".repeat(64)).unwrap();
        let phash = ImageHash::new("dhash", "0000000000000000").unwrap();
        let manifest = SingleTestManifest::new(hash, phash.clone(), 100, 100).unwrap();

        // Save under legacy uppercase path Auth/Login.json
        manifest.save(manifest_dir.join("Auth/Login.json")).unwrap();

        // Load into WorkspaceIndex (canonicalizes key to auth/login)
        let mut loaded = WorkspaceIndex::load(&manifest_dir).unwrap();
        assert_eq!(loaded.entries().len(), 1);
        assert!(loaded.get("auth/login").is_some());

        // Update / save under canonical key
        let updated_hash = ImageHash::new("sha256", "b".repeat(64)).unwrap();
        let updated_manifest = SingleTestManifest::new(updated_hash, phash, 100, 100).unwrap();
        loaded
            .save_test(&manifest_dir, "auth/login", &updated_manifest)
            .unwrap();

        // Verify reloading does not find duplicates and contains updated manifest
        let reloaded = WorkspaceIndex::load(&manifest_dir).unwrap();
        assert_eq!(reloaded.entries().len(), 1);
        assert_eq!(
            reloaded.get("auth/login").unwrap().hash.value(),
            &"b".repeat(64)
        );
    }
}
