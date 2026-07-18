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

pub(crate) fn parse_label(s: &str) -> Result<(String, String), String> {
    let (key, val) = s
        .split_once('=')
        .ok_or_else(|| format!("invalid label: no '=' found in '{}'", s))?;
    let key = key.trim().to_string();
    let val = val.trim().to_string();
    if key.is_empty() {
        return Err("invalid label: key cannot be empty".to_string());
    }
    if val.is_empty() {
        return Err("invalid label: value cannot be empty".to_string());
    }
    Ok((key, val))
}

/// The available subcommands in gleon.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum Commands {
    /// Print resolved configuration and active status
    Status,
    /// Stage actual screenshots as new baselines
    Stage,
    /// Run visual diff comparison against baseline images
    Diff,
    /// Execute tests and run diff comparison
    Test,
    /// Pull latest baselines from remote storage
    Pull,
    /// Push staged changes and report to remote storage
    Push,
    /// Merge branch manifest into main's manifest
    Merge {
        /// The target branch to merge into main
        target_branch: String,
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
    fn test_parse_branch_flag() -> Result<(), clap::Error> {
        let args = ["gleon", "-b", "feature-test", "status"];
        let cli = Cli::try_parse_from(args)?;
        assert_eq!(cli.branch, Some("feature-test".to_string()));
        assert_eq!(cli.command, Commands::Status);
        Ok(())
    }

    #[test]
    fn test_parse_branch_flag_long() -> Result<(), clap::Error> {
        let args = ["gleon", "--branch", "another-branch", "diff"];
        let cli = Cli::try_parse_from(args)?;
        assert_eq!(cli.branch, Some("another-branch".to_string()));
        assert_eq!(cli.command, Commands::Diff);
        assert_eq!(cli.target_branch, "main"); // Default value
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
        assert_eq!(cli.command, Commands::Stage);
        Ok(())
    }

    #[test]
    fn test_parse_legacy_platform_flag() -> Result<(), clap::Error> {
        let args = ["gleon", "--platform", "custom-opaque", "stage"];
        let cli = Cli::try_parse_from(args)?;
        assert_eq!(cli.platform, Some("custom-opaque".to_string()));
        assert_eq!(cli.command, Commands::Stage);
        Ok(())
    }

    #[test]
    fn test_parse_merge_subcommand() -> Result<(), clap::Error> {
        let args = ["gleon", "merge", "feature-branch"];
        let cli = Cli::try_parse_from(args)?;
        assert_eq!(
            cli.command,
            Commands::Merge {
                target_branch: "feature-branch".to_string()
            }
        );
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
}
