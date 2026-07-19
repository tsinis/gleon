//! File scanner and image decoder for visual regression tests.

use crate::config::{GleonConfig, GlobPattern};
use globset::GlobSetBuilder;
use image::RgbaImage;
use std::path::{Path, PathBuf};

/// Errors that can occur during visual regression testing files scanning.
#[derive(Debug, thiserror::Error)]
pub enum ScannerError {
    /// IO error during file or directory access.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Error compiling a glob pattern.
    #[error("Pattern compilation error: {0}")]
    Pattern(#[from] globset::Error),

    /// Invalid test name format.
    #[error("Invalid test name '{name}': {reason}")]
    InvalidTestName {
        /// The invalid test case name.
        name: String,
        /// The validation failure reason.
        reason: String,
    },
}

/// A single test screenshot file within a TestCase.
#[derive(Debug, Clone)]
pub struct TestImage {
    /// Relative path from the base directory (e.g. "billing/stripe/form.png")
    pub relative_path: PathBuf,
    /// Absolute path to the file on disk
    pub absolute_path: PathBuf,
    /// Decoded image buffer, or error message string if decoding failed.
    pub image: Result<RgbaImage, String>,
}

/// A grouping of screenshots under a single test case directory.
#[derive(Debug, Clone)]
pub struct TestCase {
    /// The test name (relative parent directory path, e.g. "billing/stripe")
    pub name: String,
    /// The screenshots belonging to this test case
    pub images: Vec<TestImage>,
}

/// Details of a visual comparison mismatch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MismatchDetail {
    /// Count of mismatched pixels.
    Pixel {
        /// Number of mismatched pixels.
        diff_count: u32,
    },
    /// SSIM similarity score.
    Ssim {
        /// Structural Similarity Index score.
        ssim_score: f64,
    },
}

/// Represents the result of running a test on a single screenshot.
#[derive(Debug)]
pub enum TestImageResult {
    /// The actual image matches the baseline.
    Success {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
    },
    /// The screenshot file failed to decode.
    DecodeError {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
        /// The decoding error message.
        error: String,
    },
    /// The actual image dimensions do not match the baseline image.
    DimensionMismatch {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
        /// Size of the baseline image.
        baseline_size: (u32, u32),
        /// Size of the actual image.
        actual_size: (u32, u32),
    },
    /// The actual image content differs from the baseline image.
    Mismatch {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
        /// Specific detail about the comparison mismatch.
        detail: MismatchDetail,
        /// Generated diff image with highlighted differences.
        diff_image: RgbaImage,
    },
}

/// Represents the final evaluation result of a complete test case.
#[derive(Debug)]
pub struct TestCaseResult {
    /// The test case name.
    pub name: String,
    /// Results of all screenshots within the test case.
    pub results: Vec<TestImageResult>,
}

/// Validates that all segments of a test name contain only allowed characters `[a-z0-9_.-]`.
pub fn validate_test_name(name: &str) -> Result<(), String> {
    if name == "." {
        return Ok(());
    }
    for segment in name.split('/') {
        if segment.is_empty() {
            return Err("Test name segment cannot be empty".to_string());
        }
        for c in segment.chars() {
            if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '_' && c != '-' && c != '.' {
                return Err(format!(
                    "Invalid character '{}' in test name segment '{}'. Only lowercase alphanumeric, '_', '-', and '.' are allowed.",
                    c, segment
                ));
            }
        }
    }
    Ok(())
}

/// Scanner for visual regression test screenshots.
pub struct FileScanner;

