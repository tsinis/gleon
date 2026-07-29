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
        Some(p) => manifests_root.join(p),
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
}
