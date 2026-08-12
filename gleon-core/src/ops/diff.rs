//! Diff operation for running visual comparison tests against baseline snapshots.

use crate::config::ConfigError;
use crate::context::{ContextError, ResolvedContext};
use crate::engine::{ComparisonResult, compare_images};
use crate::manifest::{ManifestError, WorkspaceIndex};
use crate::masking::apply_masks;
use crate::report::{ReportError, ReportGenerator};
use crate::scanner::{FileScanner, ScannerError, TestCaseResult, TestImageResult};
use sha2::Digest;
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

pub(crate) fn process_diff_case(
    case: &crate::scanner::TestCase,
    workspace_index: &WorkspaceIndex,
    actual_dir: &Path,
    diffs_dir: &Path,
    gleon_dir: &Path,
) -> TestImageResult {
    let test_name = case.name.clone();
    let actual_bytes = match std::fs::read(&case.image.absolute_path) {
        Ok(b) => b,
        Err(e) => {
            return TestImageResult::IoError {
                relative_path: case.image.relative_path.clone(),
                error: format!("Failed to read actual screenshot file: {}", e),
            };
        }
    };

    let single_manifest_opt = workspace_index.get(&test_name);

    if let Some(baseline_entry) = single_manifest_opt {
        let is_byte_identical = baseline_entry.hash.scheme() == "sha256" && {
            let actual_sha256 = hex::encode(sha2::Sha256::digest(&actual_bytes));
            actual_sha256 == baseline_entry.hash.value()
        };
        if is_byte_identical {
            let baseline_blob_path =
                crate::storage::local_blob_path(&gleon_dir.join("blobs"), &baseline_entry.hash);
            if !crate::storage::is_usable_blob(&baseline_blob_path) {
                return TestImageResult::MissingBaseline {
                    relative_path: case.image.relative_path.clone(),
                    reason: format!("Baseline blob not found: {}", baseline_entry.hash.value()),
                };
            }
            if let Err(e) = crate::manifest::SingleTestManifest::validate_image_bytes(&actual_bytes)
            {
                return TestImageResult::DecodeError {
                    relative_path: case.image.relative_path.clone(),
                    error: format!("Invalid actual image dimensions/format: {}", e),
                };
            }
            return TestImageResult::Success {
                relative_path: case.image.relative_path.clone(),
            };
        }
    }

    let actual_dest_path = actual_dir.join(&case.image.relative_path);

    let parent = actual_dest_path.parent().unwrap_or_else(|| Path::new("."));
    if let Err(e) = std::fs::create_dir_all(parent) {
        return TestImageResult::IoError {
            relative_path: case.image.relative_path.clone(),
            error: format!("Failed to create directory for actual screenshot: {}", e),
        };
    }
    if let Err(e) = crate::io::save_file_atomically(&actual_dest_path, &actual_bytes) {
        return TestImageResult::IoError {
            relative_path: case.image.relative_path.clone(),
            error: format!("Failed to save actual screenshot: {}", e),
        };
    }

    let baseline_entry = match single_manifest_opt {
        Some(entry) => entry,
        None => {
            return TestImageResult::MissingBaseline {
                relative_path: case.image.relative_path.clone(),
                reason: format!("No staged baseline manifest for test '{}'", test_name),
            };
        }
    };

    let baseline_blob_path =
        crate::storage::local_blob_path(&gleon_dir.join("blobs"), &baseline_entry.hash);

    let baseline_bytes = match std::fs::read(&baseline_blob_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return TestImageResult::MissingBaseline {
                relative_path: case.image.relative_path.clone(),
                reason: format!("Baseline blob not found: {}", baseline_entry.hash.value()),
            };
        }
        Err(e) => {
            return TestImageResult::IoError {
                relative_path: case.image.relative_path.clone(),
                error: format!("Failed to read baseline blob file: {}", e),
            };
        }
    };

    if let Err(e) = crate::manifest::SingleTestManifest::validate_image_bytes(&baseline_bytes) {
        return TestImageResult::DecodeError {
            relative_path: case.image.relative_path.clone(),
            error: format!("Invalid baseline dimensions/format: {}", e),
        };
    }

    let baseline_dyn_img = match image::load_from_memory(&baseline_bytes) {
        Ok(img) => img,
        Err(e) => {
            return TestImageResult::DecodeError {
                relative_path: case.image.relative_path.clone(),
                error: format!("Failed to decode baseline blob: {}", e),
            };
        }
    };
    let mut baseline_rgba = baseline_dyn_img.to_rgba8();

    if let Err(e) = crate::manifest::SingleTestManifest::validate_image_bytes(&actual_bytes) {
        return TestImageResult::DecodeError {
            relative_path: case.image.relative_path.clone(),
            error: format!("Invalid actual image dimensions/format: {}", e),
        };
    }

    let actual_dyn_img = match image::load_from_memory(&actual_bytes) {
        Ok(img) => img,
        Err(e) => {
            return TestImageResult::DecodeError {
                relative_path: case.image.relative_path.clone(),
                error: format!("Failed to decode actual screenshot: {}", e),
            };
        }
    };
    let mut actual_rgba = actual_dyn_img.to_rgba8();

    let matched_zones = case.rule.matched_mask_zones(&case.image.relative_path);
    if !matched_zones.is_empty() {
        apply_masks(&mut baseline_rgba, &matched_zones);
        apply_masks(&mut actual_rgba, &matched_zones);
    }

    let comp_result = compare_images(
        &baseline_rgba,
        &actual_rgba,
        case.rule.mode,
        &case.rule.diff,
    );

    let raw_file_name = case
        .image
        .relative_path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("screenshot.png"))
        .to_string_lossy();

    match comp_result {
        ComparisonResult::Match => TestImageResult::Success {
            relative_path: case.image.relative_path.clone(),
        },
        ComparisonResult::DimensionMismatch {
            baseline_size,
            actual_size,
        } => TestImageResult::DimensionMismatch {
            relative_path: case.image.relative_path.clone(),
            baseline_size,
            actual_size,
            baseline_path: baseline_blob_path,
            actual_path: actual_dest_path,
        },
        ComparisonResult::Mismatch { detail, diff_image } => {
            let case_diff_dir = diffs_dir.join(&test_name);
            if let Err(e) = std::fs::create_dir_all(&case_diff_dir) {
                return TestImageResult::IoError {
                    relative_path: case.image.relative_path.clone(),
                    error: format!("Failed to create directory for diff: {}", e),
                };
            }
            let mut diff_file_name = std::ffi::OsString::from("diff_");
            diff_file_name.push(&*raw_file_name);
            let diff_file_path = case_diff_dir.join(&diff_file_name);

            let mut cursor = std::io::Cursor::new(Vec::new());
            if let Err(e) = diff_image.write_to(&mut cursor, image::ImageFormat::Png) {
                return TestImageResult::EncodeError {
                    relative_path: case.image.relative_path.clone(),
                    actual_path: actual_dest_path,
                    error: format!("Failed to encode diff visualization: {}", e),
                };
            }
            let encoded = cursor.into_inner();
            if let Err(e) = crate::io::save_file_atomically(&diff_file_path, &encoded) {
                return TestImageResult::IoError {
                    relative_path: case.image.relative_path.clone(),
                    error: format!("Failed to save diff visualization: {}", e),
                };
            }

            TestImageResult::Mismatch {
                relative_path: case.image.relative_path.clone(),
                detail,
                diff_path: diff_file_path,
                baseline_path: baseline_blob_path,
                actual_path: actual_dest_path,
            }
        }
    }
}

