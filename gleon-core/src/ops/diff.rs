//! Diff operation for running visual comparison tests against baseline snapshots.

use crate::config::ConfigError;
use crate::context::{ContextError, ResolvedContext};
use crate::engine::{ComparisonResult, compare_images};
use crate::manifest::{Manifest, ManifestError, ManifestIndex};
use crate::masking::apply_masks;
use crate::report::{ReportError, ReportGenerator};
use crate::scanner::{FileScanner, ScannerError, TestCaseResult, TestImageResult};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during diff execution.
#[derive(Debug, Error)]
pub enum DiffOpError {
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

    let platform_key = context
        .platform
        .to_key()
        .map_err(|e| DiffOpError::Context(ContextError::Platform(e)))?;

    let index_path = gleon_dir
        .join("branches")
        .join(&context.branch)
        .join(&platform_key)
        .join("manifest_index.json");

    let manifest_index = match ManifestIndex::load(&index_path) {
        Ok(idx) => Some(idx),
        Err(ManifestError::Io(crate::io::IoError::Io(e)))
            if e.kind() == std::io::ErrorKind::NotFound =>
        {
            None
        }
        Err(e) => return Err(DiffOpError::Manifest(e)),
    };

    let runs_dir = gleon_dir.join("runs").join("latest");
    let diffs_dir = runs_dir.join("diffs");
    std::fs::create_dir_all(&diffs_dir)?;

    use rayon::prelude::*;

    let config = context.config.as_ref().cloned().unwrap_or_default();

    let test_cases = FileScanner::scan_workspace(&config, base_dir)?;

    let (test_case_results, total_tests, failed_tests) = test_cases
        .into_par_iter()
        .map(|case| {
            let mut image_results = Vec::new();
            let mut case_total = 0;
            let mut case_failed = 0;

            // Check if baseline manifest exists for this test case
            let manifest_opt = manifest_index.as_ref().and_then(|idx| {
                idx.test_manifests.get(&case.name).and_then(|hash| {
                    let manifest_path = gleon_dir
                        .join("blobs")
                        .join(hash.scheme())
                        .join(hash.value());
                    Manifest::load(manifest_path).ok()
                })
            });

            for img in &case.images {
                case_total += 1;
                let rel_path_str = FileScanner::normalize_path_str(&img.relative_path);

                let entry_opt = manifest_opt
                    .as_ref()
                    .and_then(|m| m.entries.get(rel_path_str.as_ref()));

                let baseline_entry = match entry_opt {
                    Some(entry) => entry,
                    None => {
                        case_failed += 1;
                        image_results.push(TestImageResult::DecodeError {
                            relative_path: img.relative_path.clone(),
                            error: "No baseline manifest entry found".to_string(),
                        });
                        continue;
                    }
                };

                let baseline_blob_path = gleon_dir
                    .join("blobs")
                    .join(baseline_entry.hash.scheme())
                    .join(baseline_entry.hash.value());

                let baseline_bytes = match std::fs::read(&baseline_blob_path) {
                    Ok(b) => b,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        case_failed += 1;
                        image_results.push(TestImageResult::DecodeError {
                            relative_path: img.relative_path.clone(),
                            error: format!(
                                "Baseline blob not found: {}",
                                baseline_entry.hash.value()
                            ),
                        });
                        continue;
                    }
                    Err(e) => {
                        case_failed += 1;
                        image_results.push(TestImageResult::DecodeError {
                            relative_path: img.relative_path.clone(),
                            error: format!("Failed to read baseline blob file: {}", e),
                        });
                        continue;
                    }
                };

                let baseline_dyn_img = match image::load_from_memory(&baseline_bytes) {
                    Ok(img) => img,
                    Err(e) => {
                        case_failed += 1;
                        image_results.push(TestImageResult::DecodeError {
                            relative_path: img.relative_path.clone(),
                            error: format!("Failed to decode baseline blob: {}", e),
                        });
                        continue;
                    }
                };
                let mut baseline_rgba = baseline_dyn_img.to_rgba8();

                let actual_dyn_img = match image::open(&img.absolute_path) {
                    Ok(img) => img,
                    Err(e) => {
                        case_failed += 1;
                        image_results.push(TestImageResult::DecodeError {
                            relative_path: img.relative_path.clone(),
                            error: format!("Failed to decode actual screenshot: {}", e),
                        });
                        continue;
                    }
                };
                let mut actual_rgba = actual_dyn_img.to_rgba8();

                // Apply ignore-zone masks if defined (idempotent for baseline, handles newly added mask rules)
                let matched_zones = case.rule.matched_mask_zones(&img.relative_path);
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
                    ComparisonResult::Match => {
                        image_results.push(TestImageResult::Success {
                            relative_path: img.relative_path.clone(),
                        });
                    }
                    ComparisonResult::DimensionMismatch {
                        baseline_size,
                        actual_size,
                    } => {
                        case_failed += 1;
                        image_results.push(TestImageResult::DimensionMismatch {
                            relative_path: img.relative_path.clone(),
                            baseline_size,
                            actual_size,
                            baseline_path: baseline_blob_path,
                            actual_path: img.absolute_path.clone(),
                        });
                    }
                    ComparisonResult::Mismatch { detail, diff_image } => {
                        case_failed += 1;
                        // Write diff visualization image to .gleon/runs/latest/diffs/<case_name>/<file_name>
                        let case_diff_dir = diffs_dir.join(&case.name);
                        let diff_file_name = img
                            .relative_path
                            .file_name()
                            .unwrap_or_else(|| std::ffi::OsStr::new("diff.png"));
                        let diff_path = case_diff_dir.join(diff_file_name);

                        if let Err(e) = crate::io::write_file_atomically(&diff_path, |writer| {
                            diff_image
                                .write_to(writer, image::ImageFormat::Png)
                                .map_err(|e| crate::io::IoError::Io(std::io::Error::other(e)))
                        }) {
                            tracing::warn!("Failed to save diff image to {:?}: {}", diff_path, e);
                        }

                        image_results.push(TestImageResult::Mismatch {
                            relative_path: img.relative_path.clone(),
                            detail,
                            diff_path,
                            baseline_path: baseline_blob_path,
                            actual_path: img.absolute_path.clone(),
                        });
                    }
                }
            }

