//! CLI Argument parser definition for gleon.

use clap::{Parser, Subcommand};

/// The main CLI structure for gleon.
#[derive(Parser, Debug)]
#[command(
    name = "gleon",
    version,
    about = "Universal visual regression testing CLI"
)]
pub struct Cli {
    /// Override the active git branch context
    #[arg(short = 'b', long = "branch", global = true)]
    pub branch: Option<String>,

    /// Override the OS component of the platform context (e.g. macos, linux, windows)
    #[arg(long = "os", global = true)]
    pub os: Option<String>,

    /// Override the CPU architecture component of the platform context (e.g. aarch64, x86_64)
    #[arg(long = "arch", global = true)]
    pub arch: Option<String>,

    /// Override the renderer identifier of the platform context (e.g. flutter-3.22, chrome-126)
    #[arg(long = "renderer", global = true)]
    pub renderer: Option<String>,

    /// Additional isolation labels (repeatable: --label key=val)
    #[arg(long = "label", global = true, value_parser = parse_label)]
    pub labels: Vec<(String, String)>,

    /// Override the active platform with an opaque custom string
    #[arg(short = 'p', long = "platform", global = true)]
    pub platform: Option<String>,

    /// Enable verbose logging (DEBUG level)
    #[arg(short = 'v', long = "verbose", global = true, conflicts_with = "quiet")]
    pub verbose: bool,

    /// Suppress informational output (only show WARN/ERROR)
    #[arg(short = 'q', long = "quiet", global = true)]
    pub quiet: bool,

    /// Path to a custom configuration file
    #[arg(short = 'c', long = "config", global = true)]
    pub config: Option<std::path::PathBuf>,

    /// Enforce strict licensing compliance (hard fail with exit code 42 on violations)
    #[arg(long = "strict", global = true, env = "GLEON_STRICT")]
    pub strict: bool,

    /// The target branch to compare against (defaults to 'main')
    #[arg(
        long = "target-branch",
        global = true,
        env = "GLEON_TARGET_BRANCH",
        default_value = "main"
    )]
    pub target_branch: String,

    /// The subcommand to execute
    #[command(subcommand)]
    pub command: Commands,
}

impl Cli {
    /// Constructs a `Cli` instance populated with default test values for the given command.
    pub fn for_test(command: Commands) -> Self {
        Self {
            branch: Some("main".to_string()),
            os: None,
            arch: None,
            renderer: None,
            labels: vec![],
            platform: None,
            verbose: false,
            quiet: false,
            config: None,
            strict: false,
            target_branch: "main".to_string(),
            command,
        }
    }
}

pub(crate) fn parse_label(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .ok_or_else(|| format!("invalid label: no '=' found in '{}'", s))
        .and_then(|(key, val)| {
            let key = key.trim().to_string();
            let val = val.trim().to_string();
            if key.is_empty() {
                return Err("invalid label: key cannot be empty".to_string());
            }
            if val.is_empty() {
                return Err("invalid label: value cannot be empty".to_string());
            }
            Ok((key, val))
        })
}