/// Executes diff comparison for the workspace at `base_dir`.
pub fn run_diff(
    context: &ResolvedContext,
    base_dir: &Path,
) -> Result<DiffReportResult, DiffOpError> {
    let gleon_dir = base_dir.join(".gleon");
    if std::fs::metadata(&gleon_dir).is_err() {
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
    match std::fs::remove_dir_all(&runs_dir) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(DiffOpError::Io(e)),
    }
    let diffs_dir = runs_dir.join("diffs");
    let actual_dir = runs_dir.join("actual");
    std::fs::create_dir_all(&diffs_dir).map_err(DiffOpError::Io)?;
    std::fs::create_dir_all(&actual_dir).map_err(DiffOpError::Io)?;

    let config = context.config.as_ref().cloned().unwrap_or_default();
    let test_cases = match FileScanner::scan_workspace(&config, base_dir) {
        Ok(tc) => tc,
        Err(e) => return Err(DiffOpError::Scanner(e)),
    };

    let progress_bar = crate::ui::create_progress_bar(test_cases.len() as u64);

    use rayon::prelude::*;
    let case_results: Vec<TestCaseResult> = test_cases
        .into_par_iter()
        .map(|case| {
            progress_bar.set_message(case.image.relative_path.display().to_string());
            let result =
                process_diff_case(&case, &workspace_index, &actual_dir, &diffs_dir, &gleon_dir);
            progress_bar.inc(1);
            TestCaseResult {
                name: case.name,
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

        // Create actual screenshot with non-matching hash
        let img = image::RgbaImage::new(10, 10);
        img.save(temp.path().join("login.png")).unwrap();

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

    #[cfg(all(unix, not(miri)))]
    #[test]
    fn test_diff_actual_read_generic_io_error() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        let ctx = ResolvedContext::default();
        let plat_key = ctx.platform.to_key().unwrap();
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

        let screenshot = temp.path().join("login.png");
        std::fs::write(&screenshot, "fake png").unwrap();

        // Remove read permissions
        let mut perms = std::fs::metadata(&screenshot).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&screenshot, perms.clone()).unwrap();

        let res = run_diff(&ctx, temp.path());

        // Restore permissions before assertions
        perms.set_mode(0o644);
        std::fs::set_permissions(&screenshot, perms).unwrap();

        let report = res.unwrap();
        assert_eq!(report.failed_tests, 1);
        assert!(!report.passed);
    }

    #[test]
    #[cfg(all(unix, not(miri)))]
    fn test_diff_write_io_errors_via_poisoning() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        let mut ctx = ResolvedContext::default();
        ctx.platform.os = "linux".to_string();
        let key = ctx.platform.to_key().unwrap();

        let manifests_dir = gleon_dir.join("manifests").join(&key);
        std::fs::create_dir_all(&manifests_dir).unwrap();
        let hash = "1111111111111111111111111111111111111111111111111111111111111111";
        let manifest = crate::manifest::SingleTestManifest::new(
            crate::manifest::ImageHash::new("sha256", hash).unwrap(),
            crate::manifest::ImageHash::new("dhash", "0000000000000000").unwrap(),
            1,
            1,
        )
        .unwrap();
        manifest.save(manifests_dir.join("test.json")).unwrap();

        let blobs_dir = gleon_dir.join("blobs").join("sha256");
        std::fs::create_dir_all(&blobs_dir).unwrap();
        let img = image::ImageBuffer::<image::Rgba<u8>, _>::new(1, 1);
        img.save_with_format(blobs_dir.join(hash), image::ImageFormat::Png)
            .unwrap();

        // Mismatching actual image (same dimensions, different color)
        let mut bad_img = image::ImageBuffer::<image::Rgba<u8>, _>::new(1, 1);
        bad_img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        bad_img.save(temp.path().join("test.png")).unwrap();

        let runs_dir = gleon_dir.join("runs").join("latest");
        let actual_dir = runs_dir.join("actuals");
        let diffs_dir = runs_dir.join("diffs");
        std::fs::create_dir_all(&actual_dir).unwrap();
        std::fs::create_dir_all(&diffs_dir).unwrap();

        // Put a file inside actual_dir and diffs_dir so they cannot be deleted if read-only
        std::fs::write(actual_dir.join("dummy"), "").unwrap();
        std::fs::write(diffs_dir.join("dummy"), "").unwrap();

        // Make actual_dir and diffs_dir read-only.
        // remove_dir_all(&runs_dir) will fail to delete them because it can't delete 'dummy'.
        // They will survive, and then save_file_atomically will fail because they are read-only!
        let mut perms = std::fs::metadata(&actual_dir).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(&actual_dir, perms.clone()).unwrap();
        std::fs::set_permissions(&diffs_dir, perms.clone()).unwrap();

        let result = run_diff(&ctx, temp.path());
        assert!(result.is_err());

        // Restore permissions so tempdir can be cleaned up
        perms.set_mode(0o755);
        std::fs::set_permissions(&actual_dir, perms.clone()).unwrap();
        std::fs::set_permissions(&diffs_dir, perms).unwrap();
    }

    #[test]
    fn test_diff_empty_fallback_platform() {
        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        let ctx = ResolvedContext {
            fallback_platform_key: Some("some-fallback-key".to_string()),
            ..Default::default()
        };

        // No manifests created for primary or fallback platform, so fb_index will be empty.
        // run_diff should run and return no test results because scanner finds nothing (no screenshots created).
        let res = run_diff(&ctx, temp.path()).unwrap();
        assert_eq!(res.total_tests, 0);
    }

    #[test]
    #[cfg(all(unix, not(miri)))]
    fn test_process_diff_case_io_errors() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: `libc::geteuid()` is a side-effect-free POSIX syscall query that returns the process EUID.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let gleon_dir = temp.path().join(".gleon");
        let actual_dir = temp.path().join("actual");
        let diffs_dir = temp.path().join("diffs");

        std::fs::create_dir_all(&gleon_dir).unwrap();
        std::fs::create_dir_all(&actual_dir).unwrap();
        std::fs::create_dir_all(&diffs_dir).unwrap();

        let case = crate::scanner::TestCase {
            name: "test".to_string(),
            image: crate::scanner::TestImage {
                absolute_path: temp.path().join("test.png"),
                relative_path: PathBuf::from("test.png"),
            },
            rule: std::sync::Arc::new(crate::config::ScreenshotRule {
                include: vec![],
                mode: crate::config::Mode::Pixel,
                diff: crate::config::DiffConfig {
                    threshold: 0.0,
                    anti_alias: false,
                    min_similarity: 1.0,
                },
                masks: vec![],
            }),
        };

        // 1. Missing actual image
        let res1 = super::process_diff_case(
            &case,
            &crate::manifest::WorkspaceIndex::new(),
            &actual_dir,
            &diffs_dir,
            &gleon_dir,
        );
        assert!(matches!(res1, TestImageResult::IoError { .. }));

        // 2. Decode error on actual image (requires baseline to bypass MissingBaseline)
        std::fs::write(&case.image.absolute_path, "fake png").unwrap();
        let mut index = crate::manifest::WorkspaceIndex::new();
        let hash_val = "1111111111111111111111111111111111111111111111111111111111111111";
        index.insert(
            "test".to_string(),
            crate::manifest::SingleTestManifest {
                schema_version: 1,
                hash: crate::manifest::ImageHash::new("sha256", hash_val).unwrap(),
                phash: crate::manifest::ImageHash::new("dhash", "2222222222222222").unwrap(),
                width: 1,
                height: 1,
            },
        );
        let blob_dir = gleon_dir.join("blobs").join("sha256");
        std::fs::create_dir_all(&blob_dir).unwrap();
        std::fs::write(blob_dir.join(hash_val), "fake png").unwrap();
        let res2 = super::process_diff_case(&case, &index, &actual_dir, &diffs_dir, &gleon_dir);
        assert!(matches!(res2, TestImageResult::DecodeError { .. }));

        // Create valid image
        let img = image::ImageBuffer::<image::Rgba<u8>, _>::new(1, 1);
        img.save(&case.image.absolute_path).unwrap();

        // 3. IoError on create actual dir (make actual_dir read-only)
        let mut perms = std::fs::metadata(&actual_dir).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(&actual_dir, perms.clone()).unwrap();

        let case2 = crate::scanner::TestCase {
            name: "test".to_string(),
            image: crate::scanner::TestImage {
                absolute_path: case.image.absolute_path.clone(),
                relative_path: PathBuf::from("subdir/test.png"), // Requires create_dir_all
            },
            rule: std::sync::Arc::new(crate::config::ScreenshotRule {
                include: vec![],
                mode: crate::config::Mode::Pixel,
                diff: crate::config::DiffConfig {
                    threshold: 0.0,
                    anti_alias: false,
                    min_similarity: 1.0,
                },
                masks: vec![],
            }),
        };
        let res3 = super::process_diff_case(
            &case2,
            &crate::manifest::WorkspaceIndex::new(),
            &actual_dir,
            &diffs_dir,
            &gleon_dir,
        );

        // Restore permissions before assertions
        perms.set_mode(0o755);
        std::fs::set_permissions(&actual_dir, perms).unwrap();

        assert!(matches!(res3, TestImageResult::IoError { .. }));
    }
}