            let case_res = TestCaseResult {
                name: case.name.clone(),
                results: image_results,
            };

            (case_res, case_total, case_failed)
        })
        .fold(
            || (Vec::new(), 0, 0),
            |(mut res_acc, tot_acc, fail_acc), (c_res, c_tot, c_fail)| {
                res_acc.push(c_res);
                (res_acc, tot_acc + c_tot, fail_acc + c_fail)
            },
        )
        .reduce(
            || (Vec::new(), 0, 0),
            |(mut r1, t1, f1), (r2, t2, f2)| {
                r1.extend(r2);
                (r1, t1 + t2, f1 + f2)
            },
        );

    // Write report files to .gleon/runs/latest/
    if let Some(html_content) = ReportGenerator::generate_html(&test_case_results, Some(&runs_dir))?
    {
        crate::io::save_file_atomically(runs_dir.join("report.html"), html_content.as_bytes())
            .map_err(std::io::Error::other)?;
    }

    let junit_content = ReportGenerator::generate_junit_xml(&test_case_results)?;
    crate::io::save_file_atomically(runs_dir.join("junit.xml"), junit_content.as_bytes())
        .map_err(std::io::Error::other)?;

    let md_content = ReportGenerator::generate_markdown(&test_case_results);
    crate::io::save_file_atomically(runs_dir.join("report.md"), md_content.as_bytes())
        .map_err(std::io::Error::other)?;

    Ok(DiffReportResult {
        total_tests,
        failed_tests,
        passed: failed_tests == 0,
        runs_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(miri))]
    use crate::cli::Cli;
    #[cfg(not(miri))]
    use crate::manifest::{ImageHash, Manifest, ManifestIndex};
    #[cfg(not(miri))]
    use crate::ops::{init_workspace, stage_workspace};
    #[cfg(not(miri))]
    use std::fs;
    #[cfg(not(miri))]
    use tempfile::tempdir;

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

        let err6 = DiffOpError::Report(ReportError::Render {
            template: "report.html",
            source: minijinja::Error::new(minijinja::ErrorKind::UndefinedError, "test"),
        });
        assert!(err6.to_string().contains("Report error"));

        let err7 = DiffOpError::Io(std::io::Error::other("io test"));
        assert!(err7.to_string().contains("IO error"));
    }

    #[test]
    fn test_diff_report_result_derived() {
        let res = DiffReportResult {
            total_tests: 5,
            failed_tests: 0,
            passed: true,
            runs_dir: PathBuf::from("runs/latest"),
        };
        let cloned = res.clone();
        assert_eq!(res, cloned);
        assert!(!format!("{:?}", res).is_empty());
    }

    #[test]
    #[cfg(not(miri))]
    fn test_diff_missing_manifest_entry_and_missing_blob() {
        let dir = tempdir().unwrap();
        let base_path = dir.path();

        let cli_init = Cli::for_test(crate::cli::Commands::Init);
        let ctx_init = ResolvedContext::from_cli(&cli_init, base_path).unwrap();
        init_workspace(&ctx_init, base_path).unwrap();

        let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let png_bytes = fs::read(fixtures_dir.join("200x100.png")).unwrap();

        let billing_dir = base_path.join("billing");
        fs::create_dir_all(&billing_dir).unwrap();
        fs::write(billing_dir.join("form.png"), &png_bytes).unwrap();
        fs::write(billing_dir.join("missing_entry.png"), &png_bytes).unwrap();

        let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing/**/*.png"
"#;
        fs::write(base_path.join("gleon.yaml"), config_yaml).unwrap();

        let cli = Cli::for_test(crate::cli::Commands::Stage { paths: vec![] });
        let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();
        stage_workspace(&ctx, base_path, None).unwrap();

        // Now remove form.png entry or tamper with manifest index
        let platform_key = ctx.platform.to_key().unwrap();
        let index_path = base_path
            .join(".gleon/branches/main")
            .join(&platform_key)
            .join("manifest_index.json");

        let index = ManifestIndex::load(&index_path).unwrap();
        let manifest_hash = index.test_manifests.get("billing").unwrap().clone();
        let manifest_path = base_path
            .join(".gleon/blobs")
            .join(manifest_hash.scheme())
            .join(manifest_hash.value());

        let mut manifest = Manifest::load(&manifest_path).unwrap();
        // Remove missing_entry.png from manifest entries so it triggers "No baseline manifest entry found"
        manifest.entries.remove("billing/missing_entry.png");

        // Set form.png to point to a non-existent blob sha
        let fake_hash = ImageHash::new(
            "sha256",
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        if let Some(entry) = manifest.entries.get_mut("billing/form.png") {
            entry.hash = fake_hash;
        }

        manifest.save(&manifest_path).unwrap();

        let cli_diff = Cli::for_test(crate::cli::Commands::Diff);
        let ctx_diff = ResolvedContext::from_cli(&cli_diff, base_path).unwrap();
        let res = run_diff(&ctx_diff, base_path).unwrap();

        assert!(!res.passed);
        assert_eq!(res.failed_tests, 2);
    }

    #[test]
    #[cfg(not(miri))]
    fn test_diff_corrupt_actual_image_and_dimension_mismatch() {
        let dir = tempdir().unwrap();
        let base_path = dir.path();

        let cli_init = Cli::for_test(crate::cli::Commands::Init);
        let ctx_init = ResolvedContext::from_cli(&cli_init, base_path).unwrap();
        init_workspace(&ctx_init, base_path).unwrap();

        let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let png_200 = fs::read(fixtures_dir.join("200x100.png")).unwrap();
        let png_100 = fs::read(fixtures_dir.join("diff_16px_corners_100x100.png")).unwrap();

        let billing_dir = base_path.join("billing");
        fs::create_dir_all(&billing_dir).unwrap();
        fs::write(billing_dir.join("form.png"), &png_200).unwrap();
        fs::write(billing_dir.join("corrupt.png"), &png_200).unwrap();

        let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing/**/*.png"
"#;
        fs::write(base_path.join("gleon.yaml"), config_yaml).unwrap();

        let cli = Cli::for_test(crate::cli::Commands::Stage { paths: vec![] });
        let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();
        stage_workspace(&ctx, base_path, None).unwrap();

        // 1. Overwrite form.png with 100x100 image (Dimension Mismatch)
        fs::write(billing_dir.join("form.png"), &png_100).unwrap();
        // 2. Overwrite corrupt.png with corrupt bytes (Decode Error)
        fs::write(billing_dir.join("corrupt.png"), b"not a png image").unwrap();

        let cli_diff = Cli::for_test(crate::cli::Commands::Diff);
        let ctx_diff = ResolvedContext::from_cli(&cli_diff, base_path).unwrap();
        let res = run_diff(&ctx_diff, base_path).unwrap();

        assert!(!res.passed);
        assert_eq!(res.failed_tests, 2);
    }
}
