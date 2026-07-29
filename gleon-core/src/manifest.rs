//! Manifest definitions for gleon.

pub mod conflict;
pub mod index;
pub mod single;

pub use conflict::{ConflictManifest, ConflictParseError, parse_conflict_manifest};
pub use index::{WorkspaceIndex, validate_test_path};
pub use single::{SUPPORTED_SINGLE_MANIFEST_SCHEMA_VERSION, SingleTestManifest};

use crate::io::IoError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Errors that can occur during manifest operations.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// IO or JSON serialization error.
    #[error("IO error: {0}")]
    Io(#[from] IoError),

    /// Standard I/O error.
    #[error("I/O error: {0}")]
    StdIo(#[source] std::io::Error),

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
        validate_hash_parts(&scheme_cow, value)
            .map_err(ManifestError::Validation)
            .map(|()| ImageHash {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_hash_parse_and_display() {
        let hash = "sha256:a1b2c3d4e5f67890".parse::<ImageHash>().unwrap();
        assert_eq!(hash.scheme(), "sha256");
        assert_eq!(hash.value(), "a1b2c3d4e5f67890");
        assert_eq!(hash.to_string(), "sha256:a1b2c3d4e5f67890");
    }
}