impl FileScanner {
    /// Scans screenshots inside `base_dir` using include and exclude glob patterns,
    /// groups them into TestCases by relative parent directory, and decodes PNG files.
    pub fn scan_files(
        include_globs: &[GlobPattern],
        exclude_globs: &[GlobPattern],
        base_dir: &Path,
    ) -> Result<Vec<TestCase>, ScannerError> {
        let mut include_builder = GlobSetBuilder::new();
        for pat in include_globs {
            include_builder.add(pat.as_glob().clone());
        }
        let include_set = include_builder.build()?;

        let mut exclude_builder = GlobSetBuilder::new();
        for pat in exclude_globs {
            exclude_builder.add(pat.as_glob().clone());
        }
        let exclude_set = exclude_builder.build()?;

        let mut temp_cases = std::collections::BTreeMap::<String, Vec<TestImage>>::new();

        // Recursively traverse base_dir using ignore walker.
        // We set standard_filters(false) because we rely on config include/exclude.
        let walker = ignore::WalkBuilder::new(base_dir)
            .standard_filters(false)
            .build();

        for entry_res in walker {
            let entry = match entry_res {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!("Skipping unreadable directory or path: {}", err);
                    continue;
                }
            };

            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            // Get path relative to base_dir
            let rel_path = match path.strip_prefix(base_dir) {
                Ok(p) => p.to_path_buf(),
                Err(_) => continue,
            };

            // Convert to string using slash separators for glob matching
            let rel_path_str = rel_path.to_string_lossy().replace('\\', "/");

            // We only match PNG files
            if !rel_path_str.ends_with(".png") {
                continue;
            }

            // Filter using GlobSets
            if !include_set.is_match(&rel_path_str) {
                continue;
            }
            if exclude_set.is_match(&rel_path_str) {
                continue;
            }

            // Resolve test name (relative parent directory path)
            let test_name = if let Some(parent) = rel_path.parent() {
                let parent_str = parent.to_string_lossy().replace('\\', "/");
                if parent_str.is_empty() {
                    ".".to_string()
                } else {
                    parent_str
                }
            } else {
                ".".to_string()
            };

            // Validate the test name format strictly
            if let Err(reason) = validate_test_name(&test_name) {
                return Err(ScannerError::InvalidTestName {
                    name: test_name,
                    reason,
                });
            }

            // Decode image
            let image_result = image::open(path)
                .map(|img| img.to_rgba8())
                .map_err(|e| format!("Failed to decode image: {}", e));

            let test_image = TestImage {
                relative_path: rel_path,
                absolute_path: path.to_path_buf(),
                image: image_result,
            };

            temp_cases.entry(test_name).or_default().push(test_image);
        }

        // Convert the map to a sorted list of TestCases
        let cases = temp_cases
            .into_iter()
            .map(|(name, mut images)| {
                // Sort images by relative path to ensure deterministic order
                images.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
                TestCase { name, images }
            })
            .collect();

