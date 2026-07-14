//! CLI Argument parser definition for Gleon.

use clap::{Parser, Subcommand};

/// The main CLI structure for Gleon.
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

    /// Override the active platform context (e.g. macos-aarch64)
    #[arg(short = 'p', long = "platform", global = true)]
    pub platform: Option<String>,

    /// The subcommand to execute
    #[command(subcommand)]
    pub command: Commands,
}

/// The available subcommands in Gleon.
#[derive(Subcommand, Debug, Clone, PartialEq)]
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
        Ok(())
    }

    #[test]
    fn test_parse_platform_flag() -> Result<(), clap::Error> {
        let args = ["gleon", "-p", "ios-17", "stage"];
        let cli = Cli::try_parse_from(args)?;
        assert_eq!(cli.platform, Some("ios-17".to_string()));
        assert_eq!(cli.command, Commands::Stage);
        Ok(())
    }

    #[test]
    fn test_parse_platform_flag_long() -> Result<(), clap::Error> {
        let args = ["gleon", "--platform", "android-33", "gc"];
        let cli = Cli::try_parse_from(args)?;
        assert_eq!(cli.platform, Some("android-33".to_string()));
        assert_eq!(cli.command, Commands::Gc);
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
    fn test_parse_invalid_flag() {
        let args = ["gleon", "--invalid-flag", "status"];
        let result = Cli::try_parse_from(args);
        assert!(result.is_err());
    }
}
