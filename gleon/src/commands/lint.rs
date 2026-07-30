//! Implementation of the `gleon lint-manifests` subcommand.

use gleon_core::context::ResolvedContext;
use gleon_core::ops::lint_workspace_manifests;
use tracing::{error, info};

/// Runs manifest linting across the workspace.
///
/// Returns exit code `0` if all manifests are valid and unconflicted,
/// or `1` if any file contains conflict markers or schema corruption.
pub fn run_lint(ctx: &ResolvedContext, platform_filter: Option<&str>) -> anyhow::Result<i32> {
    info!("Running manifest linting...");

    let report = match lint_workspace_manifests(ctx, &ctx.base_dir, platform_filter) {
        Ok(rep) => rep,
        Err(e) => {
            error!("Error during manifest linting: {e}");
            return Ok(1);
        }
    };

    info!(
        "Inspected {} manifest file(s): {} valid.",
        report.total_files, report.valid_files
    );

    if !report.conflicted_files.is_empty() {
        error!("Git merge conflict markers (<<<<<<<) found in:");
        for (path, msg) in &report.conflicted_files {
            error!("  - {}: {}", path.display(), msg);
        }
    }

    if !report.corrupted_files.is_empty() {
        error!("Schema or syntax errors found in:");
        for (path, msg) in &report.corrupted_files {
            error!("  - {}: {}", path.display(), msg);
        }
    }

    if report.passed {
        info!("All manifest files passed linting.");
        Ok(0)
    } else {
        if !report.conflicted_files.is_empty() {
            error!(
                "Lint check failed due to Git conflicts! Run 'gleon resolve' to resolve conflicts."
            );
        } else {
            error!(
                "Lint check failed due to schema/JSON errors! Please repair reported manifest files."
            );
        }
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gleon_core::cli::{Cli, Commands};
    use tempfile::tempdir;

    #[test]
    fn test_run_lint_branches() {
        let temp = tempdir().unwrap();
        let cli = Cli::for_test(Commands::Init);
        let ctx = ResolvedContext::from_cli(&cli, temp.path()).unwrap();

        // 1. Missing directory -> Err -> Ok(1)
        assert_eq!(run_lint(&ctx, None).unwrap(), 1);

        // 2. Clean manifest -> Ok(0)
        let manifests_dir = temp
            .path()
            .join(".gleon")
            .join("manifests")
            .join("macos-aarch64");
        std::fs::create_dir_all(&manifests_dir).unwrap();
        let valid_json = include_str!("../../../gleon-core/tests/fixtures/valid_manifest.json");
        std::fs::write(manifests_dir.join("valid.json"), valid_json).unwrap();
        assert_eq!(run_lint(&ctx, None).unwrap(), 0);

        // 3. Conflicted manifest -> Ok(1) with conflict advice
        let conflicted_json = include_str!("../../../gleon-core/tests/fixtures/conflict_2way.json");
        std::fs::write(manifests_dir.join("conflict.json"), conflicted_json).unwrap();
        assert_eq!(run_lint(&ctx, None).unwrap(), 1);

        // 4. Corrupted manifest -> Ok(1) with schema advice
        std::fs::remove_file(manifests_dir.join("conflict.json")).unwrap();
        let corrupt_json = include_str!("../../../gleon-core/tests/fixtures/corrupt_manifest.json");
        std::fs::write(manifests_dir.join("corrupt.json"), corrupt_json).unwrap();
        assert_eq!(run_lint(&ctx, None).unwrap(), 1);
    }
}
