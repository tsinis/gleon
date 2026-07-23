//! Git branch resolution and gitignore validation.

use std::path::Path;

/// Errors that can occur during Git operations.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// Failed to discover or open Git repository
    #[error("Failed to discover Git repository: {0}")]
    Discover(String),

    /// Failed to read Git HEAD reference
    #[error("Failed to read Git HEAD reference: {0}")]
    HeadRead(String),

    /// Failed to build gitignore matcher
    #[error("Failed to build gitignore matcher: {0}")]
    IgnoreBuild(String),

    /// HEAD is detached and no CI environment branch variables are set
    #[error("HEAD is detached and no CI environment branch variables are set")]
    DetachedHead,

    /// Resolved branch name contains invalid characters or is empty
    #[error("Resolved branch name contains invalid characters: '{0}'")]
    InvalidBranchName(String),

    /// Repository is a shallow clone lacking history to compute merge-base
    #[error("Repository is a shallow clone (fetch-depth constraint): {0}")]
    ShallowClone(String),

    /// Merge base calculation failed
    #[error("Merge base calculation failed: {0}")]
    MergeBaseFailed(String),

    /// Path is outside the Git repository
    #[error("Path is outside the Git repository: {0}")]
    OutsideRepository(std::path::PathBuf),

    /// Commit not found or could not be resolved
    #[error("Commit lookup failed for '{ref_or_sha}': {reason}")]
    CommitLookup {
        /// The ref or SHA that was looked up.
        ref_or_sha: String,
        /// The underlying error reason.
        reason: String,
    },

    /// Commit object could not be decoded
    #[error("Failed to decode commit '{commit_sha}': {reason}")]
    CommitDecode {
        /// The commit SHA that failed to decode.
        commit_sha: String,
        /// The underlying error reason.
        reason: String,
    },

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Helper trait for mocking environment variables in tests.
pub trait EnvProvider {
    /// Gets the environment variable value.
    fn get_var(&self, key: &str) -> Option<String>;
}

/// Standard OS environment variable provider.
pub struct OsEnv;

impl EnvProvider for OsEnv {
    fn get_var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// Resolver for Git branch context.
pub struct GitResolver;

impl GitResolver {
    /// Resolves the current branch name.
    ///
    /// Precedence:
    /// 1. `GLEON_BRANCH` environment variable
    /// 2. CI environment variables
    /// 3. Discovering git and reading HEAD
    ///
    /// Returns `GitError::Discover` if no Git repository is found (callers like `ResolvedContext` handle offline fallback).
    pub fn resolve_branch() -> Result<String, GitError> {
        let env = OsEnv;
        let current_dir = std::env::current_dir().map_err(GitError::Io)?;
        Self::resolve_branch_impl(None, &current_dir, &env)
    }

    /// Internal implementation of branch resolution allowing dependency injection.
    pub fn resolve_branch_impl(
        cli_branch: Option<&str>,
        base_dir: &Path,
        env: &dyn EnvProvider,
    ) -> Result<String, GitError> {
        // 1. CLI branch override
        if let Some(branch) = cli_branch {
            let cleaned = clean_branch_name(branch);
            return validate_branch_name(&cleaned).map(|_| cleaned);
        }

        // 2. GLEON_BRANCH env var
        if let Some(branch) = env
            .get_var("GLEON_BRANCH")
            .map(|b| clean_branch_name(&b))
            .filter(|c| !c.is_empty())
        {
            return validate_branch_name(&branch).map(|_| branch);
        }

        // 3. CI env variables (provider specific)
        if let Some(branch) = resolve_ci_branch(env) {
            let cleaned = clean_branch_name(&branch);
            return validate_branch_name(&cleaned).map(|_| cleaned);
        }

        // 4. Git discovery
        match gix::discover(base_dir) {
            Ok(repo) => {
                match repo.head_name() {
                    Ok(Some(head_name)) => {
                        let branch = head_name.shorten().to_string();
                        let cleaned = clean_branch_name(&branch);
                        validate_branch_name(&cleaned).map(|_| cleaned)
                    }
                    Ok(None) => {
                        // Detached HEAD and no env overrides
                        Err(GitError::DetachedHead)
                    }
                    Err(e) => Err(GitError::HeadRead(e.to_string())),
                }
            }
            Err(e) => Err(GitError::Discover(format!(
                "Not a git repository (or no git installed). Please run inside a git repository, or provide the branch manually using the --branch CLI flag or the GLEON_BRANCH environment variable. Underlying error: {}",
                e
            ))),
        }
    }

    /// Uses the `ignore` crate to verify if screenshot paths are matched by .gitignore rules.
    /// Returns true if all provided paths are correctly ignored.
    /// Returns `GitError::Discover` if no Git repository is found.
    pub fn verify_ignored<P: AsRef<Path>>(paths: &[P]) -> Result<bool, GitError> {
        let current_dir = std::env::current_dir().map_err(GitError::Io)?;
        Self::verify_ignored_impl(paths, &current_dir)
    }

