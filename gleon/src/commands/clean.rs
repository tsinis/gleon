//! Handler for `gleon clean` subcommand.

use gleon_core::context::ResolvedContext;
use gleon_core::ops::clean::{CleanOptions, clean_workspace};
use tracing::info;

/// Runs the `clean` command.
pub fn run_clean(
    ctx: &ResolvedContext,
    dry_run: bool,
    skip_gitignore: bool,
    keep_runs: bool,
) -> anyhow::Result<i32> {
    let options = CleanOptions {
        dry_run,
        skip_gitignore,
        keep_runs,
    };

    let res = match clean_workspace(ctx, &ctx.base_dir, &options) {
        Ok(r) => r,
        Err(e) => return Err(anyhow::Error::from(e)),
    };

    if dry_run {
        info!(
            "[dry-run] Found {} screenshot(s) to remove:",
            res.deleted_files.len()
        );
        for f in &res.deleted_files {
            info!("  - {}", f.display());
        }
        if !res.gitignore_entries_added.is_empty() {
            info!("[dry-run] Would add to .gitignore:");
            for entry in &res.gitignore_entries_added {
                info!("  + {}", entry);
            }
        }
        if res.cache_cleaned {
            info!("[dry-run] Would remove .gleon/runs and .gleon/diffs directories.");
        }
    } else {
        info!(
            "Removed {} screenshot(s) ({} untracked from Git index).",
            res.deleted_files.len(),
            res.untracked_files.len()
        );
        if !res.gitignore_entries_added.is_empty() {
            info!(
                "Added {} entry/entries to .gitignore.",
                res.gitignore_entries_added.len()
            );
        }
        if res.cache_cleaned {
            info!("Cleaned .gleon/runs and .gleon/diffs cache.");
        }
    }

    Ok(0)
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use gleon_core::cli::{Cli, Commands};
    use tempfile::tempdir;

    #[test]
    fn test_run_clean_dry_run_and_actual_flow() {
        let temp = tempdir().unwrap();
        let base_path = temp.path();

        let gleon_dir = base_path.join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();
        std::fs::create_dir_all(gleon_dir.join("runs")).unwrap();

        let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include:
      - "test/**/*.png"
    mode: pixel
"#;
        std::fs::write(gleon_dir.join("gleon.yaml"), config_yaml).unwrap();

        let test_dir = base_path.join("test");
        std::fs::create_dir_all(&test_dir).unwrap();
        std::fs::write(test_dir.join("login.png"), b"image").unwrap();

        let cli = Cli::for_test(Commands::Clean {
            dry_run: true,
            skip_gitignore: false,
            keep_runs: false,
        });
        let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

        // 1. Dry run
        let exit_code = run_clean(&ctx, true, false, false).unwrap();
        assert_eq!(exit_code, 0);
        assert!(test_dir.join("login.png").exists());

        // 2. Real run
        let exit_code = run_clean(&ctx, false, false, false).unwrap();
        assert_eq!(exit_code, 0);
        assert!(!test_dir.join("login.png").exists());
        assert!(base_path.join(".gitignore").exists());
    }

    #[test]
    fn test_run_clean_with_keep_runs_and_skip_gitignore() {
        let temp = tempdir().unwrap();
        let base_path = temp.path();

        let gleon_dir = base_path.join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include:
      - "test/**/*.png"
    mode: pixel
"#;
        std::fs::write(gleon_dir.join("gleon.yaml"), config_yaml).unwrap();

        let test_dir = base_path.join("test");
        std::fs::create_dir_all(&test_dir).unwrap();
        std::fs::write(test_dir.join("login.png"), b"image").unwrap();

        let runs_dir = gleon_dir.join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();

        let cli = Cli::for_test(Commands::Clean {
            dry_run: false,
            skip_gitignore: true,
            keep_runs: true,
        });
        let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

        // 1. Dry run with keep_runs=true and skip_gitignore=true
        let exit_code = run_clean(&ctx, true, true, true).unwrap();
        assert_eq!(exit_code, 0);

        // 2. Real run with keep_runs=true and skip_gitignore=true
        let exit_code = run_clean(&ctx, false, true, true).unwrap();
        assert_eq!(exit_code, 0);
        assert!(!base_path.join(".gitignore").exists());
        assert!(runs_dir.exists());
    }

    #[test]
    fn test_run_clean_error_propagation() {
        let temp = tempdir().unwrap();
        let base_path = temp.path();

        let gleon_dir = base_path.join(".gleon");
        std::fs::create_dir_all(&gleon_dir).unwrap();

        let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include:
      - "test/**/*.png"
    mode: pixel
"#;
        std::fs::write(gleon_dir.join("gleon.yaml"), config_yaml).unwrap();

        // Create .gitignore as directory to force CleanError::Io
        std::fs::create_dir_all(base_path.join(".gitignore")).unwrap();

        let cli = Cli::for_test(Commands::Clean {
            dry_run: false,
            skip_gitignore: false,
            keep_runs: false,
        });
        let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

        let err = run_clean(&ctx, false, false, false).unwrap_err();
        assert!(err.to_string().contains("IO error"));
    }
}
