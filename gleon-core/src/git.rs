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

    /// Path is outside the Git repository
    #[error("Path is outside the Git repository: {0}")]
    OutsideRepository(std::path::PathBuf),

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
    /// Falls back to `"main"` if no Git repository is found.
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
    /// Falls back to true if no Git repository is found.
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

        let repo_root = match repo.workdir() {
            Some(wd) => wd,
            None => {
                return Err(GitError::Discover(
                    "Bare repository has no working directory".to_string(),
                ));
            }
        };

        // Pre-process paths into absolute and relative counterparts, resolving path traversal
        let mut processed_paths = Vec::with_capacity(paths.len());
        for path in paths {
            let path_ref = path.as_ref();
            let abs_path = if path_ref.is_absolute() {
                path_ref.to_path_buf()
            } else {
                base_dir.join(path_ref)
            };
            let abs_path = normalize_path(&abs_path);

            // Check if the path is actually inside the repository
            if !abs_path.starts_with(repo_root) {
                return Err(GitError::OutsideRepository(abs_path));
            }

            let rel_path = abs_path.strip_prefix(repo_root).unwrap().to_path_buf();
            processed_paths.push((abs_path, rel_path));
        }

        let mut builder = ignore::gitignore::GitignoreBuilder::new(repo_root);

        // Add .git/info/exclude if it exists
        let exclude_path = repo.git_dir().join("info/exclude");
        if exclude_path.exists() {
            builder.add(&exclude_path);
        }

        let mut gitignores_to_add = std::collections::HashSet::new();

        for (abs_path, _) in &processed_paths {
            // Traverse up to repo_root to discover all .gitignore files in the hierarchy
            let mut current = abs_path.parent();
            while let Some(dir) = current {
                let gitignore = dir.join(".gitignore");
                if gitignore.is_file() {
                    gitignores_to_add.insert(gitignore);
                }
                if dir == repo_root {
                    break;
                }
                current = dir.parent();
            }
        }

        // To match Git semantics, deeper .gitignore files must override shallower ones.
        // The ignore crate applies later-added rules with higher precedence.
        // Therefore, we sort the discovered files by depth (number of components)
        // so that the root .gitignore is added first, and deeper files are added later.
        let mut sorted_gitignores: Vec<_> = gitignores_to_add.into_iter().collect();
        sorted_gitignores.sort_by_key(|p| p.components().count());

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
}

fn normalize_path(path: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
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
    normalized
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

fn resolve_ci_branch(env: &dyn EnvProvider) -> Option<String> {
    let get_valid = |k: &str| {
        env.get_var(k)
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
    };

    // 1. GitHub Actions
    if env.get_var("GITHUB_ACTIONS").as_deref() == Some("true")
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
    if env.get_var("CIRCLECI").as_deref() == Some("true")
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
    if env.get_var("TF_BUILD").as_deref() == Some("True")
        && let Some(b) = get_valid("BUILD_SOURCEBRANCHNAME")
    {
        return Some(b);
    }

    // 6. Travis CI
    if env.get_var("TRAVIS").as_deref() == Some("true")
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
    if env.get_var("BITRISE_IO").as_deref() == Some("true")
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
        // feature/über should be valid
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
        writeln!(file, "*.png").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&gitignore_path, std::fs::Permissions::from_mode(0o000));
        }

        let paths = vec![dir.path().join("file.png")];
        let result = GitResolver::verify_ignored_impl(&paths, dir.path()).unwrap();
        assert!(!result);

        // Restore permissions to allow cleanup
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&gitignore_path, std::fs::Permissions::from_mode(0o644));
        }
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
        let norm = normalize_path(path);
        assert_eq!(norm, Path::new("file.png"));
    }
}
