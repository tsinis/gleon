//! Manifest definitions for gleon.

use crate::io::{IoError, load_json, save_json_atomically, update_json_atomically};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::path::Path;

/// Errors that can occur during manifest operations.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// IO or JSON serialization error.
    #[error("IO error: {0}")]
    Io(#[from] IoError),

    /// Validation error in manifest schema or entry content.
    #[error("Validation error: {0}")]
    Validation(String),
}

/// A strongly-typed image comparison hash, serialized as a `scheme:value` string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImageHash {
    /// The hashing scheme/algorithm (e.g. "sha256", "phash", "dhash", "ssim").
    scheme: String,
    /// The hex or alphanumeric representation of the hash.
    value: String,
}

fn validate_hash_parts(scheme: &str, value: &str) -> Result<(), String> {
    if scheme.is_empty() {
        return Err("Hash scheme cannot be empty".to_string());
    }
    if !scheme
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("Hash scheme contains invalid characters".to_string());
    }
    if value.is_empty() {
        return Err("Hash value cannot be empty".to_string());
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("Hash value contains invalid characters".to_string());
    }
    Ok(())
}

impl ImageHash {
    /// Constructs a new ImageHash, returning a validation error if invalid.
    pub fn new(scheme: impl Into<String>, value: impl Into<String>) -> Result<Self, ManifestError> {
        let mut scheme_str = scheme.into();
        if scheme_str.chars().any(|c| c.is_ascii_uppercase()) {
            scheme_str.make_ascii_lowercase();
        }
        let value_str = value.into();
        validate_hash_parts(&scheme_str, &value_str)
            .map_err(ManifestError::Validation)
            .map(|_| Self {
                scheme: scheme_str,
                value: value_str,
            })
    }

    /// Gets the hashing scheme.
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// Gets the hash value.
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl std::str::FromStr for ImageHash {
    type Err = ManifestError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (scheme, value) = s.split_once(':').ok_or_else(|| {
            ManifestError::Validation("Hash must be in 'scheme:value' format".to_string())
        })?;

        let scheme_cow = if scheme.chars().any(|c| c.is_ascii_uppercase()) {
            std::borrow::Cow::Owned(scheme.to_ascii_lowercase())
        } else {
            std::borrow::Cow::Borrowed(scheme)
        };
        validate_hash_parts(&scheme_cow, value).map_err(ManifestError::Validation)?;

        Ok(ImageHash {
            scheme: scheme_cow.into_owned(),
            value: value.to_string(),
        })
    }
}

impl<'de> Deserialize<'de> for ImageHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = std::borrow::Cow::<'de, str>::deserialize(deserializer)?;
        s.parse::<ImageHash>().map_err(serde::de::Error::custom)
    }
}

impl Serialize for ImageHash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl std::fmt::Display for ImageHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.scheme, self.value)
    }
}

fn deserialize_lowercase_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let cow = std::borrow::Cow::<'de, str>::deserialize(d)?;
    if cow.chars().any(|c| c.is_ascii_uppercase()) {
        Ok(cow.to_ascii_lowercase())
    } else {
        Ok(cow.into_owned())
    }
}

/// The manifest file structure representing previous run results.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema_version: u64,
    pub version: u64,
    /// The hashing algorithm used (always lowercase, e.g. "sha256").
    #[serde(deserialize_with = "deserialize_lowercase_string")]
    pub hash_algo: String,
    pub pixel_format: String,
    pub generator_version: String,
    #[serde(deserialize_with = "deserialize_normalized_entries")]
    pub entries: BTreeMap<String, ManifestEntry>,
}

fn deserialize_normalized_entries<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, ManifestEntry>, D::Error>
where
    D: Deserializer<'de>,
{
    let map = BTreeMap::<String, ManifestEntry>::deserialize(deserializer)?;
    if map.keys().any(|k| k.contains('\\')) {
        let mut normalized = BTreeMap::new();
        for (k, v) in map {
            normalized.insert(k.replace('\\', "/"), v);
        }
        Ok(normalized)
    } else {
        Ok(map)
    }
}

/// Metadata entry for a single verified screenshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub hash: ImageHash,
    pub phash: ImageHash,
    pub width: u32,
    pub height: u32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: String,
    pub source_commit: String,
}

pub const SUPPORTED_MANIFEST_SCHEMA_VERSION: u64 = 1;
pub const SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION: u64 = 1;