/// The available subcommands in gleon.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum Commands {
    /// Initialize gleon directory structure and default configuration
    Init,
    /// Print resolved configuration and active status
    Status {
        /// Format output as JSON
        #[arg(long = "json")]
        json: bool,
    },
    /// Stage actual screenshots as new baselines
    Stage {
        /// Optional path filters to stage
        #[arg(value_name = "PATHS")]
        paths: Vec<std::path::PathBuf>,
    },
    /// Run visual diff comparison against baseline images
    Diff {
        /// Automatically pull the latest remote baselines before diffing
        #[arg(long = "auto-pull")]
        auto_pull: bool,
        /// Interactively resolve Git merge conflicts in baseline manifests
        #[arg(long = "resolve")]
        resolve: bool,
    },
    /// Lint baseline JSON manifests for schema validity and Git conflict markers
    #[command(alias = "lint")]
    LintManifests {
        /// Optional platform filter (e.g. macos-aarch64)
        #[arg(short, long)]
        platform: Option<String>,
    },
    /// Interactively resolve Git merge conflicts in baseline manifests
    Resolve {
        /// Optional specific test path filter
        #[arg(value_name = "TEST")]
        test_path: Option<String>,
        /// Download missing baseline blobs from remote storage during resolution
        #[arg(long)]
        fetch: bool,
    },
    /// Execute tests and run diff comparison
    Test,
    /// Pull latest baselines from remote storage
    Pull {
        /// Pull blobs for all platforms under .gleon/manifests/ instead of only the active platform
        #[arg(short = 'a', long = "all", conflicts_with = "platform")]
        all_platforms: bool,
        /// Optional target platform override (e.g. macos-aarch64)
        #[arg(short = 'p', long = "platform")]
        platform: Option<String>,
    },
    /// Push staged changes and report to remote storage
    Push {
        /// Push blobs for all platforms under .gleon/manifests/ instead of only the active platform
        #[arg(short = 'a', long = "all", conflicts_with = "platform")]
        all_platforms: bool,
        /// Optional target platform override (e.g. macos-aarch64)
        #[arg(short = 'p', long = "platform")]
        platform: Option<String>,
    },
    /// Clean up unreferenced baseline blobs
    Gc,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_cli() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn test_parse_label_with_equals_in_value() {
        let (k, v) = parse_label("url=http://host:8080").unwrap();
        assert_eq!(k, "url");
        assert_eq!(v, "http://host:8080");
    }

    #[test]
    fn test_parse_branch_flag() -> Result<(), clap::Error> {
        let args = ["gleon", "-b", "feature-test", "status"];
        let cli = Cli::try_parse_from(args)?;
        assert_eq!(cli.branch, Some("feature-test".to_string()));
        assert_eq!(cli.command, Commands::Status { json: false });
        Ok(())
    }

    #[test]
    fn test_parse_branch_flag_long() -> Result<(), clap::Error> {
        let args = ["gleon", "--branch", "another-branch", "diff"];
        let cli = Cli::try_parse_from(args)?;
        assert_eq!(cli.branch, Some("another-branch".to_string()));
        assert_eq!(
            cli.command,
            Commands::Diff {
                auto_pull: false,
                resolve: false
            }
        );
        assert_eq!(cli.target_branch, "main"); // Default value
        Ok(())
    }

    #[test]
    fn test_parse_lint_and_resolve_commands() -> Result<(), clap::Error> {
        let args_lint = ["gleon", "lint-manifests", "--platform", "linux-x86_64"];
        let cli_lint = Cli::try_parse_from(args_lint)?;
        assert_eq!(
            cli_lint.command,
            Commands::LintManifests {
                platform: Some("linux-x86_64".to_string())
            }
        );

        let args_resolve = ["gleon", "resolve", "--fetch", "auth/login"];
        let cli_resolve = Cli::try_parse_from(args_resolve)?;
        assert_eq!(
            cli_resolve.command,
            Commands::Resolve {
                test_path: Some("auth/login".to_string()),
                fetch: true,
            }
        );

        let args_diff_resolve = ["gleon", "diff", "--resolve"];
        let cli_diff_resolve = Cli::try_parse_from(args_diff_resolve)?;
        assert_eq!(
            cli_diff_resolve.command,
            Commands::Diff {
                auto_pull: false,
                resolve: true,
            }
        );
        Ok(())
    }

    #[test]
    fn test_parse_target_branch_flag() -> Result<(), clap::Error> {
        let args = ["gleon", "--target-branch", "develop", "diff"];
        let cli = Cli::try_parse_from(args)?;
        assert_eq!(cli.target_branch, "develop");
        Ok(())
    }

    #[test]
    fn test_parse_platform_flags() -> Result<(), clap::Error> {
        let args = [
            "gleon",
            "--os",
            "linux",
            "--arch",
            "x86_64",
            "--renderer",
            "chrome",
            "--label",
            "theme=dark",
            "--label",
            "locale=en",
            "stage",
        ];
        let cli = Cli::try_parse_from(args)?;
        assert_eq!(cli.os, Some("linux".to_string()));
        assert_eq!(cli.arch, Some("x86_64".to_string()));
        assert_eq!(cli.renderer, Some("chrome".to_string()));
        assert_eq!(
            cli.labels,
            vec![
                ("theme".to_string(), "dark".to_string()),
                ("locale".to_string(), "en".to_string())
            ]
        );
        assert_eq!(cli.command, Commands::Stage { paths: vec![] });
        Ok(())
    }

    #[test]
    fn test_parse_legacy_platform_flag() -> Result<(), clap::Error> {
        let args = ["gleon", "--platform", "custom-opaque", "stage"];
        let cli = Cli::try_parse_from(args)?;
        assert_eq!(cli.platform, Some("custom-opaque".to_string()));
        assert_eq!(cli.command, Commands::Stage { paths: vec![] });
        Ok(())
    }

    #[test]
    fn test_parse_verbose_flag() -> Result<(), clap::Error> {
        let args = ["gleon", "-v", "status"];
        let cli = Cli::try_parse_from(args)?;
        assert!(cli.verbose);
        assert!(!cli.quiet);

        let args_long = ["gleon", "--verbose", "status"];
        let cli_long = Cli::try_parse_from(args_long)?;
        assert!(cli_long.verbose);
        assert!(!cli_long.quiet);
        Ok(())
    }

    #[test]
    fn test_parse_quiet_flag() -> Result<(), clap::Error> {
        let args = ["gleon", "-q", "status"];
        let cli = Cli::try_parse_from(args)?;
        assert!(cli.quiet);
        assert!(!cli.verbose);

        let args_long = ["gleon", "--quiet", "status"];
        let cli_long = Cli::try_parse_from(args_long)?;
        assert!(cli_long.quiet);
        assert!(!cli_long.verbose);
        Ok(())
    }

    #[test]
    fn test_parse_invalid_flag() {
        let args = ["gleon", "--invalid-flag", "status"];
        let result = Cli::try_parse_from(args);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_label_errors() {
        assert!(parse_label("no_equals_sign").is_err());
        assert!(parse_label("=value").is_err());
        assert!(parse_label("key=").is_err());
        assert!(parse_label("  =  ").is_err());
    }

    #[test]
    fn test_parse_pull_push_conflicting_all_and_platform() {
        let pull_conflict =
            Cli::try_parse_from(["gleon", "pull", "--all", "--platform", "macos-aarch64"]);
        assert!(pull_conflict.is_err());

        let push_conflict = Cli::try_parse_from(["gleon", "push", "-a", "-p", "macos-aarch64"]);
        assert!(push_conflict.is_err());

        let pull_ok = Cli::try_parse_from(["gleon", "pull", "--all"]);
        assert!(pull_ok.is_ok());

        let push_ok = Cli::try_parse_from(["gleon", "push", "--platform", "linux-x86_64"]);
        assert!(push_ok.is_ok());
    }
}
