//! File scanner and image decoder for visual regression tests.

use crate::config::{GleonConfig, GlobPattern};
use globset::GlobSetBuilder;

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

use std::borrow::Cow;

/// A single test screenshot file within a TestCase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestImage {
    /// Relative path from the base directory (e.g. "billing/stripe/form.png")
    pub relative_path: PathBuf,
    /// Absolute path to the file on disk
    pub absolute_path: PathBuf,
}

/// A single visual regression test case corresponding to one screenshot.
#[derive(Debug, Clone)]
pub struct TestCase {
    /// The test name (relative path without extension, e.g. "billing/stripe/form")
    pub name: String,
    /// The screenshot belonging to this test case
    pub image: TestImage,
    /// The configuration rule that matched this test case
    pub rule: std::sync::Arc<crate::config::ScreenshotRule>,
}

use crate::engine::MismatchDetail;

/// Represents the result of running a test on a single screenshot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
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
    DimensionMismatch {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
        /// Dimensions of the baseline image.
        baseline_size: (u32, u32),
        /// Dimensions of the actual image.
        actual_size: (u32, u32),
        /// Path to the baseline image on disk.
        baseline_path: PathBuf,
        /// Path to the actual image on disk.
        actual_path: PathBuf,
    },
    /// The screenshot failed the visual comparison threshold.
    Mismatch {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
        /// Specific detail about the comparison mismatch.
        detail: MismatchDetail,
        /// Path to the diff visualization image on disk.
        diff_path: PathBuf,
        /// Path to the baseline image on disk.
        baseline_path: PathBuf,
        /// Path to the actual image on disk.
        actual_path: PathBuf,
    },
    /// The baseline snapshot or blob is missing.
    MissingBaseline {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
        /// Reason/details for the missing baseline.
        reason: String,
    },
    /// An I/O error occurred while reading or saving the image.
    IoError {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
        /// The I/O error message.
        error: String,
    },
    /// An error occurred while encoding the diff image.
    EncodeError {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
        /// Path to the actual image on disk.
        actual_path: PathBuf,
        /// The encoding error message.
        error: String,
    },
}

impl TestImageResult {
    /// Returns the relative path of the screenshot file.
    pub fn relative_path(&self) -> &Path {
        match self {
            Self::Success { relative_path } => relative_path,
            Self::DecodeError { relative_path, .. } => relative_path,
            Self::MissingBaseline { relative_path, .. } => relative_path,
            Self::DimensionMismatch { relative_path, .. } => relative_path,
            Self::Mismatch { relative_path, .. } => relative_path,
            Self::IoError { relative_path, .. } => relative_path,
            Self::EncodeError { relative_path, .. } => relative_path,
        }
    }
}

/// Represents the final evaluation result of a complete test case.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct TestCaseResult {
    /// The test case name.
    pub name: String,
    /// Result of the single screenshot within the test case.
    pub result: TestImageResult,
}

impl TestCaseResult {
    /// Returns true if the screenshot result succeeded.
    pub fn passed(&self) -> bool {
        matches!(self.result, TestImageResult::Success { .. })
    }
}

