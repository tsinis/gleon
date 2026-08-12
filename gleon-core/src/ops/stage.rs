//! Staging operation for processing, masking, and persisting baseline screenshots.

use crate::config::ConfigError;
use crate::context::{ContextError, ResolvedContext};
use crate::engine::phash::compute_phash;
use crate::manifest::{ImageHash, ManifestError, SingleTestManifest, WorkspaceIndex};
use crate::scanner::{FileScanner, ScannerError};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during staging.
#[derive(Debug, Error)]
pub enum StageError {
    /// Workspace has not been initialized (`.gleon` missing).
    #[error("gleon workspace is not initialized. Please run 'gleon init' first.")]
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

    /// Error parsing JSON.
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<crate::io::IoError> for StageError {
    fn from(err: crate::io::IoError) -> Self {
        match err {
            crate::io::IoError::Io(e) => StageError::Io(e),
            crate::io::IoError::JsonParse(e) => StageError::JsonParse(e),
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

/// Applies the given path filters to a list of test cases in-place.
pub(crate) fn filter_test_cases(
    test_cases: &mut Vec<crate::scanner::TestCase>,
    filter_paths: Option<&[PathBuf]>,
) {
    if let Some(filters) = filter_paths {
        let normalized_filters: Vec<_> = filters
            .iter()
            .map(|f| (f, FileScanner::normalize_path_str(f).to_lowercase()))
            .collect();

        test_cases.retain(|case| {
            let rel_norm = FileScanner::normalize_path_str(&case.image.relative_path);
            normalized_filters.iter().any(|(f, norm_f)| {
                case.image.absolute_path.starts_with(f)
                    || case.image.relative_path.starts_with(f)
                    || (rel_norm.len() >= norm_f.len()
                        && rel_norm.as_bytes()[..norm_f.len()]
                            .eq_ignore_ascii_case(norm_f.as_bytes())
                        && (rel_norm.len() == norm_f.len()
                            || norm_f.as_bytes().last() == Some(&b'/')
                            || rel_norm.as_bytes()[norm_f.len()] == b'/'))
            })
        });
    }
}

/// Executes staging pipeline across the workspace.
pub fn stage_workspace(
    context: &ResolvedContext,
    base_dir: &Path,
    filter_paths: Option<&[PathBuf]>,
) -> Result<StageResult, StageError> {
    let gleon_dir = base_dir.join(".gleon");
    if std::fs::metadata(&gleon_dir).is_err() {
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

    let mut test_cases =
        FileScanner::scan_workspace(&config, base_dir).map_err(StageError::Scanner)?;

    filter_test_cases(&mut test_cases, filter_paths);

    let pb = crate::ui::create_progress_bar(test_cases.len() as u64);
    pb.set_message("Staging screenshots...");

    let mut workspace_index = WorkspaceIndex::load(&manifests_dir).map_err(StageError::Manifest)?;

    use rayon::prelude::*;

    struct StagedItem {
        case_name: String,
        sha256_hex: String,
        phash_str: String,
        width: u32,
        height: u32,
    }

    let processed_results: Result<Vec<StagedItem>, StageError> = test_cases
        .into_par_iter()
        .map(|case| {
            let png_bytes = std::fs::read(&case.image.absolute_path).map_err(StageError::Io)?;
            let dynamic_img =
                image::load_from_memory(&png_bytes).map_err(|source| StageError::ImageDecode {
                    path: case.image.relative_path.clone(),
                    source,
                })?;
            let width = dynamic_img.width();
            let height = dynamic_img.height();
            let rgba_img = dynamic_img.to_rgba8();

            let phash_str = compute_phash(&rgba_img);
            let sha256_hex = hex::encode(Sha256::digest(&png_bytes));

            // Save blob to .gleon/blobs/sha256/<sha256_hex>
            let blob_path = blobs_dir.join(&sha256_hex);
            crate::io::save_file_atomically(&blob_path, &png_bytes).map_err(StageError::from)?;

            pb.inc(1);

            Ok(StagedItem {
                case_name: case.name,
                sha256_hex,
                phash_str,
                width,
                height,
            })
        })
        .collect();

    let processed_results = match processed_results {
        Ok(res) => {
            pb.finish_and_clear();
            res
        }
        Err(e) => {
            pb.finish_and_clear();
            return Err(e);
        }
    };

    // Clean up orphan manifests when performing a full workspace stage (no path filters)
    if filter_paths.is_none() {
        let scanned_names: std::collections::HashSet<_> = processed_results
            .iter()
            .map(|item| item.case_name.as_str())
            .collect();
        let orphan_names: Vec<_> = workspace_index
            .entries()
            .keys()
            .filter(|k| !scanned_names.contains(k.as_str()))
            .cloned()
            .collect();
        for existing in orphan_names {
            workspace_index
                .remove_test(&manifests_dir, &existing)
                .map_err(StageError::Manifest)?;
        }
    }

    let mut staged_test_cases = Vec::new();
    let mut total_screenshots_staged = 0;

    for item in processed_results {
        let hash = ImageHash::new("sha256", &item.sha256_hex).map_err(StageError::Manifest)?;
        let phash = item
            .phash_str
            .parse::<ImageHash>()
            .map_err(StageError::Manifest)?;

        let new_manifest = SingleTestManifest::new(hash, phash, item.width, item.height)
            .map_err(StageError::Manifest)?;

        let is_unchanged = workspace_index
            .get(&item.case_name)
            .is_some_and(|existing| existing == &new_manifest);

        if !is_unchanged {
            workspace_index
                .save_test(&manifests_dir, &item.case_name, &new_manifest)
                .map_err(StageError::Manifest)?;
            total_screenshots_staged += 1;
            staged_test_cases.push(item.case_name);
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

    #[test]
    fn test_filter_test_cases() {
        use crate::scanner::{TestCase, TestImage};
        use std::sync::Arc;

        let rule = Arc::new(crate::config::ScreenshotRule {
            include: vec![],
            mode: crate::config::Mode::Pixel,
            diff: crate::config::DiffConfig::default(),
            masks: vec![],
        });

        let cases = vec![
            TestCase {
                name: "test1".to_string(),
                image: TestImage {
                    relative_path: PathBuf::from("a/test1.png"),
                    absolute_path: PathBuf::from("/base/a/test1.png"),
                },
                rule: rule.clone(),
            },
            TestCase {
                name: "test2".to_string(),
                image: TestImage {
                    relative_path: PathBuf::from("b/test2.png"),
                    absolute_path: PathBuf::from("/base/b/test2.png"),
                },
                rule: rule.clone(),
            },
        ];

        // 1. None should keep all
        let mut cases_clone = cases.clone();
        filter_test_cases(&mut cases_clone, None);
        assert_eq!(cases_clone.len(), 2);

        // 2. Filter keeping only test1 via absolute path
        let mut cases_clone2 = cases.clone();
        let filter1 = vec![PathBuf::from("/base/a")];
        filter_test_cases(&mut cases_clone2, Some(&filter1));
        assert_eq!(cases_clone2.len(), 1);
        assert_eq!(cases_clone2[0].name, "test1");

        // 3. Filter with mixed casing (e.g. "A/TEST1.PNG")
        let mut cases_clone3 = cases.clone();
        let filter_mixed = vec![PathBuf::from("A/TEST1.PNG")];
        filter_test_cases(&mut cases_clone3, Some(&filter_mixed));
        assert_eq!(cases_clone3.len(), 1);
        assert_eq!(cases_clone3[0].name, "test1");

        // 4. Filter string prefix bug (e.g. "a" should not match "a_other")
        let cases_prefix = vec![
            TestCase {
                name: "test_a".to_string(),
                image: TestImage {
                    relative_path: PathBuf::from("a/test1.png"),
                    absolute_path: PathBuf::from("/base/a/test1.png"),
                },
                rule: rule.clone(),
            },
            TestCase {
                name: "test_a_other".to_string(),
                image: TestImage {
                    relative_path: PathBuf::from("a_other/test1.png"),
                    absolute_path: PathBuf::from("/base/a_other/test1.png"),
                },
                rule: rule.clone(),
            },
        ];
        let mut cases_prefix_clone = cases_prefix.clone();
        let filter_prefix = vec![PathBuf::from("a")];
        filter_test_cases(&mut cases_prefix_clone, Some(&filter_prefix));
        assert_eq!(cases_prefix_clone.len(), 1);
        assert_eq!(cases_prefix_clone[0].name, "test_a");
    }
}
