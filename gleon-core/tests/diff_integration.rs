#![cfg(not(miri))]

use gleon_core::cli::{Cli, Commands};
use gleon_core::context::ResolvedContext;
use gleon_core::ops::{DiffOpError, init_workspace, run_diff, stage_workspace};
use std::fs;
use std::path::Path;

#[test]
fn test_diff_uninitialized_fails() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli = Cli {
        branch: Some("main".to_string()),
        os: None,
        arch: None,
        renderer: None,
        labels: vec![],
        platform: None,
        verbose: false,
        quiet: false,
        config: None,
        target_branch: "main".to_string(),
        command: Commands::Diff { auto_pull: false },
    };

    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();
    let result = run_diff(&ctx, base_path);

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), DiffOpError::NotInitialized));
}

#[test]
fn test_diff_full_flow_with_real_fixtures() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli_init = Cli::for_test(Commands::Init);
    let ctx_init = ResolvedContext::from_cli(&cli_init, base_path).unwrap();

    // 1. Init workspace
    init_workspace(&ctx_init, base_path).expect("init_workspace should succeed");

    // 2. Copy real PNG fixture (baseline_100x100.png)
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let baseline_png_bytes = fs::read(fixtures_dir.join("baseline_100x100.png"))
        .expect("baseline_100x100.png fixture must exist");

    let screenshot_dir = base_path.join("billing");
    fs::create_dir_all(&screenshot_dir).unwrap();
    let screenshot_file = screenshot_dir.join("form.png");
    fs::write(&screenshot_file, &baseline_png_bytes).unwrap();

    let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing/**/*.png"
"#;
    fs::write(base_path.join("gleon.yaml"), config_yaml).unwrap();

    let cli = Cli {
        branch: Some("main".to_string()),
        os: None,
        arch: None,
        renderer: None,
        labels: vec![],
        platform: None,
        verbose: false,
        quiet: false,
        config: None,
        target_branch: "main".to_string(),
        command: Commands::Diff { auto_pull: false },
    };

    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    // 3. Stage initial baseline
    stage_workspace(&ctx, base_path, None).expect("stage_workspace should succeed");

    // 4. Run diff against identical baseline -> should pass
    let report_match = run_diff(&ctx, base_path).expect("run_diff should succeed");
    assert!(report_match.passed);
    assert_eq!(report_match.total_tests, 1);
    assert_eq!(report_match.failed_tests, 0);

    // 5. Replace form.png with a modified PNG fixture (diff_16px_corners_100x100.png)
    let modified_png_bytes = fs::read(fixtures_dir.join("diff_16px_corners_100x100.png"))
        .expect("diff_16px_corners_100x100.png fixture must exist");
    fs::write(&screenshot_file, &modified_png_bytes).unwrap();

    // 6. Run diff against modified image -> should report mismatch failure
    let report_mismatch = run_diff(&ctx, base_path).expect("run_diff should succeed");
    assert!(!report_mismatch.passed);
    assert_eq!(report_mismatch.total_tests, 1);
    assert_eq!(report_mismatch.failed_tests, 1);

    // 7. Verify generated artifacts on disk
    let runs_dir = base_path.join(".gleon/runs/latest");
    assert!(runs_dir.join("diffs/billing/form.png").is_file());
    assert!(runs_dir.join("report.html").is_file());
    assert!(runs_dir.join("report.md").is_file());
    assert!(runs_dir.join("junit.xml").is_file());
}

#[test]
fn test_diff_from_nested_subdirectory() {
    let temp_dir = tempfile::tempdir().unwrap();
    let root_dir = temp_dir.path();

    let cli_init = Cli::for_test(Commands::Init);
    let ctx_init = ResolvedContext::from_cli(&cli_init, root_dir).unwrap();
    init_workspace(&ctx_init, root_dir).expect("init_workspace should succeed");

    let nested_dir = root_dir.join("src").join("billing");
    fs::create_dir_all(&nested_dir).unwrap();

    let cli = Cli::for_test(Commands::Diff { auto_pull: false });
    let ctx = ResolvedContext::from_cli(&cli, &nested_dir).unwrap();
    assert_eq!(ctx.base_dir, root_dir);

    let report =
        run_diff(&ctx, &ctx.base_dir).expect("run_diff should succeed when using ctx.base_dir");
    assert!(report.passed);
}

#[test]
fn test_diff_cross_platform_backslash_manifest_keys() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli_init = Cli::for_test(Commands::Init);
    let ctx_init = ResolvedContext::from_cli(&cli_init, base_path).unwrap();
    init_workspace(&ctx_init, base_path).expect("init_workspace should succeed");

    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let baseline_png_bytes = fs::read(fixtures_dir.join("baseline_100x100.png"))
        .expect("baseline_100x100.png fixture must exist");

    let screenshot_dir = base_path.join("billing");
    fs::create_dir_all(&screenshot_dir).unwrap();
    fs::write(screenshot_dir.join("form.png"), &baseline_png_bytes).unwrap();

    let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing/**/*.png"
"#;
    fs::write(base_path.join("gleon.yaml"), config_yaml).unwrap();

    let cli = Cli::for_test(Commands::Diff { auto_pull: false });
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    // Stage baseline
    stage_workspace(&ctx, base_path, None).expect("stage_workspace should succeed");

    // Manually mutate the staged manifest blob to use Windows backslashes `billing\\form.png`
    let blobs_dir = base_path.join(".gleon/blobs/sha256");
    for entry in fs::read_dir(&blobs_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if let Ok(content) = fs::read_to_string(&path) {
            if !content.contains("billing/form.png") {
                continue;
            }
            let mutated = content.replace("billing/form.png", "billing\\\\form.png");
            fs::write(&path, mutated).unwrap();
        }
    }

    // Run diff -> should still match billing/form.png cross-platform!
    let report = run_diff(&ctx, base_path).expect("run_diff should handle backslash manifest keys");
    assert!(report.passed);
    assert_eq!(report.total_tests, 1);
    assert_eq!(report.failed_tests, 0);
}
