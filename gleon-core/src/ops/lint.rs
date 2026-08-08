//! Lint operation for validating manifest schema integrity and checking for Git merge conflict markers.

use crate::context::ResolvedContext;
use crate::manifest::single::SingleTestManifest;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during manifest linting.
#[derive(Debug, Error)]
pub enum LintError {
    /// The target manifests directory was not found.
    #[error("Manifest directory does not exist at {0}")]
    ManifestDirNotFound(PathBuf),

    /// Invalid platform filter path provided.
    #[error("Invalid platform filter '{0}': must be a single platform segment")]
    InvalidPlatformFilter(String),

    /// Standard IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Summary report returned by manifest linting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintReport {
    /// Total manifest JSON files inspected.
    pub total_files: usize,
    /// Number of schema-valid, unconflicted manifest files.
    pub valid_files: usize,
    /// Files containing Git merge conflict markers (`<<<<<<<`), stored as `(relative_path, reason)`.
    pub conflicted_files: Vec<(PathBuf, String)>,
    /// Files failing JSON schema or data validation, stored as `(relative_path, error)`.
    pub corrupted_files: Vec<(PathBuf, String)>,
    /// Overall pass/fail status (`true` if no conflicts or corruption).
    pub passed: bool,
}

/// Validates all manifest JSON files in `.gleon/manifests/`.
///
/// If `platform_filter` is provided, only manifests under `.gleon/manifests/<platform>/` are checked.
/// Otherwise, all platform manifests are validated recursively.
///
/// # Errors
/// Returns [`LintError`] if reading directory fails or if the target directory is missing.
pub fn lint_workspace_manifests(
    _ctx: &ResolvedContext,
    base_dir: &Path,
    platform_filter: Option<&str>,
) -> Result<LintReport, LintError> {
    let manifests_root = base_dir.join(".gleon").join("manifests");

    let search_dir = match platform_filter {
        Some(p) => {
            let path = Path::new(p);
            let mut components = path.components();
            match (components.next(), components.next()) {
                (Some(std::path::Component::Normal(seg)), None) => {
                    let seg_str = seg.to_string_lossy();
                    if crate::manifest::index::validate_test_path(&seg_str).is_err() {
                        return Err(LintError::InvalidPlatformFilter(p.to_string()));
                    }
                    manifests_root.join(p)
                }
                _ => return Err(LintError::InvalidPlatformFilter(p.to_string())),
            }
        }
        None => manifests_root,
    };

    let mut total_files = 0;
    let mut valid_files = 0;
    let mut conflicted_files = Vec::new();
    let mut corrupted_files = Vec::new();

    for entry_res in WalkBuilder::new(&search_dir)
        .standard_filters(false)
        .build()
    {
        let entry = match entry_res {
            Ok(e) => e,
            Err(err) => {
                let err_msg = err.to_string();
                if let Some(io_err) = err.into_io_error() {
                    if io_err.kind() == std::io::ErrorKind::NotFound {
                        return Err(LintError::ManifestDirNotFound(search_dir));
                    }
                    return Err(LintError::Io(io_err));
                }
                tracing::warn!("Skipping unreadable manifest entry: {}", err_msg);
                continue;
            }
        };

        let path = entry.path();
        if entry.file_type().is_some_and(|ft| ft.is_file())
            && path.extension().is_some_and(|ext| ext == "json")
        {
            total_files += 1;
            let rel_path = path.strip_prefix(base_dir).unwrap_or(path).to_path_buf();

            match std::fs::read_to_string(path) {
                Ok(content) => {
                    if content.contains("<<<<<<<") {
                        conflicted_files.push((
                            rel_path,
                            "Git merge conflict markers (<<<<<<<) detected".to_string(),
                        ));
                    } else {
                        match serde_json::from_str::<SingleTestManifest>(&content) {
                            Ok(manifest) => match manifest.validate() {
                                Ok(()) => {
                                    valid_files += 1;
                                }
                                Err(e) => {
                                    corrupted_files.push((
                                        rel_path,
                                        format!("Manifest schema validation failed: {}", e),
                                    ));
                                }
                            },
                            Err(e) => {
                                corrupted_files
                                    .push((rel_path, format!("Invalid JSON syntax: {}", e)));
                            }
                        }
                    }
                }
                Err(e) => {
                    corrupted_files.push((rel_path, format!("Failed to read file: {}", e)));
                }
            }
        }
    }

    let passed = conflicted_files.is_empty() && corrupted_files.is_empty();

    Ok(LintReport {
        total_files,
        valid_files,
        conflicted_files,
        corrupted_files,
        passed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Commands};
    use tempfile::tempdir;

    #[test]
    fn test_lint_clean_manifests() {
        let temp = tempdir().unwrap();
        let manifests_dir = temp
            .path()
            .join(".gleon")
            .join("manifests")
            .join("macos-aarch64");
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let manifest_content = include_str!("../../tests/fixtures/valid_manifest.json");
        std::fs::write(manifests_dir.join("login.json"), manifest_content).unwrap();

        let cli = Cli::for_test(Commands::Init);
        let ctx = ResolvedContext::from_cli(&cli, temp.path()).unwrap();
        let report = lint_workspace_manifests(&ctx, temp.path(), None).unwrap();

        assert_eq!(report.total_files, 1);
        assert_eq!(report.valid_files, 1);
        assert!(report.passed);
        assert!(report.conflicted_files.is_empty());
        assert!(report.corrupted_files.is_empty());
    }

    #[test]
    fn test_lint_detects_conflicts_and_corruption() {
        let temp = tempdir().unwrap();
        let manifests_dir = temp
            .path()
            .join(".gleon")
            .join("manifests")
            .join("macos-aarch64");
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let conflicted = include_str!("../../tests/fixtures/conflict_2way.json");
        std::fs::write(manifests_dir.join("conflict.json"), conflicted).unwrap();

        let corrupted = include_str!("../../tests/fixtures/corrupt_manifest.json");
        std::fs::write(manifests_dir.join("corrupt.json"), corrupted).unwrap();

        let cli = Cli::for_test(Commands::Init);
        let ctx = ResolvedContext::from_cli(&cli, temp.path()).unwrap();
        let report = lint_workspace_manifests(&ctx, temp.path(), None).unwrap();

        assert_eq!(report.total_files, 2);
        assert_eq!(report.valid_files, 0);
        assert!(!report.passed);
        assert_eq!(report.conflicted_files.len(), 1);
        assert_eq!(report.corrupted_files.len(), 1);
    }

    #[test]
    fn test_lint_platform_filter_validation_and_missing_dir() {
        let temp = tempdir().unwrap();
        let cli = Cli::for_test(Commands::Init);
        let ctx = ResolvedContext::from_cli(&cli, temp.path()).unwrap();

        // 1. Missing directory
        let err = lint_workspace_manifests(&ctx, temp.path(), None);
        assert!(matches!(err, Err(LintError::ManifestDirNotFound(_))));

        // 2. Traversal platform filter
        let err_traversal = lint_workspace_manifests(&ctx, temp.path(), Some("../../etc"));
        assert!(matches!(
            err_traversal,
            Err(LintError::InvalidPlatformFilter(_))
        ));

        // 3. Absolute path filter
        let err_abs = lint_workspace_manifests(&ctx, temp.path(), Some("/tmp"));
        assert!(matches!(err_abs, Err(LintError::InvalidPlatformFilter(_))));

        // 4. Multi-segment platform filter
        let err_multi = lint_workspace_manifests(&ctx, temp.path(), Some("linux/x86_64"));
        assert!(matches!(
            err_multi,
            Err(LintError::InvalidPlatformFilter(_))
        ));

        // 5. Invalid characters in normal platform filter
        let err_invalid_chars = lint_workspace_manifests(&ctx, temp.path(), Some("LINUX"));
        assert!(matches!(
            err_invalid_chars,
            Err(LintError::InvalidPlatformFilter(_))
        ));

        // 6. Valid platform filter on missing directory
        let err_valid = lint_workspace_manifests(&ctx, temp.path(), Some("linux-x86_64"));
        assert!(matches!(err_valid, Err(LintError::ManifestDirNotFound(_))));
    }

    #[cfg(all(unix, not(miri)))]
    #[test]
    fn test_lint_unreadable_file_and_directory() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempdir().unwrap();
        let cli = Cli::for_test(Commands::Init);
        let ctx = ResolvedContext::from_cli(&cli, temp.path()).unwrap();
        let manifests_dir = temp.path().join(".gleon").join("manifests");
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let valid_json = "{}";
        let unreadable_file = manifests_dir.join("unreadable.json");
        std::fs::write(&unreadable_file, valid_json).unwrap();

        // Make file unreadable
        let mut perms = std::fs::metadata(&unreadable_file).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&unreadable_file, perms).unwrap();

        let report = lint_workspace_manifests(&ctx, temp.path(), None).unwrap();
        assert!(!report.passed);
        assert_eq!(report.corrupted_files.len(), 1);
        assert!(report.corrupted_files[0].1.contains("Failed to read file:"));

        // Restore permissions to allow cleanup
        let mut perms = std::fs::metadata(&unreadable_file).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&unreadable_file, perms).unwrap();
    }

    #[cfg(all(unix, not(miri)))]
    #[test]
    fn test_lint_unreadable_directory() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempdir().unwrap();
        let cli = Cli::for_test(Commands::Init);
        let ctx = ResolvedContext::from_cli(&cli, temp.path()).unwrap();
        let manifests_dir = temp.path().join(".gleon").join("manifests");
        std::fs::create_dir_all(&manifests_dir).unwrap();

        let sub_dir = manifests_dir.join("sub");
        std::fs::create_dir_all(&sub_dir).unwrap();
        let test_file = sub_dir.join("test.json");
        std::fs::write(&test_file, "{}").unwrap();

        // Make sub_dir unreadable
        let mut perms = std::fs::metadata(&sub_dir).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&sub_dir, perms).unwrap();

        // Running lint should fail with an IoError for the unreadable directory (since into_io_error() is Some)
        let res = lint_workspace_manifests(&ctx, temp.path(), None);
        assert!(matches!(res, Err(LintError::Io(_))));

        // Restore permissions to allow cleanup
        let mut perms = std::fs::metadata(&sub_dir).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&sub_dir, perms).unwrap();
    }

    #[test]
    fn test_lint_detects_json_syntax_and_schema_errors() {
        let temp = tempdir().unwrap();
        let manifests_dir = temp
            .path()
            .join(".gleon")
            .join("manifests")
            .join("macos-aarch64");
        std::fs::create_dir_all(&manifests_dir).unwrap();

        // 1. Invalid JSON syntax
        std::fs::write(manifests_dir.join("syntax.json"), "invalid_json").unwrap();

        // 2. Valid JSON but invalid schema validation (e.g. invalid hash scheme)
        let bad_schema = "{\"schema_version\":1,\"hash\":\"invalid:123\",\"phash\":\"dhash:0000000000000000\",\"width\":10,\"height\":10}";
        std::fs::write(manifests_dir.join("schema.json"), bad_schema).unwrap();

        let cli = Cli::for_test(Commands::Init);
        let ctx = ResolvedContext::from_cli(&cli, temp.path()).unwrap();
        let report = lint_workspace_manifests(&ctx, temp.path(), None).unwrap();

        assert_eq!(report.total_files, 2);
        assert_eq!(report.valid_files, 0);
        assert!(!report.passed);
        assert_eq!(report.corrupted_files.len(), 2);
    }

    #[test]
    fn test_lint_error_display() {
        let err_missing = LintError::ManifestDirNotFound(PathBuf::from("/missing"));
        assert!(err_missing.to_string().contains("does not exist"));

        let err_filter = LintError::InvalidPlatformFilter("bad/filter".to_string());
        assert!(err_filter.to_string().contains("Invalid platform filter"));

        let io_err = std::io::Error::other("test error");
        let err_io = LintError::Io(io_err);
        assert!(err_io.to_string().contains("IO error"));
    }
}
