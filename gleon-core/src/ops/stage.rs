//! Staging operation for processing, masking, and persisting baseline screenshots.

use crate::config::ConfigError;
use crate::context::{ContextError, ResolvedContext};
use crate::engine::phash::compute_phash;
use crate::git::GitResolver;
use crate::manifest::{
    ImageHash, Manifest, ManifestEntry, ManifestError, ManifestIndex,
    SUPPORTED_MANIFEST_SCHEMA_VERSION,
};
use crate::masking::apply_masks;
use crate::scanner::{FileScanner, ScannerError};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during staging.
#[derive(Debug, Error)]
pub enum StageError {
    /// Workspace has not been initialized (`.gleon` missing).
    #[error("Gleon workspace is not initialized. Please run 'gleon init' first.")]
    NotInitialized,

    /// Error resolving context.
    #[error("Context resolution error: {0}")]
    Context(#[from] ContextError),

    /// Error scanning files.
    #[error("Scanner error: {0}")]
    Scanner(#[from] ScannerError),

    /// Error loading configuration.
    #[error("Config error: {0}")]
    Config(#[from] ConfigError),

    /// Error loading or saving manifest.
    #[error("Manifest error: {0}")]
    Manifest(#[from] ManifestError),

    /// Error decoding image file.
    #[error("Image decode error for '{path}': {reason}")]
    ImageDecode { path: PathBuf, reason: String },

    /// Error encoding image file.
    #[error("Image encode error for '{path}': {reason}")]
    ImageEncode { path: PathBuf, reason: String },

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<crate::io::IoError> for StageError {
    fn from(err: crate::io::IoError) -> Self {
        match err {
            crate::io::IoError::Io(e) => StageError::Io(e),
            crate::io::IoError::JsonParse(e) => StageError::Io(std::io::Error::other(e)),
        }
    }
}

/// Result summary of staging screenshots.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StageResult {
    /// List of test case names staged.
    pub staged_test_cases: Vec<String>,
    /// Number of total screenshots staged.
    pub total_screenshots_staged: usize,
}

/// Executes staging pipeline across the workspace.
pub fn stage_workspace(
    context: &ResolvedContext,
    base_dir: &Path,
    filter_paths: Option<&[PathBuf]>,
) -> Result<StageResult, StageError> {
    let gleon_dir = base_dir.join(".gleon");
    if !gleon_dir.exists() {
        return Err(StageError::NotInitialized);
    }

    let platform_key = context.platform.to_key().map_err(ContextError::Platform)?;

    let blobs_dir = gleon_dir.join("blobs").join("sha256");
    std::fs::create_dir_all(&blobs_dir).map_err(StageError::Io)?;

    let branch_dir = gleon_dir
        .join("branches")
        .join(&context.branch)
        .join(&platform_key);
    std::fs::create_dir_all(&branch_dir).map_err(StageError::Io)?;

    let index_path = branch_dir.join("manifest_index.json");

    let manifest_index_opt = match ManifestIndex::load(&index_path) {
        Ok(idx) => Some(idx),
        Err(ManifestError::Io(crate::io::IoError::Io(e)))
            if e.kind() == std::io::ErrorKind::NotFound =>
        {
            None
        }
        Err(e) => return Err(StageError::Manifest(e)),
    };

    let config = context.config.as_ref().cloned().unwrap_or_default();

    // Scan workspace screenshots
    let test_cases = FileScanner::scan_workspace(&config, base_dir).map_err(StageError::Scanner)?;

    let commit_author =
        GitResolver::get_commit_author(base_dir, "HEAD").unwrap_or_else(|_| "unknown".to_string());
    let commit_sha =
        GitResolver::get_head_commit_sha(base_dir).unwrap_or_else(|_| "uncommitted".to_string());

    let mut staged_test_cases = Vec::new();
    let mut total_screenshots_staged = 0;

    let mut test_manifest_map = BTreeMap::new();

    for case in test_cases {
        // If filter_paths is specified, skip test cases that don't match any path
        if let Some(filters) = filter_paths {
            let matched = case.images.iter().any(|img| {
                filters
                    .iter()
                    .any(|f| img.relative_path.starts_with(f) || f.starts_with(&img.relative_path))
            });
            if !matched {
                continue;
            }
        }

        let existing_manifest = match manifest_index_opt
            .as_ref()
            .and_then(|idx| idx.test_manifests.get(&case.name))
        {
            Some(hash) => {
                let manifest_blob_path = blobs_dir.join(hash.value());
                Some(Manifest::load(manifest_blob_path).map_err(StageError::Manifest)?)
            }
            None => None,
        };

        let mut manifest_entries = existing_manifest.map(|m| m.entries).unwrap_or_default();

        for img in &case.images {
            if let Some(filters) = filter_paths {
                let matches_filter = filters
                    .iter()
                    .any(|f| img.relative_path.starts_with(f) || f.starts_with(&img.relative_path));
                if !matches_filter {
                    continue;
                }
            }

            let matched_zones = case.rule.matched_mask_zones(&img.relative_path);

            let (png_bytes, width, height, rgba_img) = if !matched_zones.is_empty() {
                let dynamic_img =
                    image::open(&img.absolute_path).map_err(|e| StageError::ImageDecode {
                        path: img.relative_path.clone(),
                        reason: e.to_string(),
                    })?;
                let mut rgba = dynamic_img.to_rgba8();
                apply_masks(&mut rgba, &matched_zones);
                let w = rgba.width();
                let h = rgba.height();

                let mut encoded = Vec::new();
                let mut cursor = Cursor::new(&mut encoded);
                rgba.write_to(&mut cursor, image::ImageFormat::Png)
                    .map_err(|e| StageError::ImageEncode {
                        path: img.relative_path.clone(),
                        reason: e.to_string(),
                    })?;
                (encoded, w, h, rgba)
            } else {
                let raw_bytes = std::fs::read(&img.absolute_path).map_err(StageError::Io)?;
                let dynamic_img =
                    image::load_from_memory(&raw_bytes).map_err(|e| StageError::ImageDecode {
                        path: img.relative_path.clone(),
                        reason: e.to_string(),
                    })?;
                let w = dynamic_img.width();
                let h = dynamic_img.height();
                let rgba = dynamic_img.to_rgba8();
                (raw_bytes, w, h, rgba)
            };

            // Compute perceptual hash (pHash)
            let phash_str = compute_phash(&rgba_img);

            // Compute SHA-256 of PNG blob
            let sha256_hex = hex::encode(Sha256::digest(&png_bytes));

            // Save image blob to .gleon/blobs/sha256/<sha256_hex>
            let blob_path = blobs_dir.join(&sha256_hex);
            if !blob_path.exists() {
                crate::io::save_file_atomically(&blob_path, &png_bytes)
                    .map_err(StageError::from)?;
            }

            let rel_path_str = FileScanner::normalize_path_str(&img.relative_path).into_owned();

            let is_unchanged = manifest_entries
                .get(&rel_path_str)
                .is_some_and(|existing| existing.hash.value() == sha256_hex);

            if !is_unchanged {
                let entry = ManifestEntry {
                    hash: ImageHash::new("sha256", &sha256_hex).map_err(StageError::Manifest)?,
                    phash: phash_str
                        .parse::<ImageHash>()
                        .map_err(StageError::Manifest)?,
                    width,
                    height,
                    created_at: chrono::Utc::now(),
                    created_by: commit_author.clone(),
                    source_commit: commit_sha.clone(),
                };
                manifest_entries.insert(rel_path_str, entry);
                total_screenshots_staged += 1;
            }
        }

        if manifest_entries.is_empty() {
            continue;
        }

        let test_manifest = Manifest {
            schema_version: SUPPORTED_MANIFEST_SCHEMA_VERSION,
            version: 1,
            hash_algo: "sha256".to_string(),
            pixel_format: "rgba".to_string(),
            generator_version: env!("CARGO_PKG_VERSION").to_string(),
            entries: manifest_entries,
        };

        // Serialize test manifest to JSON bytes to compute its content-addressed blob hash
        let manifest_json = serde_json::to_vec_pretty(&test_manifest)
            .map_err(|e| ManifestError::Validation(e.to_string()))?;
        let test_manifest_sha256 = hex::encode(Sha256::digest(&manifest_json));

        // Save test manifest blob to .gleon/blobs/sha256/<test_manifest_sha256>
        let test_manifest_blob_path = blobs_dir.join(&test_manifest_sha256);
        crate::io::save_file_atomically(&test_manifest_blob_path, &manifest_json)
            .map_err(StageError::from)?;

        test_manifest_map.insert(
            case.name.clone(),
            ImageHash::new("sha256", &test_manifest_sha256).map_err(StageError::Manifest)?,
        );

        staged_test_cases.push(case.name);
    }

    // Update manifest_index.json atomically
    ManifestIndex::update(&index_path, |index| {
        for (test_name, manifest_hash) in test_manifest_map {
            index.test_manifests.insert(test_name, manifest_hash);
        }
    })
    .map_err(StageError::Manifest)?;

    Ok(StageResult {
        staged_test_cases,
        total_screenshots_staged,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_error_display() {
        let err1 = StageError::NotInitialized;
        assert!(err1.to_string().contains("not initialized"));

        let err2 = StageError::Context(ContextError::Platform(
            crate::platform::PlatformError::InvalidSegment("test".to_string()),
        ));
        assert!(err2.to_string().contains("Context resolution error"));

        let err3 = StageError::Scanner(ScannerError::InvalidTestName {
            name: "bad/name".to_string(),
            reason: "reason".to_string(),
        });
        assert!(err3.to_string().contains("Scanner error"));

        let err4 = StageError::Config(ConfigError::Validation("bad config".to_string()));
        assert!(err4.to_string().contains("Config error"));

        let err5 = StageError::Manifest(ManifestError::Validation("bad manifest".to_string()));
        assert!(err5.to_string().contains("Manifest error"));

        let err6 = StageError::ImageDecode {
            path: PathBuf::from("a.png"),
            reason: "corrupt".to_string(),
        };
        assert!(err6.to_string().contains("Image decode error"));

        let err7 = StageError::ImageEncode {
            path: PathBuf::from("b.png"),
            reason: "write error".to_string(),
        };
        assert!(err7.to_string().contains("Image encode error"));

        let err8 = StageError::Io(std::io::Error::other("io test"));
        assert!(err8.to_string().contains("IO error"));
    }

    #[test]
    fn test_stage_result_derived() {
        let res = StageResult {
            staged_test_cases: vec!["test1".to_string()],
            total_screenshots_staged: 1,
        };
        let cloned = res.clone();
        assert_eq!(res, cloned);
        assert!(!format!("{:?}", res).is_empty());
        let default_res = StageResult::default();
        assert_eq!(default_res.total_screenshots_staged, 0);
    }
}
