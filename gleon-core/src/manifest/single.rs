//! Single test manifest schema and serialization.

use crate::io::{load_json, save_json_atomically};
use crate::manifest::{ImageHash, ManifestError};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Supported schema version for individual test manifests.
pub const SUPPORTED_SINGLE_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Deterministic, noise-free manifest for a single visual regression test case.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SingleTestManifest {
    /// Schema version (always 1).
    pub schema_version: u32,
    /// Primary comparison digest (must be sha256).
    pub hash: ImageHash,
    /// Perceptual hash digest (must be dhash or valid scheme).
    pub phash: ImageHash,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

impl SingleTestManifest {
    /// Creates a new `SingleTestManifest` with validation.
    pub fn new(
        hash: ImageHash,
        phash: ImageHash,
        width: u32,
        height: u32,
    ) -> Result<Self, ManifestError> {
        let manifest = Self {
            schema_version: SUPPORTED_SINGLE_MANIFEST_SCHEMA_VERSION,
            hash,
            phash,
            width,
            height,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validates field constraints and digest schemes.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version != SUPPORTED_SINGLE_MANIFEST_SCHEMA_VERSION {
            return Err(ManifestError::Validation(format!(
                "Unsupported single manifest schema version: expected {}, got {}",
                SUPPORTED_SINGLE_MANIFEST_SCHEMA_VERSION, self.schema_version
            )));
        }

        if self.hash.scheme() != "sha256" {
            return Err(ManifestError::Validation(format!(
                "Expected hash scheme 'sha256', got '{}'",
                self.hash.scheme()
            )));
        }

        if self.hash.value().len() != 64
            || !self
                .hash
                .value()
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        {
            return Err(ManifestError::Validation(format!(
                "Invalid sha256 hash value: expected 64 lowercase hex characters, got '{}'",
                self.hash.value()
            )));
        }

        if self.phash.scheme() != "dhash" {
            return Err(ManifestError::Validation(format!(
                "Expected phash scheme 'dhash', got '{}'",
                self.phash.scheme()
            )));
        }

        if self.phash.value().len() != 16
            || !self
                .phash
                .value()
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        {
            return Err(ManifestError::Validation(format!(
                "Invalid dhash value: expected 16 lowercase hex characters, got '{}'",
                self.phash.value()
            )));
        }

        if self.width == 0 || self.height == 0 {
            return Err(ManifestError::Validation(format!(
                "Invalid image dimensions: {}x{}",
                self.width, self.height
            )));
        }

        Ok(())
    }

    /// Load a single test manifest from a JSON file.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ManifestError> {
        let path = path.as_ref();
        tracing::debug!("Loading single test manifest from {:?}", path);
        let manifest: Self = load_json(path)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Save a single test manifest to a JSON file atomically.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), ManifestError> {
        let path = path.as_ref();
        tracing::debug!("Saving single test manifest to {:?}", path);
        self.validate()?;
        save_json_atomically(path, self)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_single_manifest_lifecycle() {
        let temp = tempdir().unwrap();
        let file_path = temp.path().join("login.json");

        let hash = ImageHash::new("sha256", "a".repeat(64)).unwrap();
        let phash = ImageHash::new("dhash", "0000000000000000").unwrap();

        let manifest = SingleTestManifest::new(hash, phash, 1080, 1920).unwrap();
        manifest.save(&file_path).unwrap();

        let loaded = SingleTestManifest::load(&file_path).unwrap();
        assert_eq!(manifest, loaded);
    }

    #[test]
    fn test_invalid_single_manifest() {
        let hash = ImageHash::new("md5", "abc").unwrap();
        let phash = ImageHash::new("dhash", "0000000000000000").unwrap();
        assert!(SingleTestManifest::new(hash, phash, 100, 100).is_err());

        let valid_hash = ImageHash::new("sha256", "a".repeat(64)).unwrap();
        let invalid_phash_scheme = ImageHash::new("sha256", "0000000000000000").unwrap();
        assert!(
            SingleTestManifest::new(valid_hash.clone(), invalid_phash_scheme, 100, 100).is_err()
        );

        let invalid_phash_val = ImageHash::new("dhash", "short").unwrap();
        assert!(SingleTestManifest::new(valid_hash, invalid_phash_val, 100, 100).is_err());

        let uppercase_hash = ImageHash::new("sha256", "A".repeat(64)).unwrap();
        let valid_phash = ImageHash::new("dhash", "0000000000000000").unwrap();
        assert!(SingleTestManifest::new(uppercase_hash, valid_phash, 100, 100).is_err());
    }
}
