#![cfg(not(miri))]

use gleon_core::cli::{Cli, Commands};
use gleon_core::context::ResolvedContext;
use gleon_core::ops::{StatusError, check_status, init_workspace, stage_workspace};
use std::fs;
use std::path::Path;

#[test]
fn test_status_uninitialized_fails() {
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
        command: Commands::Status { json: false },
    };

    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();
    let result = check_status(&ctx, base_path);

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), StatusError::NotInitialized));
}

#[test]
fn test_status_fresh_workspace_reports_added_with_real_fixture() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    // 1. Initialize workspace
    let cli_init = Cli::for_test(Commands::Init);
    let ctx_init = ResolvedContext::from_cli(&cli_init, base_path).unwrap();

    init_workspace(&ctx_init, base_path).expect("init_workspace should succeed");

    // 2. Copy real fixture file to base_path/billing/form.png
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let real_png_bytes =
        fs::read(fixtures_dir.join("200x100.png")).expect("200x100.png fixture must exist");

    let screenshot_dir = base_path.join("billing");
    fs::create_dir_all(&screenshot_dir).unwrap();
    let screenshot_file = screenshot_dir.join("form.png");
    fs::write(&screenshot_file, real_png_bytes).unwrap();

    // 3. Write custom config targeting billing/**/*.png
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
        command: Commands::Status { json: false },
    };

    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();
    let report = check_status(&ctx, base_path).expect("check_status should succeed");

    assert!(!report.is_clean());
    assert_eq!(report.added.len(), 1);
    assert_eq!(report.added[0], Path::new("billing/form.png"));
    assert!(report.modified.is_empty());
    assert!(report.deleted.is_empty());

    let text_output = report.format_text();
    assert!(text_output.contains("Added:\n  billing/form.png"));
}

#[test]
fn test_status_from_nested_subdirectory() {
    let temp_dir = tempfile::tempdir().unwrap();
    let root_dir = temp_dir.path();

    let cli_init = Cli::for_test(Commands::Init);
    let ctx_init = ResolvedContext::from_cli(&cli_init, root_dir).unwrap();
    init_workspace(&ctx_init, root_dir).expect("init_workspace should succeed");

    let nested_dir = root_dir.join("src").join("billing");
    fs::create_dir_all(&nested_dir).unwrap();

    let cli = Cli::for_test(Commands::Status { json: false });
    // Resolving from nested_dir discovers gleon.yaml in root_dir and sets ctx.base_dir = root_dir
    let ctx = ResolvedContext::from_cli(&cli, &nested_dir).unwrap();
    assert_eq!(ctx.base_dir, root_dir);

    let report = check_status(&ctx, &ctx.base_dir)
        .expect("check_status should succeed when using ctx.base_dir");
    assert!(report.is_clean());
}

#[test]
fn test_status_with_mask_rules_is_clean_after_staging() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli_init = Cli::for_test(Commands::Init);
    let ctx_init = ResolvedContext::from_cli(&cli_init, base_path).unwrap();
    init_workspace(&ctx_init, base_path).expect("init_workspace should succeed");

    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let real_png_bytes =
        fs::read(fixtures_dir.join("200x100.png")).expect("200x100.png fixture must exist");

    let screenshot_dir = base_path.join("masked_app");
    fs::create_dir_all(&screenshot_dir).unwrap();
    fs::write(screenshot_dir.join("screen.png"), real_png_bytes).unwrap();

    let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "masked_app/**/*.png"
    masks:
      - region: [0, 0, 50, 50]
"#;
    fs::write(base_path.join("gleon.yaml"), config_yaml).unwrap();

    let cli = Cli::for_test(Commands::Status { json: false });
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    // Stage screenshot
    stage_workspace(&ctx, base_path, None).expect("stage_workspace should succeed");

    // Check status post-staging
    let report = check_status(&ctx, base_path).expect("check_status should succeed");
    assert!(
        report.is_clean(),
        "Expected status to be clean for masked screenshots post-staging, got modified: {:?}",
        report.modified
    );
}
