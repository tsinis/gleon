#![cfg(not(miri))]

use gleon_core::cli::{Cli, Commands};
use gleon_core::context::ResolvedContext;
use gleon_core::ops::{
    ApproveError, approve_workspace, check_status, init_workspace, run_diff, stage_workspace,
};
use std::fs;
use std::path::Path;

#[test]
fn test_approve_uninitialized_fails() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli = Cli::for_test(Commands::Approve {
        paths: vec![],
        from: None,
    });

    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();
    let result = approve_workspace(&ctx, base_path, &[], None);

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ApproveError::NotInitialized));
}

#[test]
fn test_approve_full_flow_with_diff_failures() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli_init = Cli::for_test(Commands::Init);
    let ctx_init = ResolvedContext::from_cli(&cli_init, base_path).unwrap();
    init_workspace(&ctx_init, base_path).expect("init_workspace should succeed");

    // 1. Set up baseline screenshot using real static fixtures
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");

    let screenshot_dir = base_path.join("login");
    fs::create_dir_all(&screenshot_dir).unwrap();
    let screenshot_file = screenshot_dir.join("button.png");
    fs::copy(fixtures_dir.join("baseline_100x100.png"), &screenshot_file).unwrap();

    fs::copy(
        fixtures_dir.join("default_config.yaml"),
        base_path.join(".gleon").join("gleon.yaml"),
    )
    .unwrap();

    let cli = Cli::for_test(Commands::Test);
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    // Stage original baseline
    stage_workspace(&ctx, base_path, None).unwrap();
    assert!(check_status(&ctx, base_path).unwrap().is_clean());

    // 2. Change actual screenshot to updated_png and run diff -> fails & writes to .gleon/runs/latest/actual/
    fs::copy(
        fixtures_dir.join("diff_16px_corners_100x100.png"),
        &screenshot_file,
    )
    .unwrap();
    let diff_res = run_diff(&ctx, base_path).unwrap();
    assert_eq!(diff_res.failed_tests, 1);
    assert!(!diff_res.passed);
    assert!(
        base_path
            .join(".gleon")
            .join("runs")
            .join("latest")
            .join("actual")
            .join("login")
            .join("button.png")
            .exists()
    );

    // 3. Run approve without --from (defaults to .gleon/runs/latest/actual/)
    let approve_res = approve_workspace(&ctx, base_path, &[], None).unwrap();
    assert_eq!(approve_res.total_approved, 1);
    assert_eq!(
        approve_res.approved_test_cases,
        vec!["login/button".to_string()]
    );

    // 4. Verify status is clean and diff passes!
    assert!(check_status(&ctx, base_path).unwrap().is_clean());
    let diff_res_after = run_diff(&ctx, base_path).unwrap();
    assert!(diff_res_after.passed);
}

#[test]
fn test_approve_mixed_case_path_normalization() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli_init = Cli::for_test(Commands::Init);
    let ctx_init = ResolvedContext::from_cli(&cli_init, base_path).unwrap();
    init_workspace(&ctx_init, base_path).expect("init_workspace should succeed");

    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");

    // Custom directory with mixed casing
    let custom_dir = base_path.join("CustomSource").join("Auth");
    fs::create_dir_all(&custom_dir).unwrap();
    let file = custom_dir.join("LoginButton.png");
    fs::copy(fixtures_dir.join("baseline_100x100.png"), &file).unwrap();

    let cli = Cli::for_test(Commands::Approve {
        paths: vec![],
        from: Some(base_path.join("CustomSource")),
    });
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    let res =
        approve_workspace(&ctx, base_path, &[], Some(&base_path.join("CustomSource"))).unwrap();
    assert_eq!(res.total_approved, 1);
    assert_eq!(
        res.approved_test_cases,
        vec!["auth/loginbutton".to_string()]
    );

    let platform_key = ctx.platform.to_key().unwrap();
    let manifest_file = base_path
        .join(".gleon")
        .join("manifests")
        .join(platform_key)
        .join("auth/loginbutton.json");
    assert!(
        manifest_file.is_file(),
        "Manifest must be saved with canonical lowercase path"
    );
}

#[test]
fn test_approve_with_corrupt_image() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli_init = Cli::for_test(Commands::Init);
    let ctx_init = ResolvedContext::from_cli(&cli_init, base_path).unwrap();
    init_workspace(&ctx_init, base_path).unwrap();

    let screenshot_dir = base_path.join(".gleon/runs/latest/actual/billing");
    fs::create_dir_all(&screenshot_dir).unwrap();
    let screenshot_file = screenshot_dir.join("form.png");

    // Write corrupt image
    fs::write(&screenshot_file, "this is not a valid png file").unwrap();

    let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing/**/*.png"
"#;
    std::fs::create_dir_all(base_path.join(".gleon")).unwrap();
    fs::write(base_path.join(".gleon").join("gleon.yaml"), config_yaml).unwrap();

    let cli_approve = Cli {
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
        command: Commands::Approve {
            from: None,
            paths: vec![],
        },
    };
    let ctx_approve = ResolvedContext::from_cli(&cli_approve, base_path).unwrap();

    let result = approve_workspace(&ctx_approve, base_path, &[], None);
    assert!(result.is_err());

    assert!(matches!(
        result,
        Err(ApproveError::Manifest(
            gleon_core::manifest::ManifestError::Image(_)
        ))
    ));
}
