//! Workspace cleanup operation.
//!
//! Removes local screenshot PNG files matching rules in `gleon.yaml`,
//! untracks them from the Git index (using `gix`), appends wildcard entries to
//! `.gitignore`, and purges temporary `.gleon/runs/` and `.gleon/diffs/` directories.

use crate::config::ConfigError;
use crate::context::{ContextError, ResolvedContext};
use crate::git::{GitError, GitResolver};
use crate::scanner::{FileScanner, ScannerError};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Error types that can occur during the clean operation.
#[derive(Debug, thiserror::Error)]
pub enum CleanError {
    /// Context resolution error
    #[error("Context error: {0}")]
    Context(#[from] ContextError),

    /// Configuration error
    #[error("Config error: {0}")]
    Config(#[from] ConfigError),

    /// Scanner error
    #[error("Scanner error: {0}")]
    Scanner(#[from] ScannerError),

    /// Git operation error
    #[error("Git error: {0}")]
    Git(#[from] GitError),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<crate::io::IoError> for CleanError {
    fn from(err: crate::io::IoError) -> Self {
        match err {
            crate::io::IoError::Io(e) => CleanError::Io(e),
            crate::io::IoError::JsonParse(e) => {
                CleanError::Io(std::io::Error::other(e.to_string()))
            }
        }
    }
}

/// Options controlling the clean workspace operation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanOptions {
    /// If true, only simulate the cleanup without modifying disk or Git index.
    pub dry_run: bool,
    /// If true, do not update .gitignore.
    pub skip_gitignore: bool,
    /// If true, do not delete .gleon/runs and .gleon/diffs directories.
    pub keep_runs: bool,
}

/// Result summary of the clean workspace operation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanResult {
    /// List of screenshot files deleted (or to be deleted in dry run).
    pub deleted_files: Vec<PathBuf>,
    /// List of files untracked from Git index (or to be untracked in dry run).
    pub untracked_files: Vec<PathBuf>,
    /// List of entries added to .gitignore (or to be added in dry run).
    pub gitignore_entries_added: Vec<String>,
    /// Whether runs/diffs cache was cleaned.
    pub cache_cleaned: bool,
}

/// Cleans screenshot files, untracks them from Git index, updates .gitignore, and cleans runs cache.
pub fn clean_workspace(
    context: &ResolvedContext,
    base_path: &Path,
    options: &CleanOptions,
) -> Result<CleanResult, CleanError> {
    let mut result = CleanResult::default();

    let config = context.config.as_ref().cloned().unwrap_or_default();

    // 1. Scan for all screenshots matched by rules in gleon.yaml
    let test_cases =
        FileScanner::scan_workspace(&config, base_path).map_err(CleanError::Scanner)?;
    let mut discovered_paths: Vec<PathBuf> = test_cases
        .into_iter()
        .map(|case| case.image.relative_path)
        .collect();

    discovered_paths.sort();
    discovered_paths.dedup();

    tracing::debug!(
        "Discovered {} screenshot(s) matching configuration rules",
        discovered_paths.len()
    );

    // 2. Untrack from Git index if not in dry-run mode (Graceful degradation on Git failure)
    if options.dry_run {
        result.deleted_files = discovered_paths.clone();
        result.untracked_files.clear();
    } else {
        if !discovered_paths.is_empty() {
            match GitResolver::untrack_from_index(base_path, &discovered_paths) {
                Ok(untracked) => {
                    tracing::debug!("Untracked {} screenshot(s) from Git index", untracked.len());
                    result.untracked_files = untracked;
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to untrack screenshots from Git index (continuing workspace clean): {e}"
                    );
                    result.untracked_files.clear();
                }
            }
        }

        // 3. Delete files from disk and prune empty parent directories
        let mut affected_dirs = std::collections::HashSet::new();
        for rel_path in &discovered_paths {
            let full_path = base_path.join(rel_path);
            match std::fs::remove_file(&full_path) {
                Ok(()) => {
                    result.deleted_files.push(rel_path.clone());
                    if let Some(parent) = full_path.parent() {
                        affected_dirs.insert(parent.to_path_buf());
                    }

                    // Walk up and prune empty parent directories up to base_path
                    let mut parent = full_path.parent();
                    while let Some(dir) = parent {
                        if dir == base_path || !dir.starts_with(base_path) {
                            break;
                        }
                        if std::fs::remove_dir(dir).is_ok() {
                            if let Some(grandparent) = dir.parent() {
                                affected_dirs.insert(grandparent.to_path_buf());
                            }
                        } else {
                            // Directory not empty or cannot remove, stop pruning upward
                            break;
                        }
                        parent = dir.parent();
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!("Failed to delete screenshot {:?}: {}", full_path, e);
                }
            }
        }

        // Commit directory entry changes to disk on POSIX systems (deduplicated per dir)
        for dir in affected_dirs {
            if let Ok(dir_file) = std::fs::File::open(dir) {
                let _ = dir_file.sync_all();
            }
        }

        tracing::debug!(
            "Deleted {} screenshot file(s) from disk and pruned empty directories",
            result.deleted_files.len()
        );
    }

    // 4. Update .gitignore with wildcard entries
    if !options.skip_gitignore {
        let gitignore_path = base_path.join(".gitignore");
        let existing = match std::fs::read_to_string(&gitignore_path) {
            Ok(content) => content,
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(CleanError::Io(e)),
        };

        let existing_set: std::collections::HashSet<&str> =
            existing.lines().map(str::trim).collect();
        let mut new_entries = Vec::new();
        let mut seen_new = std::collections::HashSet::new();

        for rule in &config.screenshots {
            for pattern in &rule.include {
                let pat_str = pattern.as_str();
                let entry: std::borrow::Cow<'_, str> = if pat_str.starts_with("**/")
                    || pat_str.contains('/')
                    || pat_str.contains('\\')
                {
                    std::borrow::Cow::Borrowed(pat_str)
                } else {
                    let mut s = String::with_capacity(3 + pat_str.len());
                    s.push_str("**/");
                    s.push_str(pat_str);
                    std::borrow::Cow::Owned(s)
                };

                if !existing_set.contains(entry.as_ref())
                    && !existing_set.contains(pat_str)
                    && seen_new.insert(entry.to_string())
                {
                    new_entries.push(entry.into_owned());
                }
            }
        }

        if !new_entries.is_empty() {
            result.gitignore_entries_added.clone_from(&new_entries);
            if !options.dry_run {
                use std::io::Write as IoWrite;
                let mut buffer = existing;
                if !buffer.is_empty() && !buffer.ends_with('\n') {
                    buffer.push('\n');
                }
                for entry in new_entries {
                    writeln!(buffer, "{entry}").expect("writing to String cannot fail");
                }
                crate::io::write_file_atomically(&gitignore_path, |writer| {
                    writer.write_all(buffer.as_bytes()).map_err(CleanError::Io)
                })?;
                tracing::debug!(
                    "Added {} new rule(s) to .gitignore",
                    result.gitignore_entries_added.len()
                );
            }
        }
    }

    // 5. Clean cache directories (.gleon/runs and .gleon/diffs)
    if !options.keep_runs {
        let gleon_dir = base_path.join(".gleon");
        let runs_dir = gleon_dir.join("runs");
        let diffs_dir = gleon_dir.join("diffs");

        if !options.dry_run {
            if let Err(e) = std::fs::remove_dir_all(&runs_dir)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(CleanError::Io(e));
            }
            if let Err(e) = std::fs::remove_dir_all(&diffs_dir)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(CleanError::Io(e));
            }
        }
        result.cache_cleaned = true;
        tracing::debug!("Cleaned .gleon/runs and .gleon/diffs cache directories");
    }

