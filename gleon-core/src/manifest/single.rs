//! Single test manifest schema and serialization.

use crate::io::{load_json, save_json_atomically};
use crate::manifest::{ImageHash, ManifestError};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Supported schema version for individual test manifests.
pub const SUPPORTED_SINGLE_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Maximum allowed width or height in pixels to prevent OOM allocations.
pub const MAX_DIMENSION: u32 = 16384;

/// Maximum allowed total decoded pixels (67,108,864 = 8192x8192) to prevent decompression bombs.
pub const MAX_PIXELS: u64 = 67_108_864;

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

        Self::validate_dimensions(self.width, self.height)?;

        Ok(())
    }

    /// Validates width and height constraints.
    pub fn validate_dimensions(width: u32, height: u32) -> Result<(), ManifestError> {
        if width == 0 || height == 0 {
            return Err(ManifestError::Validation(format!(
                "Invalid image dimensions: {width}x{height} (must be > 0)"
            )));
        }

        if width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(ManifestError::Validation(format!(
                "Image dimensions {width}x{height} exceed the maximum allowed {MAX_DIMENSION}x{MAX_DIMENSION}"
            )));
        }

        let total_pixels = (width as u64) * (height as u64);
        if total_pixels > MAX_PIXELS {
            return Err(ManifestError::Validation(format!(
                "Total pixel count {total_pixels} ({width}x{height}) exceeds maximum allowed budget {MAX_PIXELS}"
            )));
        }

        Ok(())
    }

    /// Safely validates image dimensions from raw bytes before fully decoding the image.
    /// This prevents OOM (Out Of Memory) DoS attacks from decompression bombs.
    pub fn validate_image_bytes(bytes: &[u8]) -> Result<(), ManifestError> {
        let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(ManifestError::StdIo)?;
        let (width, height) = reader.into_dimensions().map_err(ManifestError::Image)?;
        Self::validate_dimensions(width, height)
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
        assert!(SingleTestManifest::validate_dimensions(16385, 100).is_err());
        assert!(SingleTestManifest::validate_dimensions(100, 16385).is_err());
        assert!(SingleTestManifest::validate_dimensions(10000, 10000).is_err());

        let valid_hash = ImageHash::new("sha256", "a".repeat(64)).unwrap();
        let invalid_phash_scheme = ImageHash::new("md5", "0000000000000000").unwrap();
        assert!(
            SingleTestManifest::new(valid_hash.clone(), invalid_phash_scheme, 100, 100).is_err()
        );

        let invalid_phash_val = ImageHash::new("dhash", "short").unwrap();
        assert!(SingleTestManifest::new(valid_hash.clone(), invalid_phash_val, 100, 100).is_err());

        let uppercase_hash = ImageHash::new("sha256", "A".repeat(64)).unwrap();
        let valid_phash = ImageHash::new("dhash", "0000000000000000").unwrap();
        let manifest_uppercase =
            SingleTestManifest::new(uppercase_hash, valid_phash.clone(), 100, 100).unwrap();
        assert_eq!(manifest_uppercase.hash.value(), "a".repeat(64));

        // Zero dimensions
        assert!(SingleTestManifest::new(valid_hash.clone(), valid_phash.clone(), 0, 100).is_err());

        // Exceeds max dimensions
        assert!(SingleTestManifest::new(valid_hash, valid_phash, MAX_DIMENSION + 1, 100).is_err());
    }

    #[test]
    fn test_unsupported_schema_version() {
        let hash = ImageHash::new("sha256", "a".repeat(64)).unwrap();
        let phash = ImageHash::new("dhash", "0000000000000000").unwrap();
        let mut manifest = SingleTestManifest::new(hash, phash, 100, 100).unwrap();
        manifest.schema_version = 99;
        assert!(manifest.validate().is_err());
    }
}