/// Validates that all segments of a test name contain only allowed characters `[a-z0-9_.-]`.
/// The name can use either Unix-style forward slashes (`/`) or Windows-style backslashes (`\`) as separators.
pub fn validate_test_name(name: &str) -> Result<(), String> {
    for segment in name.split(['/', '\\']) {
        if segment.is_empty() {
            return Err("Test name segment cannot be empty".to_string());
        }
        if segment == "." || segment == ".." {
            return Err(format!(
                "Test name segment cannot be relative path navigation '{}'",
                segment
            ));
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

/// Default directories unconditionally pruned during scanner traversal to prevent hanging
/// or indexing build artifacts across frontend ecosystems (Flutter, Android, iOS, Web/Node, Rust, Go).
pub const DEFAULT_PRUNED_DIRECTORIES: &[&str] = &[
    ".git",
    ".gleon",
    ".dart_tool",
    "build",
    "target",
    "node_modules",
    "vendor",
    "DerivedData",
    ".gradle",
];

/// Scanner for visual regression test screenshots.
pub struct FileScanner;

impl FileScanner {
    /// Scans screenshots inside `base_dir` using include and exclude glob patterns.
    /// Each discovered screenshot produces its own `TestCase`.
    /// The provided `rule` is attached to each resulting `TestCase` to carry mode/threshold/mask config.
    pub fn scan_files(
        include_globs: &[GlobPattern],
        exclude_globs: &[GlobPattern],
        base_dir: &Path,
        rule: std::sync::Arc<crate::config::ScreenshotRule>,
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

        let walker = Self::build_walker(base_dir, &exclude_set);

        let mut temp_cases = std::collections::BTreeMap::<String, TestImage>::new();

        for entry_res in walker {
            let entry = match entry_res {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!("Skipping unreadable directory or path: {}", err);
                    continue;
                }
            };

            if let Some((test_name, rel_path, abs_path)) =
                Self::parse_entry(&entry, base_dir, &include_set, &exclude_set)?
            {
                if temp_cases.contains_key(test_name.as_str()) {
                    tracing::warn!(
                        "Duplicate test name '{}' detected for relative path {:?}. Skipping duplicate.",
                        test_name,
                        rel_path
                    );
                } else {
                    temp_cases.insert(
                        test_name,
                        TestImage {
                            relative_path: rel_path,
                            absolute_path: abs_path,
                        },
                    );
                }
            }
        }

        let cases = temp_cases
            .into_iter()
            .map(|(name, image)| TestCase {
                name,
                image,
                rule: rule.clone(),
            })
            .collect::<Vec<_>>();

        Ok(cases)
    }

    /// Scans the workspace based on the rules in `GleonConfig` and a given base directory.
    pub fn scan_workspace(
        config: &GleonConfig,
        base_dir: &Path,
    ) -> Result<Vec<TestCase>, ScannerError> {
        let mut exclude_builder = GlobSetBuilder::new();
        for pat in &config.exclude {
            exclude_builder.add(pat.as_glob().clone());
        }
        let exclude_set = exclude_builder.build()?;

        let mut rule_sets = Vec::new();
        for rule in &config.screenshots {
            let mut include_builder = GlobSetBuilder::new();
            for pat in &rule.include {
                include_builder.add(pat.as_glob().clone());
            }
            rule_sets.push((std::sync::Arc::new(rule.clone()), include_builder.build()?));
        }

        let walker = Self::build_walker(base_dir, &exclude_set);

        let mut temp_cases = std::collections::BTreeMap::<
            String,
            (TestImage, std::sync::Arc<crate::config::ScreenshotRule>),
        >::new();

        for entry_res in walker {
            let entry = match entry_res {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!("Skipping unreadable directory or path: {}", err);
                    continue;
                }
            };

            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }
            let path = entry.path();
            if !path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
            {
                continue;
            }

            let rel_path = match path.strip_prefix(base_dir) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("Failed to strip base_dir prefix from path: {}", e);
                    continue;
                }
            };
            let rel_path_str = Self::normalize_path_str(rel_path);

            if exclude_set.is_match(rel_path_str.as_ref()) {
                continue;
            }

            let matched_rule = rule_sets
                .iter()
                .find(|(_, inc_set)| inc_set.is_match(rel_path_str.as_ref()));

            if let Some((rule_arc, _)) = matched_rule {
                let path_without_ext = rel_path.with_extension("");
                let test_name_cow = Self::normalize_path_str(&path_without_ext);
                let test_name_norm = test_name_cow.as_ref();

                if temp_cases.contains_key(test_name_norm) {
                    tracing::warn!(
                        "Duplicate test name '{}' detected for relative path {:?}. Skipping duplicate.",
                        test_name_norm,
                        rel_path
                    );
                } else {
                    if let Err(reason) = validate_test_name(test_name_norm) {
                        return Err(ScannerError::InvalidTestName {
                            name: test_name_norm.to_string(),
                            reason,
                        });
                    }
                    temp_cases.insert(
                        test_name_norm.to_string(),
                        (
                            TestImage {
                                relative_path: rel_path.to_path_buf(),
                                absolute_path: path.to_path_buf(),
                            },
                            rule_arc.clone(),
                        ),
                    );
                }
            }
        }

        let cases = temp_cases
            .into_iter()
            .map(|(name, (image, rule))| TestCase { name, image, rule })
            .collect();
        Ok(cases)
    }

    /// Builds a WalkBuilder configured for gleon directory scanning.
    fn build_walker(base_dir: &Path, exclude_set: &globset::GlobSet) -> ignore::Walk {
        let exclude_for_filter = exclude_set.clone();
        let base_dir_for_filter = base_dir.to_path_buf();

        ignore::WalkBuilder::new(base_dir)
            .standard_filters(false)
            .filter_entry(move |entry| {
                if entry.file_type().is_some_and(|ft| ft.is_dir())
                    && entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| DEFAULT_PRUNED_DIRECTORIES.contains(&name))
                {
                    return false;
                }
                if let Ok(rel_path) = entry.path().strip_prefix(&base_dir_for_filter) {
                    if rel_path.as_os_str().is_empty() {
                        return true;
                    }
                    let rel_path_str = Self::normalize_path_str(rel_path);

                    // We must check if the directory string EXACTLY matches an exclude pattern.
                    // However, ignore globs typically target files (like `**/*.png`).
                    // Active directory pruning is notoriously hard with globset unless the glob explicitly ignores a directory prefix.
                    // If the user ignores `node_modules/`, globset might not match it cleanly here without trailing slash,
                    // but we handle standard cases explicitly above to prevent hanging.
                    if exclude_for_filter.is_match(rel_path_str.as_ref()) {
                        return false;
                    }
                }
                true
            })
            .build()
    }

    /// Normalizes path separators to forward slashes for cross-platform manifest key consistency.
    pub fn normalize_path_str(path: &Path) -> Cow<'_, str> {
        match path.to_string_lossy() {
            Cow::Borrowed(s) => crate::manifest::normalize_test_name(s),
            Cow::Owned(s) => match crate::manifest::normalize_test_name(&s) {
                Cow::Borrowed(_) => Cow::Owned(s),
                Cow::Owned(new_s) => Cow::Owned(new_s),
            },
        }
    }

    /// Parses a directory entry and returns the parsed paths if it's a valid matching PNG.
    fn parse_entry(
        entry: &ignore::DirEntry,
        base_dir: &Path,
        include_set: &globset::GlobSet,
        exclude_set: &globset::GlobSet,
    ) -> Result<Option<(String, PathBuf, PathBuf)>, ScannerError> {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            return Ok(None);
        }
        let path = entry.path();

        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
        {
            return Ok(None);
        }

        let rel_path = match path.strip_prefix(base_dir) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Failed to strip base_dir prefix from path: {}", e);
                return Ok(None);
            }
        };
        let rel_path_str = Self::normalize_path_str(rel_path);

        if !include_set.is_match(rel_path_str.as_ref())
            || exclude_set.is_match(rel_path_str.as_ref())
        {
            return Ok(None);
        }

        let path_without_ext = rel_path.with_extension("");
        let test_name_cow = Self::normalize_path_str(&path_without_ext);
        let test_name_str = test_name_cow.as_ref();

        if let Err(reason) = validate_test_name(test_name_str) {
            return Err(ScannerError::InvalidTestName {
                name: test_name_str.to_string(),
                reason,
            });
        }

        Ok(Some((
            test_name_str.to_string(),
            rel_path.to_path_buf(),
            path.to_path_buf(),
        )))
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
        assert!(validate_test_name(".").is_err());
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
        assert!(validate_test_name("billing/..").is_err());
        assert!(validate_test_name("billing/.").is_err());
        assert!(validate_test_name("billing/../stripe").is_err());
    }

    #[test]
    fn test_normalize_path_str() {
        let p1 = Path::new("billing/stripe/form.png");
        assert_eq!(
            FileScanner::normalize_path_str(p1),
            "billing/stripe/form.png"
        );

        let p2 = Path::new("billing\\stripe\\form.png");
        assert_eq!(
            FileScanner::normalize_path_str(p2),
            "billing/stripe/form.png"
        );
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

        let cases = FileScanner::scan_files(
            &include,
            &exclude,
            base_path,
            std::sync::Arc::new(crate::config::ScreenshotRule {
                include: include.clone(),
                mode: crate::config::Mode::Pixel,
                diff: crate::config::DiffConfig::default(),
                masks: vec![],
            }),
        )
        .unwrap();

        // We expect two test cases: "billing/stripe/form" and "settings/corrupt"
        assert_eq!(cases.len(), 2);

        // First test case: billing/stripe
        assert_eq!(cases[0].name, "billing/stripe/form");
        assert_eq!(
            cases[0].image.relative_path,
            Path::new("billing/stripe/form.png")
        );
        assert_eq!(
            cases[1].image.relative_path,
            Path::new("settings/corrupt.png")
        );
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

        let cases = FileScanner::scan_files(
            &include,
            &exclude,
            base_path,
            std::sync::Arc::new(crate::config::ScreenshotRule {
                include: include.clone(),
                mode: crate::config::Mode::Pixel,
                diff: crate::config::DiffConfig::default(),
                masks: vec![],
            }),
        )
        .unwrap();

        // Only billing/stripe/form should remain
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].name, "billing/stripe/form");
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

        let cases = FileScanner::scan_files(
            &include,
            &exclude,
            base_path,
            std::sync::Arc::new(crate::config::ScreenshotRule {
                include: include.clone(),
                mode: crate::config::Mode::Pixel,
                diff: crate::config::DiffConfig::default(),
                masks: vec![],
            }),
        )
        .unwrap();
        assert!(
            cases.is_empty(),
            "Expected empty results when no PNG files match include patterns"
        );
    }

    #[test]
    fn test_scan_files_include_mismatch() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_path = temp_dir.path();

        let billing_dir = base_path.join("billing");
        std::fs::create_dir_all(&billing_dir).unwrap();
        // This is a PNG but won't match our specific include pattern "settings/**/*.png"
        std::fs::write(billing_dir.join("form.png"), VALID_PNG_BYTES).unwrap();

        let include = vec![GlobPattern::new("settings/**/*.png").unwrap()];
        let exclude = vec![];

        let cases = FileScanner::scan_files(
            &include,
            &exclude,
            base_path,
            std::sync::Arc::new(crate::config::ScreenshotRule {
                include: include.clone(),
                mode: crate::config::Mode::Pixel,
                diff: crate::config::DiffConfig::default(),
                masks: vec![],
            }),
        )
        .unwrap();
        assert!(
            cases.is_empty(),
            "Expected empty results when PNG does not match include set"
        );
    }

    #[test]
    fn test_scan_workspace() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_path = temp_dir.path();

        let billing_dir = base_path.join("billing");
        std::fs::create_dir_all(&billing_dir).unwrap();
        std::fs::write(billing_dir.join("form.png"), VALID_PNG_BYTES).unwrap();

        // Construct mock GleonConfig
        let raw_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing/**/*.png"