    Ok(result)
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::cli::{Cli, Commands};
    use tempfile::tempdir;

    #[test]
    fn test_clean_workspace_dry_run_and_execution() {
        let temp = tempdir().unwrap();
        let base_path = temp.path();

        // 1. Setup gleon.yaml
        let gleon_dir = base_path.join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();
        std::fs::create_dir_all(gleon_dir.join("runs")).unwrap();
        std::fs::create_dir_all(gleon_dir.join("diffs")).unwrap();

        let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include:
      - "test/goldens/**/*.png"
    mode: pixel
"#;
        std::fs::write(gleon_dir.join("gleon.yaml"), config_yaml).unwrap();

        // Create sample screenshot
        let golden_dir = base_path.join("test").join("goldens");
        std::fs::create_dir_all(&golden_dir).unwrap();
        let golden_file = golden_dir.join("login.png");
        std::fs::write(&golden_file, b"sample png").unwrap();

        let cli = Cli::for_test(Commands::Clean {
            dry_run: false,
            skip_gitignore: false,
            keep_runs: false,
        });
        let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

        // 2. Test dry-run
        let dry_opts = CleanOptions {
            dry_run: true,
            skip_gitignore: false,
            keep_runs: false,
        };
        let dry_res = clean_workspace(&ctx, base_path, &dry_opts).unwrap();
        assert_eq!(dry_res.deleted_files.len(), 1);
        assert_eq!(
            dry_res.deleted_files[0],
            PathBuf::from("test/goldens/login.png")
        );
        assert!(golden_file.exists()); // File still exists after dry run

        // 3. Test actual execution
        let exec_opts = CleanOptions::default();
        let exec_res = clean_workspace(&ctx, base_path, &exec_opts).unwrap();
        assert_eq!(exec_res.deleted_files.len(), 1);
        assert!(!golden_file.exists()); // File removed
        assert!(!golden_dir.exists()); // Empty parent dir pruned
        assert!(!gleon_dir.join("runs").exists()); // Runs dir removed
        assert!(!gleon_dir.join("diffs").exists()); // Diffs dir removed

        // 4. Verify .gitignore content
        let gitignore = std::fs::read_to_string(base_path.join(".gitignore")).unwrap();
        assert!(gitignore.contains("test/goldens/**/*.png"));
    }

