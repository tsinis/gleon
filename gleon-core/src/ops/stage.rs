//! Staging operation for processing, masking, and persisting baseline screenshots.

use crate::config::ConfigError;
use crate::context::{ContextError, ResolvedContext};
use crate::engine::phash::compute_phash;
use crate::manifest::{ImageHash, ManifestError, SingleTestManifest, WorkspaceIndex};
use crate::masking::apply_masks;
use crate::scanner::{FileScanner, ScannerError};
use sha2::{Digest, Sha256};
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
    #[error("Image decode error for '{path}'")]
    ImageDecode {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },

    /// Error encoding image file.
    #[error("Image encode error for '{path}'")]
    ImageEncode {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },

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

    let platform_key = match context.platform.to_key() {
        Ok(key) => key,
        Err(e) => return Err(StageError::Context(ContextError::Platform(e))),
    };

    let blobs_dir = gleon_dir.join("blobs").join("sha256");
    let manifests_dir = gleon_dir.join("manifests").join(&platform_key);
    std::fs::create_dir_all(&blobs_dir).map_err(StageError::Io)?;
    std::fs::create_dir_all(&manifests_dir).map_err(StageError::Io)?;

    let config = context.config.as_ref().cloned().unwrap_or_default();

    // Scan workspace screenshots
    let test_cases = FileScanner::scan_workspace(&config, base_dir).map_err(StageError::Scanner)?;

    let mut workspace_index = WorkspaceIndex::load(&manifests_dir)?;

    let mut staged_test_cases = Vec::new();
    let mut total_screenshots_staged = 0;

    use rayon::prelude::*;

    let processed_results: Result<Vec<_>, StageError> = test_cases
        .into_par_iter()
        .filter(|case| {
            if let Some(filters) = filter_paths {
                filters.iter().any(|f| {
                    case.image.absolute_path.starts_with(f)
                        || case.image.relative_path.starts_with(f)
                        || f.starts_with(&case.image.relative_path)
                })
            } else {
                true
            }
        })
        .map(|case| {
            let matched_zones = case.rule.matched_mask_zones(&case.image.relative_path);

            let (png_bytes, width, height, rgba_img) = if !matched_zones.is_empty() {
                let dynamic_img = image::open(&case.image.absolute_path).map_err(|source| {
                    StageError::ImageDecode {
                        path: case.image.relative_path.clone(),
                        source,
                    }
                })?;
                let mut rgba = dynamic_img.to_rgba8();
                apply_masks(&mut rgba, &matched_zones);
                let w = rgba.width();
                let h = rgba.height();

                let mut encoded = Vec::new();
                let mut cursor = Cursor::new(&mut encoded);
                rgba.write_to(&mut cursor, image::ImageFormat::Png)
                    .map_err(|source| StageError::ImageEncode {
                        path: case.image.relative_path.clone(),
                        source,
                    })?;
                (encoded, w, h, rgba)
            } else {
                let raw_bytes = std::fs::read(&case.image.absolute_path).map_err(StageError::Io)?;
                let dynamic_img = image::load_from_memory(&raw_bytes).map_err(|source| {
                    StageError::ImageDecode {
                        path: case.image.relative_path.clone(),
                        source,
                    }
                })?;
                let w = dynamic_img.width();
                let h = dynamic_img.height();
                let rgba = dynamic_img.to_rgba8();
                (raw_bytes, w, h, rgba)
            };

            let phash_str = compute_phash(&rgba_img);
            let sha256_hex = hex::encode(Sha256::digest(&png_bytes));

            // Save blob to .gleon/blobs/sha256/<sha256_hex>
            let blob_path = blobs_dir.join(&sha256_hex);
            crate::io::save_file_atomically(&blob_path, &png_bytes).map_err(StageError::from)?;

            Ok((case.name, sha256_hex, phash_str, width, height))
        })
        .collect();

    let processed_results = processed_results?;

    // Clean up orphan manifests when performing a full workspace stage (no path filters)
    if filter_paths.is_none() {
        let scanned_names: std::collections::HashSet<_> = processed_results
            .iter()
            .map(|(name, ..)| name.as_str())
            .collect();
        let existing_names: Vec<_> = workspace_index.entries().keys().cloned().collect();
        for existing in existing_names {
            if !scanned_names.contains(existing.as_str()) {
                workspace_index.remove_test(&manifests_dir, &existing)?;
            }
        }
    }

    for (case_name, sha256_hex, phash_str, width, height) in processed_results {
        let hash = ImageHash::new("sha256", &sha256_hex).map_err(StageError::Manifest)?;
        let phash = phash_str
            .parse::<ImageHash>()
            .map_err(StageError::Manifest)?;

        let new_manifest = SingleTestManifest::new(hash, phash, width, height)?;

        let is_unchanged = workspace_index
            .get(&case_name)
            .is_some_and(|existing| existing == &new_manifest);

        if !is_unchanged {
            workspace_index.save_test(&manifests_dir, &case_name, &new_manifest)?;
            total_screenshots_staged += 1;
            staged_test_cases.push(case_name);
        }
    }

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

        let img_err = image::ImageError::Limits(image::error::LimitError::from_kind(
            image::error::LimitErrorKind::DimensionError,
        ));
        let err6 = StageError::ImageDecode {
            path: PathBuf::from("a.png"),
            source: img_err,
        };
        assert!(err6.to_string().contains("Image decode error"));
        assert!(std::error::Error::source(&err6).is_some());

        let img_err2 = image::ImageError::Limits(image::error::LimitError::from_kind(
            image::error::LimitErrorKind::DimensionError,
        ));
        let err7 = StageError::ImageEncode {
            path: PathBuf::from("b.png"),
            source: img_err2,
        };
        assert!(err7.to_string().contains("Image encode error"));
        assert!(std::error::Error::source(&err7).is_some());

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
