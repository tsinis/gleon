use crate::cli::Cli;
use crate::config::{ConfigError, GleonConfig};
use crate::platform::{PlatformEnv, PlatformError, PlatformInfo, PlatformResolver};

#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),
    #[error("Platform error: {0}")]
    Platform(#[from] PlatformError),
    #[error("Git error: {0}")]
    Git(#[from] crate::git::GitError),
}

/// Traverses parent directories starting from `start_dir` to find `gleon.yaml`.
/// Mutates the path in-place using `pop()` to avoid heap allocations.
/// Returns `Some((config_path, root_dir))` if found, or `None` if not found.
pub fn find_config_and_root(
    start_dir: &std::path::Path,
) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let mut current = start_dir.to_path_buf();
    loop {
        let candidate = current.join("gleon.yaml");
        if candidate.is_file() {
            return Some((candidate, current));
        }
        if !current.pop() {
            break;
        }
    }
    None
}

#[derive(Debug)]
#[non_exhaustive]
pub struct ResolvedContext {
    pub config: Option<GleonConfig>,
    pub platform: PlatformInfo,
    pub fallback_platform_key: Option<String>,
    pub branch: String,
    pub target_branch: String,
    pub base_dir: std::path::PathBuf,
}

impl Default for ResolvedContext {
    fn default() -> Self {
        Self {
            config: None,
            platform: PlatformInfo {
                os: "unknown".to_string(),
                arch: None,
                renderer: None,
                labels: std::collections::BTreeMap::new(),
            },
            fallback_platform_key: None,
            branch: "main".to_string(),
            target_branch: "main".to_string(),
            base_dir: std::path::PathBuf::from("."),
        }
    }
}

impl ResolvedContext {
    pub fn from_cli(cli: &Cli, base_dir: &std::path::Path) -> Result<Self, ContextError> {
        let env = PlatformEnv::from_env();
        Self::from_cli_impl(cli, base_dir, &crate::git::OsEnv, &env)
    }

