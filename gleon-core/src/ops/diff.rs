//! Diff operation for running visual comparison tests against baseline snapshots.

use crate::config::ConfigError;
use crate::context::{ContextError, ResolvedContext};
use crate::engine::{ComparisonResult, compare_images};
use crate::manifest::{ManifestError, WorkspaceIndex};
use crate::masking::apply_masks;
use crate::report::{ReportError, ReportGenerator};
use crate::scanner::{FileScanner, ScannerError, TestCaseResult, TestImageResult};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during diff execution.
#[derive(Debug, Error)]
pub enum DiffOpError {
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

    /// Error loading manifest or manifest index.
    #[error("Manifest error: {0}")]
    Manifest(#[from] ManifestError),

    /// Error generating report files.
    #[error("Report error: {0}")]
    Report(#[from] ReportError),

    /// Image processing error.
    #[error("Image error: {0}")]
    Image(#[from] image::ImageError),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result summary of executing `gleon diff`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffReportResult {
    pub total_tests: usize,
    pub failed_tests: usize,
    pub passed: bool,
    pub runs_dir: PathBuf,
}

/// Executes diff comparison for the workspace at `base_dir`.
pub fn run_diff(
    context: &ResolvedContext,
    base_dir: &Path,
) -> Result<DiffReportResult, DiffOpError> {
    let gleon_dir = base_dir.join(".gleon");
    if !gleon_dir.exists() {
        return Err(DiffOpError::NotInitialized);
    }

    let platform_key = match context.platform.to_key() {
        Ok(key) => key,
        Err(e) => return Err(DiffOpError::Context(ContextError::Platform(e))),
    };

    let manifests_dir = gleon_dir.join("manifests").join(&platform_key);
    let mut workspace_index = match WorkspaceIndex::load(&manifests_dir) {
        Ok(idx) => idx,
        Err(e) => return Err(DiffOpError::Manifest(e)),
    };

    if workspace_index.is_empty()
        && let Some(fallback_key) = context.fallback_platform_key.as_deref()
    {
        let fallback_dir = gleon_dir.join("manifests").join(fallback_key);
        let fb_index = WorkspaceIndex::load(&fallback_dir).map_err(DiffOpError::Manifest)?;
        if !fb_index.is_empty() {
            tracing::warn!(
                "No manifests found for platform '{}'. Falling back to manifests from platform '{}'.",
                platform_key,
                fallback_key
            );
            workspace_index = fb_index;
        }
    }

    let runs_dir = gleon_dir.join("runs").join("latest");
    let diffs_dir = runs_dir.join("diffs");
    std::fs::create_dir_all(&diffs_dir).map_err(DiffOpError::Io)?;

    use rayon::prelude::*;

    let config = context.config.as_ref().cloned().unwrap_or_default();
    let test_cases = match FileScanner::scan_workspace(&config, base_dir) {
        Ok(tc) => tc,
        Err(e) => return Err(DiffOpError::Scanner(e)),
    };

    let progress_bar = crate::ui::create_progress_bar(test_cases.len() as u64);

    let case_results: Vec<TestCaseResult> = test_cases
        .into_par_iter()
        .map(|case| {
            let test_name = case.name;
            progress_bar.set_message(case.image.relative_path.display().to_string());

            let result = (|| {
                let single_manifest_opt = workspace_index.get(&test_name);

                let baseline_entry = match single_manifest_opt {
                    Some(entry) => entry,
                    None => {
                        return TestImageResult::MissingBaseline {
                            relative_path: case.image.relative_path,
                            reason: format!("No staged baseline manifest for test '{}'", test_name),
                        };
                    }
                };

                let baseline_blob_path = gleon_dir
                    .join("blobs")
                    .join(baseline_entry.hash.scheme())
                    .join(baseline_entry.hash.value());

                let baseline_bytes = match std::fs::read(&baseline_blob_path) {
                    Ok(b) => b,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return TestImageResult::MissingBaseline {
                            relative_path: case.image.relative_path,
                            reason: format!(
                                "Baseline blob not found: {}",
                                baseline_entry.hash.value()
                            ),
                        };
                    }
                    Err(e) => {
                        return TestImageResult::DecodeError {
                            relative_path: case.image.relative_path,
                            error: format!("Failed to read baseline blob file: {}", e),
                        };
                    }
                };

                let baseline_dyn_img = match image::load_from_memory(&baseline_bytes) {
                    Ok(img) => img,
                    Err(e) => {
                        return TestImageResult::DecodeError {
                            relative_path: case.image.relative_path,
                            error: format!("Failed to decode baseline blob: {}", e),
                        };
                    }
                };
                let mut baseline_rgba = baseline_dyn_img.to_rgba8();

                let actual_dyn_img = match image::open(&case.image.absolute_path) {
                    Ok(img) => img,
                    Err(e) => {
                        return TestImageResult::DecodeError {
                            relative_path: case.image.relative_path,
                            error: format!("Failed to decode actual screenshot: {}", e),
                        };
                    }
                };
                let mut actual_rgba = actual_dyn_img.to_rgba8();

                // Apply ignore-zone masks if defined
                let matched_zones = case.rule.matched_mask_zones(&case.image.relative_path);
                if !matched_zones.is_empty() {
                    apply_masks(&mut baseline_rgba, &matched_zones);
                    apply_masks(&mut actual_rgba, &matched_zones);
                }

                // Perform engine comparison
                let comp_result = compare_images(
                    &baseline_rgba,
                    &actual_rgba,
                    case.rule.mode,
                    &case.rule.diff,
                );

                match comp_result {
                    ComparisonResult::Match => TestImageResult::Success {
                        relative_path: case.image.relative_path,
                    },
                    ComparisonResult::DimensionMismatch {
                        baseline_size,
                        actual_size,
                    } => TestImageResult::DimensionMismatch {
                        relative_path: case.image.relative_path,
                        baseline_size,
                        actual_size,
                        baseline_path: baseline_blob_path,
                        actual_path: case.image.absolute_path,
                    },
                    ComparisonResult::Mismatch { detail, diff_image } => {
                        // Write diff visualization image to .gleon/runs/latest/diffs/<case_name>/<file_name>
                        let case_diff_dir = diffs_dir.join(&test_name);
                        let raw_file_name = case
                            .image
                            .relative_path
                            .file_name()
                            .unwrap_or_else(|| std::ffi::OsStr::new("diff.png"))
                            .to_string_lossy();
                        let diff_file_name = format!("diff_{raw_file_name}");
                        let diff_file_path = case_diff_dir.join(&diff_file_name);

                        let mut encoded = Vec::new();
                        let mut cursor = std::io::Cursor::new(&mut encoded);
                        if let Err(e) = diff_image.write_to(&mut cursor, image::ImageFormat::Png) {
                            tracing::warn!("Failed to encode diff image to PNG: {}", e);
                        } else if let Err(e) =
                            crate::io::save_file_atomically(&diff_file_path, &encoded)
                        {
                            tracing::warn!(
                                "Failed to save diff image atomically to {:?}: {}",
                                diff_file_path,
                                e
                            );
                        }

                        TestImageResult::Mismatch {
                            relative_path: case.image.relative_path,
                            detail,
                            diff_path: diff_file_path,
                            baseline_path: baseline_blob_path,
                            actual_path: case.image.absolute_path,
                        }
                    }
                }
            })();

            progress_bar.inc(1);

            TestCaseResult {
                name: test_name,
                result,
            }
        })
        .collect();

    progress_bar.finish_and_clear();

    let total_tests = case_results.len();
    let failed_tests = case_results
        .iter()
        .filter(|tc| !matches!(tc.result, TestImageResult::Success { .. }))
        .count();
    let passed = failed_tests == 0;

    // Generate HTML and JUnit XML reports
    ReportGenerator::generate_all(&runs_dir, &case_results).map_err(DiffOpError::Report)?;

    Ok(DiffReportResult {
        total_tests,
        failed_tests,
        passed,
        runs_dir,
    })
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn test_diff_error_display() {
        let err1 = DiffOpError::NotInitialized;
        assert!(err1.to_string().contains("not initialized"));

        let err2 = DiffOpError::Context(ContextError::Platform(
            crate::platform::PlatformError::InvalidSegment("test".to_string()),
        ));
        assert!(err2.to_string().contains("Context resolution error"));

        let err3 = DiffOpError::Scanner(ScannerError::InvalidTestName {
            name: "bad/name".to_string(),
            reason: "reason".to_string(),
        });
        assert!(err3.to_string().contains("Scanner error"));

        let err4 = DiffOpError::Config(ConfigError::Validation("bad config".to_string()));
        assert!(err4.to_string().contains("Config error"));

        let err5 = DiffOpError::Manifest(ManifestError::Validation("bad manifest".to_string()));
        assert!(err5.to_string().contains("Manifest error"));

        let err6 = DiffOpError::Io(std::io::Error::other("io test"));
        assert!(err6.to_string().contains("IO error"));
    }

    #[test]
    fn test_diff_invalid_platform_error() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        let mut ctx = ResolvedContext::default();
        ctx.platform.os = "../invalid".to_string(); // Invalid segment

        let err = run_diff(&ctx, temp.path()).unwrap_err();
        assert!(matches!(
            err,
            DiffOpError::Context(ContextError::Platform(_))
        ));
    }

    #[test]
    fn test_diff_manifest_load_error_and_scanner_error() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        let ctx = ResolvedContext::default();
        let plat_key = ctx.platform.to_key().unwrap();

        // 1. Corrupt manifest file in manifests_dir
        let manifests_dir = gleon_dir.join("manifests").join(&plat_key);
        std::fs::create_dir_all(&manifests_dir).unwrap();
        std::fs::write(manifests_dir.join("test.json"), "invalid json").unwrap();

        let res = run_diff(&ctx, temp.path());
        assert!(matches!(res, Err(DiffOpError::Manifest(_))));

        // Clean up corrupt manifest
        std::fs::remove_file(manifests_dir.join("test.json")).unwrap();

        // 2. Invalid screenshot directory name (exclamation mark) to trigger Scanner error
        let bad_dir = temp.path().join("invalid!name");
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::write(bad_dir.join("test.png"), "fake png").unwrap();

        let res2 = run_diff(&ctx, temp.path());
        assert!(matches!(res2, Err(DiffOpError::Scanner(_))));
    }