exclude:
  - "**/corrupt.png"
"#;
        let config: GleonConfig = serde_yaml::from_str(raw_yaml).unwrap();

        let cases = FileScanner::scan_workspace(&config, base_path).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].name, "billing/form");
        assert_eq!(cases[0].image.relative_path, Path::new("billing/form.png"));
    }

    #[test]
    fn test_normalize_path_str_backslash() {
        let backslash_path = Path::new("billing\\form.png");
        let normalized = FileScanner::normalize_path_str(backslash_path);
        assert_eq!(normalized, "billing/form.png");
    }

    #[test]
    fn test_scan_workspace_error() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_path = temp_dir.path();

        // Invalid directory name (space)
        let billing_dir = base_path.join("billing dir");
        std::fs::create_dir_all(&billing_dir).unwrap();
        std::fs::write(billing_dir.join("form.png"), VALID_PNG_BYTES).unwrap();

        let raw_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing dir/**/*.png"
"#;
        let config: GleonConfig = serde_yaml::from_str(raw_yaml).unwrap();

        let result = FileScanner::scan_workspace(&config, base_path);
        assert!(result.is_err());
        assert!(matches!(
            result.err().unwrap(),
            ScannerError::InvalidTestName { .. }
        ));
    }

    #[test]
    fn test_scan_workspace_multiple_rules_same_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_path = temp_dir.path();

        let billing_dir = base_path.join("billing");
        std::fs::create_dir_all(&billing_dir).unwrap();
        std::fs::write(billing_dir.join("form.png"), VALID_PNG_BYTES).unwrap();
        std::fs::write(billing_dir.join("receipt.png"), VALID_PNG_BYTES).unwrap();

        // Construct mock GleonConfig with two rules targeting different files in the same directory
        let raw_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing/form.png"
    mode: pixel
  - include: "billing/receipt.png"
    mode: ssim
