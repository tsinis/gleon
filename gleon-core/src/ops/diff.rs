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
                let baseline_rgba = baseline_dyn_img.to_rgba8();

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

                // Apply ignore-zone masks if defined
                let matched_zones = case.rule.matched_mask_zones(&img.relative_path);
                if !matched_zones.is_empty() {
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