    #[test]
    fn test_clean_workspace_skip_gitignore_and_keep_runs() {
        let temp = tempdir().unwrap();
        let base_path = temp.path();

        let gleon_dir = base_path.join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();
        let runs_dir = gleon_dir.join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();

        let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include:
      - "test/goldens/**/*.png"
    mode: pixel
"#;
        std::fs::write(gleon_dir.join("gleon.yaml"), config_yaml).unwrap();

        let golden_dir = base_path.join("test").join("goldens");
        std::fs::create_dir_all(&golden_dir).unwrap();
        let golden_file = golden_dir.join("app.png");
        std::fs::write(&golden_file, b"sample png").unwrap();

        let cli = Cli::for_test(Commands::Clean {
            dry_run: false,
            skip_gitignore: true,
            keep_runs: true,
        });
        let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

        let opts = CleanOptions {
            dry_run: false,
            skip_gitignore: true,
            keep_runs: true,
        };
        let res = clean_workspace(&ctx, base_path, &opts).unwrap();
        assert_eq!(res.deleted_files.len(), 1);
        assert!(!golden_file.exists());
        assert!(runs_dir.exists()); // runs preserved
        assert!(!base_path.join(".gitignore").exists()); // .gitignore not created
    }

    #[test]
    fn test_clean_workspace_appends_to_existing_gitignore_without_trailing_newline() {
        let temp = tempdir().unwrap();
        let base_path = temp.path();

        let gleon_dir = base_path.join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include:
      - "test/goldens/**/*.png"
    mode: pixel
"#;
        std::fs::write(gleon_dir.join("gleon.yaml"), config_yaml).unwrap();

        // Write existing .gitignore WITHOUT trailing newline
        std::fs::write(base_path.join(".gitignore"), b"target/").unwrap();

        let golden_dir = base_path.join("test").join("goldens");
        std::fs::create_dir_all(&golden_dir).unwrap();
        let golden_file = golden_dir.join("app.png");
        std::fs::write(&golden_file, b"sample png").unwrap();

        let cli = Cli::for_test(Commands::Clean {
            dry_run: false,
            skip_gitignore: false,
            keep_runs: false,
        });
        let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

        let opts = CleanOptions::default();
        let res = clean_workspace(&ctx, base_path, &opts).unwrap();
        assert_eq!(res.deleted_files.len(), 1);

        let gitignore = std::fs::read_to_string(base_path.join(".gitignore")).unwrap();
        assert_eq!(gitignore, "target/\ntest/goldens/**/*.png\n");
    }