/// The index mapping test paths to their respective manifest hashes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ManifestIndex {
    pub schema_version: u64,
    pub test_manifests: BTreeMap<String, ImageHash>,
}

impl Manifest {
    /// Validates that entry schemes match the manifest hash algorithm,
    /// and that sha256 digests are structurally valid (64 hex characters).
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version != SUPPORTED_MANIFEST_SCHEMA_VERSION {
            return Err(ManifestError::Validation(format!(
                "Unsupported manifest schema version: expected {}, got {}",
                SUPPORTED_MANIFEST_SCHEMA_VERSION, self.schema_version
            )));
        }
        // Normalize for comparison: hash_algo is lowercase when loaded from disk via serde,
        // but Manifest can also be constructed programmatically with any casing.
        let algo_lower: std::borrow::Cow<'_, str> =
            if self.hash_algo.chars().all(|c| !c.is_uppercase()) {
                std::borrow::Cow::Borrowed(&self.hash_algo)
            } else {
                std::borrow::Cow::Owned(self.hash_algo.to_lowercase())
            };
        for (path, entry) in &self.entries {
            if entry.hash.scheme() != algo_lower {
                return Err(ManifestError::Validation(format!(
                    "Manifest entry '{}' has hash scheme '{}', but manifest hash_algo is '{}'",
                    path,
                    entry.hash.scheme(),
                    self.hash_algo
                )));
            }
            if algo_lower == "sha256" {
                if entry.hash.value().len() != 64 {
                    return Err(ManifestError::Validation(format!(
                        "Manifest entry '{}' has invalid sha256 hash length: expected 64 hex characters, got {}",
                        path,
                        entry.hash.value().len()
                    )));
                }
                if !entry.hash.value().chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(ManifestError::Validation(format!(
                        "Manifest entry '{}' has invalid sha256 hash value: expected hex characters, got '{}'",
                        path,
                        entry.hash.value()
                    )));
                }
            }
            // phash is already validated via ImageHash validation logic
        }
        Ok(())
    }

    /// Load a manifest from a JSON file.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ManifestError> {
        let path = path.as_ref();
        tracing::debug!("Loading manifest from {:?}", path);
        let manifest: Self = load_json(path)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Save a manifest to a JSON file atomically.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), ManifestError> {
        let path = path.as_ref();
        tracing::info!("Saving manifest to {:?}", path);
        self.validate()?;
        save_json_atomically(path, self)?;
        tracing::debug!("Manifest saved successfully to {:?}", path);
        Ok(())
    }

    /// Load, modify, and save a manifest atomically under an exclusive file lock.
    pub fn update<P: AsRef<Path>, F: FnOnce(&mut Self)>(
        path: P,
        f: F,
    ) -> Result<(), ManifestError> {
        let path = path.as_ref();
        tracing::info!("Updating manifest atomically at {:?}", path);
        update_json_atomically(
            path,
            || Self {
                schema_version: SUPPORTED_MANIFEST_SCHEMA_VERSION,
                version: 1,
                hash_algo: "sha256".to_string(),
                pixel_format: "rgba".to_string(),
                generator_version: "unknown".to_string(),
                entries: BTreeMap::new(),
            },
            |manifest: &mut Self| {
                f(manifest);
                manifest.validate()
            },
        )
        .map(|_| {
            tracing::debug!("Manifest updated successfully to {:?}", path);
        })
    }
}

