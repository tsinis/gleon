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

#[derive(Debug)]
pub struct ResolvedContext {
    pub config: Option<GleonConfig>,
    pub platform: PlatformInfo,
    pub branch: String,
    pub target_branch: String,
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
        let (config, _config_specified) = if let Some(ref path) = cli.config {
            (Some(GleonConfig::load_from_file(path)?), true)
        } else {
            let default_path = base_dir.join("gleon.yaml");
            if default_path.exists() {
                (Some(GleonConfig::load_from_file(&default_path)?), false)
            } else {
                (None, false)
            }
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

        let branch = crate::git::GitResolver::resolve_branch_impl(
            cli.branch.as_deref(),
            base_dir,
            env_provider,
        )?;

        Ok(Self {
            config,
            platform,
            branch,
            target_branch: cli.target_branch.clone(),
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
            command: Commands::Status,
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
            command: Commands::Status,
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
            command: Commands::Status,
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
            command: Commands::Status,
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
            command: Commands::Status,
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

        // 2. Git resolver error
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
            command: Commands::Status,
        };
        let result = ResolvedContext::from_cli_impl(
            &cli_git_err,
            dir.path(),
            &EmptyEnv,
            &PlatformEnv::default(),
        );
        assert!(result.is_err());
    }
}