    #[test]
    fn test_clean_workspace_fails_fast_on_unreadable_gitignore() {
        let temp = tempdir().unwrap();
        let base_path = temp.path();

        let gleon_dir = base_path.join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include:
      - "test/goldens/**/*.png"
    mode: pixel
"#;
        std::fs::write(gleon_dir.join("gleon.yaml"), config_yaml).unwrap();

        // Create .gitignore AS A DIRECTORY to force a non-NotFound read error (IsADirectory)
        std::fs::create_dir_all(base_path.join(".gitignore")).unwrap();

        let golden_dir = base_path.join("test").join("goldens");
        std::fs::create_dir_all(&golden_dir).unwrap();
        let golden_file = golden_dir.join("app.png");
        std::fs::write(&golden_file, b"sample png").unwrap();

        let cli = Cli::for_test(Commands::Clean {
            dry_run: false,
            skip_gitignore: false,
            keep_runs: false,
        });
        let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

        let opts = CleanOptions::default();
        let err = clean_workspace(&ctx, base_path, &opts).unwrap_err();
        assert!(matches!(err, CleanError::Io(_)));
    }

    #[test]
    fn test_clean_error_display() {
        let io_err = CleanError::Io(std::io::Error::other("disk full"));
        assert!(io_err.to_string().contains("IO error: disk full"));

        let git_err = CleanError::Git(crate::git::GitError::DetachedHead);
        assert!(git_err.to_string().contains("Git error:"));

        let ctx_err = CleanError::Context(crate::context::ContextError::Git(
            crate::git::GitError::DetachedHead,
        ));
        assert!(ctx_err.to_string().contains("Context error:"));

        let cfg_err =
            CleanError::Config(crate::config::ConfigError::NotFound(PathBuf::from("foo")));
        assert!(cfg_err.to_string().contains("Config error:"));

        let scan_err = CleanError::Scanner(crate::scanner::ScannerError::InvalidTestName {
            name: "bad".to_string(),
            reason: "invalid".to_string(),
        });
        assert!(scan_err.to_string().contains("Scanner error:"));

        let io_from_json: CleanError =
            crate::io::IoError::JsonParse(serde_json::from_str::<String>("bad json").unwrap_err())
                .into();
        assert!(matches!(io_from_json, CleanError::Io(_)));

        let io_from_io: CleanError =
            crate::io::IoError::Io(std::io::Error::other("disk crash")).into();
        assert!(matches!(io_from_io, CleanError::Io(_)));

        let res = CleanResult::default();
        let cloned_res = res.clone();
        assert_eq!(res, cloned_res);
        assert_eq!(format!("{res:?}"), format!("{cloned_res:?}"));
    }

    #[test]
    fn test_clean_workspace_bare_vs_scoped_patterns_and_duplicates() {
        let temp = tempdir().unwrap();
        let base_path = temp.path();

        let gleon_dir = base_path.join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include:
      - "*.png"
      - "scoped/**/*.png"
      - "**/already_globbed/*.png"
    mode: pixel
"#;
        std::fs::write(gleon_dir.join("gleon.yaml"), config_yaml).unwrap();

        // Write existing .gitignore already containing scoped/**/*.png
        std::fs::write(base_path.join(".gitignore"), "scoped/**/*.png\n").unwrap();

        let root_png = base_path.join("root.png");
        std::fs::write(&root_png, b"png").unwrap();

        let cli = Cli::for_test(Commands::Clean {
            dry_run: false,
            skip_gitignore: false,
            keep_runs: false,
        });
        let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

        let opts = CleanOptions::default();
        let res = clean_workspace(&ctx, base_path, &opts).unwrap();
        assert_eq!(res.deleted_files.len(), 1);

        let gitignore = std::fs::read_to_string(base_path.join(".gitignore")).unwrap();
        // Bare pattern "*.png" gets "**/" prepended
        assert!(gitignore.contains("**/*.png"));
        // Already globbed pattern stays as is
        assert!(gitignore.contains("**/already_globbed/*.png"));
        // Existing "scoped/**/*.png" was already in .gitignore and not duplicated
        assert_eq!(gitignore.matches("scoped/**/*.png").count(), 1);
    }
}