    pub fn from_cli_impl(
        cli: &Cli,
        base_dir: &std::path::Path,
        env_provider: &dyn crate::git::EnvProvider,
        platform_env: &PlatformEnv,
    ) -> Result<Self, ContextError> {
        let (config, resolved_base_dir) = if let Some(ref path) = cli.config {
            tracing::debug!(
                "Loading configuration from explicitly provided path: {:?}",
                path
            );
            let cfg = GleonConfig::load_from_file(path)?;
            let root = find_config_and_root(base_dir)
                .map(|(_, r)| r)
                .unwrap_or_else(|| base_dir.to_path_buf());
            (Some(cfg), root)
        } else if let Some((config_path, root_dir)) = find_config_and_root(base_dir) {
            tracing::debug!(
                "Discovered gleon.yaml at {:?} (root: {:?})",
                config_path,
                root_dir
            );
            let cfg = GleonConfig::load_from_file(&config_path)?;
            (Some(cfg), root_dir)
        } else {
            (None, base_dir.to_path_buf())
        };

        let platform = PlatformResolver::resolve(
            cli.os.as_deref(),
            cli.arch.as_deref(),
            cli.renderer.as_deref(),
            &cli.labels,
            cli.platform.as_deref(),
            platform_env,
            config.as_ref().and_then(|c| c.platform.as_ref()),
        )?;

        let fallback_platform_key = if let Some(ref fb_env) = platform_env.fallback_platform {
            let plat_cfg =
                if let Ok(fields) = crate::platform::PlatformFields::parse_key_value(fb_env) {
                    crate::platform::PlatformConfig::Structured(fields)
                } else {
                    crate::platform::PlatformConfig::Opaque(fb_env.clone())
                };
            Some(plat_cfg.to_key().map_err(ContextError::Platform)?)
        } else if let Some(ref cfg) = config {
            cfg.fallback_platform
                .as_ref()
                .map(|fb_cfg| fb_cfg.to_key().map_err(ContextError::Platform))
                .transpose()?
        } else {
            None
        };

        let branch = match crate::git::GitResolver::resolve_branch_impl(
            cli.branch.as_deref(),
            &resolved_base_dir,
            env_provider,
        ) {
            Ok(b) => b,
            Err(e @ crate::git::GitError::InvalidBranchName(_)) => {
                return Err(ContextError::Git(e));
            }
            Err(e) => {
                tracing::debug!(
                    "Git branch resolution failed: {}. Falling back to 'main' for offline mode.",
                    e
                );
                "main".to_string()
            }
        };

        Ok(Self {
            config,
            platform,
            fallback_platform_key,
            branch,
            target_branch: cli.target_branch.clone(),
            base_dir: resolved_base_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Commands;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    struct EmptyEnv;
    impl crate::git::EnvProvider for EmptyEnv {
        fn get_var(&self, _key: &str) -> Option<String> {
            None
        }
    }

    fn create_mock_git_repo(path: &std::path::Path, head_content: &str) {
        let git_dir = path.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::create_dir_all(git_dir.join("objects")).unwrap();
        std::fs::create_dir_all(git_dir.join("refs")).unwrap();
        std::fs::write(git_dir.join("HEAD"), head_content).unwrap();
    }

    #[test]
    fn test_from_cli_with_config_path() {
        let dir = tempdir().unwrap();
        create_mock_git_repo(dir.path(), "ref: refs/heads/main\n");
        let config_path = dir.path().join("my_config.yaml");
        let mut file = File::create(&config_path).unwrap();
        writeln!(
            file,
            "required_version: \">=0.1.0\"\nscreenshots:\n  - include: \"*.png\""
        )
        .unwrap();

        let cli = Cli {
            branch: None,
            target_branch: "develop".to_string(),
            os: None,
            arch: None,
            renderer: None,
            labels: vec![],
            platform: None,
            verbose: false,
            quiet: false,
            config: Some(config_path),
            command: Commands::Status { json: false },
        };

        let context =
            ResolvedContext::from_cli_impl(&cli, dir.path(), &EmptyEnv, &PlatformEnv::default())
                .unwrap();
        assert!(context.config.is_some());
        assert_eq!(context.branch, "main");
        assert_eq!(context.target_branch, "develop");
    }

    #[test]
    fn test_from_cli_no_config_no_default_file() {
        let dir = tempdir().unwrap();
        create_mock_git_repo(dir.path(), "ref: refs/heads/main\n");
        let cli = Cli {
            branch: None,
            target_branch: "develop".to_string(),
            os: None,
            arch: None,
            renderer: None,
            labels: vec![],
            platform: None,
            verbose: false,
            quiet: false,
            config: None,
            command: Commands::Status { json: false },
        };

        let context =
            ResolvedContext::from_cli_impl(&cli, dir.path(), &EmptyEnv, &PlatformEnv::default())
                .unwrap();
        assert!(context.config.is_none());
        assert_eq!(context.branch, "main");
        assert_eq!(context.target_branch, "develop");
    }

    #[test]
    fn test_from_cli_no_config_with_default_file() {
        let dir = tempdir().unwrap();
        create_mock_git_repo(dir.path(), "ref: refs/heads/main\n");
        let default_path = dir.path().join("gleon.yaml");
        let mut file = File::create(&default_path).unwrap();
        writeln!(
            file,
            "required_version: \">=0.1.0\"\nscreenshots:\n  - include: \"*.png\""
        )
        .unwrap();

        let cli = Cli {
            branch: None,
            target_branch: "develop".to_string(),
            os: None,
            arch: None,
            renderer: None,
            labels: vec![],
            platform: None,
            verbose: false,
            quiet: false,
            config: None,
            command: Commands::Status { json: false },
        };

        let context =
            ResolvedContext::from_cli_impl(&cli, dir.path(), &EmptyEnv, &PlatformEnv::default())
                .unwrap();
        assert!(context.config.is_some());
        assert_eq!(context.branch, "main");
        assert_eq!(context.target_branch, "develop");
    }

    #[test]
    fn test_from_cli_production_wrapper() {
        let dir = tempdir().unwrap();
        create_mock_git_repo(dir.path(), "ref: refs/heads/main\n");
        let cli = Cli {
            branch: Some("main".to_string()),
            target_branch: "develop".to_string(),
            os: Some("linux".to_string()),
            arch: Some("x86_64".to_string()),
            renderer: None,
            labels: vec![],
            platform: None,
            verbose: false,
            quiet: false,
            config: None,
            command: Commands::Status { json: false },
        };
        let context = ResolvedContext::from_cli(&cli, dir.path()).unwrap();
        assert_eq!(context.branch, "main");
    }

    #[test]
    fn test_from_cli_errors() {
        let dir = tempdir().unwrap();

        // 1. Platform resolver error
        let cli_platform_err = Cli {
            branch: Some("main".to_string()),
            target_branch: "develop".to_string(),
            os: None,
            arch: None,
            renderer: None,
            labels: vec![],
            platform: Some("custom-opaque".to_string()),
            verbose: false,
            quiet: false,
            config: None,
            command: Commands::Status { json: false },
        };
        let platform_env_conflict = PlatformEnv {
            os: Some("linux".to_string()),
            ..Default::default()
        };
        let result = ResolvedContext::from_cli_impl(
            &cli_platform_err,
            dir.path(),
            &EmptyEnv,
            &platform_env_conflict,
        );
        assert!(result.is_err());

        // 2. Git resolver error propagation (invalid branch name is returned as Err)
        let cli_git_err = Cli {
            branch: Some("invalid branch name space".to_string()),
            target_branch: "develop".to_string(),
            os: None,
            arch: None,
            renderer: None,
            labels: vec![],
            platform: None,
            verbose: false,
            quiet: false,
            config: None,
            command: Commands::Status { json: false },
        };
        let result = ResolvedContext::from_cli_impl(
            &cli_git_err,
            dir.path(),
            &EmptyEnv,
            &PlatformEnv::default(),
        );
        assert!(matches!(
            result,
            Err(ContextError::Git(crate::git::GitError::InvalidBranchName(
                _
            )))
        ));
    }

    #[test]
    fn test_from_cli_root_discovery() {
        let dir = tempdir().unwrap();
        let root_dir = dir.path();
        let nested_dir = root_dir.join("src/features/billing");
        std::fs::create_dir_all(&nested_dir).unwrap();

        let config_path = root_dir.join("gleon.yaml");
        let yaml_content = "required_version: \">=0.1.0\"\nscreenshots:\n  - include: \"*.png\"";
        std::fs::write(&config_path, yaml_content).unwrap();

        let cli = Cli {
            branch: None,
            target_branch: "main".to_string(),
            os: None,
            arch: None,
            renderer: None,
            labels: vec![],
            platform: None,
            verbose: false,
            quiet: false,
            config: None,
            command: Commands::Status { json: false },
        };

        // Call from nested_dir
        let ctx =
            ResolvedContext::from_cli_impl(&cli, &nested_dir, &EmptyEnv, &PlatformEnv::default())
                .unwrap();

        assert!(ctx.config.is_some());
        assert_eq!(ctx.base_dir, root_dir);
    }

    #[test]
    fn test_from_cli_corrupted_discovered_config() {
        let dir = tempdir().unwrap();
        let root_dir = dir.path();
        let config_path = root_dir.join("gleon.yaml");
        std::fs::write(&config_path, "invalid_yaml: : : [bad syntax]").unwrap();

        let cli = Cli {
            branch: None,
            target_branch: "main".to_string(),
            os: None,
            arch: None,
            renderer: None,
            labels: vec![],
            platform: None,
            verbose: false,
            quiet: false,
            config: None,
            command: Commands::Status { json: false },
        };

        let result =
            ResolvedContext::from_cli_impl(&cli, root_dir, &EmptyEnv, &PlatformEnv::default());

        assert!(result.is_err());
        assert!(matches!(result, Err(ContextError::Config(_))));
    }

    #[test]
    fn test_fallback_platform_key_resolution() {
        let dir = tempdir().unwrap();
        let root_dir = dir.path();
        let config_path = root_dir.join("gleon.yaml");
        let yaml_content = "required_version: \">=0.1.0\"\nfallback_platform:\n  os: linux\n  arch: x86_64\nscreenshots:\n  - include: \"*.png\"";
        std::fs::write(&config_path, yaml_content).unwrap();

        let cli = Cli {
            branch: None,
            target_branch: "main".to_string(),
            os: None,
            arch: None,
            renderer: None,
            labels: vec![],
            platform: None,
            verbose: false,
            quiet: false,
            config: None,
            command: Commands::Status { json: false },
        };

        // 1. Resolve from config
        let ctx =
            ResolvedContext::from_cli_impl(&cli, root_dir, &EmptyEnv, &PlatformEnv::default())
                .unwrap();
        assert_eq!(
            ctx.fallback_platform_key.as_deref(),
            Some("5:linux-6:x86_64")
        );

        // 2. Resolve from env (overrides config)
        let env = PlatformEnv {
            fallback_platform: Some("macos-aarch64".to_string()),
            ..Default::default()
        };
        let ctx_env = ResolvedContext::from_cli_impl(&cli, root_dir, &EmptyEnv, &env).unwrap();
        assert_eq!(
            ctx_env.fallback_platform_key.as_deref(),
            Some("5:macos-7:aarch64")
        );
    }

    #[test]
    fn test_resolved_context_default() {
        let ctx = ResolvedContext::default();
        assert_eq!(ctx.platform.os, "unknown");
        assert_eq!(ctx.platform.arch, None);
        assert_eq!(ctx.platform.renderer, None);
        assert_eq!(ctx.branch, "main");
    }

    #[test]
    fn test_invalid_fallback_platform_error() {
        let temp = tempdir().unwrap();
        let root_dir = temp.path();

        let cli = Cli {
            os: None,
            arch: None,
            renderer: None,
            labels: vec![],
            platform: None,
            branch: None,
            target_branch: "main".to_string(),
            verbose: false,
            quiet: false,
            config: None,
            command: Commands::Status { json: false },
        };

        // Invalid fallback in env
        let env_invalid = PlatformEnv {
            fallback_platform: Some("INVALID/OS".to_string()),
            ..Default::default()
        };
        let err =
            ResolvedContext::from_cli_impl(&cli, root_dir, &EmptyEnv, &env_invalid).unwrap_err();
        assert!(matches!(err, ContextError::Platform(_)));

        // Invalid fallback in config
        let config_path = root_dir.join("invalid_gleon.yaml");
        let invalid_cfg = GleonConfig {
            fallback_platform: Some(crate::platform::PlatformConfig::Opaque(
                "INVALID/OS".to_string(),
            )),
            ..Default::default()
        };
        let yaml_str = serde_yaml::to_string(&invalid_cfg).unwrap();
        std::fs::write(&config_path, yaml_str).unwrap();

        let cli_cfg = Cli {
            config: Some(config_path),
            ..cli
        };
        let err_cfg =
            ResolvedContext::from_cli_impl(&cli_cfg, root_dir, &EmptyEnv, &PlatformEnv::default())
                .unwrap_err();
        assert!(matches!(
            err_cfg,
            ContextError::Config(_) | ContextError::Platform(_)
        ));
    }
}