        Ok(cases)
    }

    /// Scans the workspace based on the rules in `GleonConfig` and a given base directory.
    pub fn scan_workspace(
        config: &GleonConfig,
        base_dir: &Path,
    ) -> Result<Vec<TestCase>, ScannerError> {
        let mut include_globs = Vec::new();
        for rule in &config.screenshots {
            include_globs.extend(rule.include.clone());
        }
        Self::scan_files(&include_globs, &config.exclude, base_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tiny 1x1 valid PNG bytes
    const VALID_PNG_BYTES: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn test_validate_test_name() {
        assert!(validate_test_name(".").is_ok());
        assert!(validate_test_name("billing").is_ok());
        assert!(validate_test_name("billing/stripe").is_ok());
        assert!(validate_test_name("billing/stripe-v2").is_ok());
        assert!(validate_test_name("billing/stripe.v2").is_ok());
        assert!(validate_test_name("billing/stripe_v2").is_ok());

        assert!(validate_test_name("billing/Stripe").is_err());
        assert!(validate_test_name("billing/").is_err());
        assert!(validate_test_name("/billing").is_err());
        assert!(validate_test_name("billing//stripe").is_err());
        assert!(validate_test_name("billing/stripe$").is_err());
    }

    #[test]
    fn test_scan_files_success_and_corrupt() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_path = temp_dir.path();

        // Create billing/stripe/form.png (valid)
        let billing_dir = base_path.join("billing").join("stripe");
        std::fs::create_dir_all(&billing_dir).unwrap();
        std::fs::write(billing_dir.join("form.png"), VALID_PNG_BYTES).unwrap();

        // Create settings/corrupt.png (invalid png)
        let settings_dir = base_path.join("settings");
        std::fs::create_dir_all(&settings_dir).unwrap();
        std::fs::write(settings_dir.join("corrupt.png"), b"not a png").unwrap();

        // Create ignored file (e.g. not a png)
        std::fs::write(billing_dir.join("notes.txt"), b"some text").unwrap();

        let include = vec![GlobPattern::new("**/*.png").unwrap()];
        let exclude = vec![];

        let cases = FileScanner::scan_files(&include, &exclude, base_path).unwrap();

        // We expect two test cases: "billing/stripe" and "settings"
        assert_eq!(cases.len(), 2);

        // First test case: billing/stripe
        assert_eq!(cases[0].name, "billing/stripe");
        assert_eq!(cases[0].images.len(), 1);
        assert_eq!(
            cases[0].images[0].relative_path,
            Path::new("billing/stripe/form.png")
        );
        assert!(cases[0].images[0].image.is_ok());

        // Second test case: settings
        assert_eq!(cases[1].name, "settings");
        assert_eq!(cases[1].images.len(), 1);
        assert_eq!(
            cases[1].images[0].relative_path,
            Path::new("settings/corrupt.png")
        );
        assert!(cases[1].images[0].image.is_err());
    }

    #[test]
    fn test_scan_files_with_excludes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_path = temp_dir.path();

        let billing_dir = base_path.join("billing").join("stripe");
        std::fs::create_dir_all(&billing_dir).unwrap();
        std::fs::write(billing_dir.join("form.png"), VALID_PNG_BYTES).unwrap();

        let settings_dir = base_path.join("settings");
        std::fs::create_dir_all(&settings_dir).unwrap();
        std::fs::write(settings_dir.join("profile.png"), VALID_PNG_BYTES).unwrap();

        let include = vec![GlobPattern::new("**/*.png").unwrap()];
        // Exclude everything under settings/
        let exclude = vec![GlobPattern::new("settings/**/*.png").unwrap()];

        let cases = FileScanner::scan_files(&include, &exclude, base_path).unwrap();

        // Only billing/stripe should remain
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].name, "billing/stripe");
    }

    #[test]
    fn test_scan_files_empty_results() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_path = temp_dir.path();

        let billing_dir = base_path.join("billing");
        std::fs::create_dir_all(&billing_dir).unwrap();
        std::fs::write(billing_dir.join("notes.txt"), b"not a png").unwrap();

        let include = vec![GlobPattern::new("**/*.png").unwrap()];
        let exclude = vec![];

        let cases = FileScanner::scan_files(&include, &exclude, base_path).unwrap();
        assert!(
            cases.is_empty(),
            "Expected empty results when no PNG files match include patterns"
        );
    }

    #[test]
    fn test_scan_invalid_test_name_returns_error() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_path = temp_dir.path();

        // Folder with invalid character (uppercase)
        let billing_dir = base_path.join("Billing");
        std::fs::create_dir_all(&billing_dir).unwrap();
        std::fs::write(billing_dir.join("form.png"), VALID_PNG_BYTES).unwrap();

        let include = vec![GlobPattern::new("**/*.png").unwrap()];
        let exclude = vec![];

        let result = FileScanner::scan_files(&include, &exclude, base_path);
        assert!(result.is_err());
        assert!(matches!(
            result.err().unwrap(),
            ScannerError::InvalidTestName { .. }
        ));
    }

    #[cfg(all(unix, not(miri)))]
    fn make_unreadable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[cfg(all(unix, not(miri)))]
    fn make_readable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    #[cfg(all(unix, not(miri)))]
    fn test_unreadable_directory_ignored() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_path = temp_dir.path();

        let unreadable_dir = base_path.join("unreadable");
        std::fs::create_dir(&unreadable_dir).unwrap();
        std::fs::write(unreadable_dir.join("image.png"), VALID_PNG_BYTES).unwrap();

        let readable_dir = base_path.join("readable");
        std::fs::create_dir(&readable_dir).unwrap();
        std::fs::write(readable_dir.join("image.png"), VALID_PNG_BYTES).unwrap();

        make_unreadable(&unreadable_dir);

        let include = vec![GlobPattern::new("**/*.png").unwrap()];
        let exclude = vec![];

        let result = FileScanner::scan_files(&include, &exclude, base_path);

        // Always restore permissions before running assertions!
        make_readable(&unreadable_dir);

        let cases = result.unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].name, "readable");
    }
}