    /// Internal implementation of verify_ignored allowing dependency injection of search path.
    pub fn verify_ignored_impl<P: AsRef<Path>>(
        paths: &[P],
        base_dir: &Path,
    ) -> Result<bool, GitError> {
        let repo = match gix::discover(base_dir) {
            Ok(repo) => repo,
            Err(e) => {
                return Err(GitError::Discover(format!(
                    "Not a git repository (or no git installed). verify_ignored requires a git repository. Underlying error: {}",
                    e
                )));
            }
        };

        let repo_root = repo.workdir().ok_or_else(|| {
            GitError::Discover("Bare repository has no working directory".to_string())
        })?;
        let repo_root = normalize_path(repo_root)?;

        // Pre-process paths into absolute and relative counterparts, resolving path traversal
        let mut processed_paths = Vec::with_capacity(paths.len());
        for path in paths {
            let path_ref = path.as_ref();
            let abs_path = if path_ref.is_absolute() {
                path_ref.to_path_buf()
            } else {
                base_dir.join(path_ref)
            };
            let abs_path = normalize_path(&abs_path)?;

            // Check if the path is actually inside the repository
            if !abs_path.starts_with(&repo_root) {
                return Err(GitError::OutsideRepository(abs_path));
            }

            let rel_path = abs_path.strip_prefix(&repo_root).unwrap().to_path_buf();
            processed_paths.push((abs_path, rel_path));
        }

        let mut builder = ignore::gitignore::GitignoreBuilder::new(&repo_root);

        // Add .git/info/exclude
        let exclude_path = repo.git_dir().join("info/exclude");
        if let Some(err) = builder.add(&exclude_path) {
            tracing::debug!("Failed to add .git/info/exclude: {}", err);
        }

        let mut gitignores_to_add = std::collections::HashSet::new();
        let mut visited_dirs = std::collections::HashSet::new();

        for (abs_path, _) in &processed_paths {
            // Traverse up to repo_root to discover all .gitignore files in the hierarchy
            let mut current = abs_path.clone();
            while current.pop() {
                let dir = &current;
                if !dir.starts_with(&repo_root) {
                    break;
                }
                if visited_dirs.contains(dir) {
                    // Already visited this directory and its parents!
                    break;
                }
                visited_dirs.insert(dir.to_path_buf());

                let gitignore = dir.join(".gitignore");
                gitignores_to_add.insert(gitignore);
                if dir == &repo_root {
                    break;
                }
            }
        }

        // To match Git semantics, deeper .gitignore files must override shallower ones.
        // The ignore crate applies later-added rules with higher precedence.
        // Therefore, we sort the discovered files by depth (number of components)
        // so that the root .gitignore is added first, and deeper files are added later.
        let mut sorted_gitignores: Vec<_> = gitignores_to_add.into_iter().collect();
        sorted_gitignores.sort_by(|a, b| {
            a.components()
                .count()
                .cmp(&b.components().count())
                .then_with(|| a.cmp(b))
        });

        for gitignore in sorted_gitignores {
            if let Some(err) = builder.add(&gitignore) {
                tracing::debug!("Failed to add ignore file {:?}: {}", gitignore, err);
            }
        }

        let matcher = builder
            .build()
            .map_err(|e| GitError::IgnoreBuild(e.to_string()))?;

        for (abs_path, rel_path) in &processed_paths {
            // If the file/dir doesn't exist yet, we check if it matches ignore rules based on name.
            // Under ignore crate, matched path checks can be run with is_dir flag.
            let is_dir = abs_path.is_dir();
            let matched = matcher.matched_path_or_any_parents(rel_path, is_dir);
            if !matched.is_ignore() {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Computes the SHA-256 hash of the raw UTF-8 branch name for safe flat-key storage.
    pub fn branch_path_token(branch_name: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(branch_name.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Resolves the merge-base between HEAD and the given target branch.
    /// Detects shallow clones and returns a specific error for fallback logging.
    pub fn resolve_merge_base(base_dir: &Path, target_branch: &str) -> Result<String, GitError> {
        let repo = match gix::discover(base_dir) {
            Ok(repo) => repo,
            Err(e) => return Err(GitError::Discover(e.to_string())),
        };

        // Check if the repository is a shallow clone by checking for the existence of .git/shallow
        if repo.shallow_file().exists() {
            return Err(GitError::ShallowClone(
                "Shallow clone detected (.git/shallow exists)".to_string(),
            ));
        }

        let head_commit = match repo.head_commit() {
            Ok(commit) => commit,
            Err(e) => return Err(GitError::HeadRead(e.to_string())),
        };
        let head_id = head_commit.id;

        let target_id = match repo.rev_parse_single(target_branch) {
            Ok(id) => id,
            Err(e) => {
                return Err(GitError::MergeBaseFailed(format!(
                    "Failed to resolve target branch '{}': {}",
                    target_branch, e
                )));
            }
        };

        match repo.merge_base(head_id, target_id) {
            Ok(base_id) => Ok(base_id.to_string()),
            Err(e) => Err(GitError::MergeBaseFailed(e.to_string())),
        }
    }

    /// Gets the author name and email of the given commit, defaulting to "unknown".
    pub fn get_commit_author(base_dir: &Path, commit_sha: &str) -> Result<String, GitError> {
        let repo = match gix::discover(base_dir) {
            Ok(repo) => repo,
            Err(e) => return Err(GitError::Discover(e.to_string())),
        };

        let id = match gix::ObjectId::from_hex(commit_sha.as_bytes())
            .or_else(|_| repo.rev_parse_single(commit_sha).map(|id| id.detach()))
        {
            Ok(id) => id,
            Err(e) => {
                return Err(GitError::CommitLookup {
                    ref_or_sha: commit_sha.to_string(),
                    reason: e.to_string(),
                });
            }
        };

        let commit = match repo.find_commit(id) {
            Ok(commit) => commit,
            Err(e) => {
                return Err(GitError::CommitLookup {
                    ref_or_sha: id.to_string(),
                    reason: e.to_string(),
                });
            }
        };

        let decoded = match commit.decode() {
            Ok(decoded) => decoded,
            Err(e) => {
                return Err(GitError::CommitDecode {
                    commit_sha: id.to_string(),
                    reason: e.to_string(),
                });
            }
        };

        if let Ok(sig) = gix::actor::SignatureRef::from_bytes(decoded.author.as_ref()) {
            let actor = sig.actor();
            let name = actor.name.to_string();
            let email = actor.email.to_string();
            if name.is_empty() && email.is_empty() {
                tracing::debug!(
                    "Commit author name and email are empty for commit '{}'",
                    commit_sha
                );
                Ok("unknown".to_string())
            } else {
                Ok(format!("{} <{}>", name, email))
            }
        } else {
            tracing::debug!(
                "Failed to parse author signature bytes for commit '{}'",
                commit_sha
            );
            Ok("unknown".to_string())
        }
    }

    /// Gets the repository's full hexadecimal object ID of HEAD (supporting both SHA-1 and SHA-256 widths).
    pub fn get_head_commit_sha(base_dir: &Path) -> Result<String, GitError> {
        let repo = match gix::discover(base_dir) {
            Ok(repo) => repo,
            Err(e) => return Err(GitError::Discover(e.to_string())),
        };

        let head_commit = match repo.head_commit() {
            Ok(commit) => commit,
            Err(e) => return Err(GitError::HeadRead(e.to_string())),
        };

        Ok(head_commit.id.to_string())
    }
}

fn normalize_path(path: &Path) -> Result<std::path::PathBuf, GitError> {
    use std::path::Component;
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(GitError::OutsideRepository(path.to_path_buf()));
                }
            }
            Component::Normal(c) => {
                normalized.push(c);
            }
            Component::CurDir => continue,
            other => {
                normalized.push(other.as_os_str());
            }
        }
    }
    Ok(normalized)
}

fn clean_branch_name(name: &str) -> String {
    let mut cleaned = name.trim();
    if let Some(stripped) = cleaned.strip_prefix("refs/heads/") {
        cleaned = stripped;
    }
    cleaned.trim().to_string()
}

fn validate_branch_name(name: &str) -> Result<(), GitError> {
    if name.is_empty() {
        return Err(GitError::InvalidBranchName(
            "Branch name cannot be empty".to_string(),
        ));
    }
    if gix::refs::PartialName::try_from(name).is_err() {
        return Err(GitError::InvalidBranchName(name.to_string()));
    }
    Ok(())
}

fn is_env_truthy(val: Option<String>) -> bool {
    val.as_deref()
        .is_some_and(|v| v.eq_ignore_ascii_case("true") || v == "1")
}

fn resolve_ci_branch(env: &dyn EnvProvider) -> Option<String> {
    let get_valid = |k: &str| {
        env.get_var(k)
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
    };

    // 1. GitHub Actions
    if is_env_truthy(env.get_var("GITHUB_ACTIONS"))
        && let Some(b) = get_valid("GITHUB_HEAD_REF").or_else(|| get_valid("GITHUB_REF_NAME"))
    {
        return Some(b);
    }

    // 2. GitLab CI
    if env.get_var("GITLAB_CI").is_some()
        && let Some(b) = get_valid("CI_MERGE_REQUEST_SOURCE_BRANCH_NAME")
            .or_else(|| get_valid("CI_COMMIT_BRANCH"))
            .or_else(|| get_valid("CI_COMMIT_REF_NAME"))
    {
        return Some(b);
    }

    // 3. CircleCI
    if is_env_truthy(env.get_var("CIRCLECI"))
        && let Some(b) = get_valid("CIRCLE_BRANCH")
    {
        return Some(b);
    }

    // 4. Bitbucket Pipelines
    if env.get_var("BITBUCKET_COMMIT").is_some()
        && let Some(b) = get_valid("BITBUCKET_BRANCH")
    {
        return Some(b);
    }

    // 5. Azure DevOps
    if is_env_truthy(env.get_var("TF_BUILD"))
        && let Some(b) = get_valid("BUILD_SOURCEBRANCHNAME")
    {
        return Some(b);
    }

    // 6. Travis CI
    if is_env_truthy(env.get_var("TRAVIS"))
        && let Some(b) = get_valid("TRAVIS_BRANCH")
    {
        return Some(b);
    }

    // 7. Codemagic
    if env.get_var("CM_BUILD_ID").is_some()
        && let Some(b) = get_valid("CM_BRANCH")
    {
        return Some(b);
    }

    // 8. Bitrise
    if is_env_truthy(env.get_var("BITRISE_IO"))
        && let Some(b) = get_valid("BITRISE_GIT_BRANCH")
    {
        return Some(b);
    }

    // 9. Generic CI Fallback list
    if env.get_var("CI").is_some() {
        let ci_vars = [
            "GITHUB_HEAD_REF",
            "GITHUB_REF_NAME",
            "CI_COMMIT_BRANCH",
            "CI_COMMIT_REF_NAME",
            "CIRCLE_BRANCH",
            "BITBUCKET_BRANCH",
            "BUILD_SOURCEBRANCHNAME",
            "TRAVIS_BRANCH",
            "CM_BRANCH",
            "BITRISE_GIT_BRANCH",
        ];
        for var in ci_vars {
            if let Some(b) = get_valid(var) {
                return Some(b);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    struct MockEnv {
        vars: HashMap<String, String>,
    }

    impl EnvProvider for MockEnv {
        fn get_var(&self, key: &str) -> Option<String> {
            self.vars.get(key).cloned()
        }
    }

    fn create_mock_git_repo(path: &Path, head_content: &str) {
        let git_dir = path.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::create_dir_all(git_dir.join("objects")).unwrap();
        std::fs::create_dir_all(git_dir.join("refs")).unwrap();
        std::fs::write(git_dir.join("HEAD"), head_content).unwrap();
    }

    #[test]
    fn test_resolve_branch_cli_override() {
        let dir = tempdir().unwrap();
        let env = MockEnv {
            vars: HashMap::new(),
        };
        let result =
            GitResolver::resolve_branch_impl(Some("refs/heads/cli-branch\r\n"), dir.path(), &env);
        assert_eq!(result.unwrap(), "cli-branch");
    }

    #[test]
    fn test_resolve_branch_env_override() {
        let dir = tempdir().unwrap();
        let mut vars = HashMap::new();
        vars.insert(
            "GLEON_BRANCH".to_string(),
            "refs/heads/env-branch\r\n".to_string(),
        );
        let env = MockEnv { vars };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert_eq!(result.unwrap(), "env-branch");
    }

    #[test]
    fn test_resolve_branch_github_actions_pr() {
        let dir = tempdir().unwrap();
        let mut vars = HashMap::new();
        vars.insert("GITHUB_ACTIONS".to_string(), "true".to_string());
        vars.insert("GITHUB_HEAD_REF".to_string(), "feature-pr".to_string());
        vars.insert("GITHUB_REF_NAME".to_string(), "main".to_string());
        let env = MockEnv { vars };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert_eq!(result.unwrap(), "feature-pr");
    }

    #[test]
    fn test_resolve_branch_github_actions_push() {
        let dir = tempdir().unwrap();
        let mut vars = HashMap::new();
        vars.insert("GITHUB_ACTIONS".to_string(), "true".to_string());
        vars.insert("GITHUB_REF_NAME".to_string(), "feature-push".to_string());
        let env = MockEnv { vars };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert_eq!(result.unwrap(), "feature-push");
    }

    #[test]
    fn test_resolve_branch_gitlab_ci_mr() {
        let dir = tempdir().unwrap();
        let mut vars = HashMap::new();
        vars.insert("GITLAB_CI".to_string(), "true".to_string());
        vars.insert(
            "CI_MERGE_REQUEST_SOURCE_BRANCH_NAME".to_string(),
            "gitlab-mr".to_string(),
        );
        vars.insert("CI_COMMIT_BRANCH".to_string(), "main".to_string());
        let env = MockEnv { vars };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert_eq!(result.unwrap(), "gitlab-mr");
    }

    #[test]
    fn test_resolve_branch_gitlab_ci_branch() {
        let dir = tempdir().unwrap();
        let mut vars = HashMap::new();
        vars.insert("GITLAB_CI".to_string(), "true".to_string());
        vars.insert("CI_COMMIT_BRANCH".to_string(), "gitlab-branch".to_string());
        let env = MockEnv { vars };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert_eq!(result.unwrap(), "gitlab-branch");
    }

    #[test]
    fn test_resolve_branch_gix_success() {
        let dir = tempdir().unwrap();
        create_mock_git_repo(dir.path(), "ref: refs/heads/gix-branch\r\n");
        let env = MockEnv {
            vars: HashMap::new(),
        };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert_eq!(result.unwrap(), "gix-branch");
    }

    #[test]
    fn test_resolve_branch_gix_detached_head() {
        let dir = tempdir().unwrap();
        create_mock_git_repo(dir.path(), "0123456789abcdef0123456789abcdef01234567\n");
        let env = MockEnv {
            vars: HashMap::new(),
        };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert!(matches!(result, Err(GitError::DetachedHead)));
    }

    #[test]
    fn test_resolve_branch_no_git_fallback() {
        let dir = tempdir().unwrap();
        let env = MockEnv {
            vars: HashMap::new(),
        };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert!(matches!(result, Err(GitError::Discover(_))));
    }

    #[test]
    fn test_resolve_branch_generic_ci_fallback() {
        let dir = tempdir().unwrap();
        let mut vars = HashMap::new();
        vars.insert("CI".to_string(), "true".to_string());
        vars.insert(
            "GITHUB_REF_NAME".to_string(),
            "generic-ci-branch".to_string(),
        );
        let env = MockEnv { vars };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert_eq!(result.unwrap(), "generic-ci-branch");
    }

    #[test]
    fn test_resolve_branch_generic_ci_no_ci_env() {
        let dir = tempdir().unwrap();
        let mut vars = HashMap::new();
        vars.insert(
            "GITHUB_REF_NAME".to_string(),
            "generic-ci-branch".to_string(),
        );
        let env = MockEnv { vars };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert!(matches!(result, Err(GitError::Discover(_))));
    }

    #[test]
    fn test_validation_invalid_characters() {
        let dir = tempdir().unwrap();
        let env = MockEnv {
            vars: HashMap::new(),
        };
        let result =
            GitResolver::resolve_branch_impl(Some("feature space branch"), dir.path(), &env);
        assert!(matches!(result, Err(GitError::InvalidBranchName(_))));
    }

    #[test]
    fn test_git_ref_validation_cases() {
        // feature/xxx should be valid
        assert!(validate_branch_name("feature/über").is_ok());
        // foo..bar should be invalid (double dot is not allowed in git refs)
        assert!(validate_branch_name("foo..bar").is_err());
        // .hidden should be invalid (starts with dot)
        assert!(validate_branch_name(".hidden").is_err());
        // foo/ should be invalid (ends with slash)
        assert!(validate_branch_name("foo/").is_err());
    }

    #[test]
    fn test_verify_ignored_success() {
        let dir = tempdir().unwrap();
        create_mock_git_repo(dir.path(), "ref: refs/heads/main\n");

        let gitignore_path = dir.path().join(".gitignore");
        let mut file = File::create(&gitignore_path).unwrap();
        writeln!(file, "**/goldens/**/*.png").unwrap();
        writeln!(file, "ignored_file.txt").unwrap();

        let paths = vec![
            dir.path().join("ignored_file.txt"),
            dir.path().join("src/goldens/test.png"),
        ];

        let result = GitResolver::verify_ignored_impl(&paths, dir.path()).unwrap();
        assert!(result);

        let non_ignored = vec![dir.path().join("src/main.rs")];
        let result_non_ignored =
            GitResolver::verify_ignored_impl(&non_ignored, dir.path()).unwrap();
        assert!(!result_non_ignored);
    }

    #[test]
    fn test_verify_ignored_fallback() {
        let dir = tempdir().unwrap();
        let paths = vec![dir.path().join("ignored_file.txt")];
        let result = GitResolver::verify_ignored_impl(&paths, dir.path());
        assert!(matches!(result, Err(GitError::Discover(_))));
    }

    #[test]
    fn test_verify_ignored_precedence() {
        let dir = tempdir().unwrap();
        create_mock_git_repo(dir.path(), "ref: refs/heads/main\n");

        let sub_dir = dir.path().join("sub");
        std::fs::create_dir(&sub_dir).unwrap();

        // Root gitignore: ignore all .png
        let mut root_ignore = File::create(dir.path().join(".gitignore")).unwrap();
        writeln!(root_ignore, "*.png").unwrap();

        // Sub gitignore: un-ignore specific.png
        let mut sub_ignore = File::create(sub_dir.join(".gitignore")).unwrap();
        writeln!(sub_ignore, "!specific.png").unwrap();

        let paths = vec![
            dir.path().join("normal.png"),
            sub_dir.join("normal.png"),
            sub_dir.join("specific.png"),
        ];

        let result = GitResolver::verify_ignored_impl(&paths, dir.path()).unwrap();
        // Result should be false because specific.png is NOT ignored.
        assert!(!result);

        // Verify individually
        let paths1 = vec![dir.path().join("normal.png")];
        assert!(GitResolver::verify_ignored_impl(&paths1, dir.path()).unwrap());

        let paths2 = vec![sub_dir.join("normal.png")];
        assert!(GitResolver::verify_ignored_impl(&paths2, dir.path()).unwrap());

        let paths3 = vec![sub_dir.join("specific.png")];
        assert!(!GitResolver::verify_ignored_impl(&paths3, dir.path()).unwrap());
    }

    #[test]
    fn test_verify_ignored_relative_base_dir() {
        let dir = tempdir().unwrap();
        create_mock_git_repo(dir.path(), "ref: refs/heads/main\n");

        let sub_dir = dir.path().join("sub");
        std::fs::create_dir(&sub_dir).unwrap();

        let mut ignore_file = File::create(dir.path().join(".gitignore")).unwrap();
        writeln!(ignore_file, "sub/ignored.png").unwrap();

        // Pass a relative path "ignored.png" and base_dir "sub".
        // It should resolve to "sub/ignored.png" and be ignored.
        let paths = vec![std::path::PathBuf::from("ignored.png")];
        let result = GitResolver::verify_ignored_impl(&paths, &sub_dir).unwrap();
        assert!(result);

        // Relative path "ignored.png" from root base_dir should NOT be ignored
        // because the rule specifies "sub/ignored.png", not "ignored.png".
        let result2 = GitResolver::verify_ignored_impl(&paths, dir.path()).unwrap();
        assert!(!result2);
    }

    #[test]
    fn test_public_wrappers() {
        let branch = GitResolver::resolve_branch();
        assert!(branch.is_ok());

        let ignored = GitResolver::verify_ignored(&["target/debug/test_dummy_file.png"]);
        assert!(ignored.is_ok());
    }

    #[test]
    fn test_is_env_truthy() {
        assert!(is_env_truthy(Some("true".to_string())));
        assert!(is_env_truthy(Some("True".to_string())));
        assert!(is_env_truthy(Some("TRUE".to_string())));
        assert!(is_env_truthy(Some("1".to_string())));
        assert!(!is_env_truthy(Some("false".to_string())));
        assert!(!is_env_truthy(Some("0".to_string())));
        assert!(!is_env_truthy(None));
    }

    #[test]
    fn test_get_head_commit_sha_no_git() {
        let dir = tempdir().unwrap();
        let result = GitResolver::get_head_commit_sha(dir.path());
        assert!(matches!(result, Err(GitError::Discover(_))));
    }

    #[test]
    #[cfg(not(miri))]
    fn test_get_head_commit_sha_real_repo() {
        let dir = tempdir().expect("tempdir creation should succeed");
        gix::init(dir.path()).expect("gix init should succeed");
        std::fs::write(
            dir.path().join(".git/config"),
            "[user]\n\tname = Gleon Test\n\temail = test@gleon.dev\n",
        )
        .expect("write .git/config should succeed");
        let repo = gix::open(dir.path()).expect("gix open should succeed");
        let empty_tree_id = repo.empty_tree().id();
        let commit_id = repo
            .commit(
                "HEAD",
                "Initial commit",
                empty_tree_id,
                gix::commit::NO_PARENT_IDS,
            )
            .expect("commit should succeed");

        let sha = GitResolver::get_head_commit_sha(dir.path())
            .expect("get_head_commit_sha should succeed");
        assert_eq!(sha, commit_id.to_string());
    }

    #[test]
    #[cfg(not(miri))]
    fn test_verify_ignored_bare_repo() {
        let dir = tempdir().unwrap();
        gix::init_bare(dir.path()).unwrap();
        let paths = vec![dir.path().join("file.png")];
        let result = GitResolver::verify_ignored_impl(&paths, dir.path());
        assert!(matches!(
            result,
            Err(GitError::Discover(ref msg)) if msg.contains("Bare repository")
        ));
    }

    #[test]
    fn test_resolve_branch_ci_providers() {
        let dir = tempdir().unwrap();

        // CircleCI
        let mut vars = HashMap::new();
        vars.insert("CIRCLECI".to_string(), "true".to_string());
        vars.insert("CIRCLE_BRANCH".to_string(), "circle-branch".to_string());
        let env = MockEnv { vars };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert_eq!(result.unwrap(), "circle-branch");

        // Bitbucket
        let mut vars = HashMap::new();
        vars.insert("BITBUCKET_COMMIT".to_string(), "123".to_string());
        vars.insert(
            "BITBUCKET_BRANCH".to_string(),
            "bitbucket-branch".to_string(),
        );
        let env = MockEnv { vars };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert_eq!(result.unwrap(), "bitbucket-branch");

        // Azure DevOps
        let mut vars = HashMap::new();
        vars.insert("TF_BUILD".to_string(), "True".to_string());
        vars.insert(
            "BUILD_SOURCEBRANCHNAME".to_string(),
            "azure-branch".to_string(),
        );
        let env = MockEnv { vars };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert_eq!(result.unwrap(), "azure-branch");

        // Travis
        let mut vars = HashMap::new();
        vars.insert("TRAVIS".to_string(), "true".to_string());
        vars.insert("TRAVIS_BRANCH".to_string(), "travis-branch".to_string());
        let env = MockEnv { vars };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert_eq!(result.unwrap(), "travis-branch");

        // Codemagic
        let mut vars = HashMap::new();
        vars.insert("CM_BUILD_ID".to_string(), "123".to_string());
        vars.insert("CM_BRANCH".to_string(), "codemagic-branch".to_string());
        let env = MockEnv { vars };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert_eq!(result.unwrap(), "codemagic-branch");

        // Bitrise
        let mut vars = HashMap::new();
        vars.insert("BITRISE_IO".to_string(), "true".to_string());
        vars.insert(
            "BITRISE_GIT_BRANCH".to_string(),
            "bitrise-branch".to_string(),
        );
        let env = MockEnv { vars };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert_eq!(result.unwrap(), "bitrise-branch");
    }

    #[test]
    fn test_validation_empty_branch() {
        assert!(validate_branch_name("").is_err());
    }

    #[test]
    fn test_clean_branch_name_helper() {
        assert_eq!(
            clean_branch_name("  refs/heads/feature/branch  "),
            "feature/branch"
        );
        assert_eq!(clean_branch_name("  feature-branch  "), "feature-branch");
    }

    #[test]
    fn test_resolve_branch_gix_corrupt_head() {
        let dir = tempdir().unwrap();
        // Create a git repo where HEAD contains invalid data to cause HeadRead error
        create_mock_git_repo(dir.path(), "invalid_data_no_ref_format");
        let env = MockEnv {
            vars: HashMap::new(),
        };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_ignored_invalid_ignore_file() {
        let dir = tempdir().unwrap();
        create_mock_git_repo(dir.path(), "ref: refs/heads/main\n");

        let gitignore_path = dir.path().join(".gitignore");
        let mut file = File::create(&gitignore_path).unwrap();
        // A double-bracket without a closing one or a consecutive wildcard like ***
        // will cause a glob compile/parse error in the builder.
        writeln!(file, "*[").unwrap();

        let paths = vec![dir.path().join("file.png")];
        // verify_ignored_impl returns false if the gitignore file fails parsing but other paths are checked
        let result = GitResolver::verify_ignored_impl(&paths, dir.path()).unwrap();
        assert!(!result);
    }

    #[test]
    fn test_resolve_branch_invalid_env_override() {
        let dir = tempdir().unwrap();
        let mut vars = HashMap::new();
        vars.insert("GLEON_BRANCH".to_string(), "invalid name space".to_string());
        let env = MockEnv { vars };
        let result = GitResolver::resolve_branch_impl(None, dir.path(), &env);
        assert!(matches!(result, Err(GitError::InvalidBranchName(_))));
    }

    #[test]
    fn test_normalize_path_cur_dir() {
        let path = Path::new("./file.png");
        let norm = normalize_path(path).unwrap();
        assert_eq!(norm, Path::new("file.png"));
    }

    #[test]
    fn test_branch_path_token() {
        let token1 = GitResolver::branch_path_token("release");
        let token2 = GitResolver::branch_path_token("release/1.0");
        assert_ne!(token1, token2);
        assert_eq!(token1.len(), 64);
        assert_eq!(token2.len(), 64);
    }

    #[test]
    fn test_get_commit_author_errors_propagated() {
        let dir = tempdir().unwrap();
        let result =
            GitResolver::get_commit_author(dir.path(), "0000000000000000000000000000000000000000");
        assert!(matches!(result, Err(GitError::Discover(_))));
    }

    #[test]
    #[cfg(not(miri))]
    fn test_get_commit_author_gix() {
        let dir = tempdir().unwrap();
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["init"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["config", "user.name", "gleon Author"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["config", "user.email", "author@gleon.dev"])
            .output()
            .unwrap();

        std::fs::write(dir.path().join("dummy.txt"), "hello").unwrap();
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["add", "."])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["commit", "-m", "initial commit"])
            .output()
            .unwrap();

        let repo = gix::discover(dir.path()).unwrap();
        let head_commit = repo.head_commit().unwrap();
        let sha = head_commit.id.to_string();

        let author = GitResolver::get_commit_author(dir.path(), &sha).unwrap();
        assert_eq!(author, "gleon Author <author@gleon.dev>");
    }

    #[test]
    #[cfg(not(miri))]
    fn test_resolve_merge_base_shallow_clone_error() {
        let dir = tempdir().unwrap();
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["init"])
            .output()
            .unwrap();

        // Write .git/shallow to simulate a shallow repository
        let shallow_path = dir.path().join(".git/shallow");
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(&shallow_path, "0000000000000000000000000000000000000000\n").unwrap();

        let result = GitResolver::resolve_merge_base(dir.path(), "main");
        assert!(matches!(result, Err(GitError::ShallowClone(_))));
    }

    #[test]
    #[cfg(all(unix, not(miri)))]
    fn test_verify_ignored_unreadable_gitignore() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        create_mock_git_repo(dir.path(), "ref: refs/heads/main\n");
        let gitignore_path = dir.path().join(".gitignore");
        std::fs::write(&gitignore_path, "*.png").unwrap();
        std::fs::set_permissions(&gitignore_path, std::fs::Permissions::from_mode(0o000)).unwrap();

        // If we are root, writing to 0o000 file will succeed.
        let is_root = std::fs::write(&gitignore_path, "probe").is_ok();
        if is_root {
            return;
        }

        let paths = vec![dir.path().join("file.png")];
        let result = GitResolver::verify_ignored_impl(&paths, dir.path()).unwrap();
        assert!(!result);
    }

    #[test]
    #[cfg(all(unix, not(miri)))]
    fn test_verify_ignored_unreadable_info_exclude() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        create_mock_git_repo(dir.path(), "ref: refs/heads/main\n");
        let exclude_path = dir.path().join(".git/info/exclude");
        std::fs::create_dir_all(exclude_path.parent().unwrap()).unwrap();
        std::fs::write(&exclude_path, "*.png").unwrap();
        std::fs::set_permissions(&exclude_path, std::fs::Permissions::from_mode(0o000)).unwrap();

        // If we are root, writing to 0o000 file will succeed.
        let is_root = std::fs::write(&exclude_path, "probe").is_ok();
        if is_root {
            return;
        }

        let paths = vec![dir.path().join("file.png")];
        // The unreadable exclude file causes builder.add() to return Some(ignore::Error),
        // which the implementation catches and logs via tracing::debug!.
        // This failure does NOT cause a panic. The test should succeed and return false.
        let result = GitResolver::verify_ignored_impl(&paths, dir.path()).unwrap();
        assert!(!result);
    }

    #[test]
    #[cfg(not(miri))]
    fn test_resolve_merge_base_invalid_target_branch() {
        let dir = tempdir().unwrap();
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["init"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["config", "user.name", "Test"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["config", "user.email", "test@test.com"])
            .output()
            .unwrap();

        std::fs::write(dir.path().join("dummy.txt"), "hello").unwrap();
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["add", "."])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["commit", "-m", "initial commit"])
            .output()
            .unwrap();

        let result = GitResolver::resolve_merge_base(dir.path(), "non-existent");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GitError::MergeBaseFailed(_)));
    }

    #[test]
    #[cfg(not(miri))]
    fn test_get_commit_author_invalid_ref() {
        let dir = tempdir().unwrap();
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["init"])
            .output()
            .unwrap();

        let result = GitResolver::get_commit_author(dir.path(), "invalid-ref");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, GitError::CommitLookup { .. }));
    }

    #[test]
    #[cfg(not(miri))]
    fn test_get_commit_author_empty_signature() {
        let dir = tempdir().unwrap();
        std::process::Command::new("git")
            .current_dir(dir.path())
            .args(["init"])
            .output()
            .unwrap();

        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(dir.path())
            .args(["hash-object", "-t", "commit", "-w", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());

        let child = cmd.spawn().unwrap();
        use std::io::Write;
        {
            let mut stdin = child.stdin.as_ref().unwrap();
            writeln!(stdin, "tree 4b825dc642cb6eb9a0ff3e4897c85c126437f451").unwrap();
            writeln!(stdin, "author  <> 0 +0000").unwrap();
            writeln!(stdin, "committer Test <test@test.com> 0 +0000").unwrap();
            writeln!(stdin).unwrap();
            writeln!(stdin, "empty author").unwrap();
        }

        let output = child.wait_with_output().unwrap();
        let sha = String::from_utf8(output.stdout).unwrap().trim().to_string();

        let author = GitResolver::get_commit_author(dir.path(), &sha).unwrap();
        assert_eq!(author, "unknown");
    }

    #[test]
    fn test_verify_ignored_out_of_bounds_prevention() {
        let dir = tempdir().unwrap();
        create_mock_git_repo(dir.path(), "ref: refs/heads/main\n");

        // We pass the root directory itself to simulate the pathological case where pop()
        // would escape the repository.
        let paths = vec![dir.path().to_path_buf()];

        let result = GitResolver::verify_ignored_impl(&paths, dir.path()).unwrap();
        // Since no ignore rules are defined that match the root itself, it should return false
        assert!(!result);
    }
}
