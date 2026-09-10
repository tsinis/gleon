//! Shared directory-traversal and glob-set construction helpers built on `ignore`/`globset`.

use crate::config::GlobPattern;
use crate::naming::DEFAULT_PRUNED_DIRECTORIES;
use globset::{GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use std::path::Path;

/// Returns `true` if `name` is one of [`DEFAULT_PRUNED_DIRECTORIES`].
#[must_use]
pub fn is_default_pruned_dir(name: &str) -> bool {
    DEFAULT_PRUNED_DIRECTORIES.contains(&name)
}

/// Builds a `WalkBuilder` rooted at `dir` that prunes [`DEFAULT_PRUNED_DIRECTORIES`].
///
/// `.gleon` itself is exempted so callers walking a tree rooted inside `.gleon/` can still
/// descend into it. This is the walker shape shared by manifest and manifest-adjacent
/// traversal (workspace index loading, manifest linting, conflict scanning). It is
/// intentionally narrower than [`crate::scanner::FileScanner`]'s walker, which also prunes
/// `.gleon` itself and layers in glob-based exclude matching.
#[must_use]
pub fn pruned_walker(dir: &Path) -> WalkBuilder {
    let mut builder = WalkBuilder::new(dir);
    builder.standard_filters(false).filter_entry(|entry| {
        if entry.file_type().is_some_and(|ft| ft.is_dir())
            && matches!(entry.file_name().to_str(), Some(name) if name != ".gleon" && is_default_pruned_dir(name))
        {
            return false;
        }
        true
    });
    builder
}

/// Compiles a `GlobSet` from a list of patterns.
///
/// # Errors
/// Returns the underlying `globset::Error` if any pattern fails to compile (this should not
/// happen for patterns already validated by [`GlobPattern`]).
pub fn build_globset(patterns: &[GlobPattern]) -> Result<GlobSet, globset::Error> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(pattern.as_glob().clone());
    }
    builder.build()
}

#[cfg(all(test, not(miri)))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::pedantic,
    clippy::nursery
)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_is_default_pruned_dir() {
        assert!(is_default_pruned_dir("node_modules"));
        assert!(is_default_pruned_dir(".git"));
        assert!(!is_default_pruned_dir("src"));
    }

    #[test]
    fn test_pruned_walker_skips_pruned_dirs_but_walks_into_gleon() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        std::fs::write(root.join("node_modules").join("dead.txt"), "x").unwrap();
        std::fs::create_dir_all(root.join(".gleon").join("manifests")).unwrap();
        std::fs::write(root.join(".gleon").join("manifests").join("live.txt"), "x").unwrap();

        let names: Vec<_> = pruned_walker(root)
            .build()
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .collect();

        assert!(!names.contains(&"dead.txt".to_string()));
        assert!(names.contains(&"live.txt".to_string()));
    }

    #[test]
    fn test_build_globset_matches_patterns() {
        let patterns = vec![GlobPattern::new("**/*.png").unwrap()];
        let set = build_globset(&patterns).unwrap();
        assert!(set.is_match("billing/stripe.png"));
        assert!(!set.is_match("billing/stripe.jpg"));
    }
}
