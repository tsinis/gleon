//! Manifest definitions for gleon.

use crate::config::ConfigError;
use crate::io::{load_json, save_json_atomically};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::path::Path;

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
    pub fn new(scheme: impl Into<String>, value: impl Into<String>) -> Result<Self, ConfigError> {
        let scheme_str = scheme.into().to_lowercase();
        let value_str = value.into();
        validate_hash_parts(&scheme_str, &value_str)
            .map_err(ConfigError::Validation)
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

impl<'de> Deserialize<'de> for ImageHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).and_then(|s| {
            let (scheme, value) = s
                .split_once(':')
                .ok_or_else(|| serde::de::Error::custom("Hash must be in 'scheme:value' format"))?;

            let scheme_lower = scheme.to_lowercase();
            validate_hash_parts(&scheme_lower, value).map_err(serde::de::Error::custom)?;

            Ok(ImageHash {
                scheme: scheme_lower,
                value: value.to_string(),
            })
        })
    }
}

impl Serialize for ImageHash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = format!("{}:{}", self.scheme, self.value);
        serializer.serialize_str(&s)
    }
}

impl std::fmt::Display for ImageHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.scheme, self.value)
    }
}

fn deserialize_lowercase_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    String::deserialize(d).map(|s| s.to_lowercase())
}

/// The manifest file structure representing previous run results.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema_version: u64,
    /// The hashing algorithm used (always lowercase, e.g. "sha256").
    #[serde(deserialize_with = "deserialize_lowercase_string")]
    pub hash_algo: String,
    pub pixel_format: String,
    pub generator_version: String,
    pub entries: BTreeMap<String, ManifestEntry>,
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
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != SUPPORTED_MANIFEST_SCHEMA_VERSION {
            return Err(ConfigError::Validation(format!(
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
                return Err(ConfigError::Validation(format!(
                    "Manifest entry '{}' has hash scheme '{}', but manifest hash_algo is '{}'",
                    path,
                    entry.hash.scheme(),
                    self.hash_algo
                )));
            }
            if algo_lower == "sha256" {
                if entry.hash.value().len() != 64 {
                    return Err(ConfigError::Validation(format!(
                        "Manifest entry '{}' has invalid sha256 hash length: expected 64 hex characters, got {}",
                        path,
                        entry.hash.value().len()
                    )));
                }
                if !entry.hash.value().chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(ConfigError::Validation(format!(
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
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        tracing::debug!("Loading manifest from {:?}", path);
        load_json::<Self, _>(path).and_then(|manifest| manifest.validate().map(|_| manifest))
    }

    /// Save a manifest to a JSON file atomically.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), ConfigError> {
        let path = path.as_ref();
        tracing::info!("Saving manifest to {:?}", path);
        self.validate()
            .and_then(|_| save_json_atomically(path, self))
            .map(|_| {
                tracing::debug!("Manifest saved successfully to {:?}", path);
            })
    }
}

impl ManifestIndex {
    /// Validates that the schema version is supported.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION {
            return Err(ConfigError::Validation(format!(
                "Unsupported manifest index schema version: expected {}, got {}",
                SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION, self.schema_version
            )));
        }
        Ok(())
    }

    /// Load a manifest index from a JSON file.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        tracing::debug!("Loading manifest index from {:?}", path);
        load_json::<Self, _>(path).and_then(|index| index.validate().map(|_| index))
    }

    /// Save a manifest index to a JSON file atomically.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), ConfigError> {
        let path = path.as_ref();
        tracing::info!("Saving manifest index to {:?}", path);
        self.validate()
            .and_then(|_| save_json_atomically(path, self))
            .map(|_| {
                tracing::debug!("Manifest index saved successfully to {:?}", path);
            })
    }
}