impl ManifestIndex {
    /// Validates that the schema version is supported.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version != SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION {
            return Err(ManifestError::Validation(format!(
                "Unsupported manifest index schema version: expected {}, got {}",
                SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION, self.schema_version
            )));
        }
        Ok(())
    }

    /// Load a manifest index from a JSON file.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ManifestError> {
        let path = path.as_ref();
        tracing::debug!("Loading manifest index from {:?}", path);
        let index: Self = load_json(path)?;
        index.validate()?;
        Ok(index)
    }

    /// Save a manifest index to a JSON file atomically.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), ManifestError> {
        let path = path.as_ref();
        tracing::info!("Saving manifest index to {:?}", path);
        self.validate()?;
        save_json_atomically(path, self)?;
        tracing::debug!("Manifest index saved successfully to {:?}", path);
        Ok(())
    }

    /// Load, modify, and save a manifest index atomically under an exclusive file lock.
    pub fn update<P: AsRef<Path>, F: FnOnce(&mut Self)>(
        path: P,
        f: F,
    ) -> Result<(), ManifestError> {
        let path = path.as_ref();
        tracing::info!("Updating manifest index atomically at {:?}", path);
        update_json_atomically(
            path,
            || Self {
                schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
                test_manifests: BTreeMap::new(),
            },
            |index: &mut Self| {
                f(index);
                index.validate()
            },
        )
        .map(|_| {
            tracing::debug!("Manifest index updated successfully at {:?}", path);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_hash_invalid_format_deserialization() {
        // Missing colon
        let bad_json_1 =
            "\"sha2560000000000000000000000000000000000000000000000000000000000000000\"";
        assert!(serde_json::from_str::<ImageHash>(bad_json_1).is_err());

        // Empty scheme
        let bad_json_2 = "\":00000000\"";
        assert!(serde_json::from_str::<ImageHash>(bad_json_2).is_err());

        // Empty value
        let bad_json_3 = "\"sha256:\"";
        assert!(serde_json::from_str::<ImageHash>(bad_json_3).is_err());

        // Invalid characters in value
        let bad_json_4 = "\"sha256:invalid?hash\"";
        assert!(serde_json::from_str::<ImageHash>(bad_json_4).is_err());

        // Invalid characters in scheme
        let bad_json_5 = "\"sha:256:00000000\"";
        assert!(serde_json::from_str::<ImageHash>(bad_json_5).is_err());
    }

    #[test]
    fn test_image_hash_display() {
        let hash = ImageHash::new(
            "sha256",
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        let formatted = format!("{}", hash);
        assert_eq!(
            formatted,
            "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn test_image_hash_constructor_validation() {
        assert!(ImageHash::new("sha256", "abc").is_ok());
        assert!(ImageHash::new("sha:256", "abc").is_err());
        assert!(ImageHash::new("", "abc").is_err());
        assert!(ImageHash::new("sha256", "").is_err());
    }

    #[test]
    fn test_image_hash_from_str_missing_colon_fails() {
        let err = "0123456789abcdef".parse::<ImageHash>().unwrap_err();
        assert!(matches!(err, ManifestError::Validation(ref msg) if msg.contains("scheme:value")));
    }

    #[test]
    fn test_manifest_json_snapshot() {
        use std::path::PathBuf;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("manifest.json");

        let mut entries = BTreeMap::new();

        let _ = entries.insert(
            "src/login.png".to_string(),
            ManifestEntry {
                hash: ImageHash::new(
                    "sha256",
                    "0000000000000000000000000000000000000000000000000000000000000000",
                )
                .unwrap(),
                phash: ImageHash::new("dhash", "0123456789abcdef").unwrap(),
                width: 800,
                height: 600,
                created_at: chrono::DateTime::from_timestamp(1710000000, 0).unwrap(),
                created_by: "Test User <test@example.com>".to_string(),
                source_commit: "abcdef1234567890".to_string(),
            },
        );

        let manifest = Manifest {
            schema_version: 1,
            version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries,
        };

        manifest.save(&file_path).unwrap();

        let generated_json = std::fs::read_to_string(&file_path).unwrap();

        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let fixture_path = manifest_dir.join("tests/fixtures/default_manifest.json");
        let expected_json = std::fs::read_to_string(fixture_path).unwrap();

        assert_eq!(generated_json.trim(), expected_json.trim());
    }

    #[test]
    fn test_manifest_index_serialization() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("manifest_index.json");

        let mut test_manifests = BTreeMap::new();
        test_manifests.insert(
            "tests/ui/login".to_string(),
            ImageHash::new(
                "sha256",
                "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            )
            .unwrap(),
        );

        let index = ManifestIndex {
            schema_version: 1,
            test_manifests,
        };

        index.save(&file_path).unwrap();

        let loaded_index = ManifestIndex::load(&file_path).unwrap();
        assert_eq!(index, loaded_index);
    }

    #[test]
    fn test_manifest_roundtrip() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("manifest.json");

        let mut screenshots = BTreeMap::new();
        let _ = screenshots.insert(
            "src/login.png".to_string(),
            ManifestEntry {
                hash: ImageHash::new(
                    "sha256",
                    "0000000000000000000000000000000000000000000000000000000000000000",
                )
                .unwrap(),
                phash: ImageHash::new("dhash", "0123456789abcdef").unwrap(),
                width: 800,
                height: 600,
                created_at: chrono::DateTime::from_timestamp(1710000000, 0).unwrap(),
                created_by: "Test User <test@example.com>".to_string(),
                source_commit: "abcdef123".to_string(),
            },
        );

        let manifest = Manifest {
            schema_version: 1,
            version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: screenshots,
        };

        manifest.save(&file_path).unwrap();
        let loaded = Manifest::load(&file_path).unwrap();
        assert_eq!(manifest, loaded);
    }

    #[test]
    fn test_manifest_validation_failures() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("invalid_manifest.json");

        // 1. Hash algorithm mismatch
        let mut entries = BTreeMap::new();
        entries.insert(
            "test.png".to_string(),
            ManifestEntry {
                hash: ImageHash::new(
                    "pixel",
                    "0000000000000000000000000000000000000000000000000000000000000000",
                )
                .unwrap(),
                phash: ImageHash::new("dhash", "0000000000000000").unwrap(),
                width: 10,
                height: 10,
                created_at: chrono::DateTime::from_timestamp(1710000000, 0).unwrap(),
                created_by: "user".to_string(),
                source_commit: "abc".to_string(),
            },
        );
        let manifest_mismatch = Manifest {
            schema_version: 1,
            version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: entries.clone(),
        };
        let save_err = manifest_mismatch.save(&file_path).unwrap_err();
        assert!(
            matches!(save_err, ManifestError::Validation(ref msg) if msg.contains("hash scheme"))
        );

        // 2. Truncated sha256 value
        let mut entries_truncated = BTreeMap::new();
        entries_truncated.insert(
            "test.png".to_string(),
            ManifestEntry {
                hash: ImageHash::new("sha256", "0".repeat(63)).unwrap(),
                phash: ImageHash::new("dhash", "0000000000000000").unwrap(),
                width: 10,
                height: 10,
                created_at: chrono::DateTime::from_timestamp(1710000000, 0).unwrap(),
                created_by: "user".to_string(),
                source_commit: "abc".to_string(),
            },
        );
        let manifest_truncated = Manifest {
            schema_version: 1,
            version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: entries_truncated,
        };
        let save_err2 = manifest_truncated.save(&file_path).unwrap_err();
        assert!(
            matches!(save_err2, ManifestError::Validation(ref msg) if msg.contains("invalid sha256 hash length"))
        );

        // 3. Non-hex characters in sha256
        let mut entries_non_hex = BTreeMap::new();
        entries_non_hex.insert(
            "test.png".to_string(),
            ManifestEntry {
                hash: ImageHash::new("sha256", "z".repeat(64)).unwrap(),
                phash: ImageHash::new("dhash", "0000000000000000").unwrap(),
                width: 10,
                height: 10,
                created_at: chrono::DateTime::from_timestamp(1710000000, 0).unwrap(),
                created_by: "user".to_string(),
                source_commit: "abc".to_string(),
            },
        );
        let manifest_non_hex = Manifest {
            schema_version: 1,
            version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: entries_non_hex,
        };
        let save_err3 = manifest_non_hex.save(&file_path).unwrap_err();
        assert!(
            matches!(save_err3, ManifestError::Validation(ref msg) if msg.contains("invalid sha256 hash value"))
        );

        // 4. Case-insensitive hash algorithm validation
        let mut entries_mixed_case = BTreeMap::new();
        entries_mixed_case.insert(
            "test.png".to_string(),
            ManifestEntry {
                hash: ImageHash::new("sHa256", "0".repeat(64)).unwrap(),
                phash: ImageHash::new("dhash", "0000000000000000").unwrap(),
                width: 10,
                height: 10,
                created_at: chrono::DateTime::from_timestamp(1710000000, 0).unwrap(),
                created_by: "user".to_string(),
                source_commit: "abc".to_string(),
            },
        );
        let manifest_mixed_case = Manifest {
            schema_version: 1,
            version: 1,
            hash_algo: "ShA256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: entries_mixed_case,
        };
        assert!(manifest_mixed_case.save(&file_path).is_ok());

        // 5. Non-sha256 algorithm validation
        let mut entries_phash = BTreeMap::new();
        entries_phash.insert(
            "test.png".to_string(),
            ManifestEntry {
                hash: ImageHash::new("phash", "abc").unwrap(),
                phash: ImageHash::new("dhash", "0000000000000000").unwrap(),
                width: 10,
                height: 10,
                created_at: chrono::DateTime::from_timestamp(1710000000, 0).unwrap(),
                created_by: "user".to_string(),
                source_commit: "abc".to_string(),
            },
        );
        let manifest_phash = Manifest {
            schema_version: 1,
            version: 1,
            hash_algo: "phash".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: entries_phash,
        };
        assert!(manifest_phash.save(&file_path).is_ok());
    }

    #[test]
    fn test_manifest_atomic_save_success() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("manifest.json");

        let manifest = Manifest {
            schema_version: 1,
            version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: BTreeMap::new(),
        };
        manifest.save(&file_path).unwrap();
        let loaded = Manifest::load(&file_path).unwrap();
        assert_eq!(manifest, loaded);
    }

    #[test]
    fn test_manifest_save_invalid_filename() {
        let manifest = Manifest {
            schema_version: 1,
            version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: BTreeMap::new(),
        };
        let err = manifest.save(Path::new("/")).unwrap_err();
        assert!(matches!(err, ManifestError::Io(_)));
    }

    #[test]
    fn test_manifest_save_persist_failure() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("manifest_dir");
        std::fs::create_dir(&file_path).unwrap();

        let manifest = Manifest {
            schema_version: 1,
            version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: BTreeMap::new(),
        };

        let err = manifest.save(&file_path).unwrap_err();
        assert!(matches!(err, ManifestError::Io(_)));
    }

    #[test]
    fn test_manifest_load_not_found() {
        use std::path::PathBuf;

        let path = PathBuf::from("nonexistent_manifest_file.json");
        let err = Manifest::load(&path).unwrap_err();
        assert!(matches!(err, ManifestError::Io(_)));
    }

    #[test]
    fn test_manifest_save_creates_parent_dir() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let nested_path = dir.path().join("subdir/nested/manifest.json");

        let manifest = Manifest {
            schema_version: 1,
            version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: BTreeMap::new(),
        };
        manifest.save(&nested_path).unwrap();

        assert!(nested_path.exists());
        let loaded = Manifest::load(&nested_path).unwrap();
        assert_eq!(loaded.schema_version, 1);
    }

    #[test]
    fn test_manifest_save_create_dir_all_failure() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let blocker_file = dir.path().join("blocker-file");
        std::fs::write(&blocker_file, b"data").unwrap();

        let nested_path = blocker_file.join("subdir/manifest.json");
        let manifest = Manifest {
            schema_version: 1,
            version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: BTreeMap::new(),
        };
        let err = manifest.save(&nested_path).unwrap_err();
        assert!(matches!(err, ManifestError::Io(_)));
    }

    #[test]
    fn test_manifest_validation_unsupported_version() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("unsupported_manifest.json");
        let manifest = Manifest {
            schema_version: 2,
            version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: "1.0.0".to_string(),
            entries: BTreeMap::new(),
        };
        assert!(manifest.save(&file_path).is_err());
    }

    #[test]
    fn test_manifest_update_validates_mutated_content() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("manifest.json");

        let result = Manifest::update(&file_path, |manifest| {
            manifest.schema_version = 99;
        });

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ManifestError::Validation(_)));
    }

    #[test]
    fn test_manifest_index_validation_failures() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("invalid_index.json");

        let index_bad_version = ManifestIndex {
            schema_version: 2,
            test_manifests: BTreeMap::new(),
        };
        assert!(index_bad_version.save(&file_path).is_err());

        let bad_json = r#"{
            "schemaVersion": 1,
            "testManifests": {
                "tests/ui/login": "not-prefixed-hash-value"
            }
        }"#;
        std::fs::write(&file_path, bad_json).unwrap();
        assert!(ManifestIndex::load(&file_path).is_err());
    }

    #[test]
    fn test_image_hash_deserialization_errors() {
        // Non-string type (number)
        let res_num: Result<ImageHash, _> = serde_json::from_str("123");
        assert!(res_num.is_err());
    }

    #[test]
    fn test_manifest_load_corrupted() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("corrupted_manifest.json");
        std::fs::write(&file_path, "{ invalid json }").unwrap();

        let result = Manifest::load(&file_path);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ManifestError::Io(IoError::JsonParse(_))
        ));
    }
}