    #[test]
    fn test_diff_blob_read_generic_io_error() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        let ctx = ResolvedContext::default();
        let plat_key = ctx.platform.to_key().unwrap();

        // Create a valid manifest entry
        let manifests_dir = gleon_dir.join("manifests").join(&plat_key);
        std::fs::create_dir_all(&manifests_dir).unwrap();
        let hash = crate::manifest::ImageHash::new(
            "sha256",
            "1111111111111111111111111111111111111111111111111111111111111111",
        )
        .unwrap();
        let phash = crate::manifest::ImageHash::new("dhash", "0000000000000000").unwrap();
        let manifest = crate::manifest::SingleTestManifest::new(hash, phash, 100, 100).unwrap();
        manifest.save(manifests_dir.join("login.json")).unwrap();

        // Create actual screenshot
        let screenshots_dir = temp.path().join("login");
        std::fs::create_dir_all(&screenshots_dir).unwrap();
        let img = image::RgbaImage::new(10, 10);
        img.save(screenshots_dir.join("spec.png")).unwrap();

        // Create blob path as a DIRECTORY so std::fs::read returns EISDIR (generic IO error, not NotFound)
        let blob_dir = gleon_dir
            .join("blobs")
            .join("sha256")
            .join("1111111111111111111111111111111111111111111111111111111111111111");
        std::fs::create_dir_all(&blob_dir).unwrap();

        let res = run_diff(&ctx, temp.path()).unwrap();
        assert_eq!(res.failed_tests, 1);
        assert!(!res.passed);
    }
}