"#;
        let config: GleonConfig = serde_yaml::from_str(raw_yaml).unwrap();

        let cases = FileScanner::scan_workspace(&config, base_path).unwrap();
        // We should get 2 separate TestCases for the "billing" directory
        // because they matched different rules.
        assert_eq!(cases.len(), 2);

        let pixel_case = cases
            .iter()
            .find(|c| c.rule.mode == crate::config::Mode::Pixel)
            .unwrap();
        let ssim_case = cases
            .iter()
            .find(|c| c.rule.mode == crate::config::Mode::Ssim)
            .unwrap();

        assert_eq!(pixel_case.name, "billing/form");
        assert_eq!(
            pixel_case.image.relative_path,
            std::path::Path::new("billing/form.png")
        );

        assert_eq!(ssim_case.name, "billing/receipt");
        assert_eq!(
            ssim_case.image.relative_path,
            std::path::Path::new("billing/receipt.png")
        );
    }

    #[test]
    fn test_scan_invalid_test_name_returns_error() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_path = temp_dir.path();

        // Folder with invalid character (space)
        let billing_dir = base_path.join("billing dir");
        std::fs::create_dir_all(&billing_dir).unwrap();
        std::fs::write(billing_dir.join("form.png"), VALID_PNG_BYTES).unwrap();

        let include = vec![GlobPattern::new("**/*.png").unwrap()];
        let exclude = vec![];

        let result = FileScanner::scan_files(
            &include,
            &exclude,
            base_path,
            std::sync::Arc::new(crate::config::ScreenshotRule {
                include: include.clone(),
                mode: crate::config::Mode::Pixel,
                diff: crate::config::DiffConfig::default(),
                masks: vec![],
            }),
        );
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

        let result = FileScanner::scan_files(
            &include,
            &exclude,
            base_path,
            std::sync::Arc::new(crate::config::ScreenshotRule {
                include: include.clone(),
                mode: crate::config::Mode::Pixel,
                diff: crate::config::DiffConfig::default(),
                masks: vec![],
            }),
        );

        // Always restore permissions before running assertions!
        make_readable(&unreadable_dir);

        let cases = result.unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].name, "readable/image");
    }

    #[test]
    fn test_derived_traits() {
        // This test ensures that derived traits (like Debug) are executed.
        let io_err = ScannerError::Io(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert!(!format!("{:?}", io_err).is_empty());
        assert!(!format!("{}", io_err).is_empty());

        let invalid_err = ScannerError::InvalidTestName {
            name: "Foo".to_string(),
            reason: "UpperCase".to_string(),
        };
        assert!(!format!("{:?}", invalid_err).is_empty());
        assert!(!format!("{}", invalid_err).is_empty());

        let pattern_err = ScannerError::Pattern(globset::Glob::new("[").unwrap_err());
        assert!(!format!("{:?}", pattern_err).is_empty());

        let mismatch_detail = MismatchDetail::Pixel { diff_count: 42 };
        assert!(!format!("{:?}", mismatch_detail).is_empty());
        assert!(mismatch_detail == MismatchDetail::Pixel { diff_count: 42 });

        let ssim_detail = MismatchDetail::Ssim { ssim_score: 0.99 };
        assert!(!format!("{:?}", ssim_detail).is_empty());

        let image_res = TestImageResult::DecodeError {
            relative_path: PathBuf::from("a.png"),
            error: "bad data".to_string(),
        };
        assert!(!format!("{:?}", image_res).is_empty());

        let tc_res = TestCaseResult {
            name: "test".to_string(),
            result: image_res,
        };
        assert!(!format!("{:?}", tc_res).is_empty());
    }

    #[test]
    fn test_scan_files_case_insensitive_extension() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_path = temp_dir.path();

        let billing_dir = base_path.join("billing");
        std::fs::create_dir_all(&billing_dir).unwrap();
        std::fs::write(billing_dir.join("form.PNG"), VALID_PNG_BYTES).unwrap();
        std::fs::write(billing_dir.join("profile.PnG"), VALID_PNG_BYTES).unwrap();

        let include = vec![GlobPattern::new("**/*.png").unwrap()];
        let exclude = vec![];

        let cases = FileScanner::scan_files(
            &include,
            &exclude,
            base_path,
            std::sync::Arc::new(crate::config::ScreenshotRule {
                include: include.clone(),
                mode: crate::config::Mode::Pixel,
                diff: crate::config::DiffConfig::default(),
                masks: vec![],
            }),
        )
        .unwrap();
        assert_eq!(
            cases.len(),
            2,
            "Expected to find both uppercase and mixed-case PNG files"
        );
    }

    #[test]
    fn test_parse_entry_strip_prefix_failure() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_path = temp_dir.path();

        let billing_dir = base_path.join("billing");
        std::fs::create_dir_all(&billing_dir).unwrap();
        let img_path = billing_dir.join("form.png");
        std::fs::write(&img_path, VALID_PNG_BYTES).unwrap();

        // Perform a walk to get a real DirEntry
        let walker = ignore::WalkBuilder::new(base_path).build();
        let mut entry_opt = None;
        for entry_res in walker {
            let entry = entry_res.unwrap();
            if entry.path().is_file() {
                entry_opt = Some(entry);
                break;
            }
        }
        let entry = entry_opt.expect("Should have found a file entry");

        // Now compile globs that match the absolute path
        let include_set = globset::GlobSetBuilder::new()
            .add(globset::Glob::new("**/billing/*.png").unwrap())
            .build()
            .unwrap();
        let exclude_set = globset::GlobSetBuilder::new().build().unwrap();

        // Pass a completely different base_dir
        let different_base = Path::new("/some/different/dir");

        // This should fail to strip prefix.
        let res =
            FileScanner::parse_entry(&entry, different_base, &include_set, &exclude_set).unwrap();
        assert!(
            res.is_none(),
            "Expected parse_entry to skip when prefix stripping fails, but got {:?}",
            res
        );
    }

    #[test]
    fn test_test_image_and_case_clone() {
        let test_image = TestImage {
            relative_path: PathBuf::from("rel.png"),
            absolute_path: PathBuf::from("abs.png"),
        };

        // Ensure they are cloneable
        let cloned_img = test_image.clone();
        assert_eq!(cloned_img.relative_path, test_image.relative_path);

        let test_case = TestCase {
            name: "test_case".to_string(),
            image: test_image.clone(),
            rule: std::sync::Arc::new(crate::config::ScreenshotRule {
                include: vec![],
                mode: crate::config::Mode::Pixel,
                diff: crate::config::DiffConfig {
                    threshold: 0.0,
                    anti_alias: false,
                    min_similarity: 0.99,
                },
                masks: vec![],
            }),
        };

        let cloned_case = test_case.clone();
        assert_eq!(cloned_case.name, test_case.name);
        assert_eq!(
            cloned_case.image.relative_path,
            test_case.image.relative_path
        );
    }

    #[test]
    fn test_normalize_separators() {
        let p1 = Path::new("billing/stripe/form.png");
        let res1 = FileScanner::normalize_path_str(p1);
        assert_eq!(res1, "billing/stripe/form.png");
        assert!(matches!(res1, std::borrow::Cow::Borrowed(_)));

        let p2 = Path::new("clean_path.png");
        let res2 = FileScanner::normalize_path_str(p2);
        assert_eq!(res2, "clean_path.png");
        assert!(matches!(res2, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn test_scan_workspace_nested_entries_vacant_and_occupied() {
        let temp_dir = tempfile::tempdir().unwrap();
        let base_path = temp_dir.path();

        let billing_dir = base_path.join("billing").join("stripe");
        std::fs::create_dir_all(&billing_dir).unwrap();
        std::fs::write(billing_dir.join("form1.png"), VALID_PNG_BYTES).unwrap();
        std::fs::write(billing_dir.join("form2.png"), VALID_PNG_BYTES).unwrap();

        let raw_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing/stripe/*.png"
"#;
        let config: GleonConfig = serde_yaml::from_str(raw_yaml).unwrap();

        let cases = FileScanner::scan_workspace(&config, base_path).unwrap();
        assert_eq!(cases.len(), 2);
    }
}
