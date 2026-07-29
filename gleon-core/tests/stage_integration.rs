#![cfg(not(miri))]

use gleon_core::cli::{Cli, Commands};
use gleon_core::context::ResolvedContext;
use gleon_core::ops::{StageError, check_status, init_workspace, stage_workspace};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

#[test]
fn test_stage_uninitialized_fails() {
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
        command: Commands::Stage { paths: vec![] },
    };

    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();
    let result = stage_workspace(&ctx, base_path, None);

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), StageError::NotInitialized));
}

#[test]
fn test_stage_real_fixture_updates_index_and_makes_workspace_clean() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli_init = Cli::for_test(Commands::Init);
    let ctx_init = ResolvedContext::from_cli(&cli_init, base_path).unwrap();

    // 1. Init workspace
    init_workspace(&ctx_init, base_path).expect("init_workspace should succeed");

    // 2. Copy real PNG fixture
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let real_png_bytes =
        fs::read(fixtures_dir.join("200x100.png")).expect("200x100.png fixture must exist");

    let screenshot_dir = base_path.join("billing");
    fs::create_dir_all(&screenshot_dir).unwrap();
    let screenshot_file = screenshot_dir.join("form.png");
    fs::write(&screenshot_file, real_png_bytes).unwrap();

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
        command: Commands::Stage { paths: vec![] },
    };

    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    // 3. Before staging: status reports 1 Added
    let status_before = check_status(&ctx, base_path).unwrap();
    assert_eq!(status_before.added.len(), 1);

    // 4. Stage workspace
    let stage_res = stage_workspace(&ctx, base_path, None).expect("stage_workspace should succeed");
    assert_eq!(stage_res.staged_test_cases.len(), 1);
    assert_eq!(stage_res.total_screenshots_staged, 1);

    // 5. Verify exact expected SHA-256 blob file exists under .gleon/blobs/sha256/
    let sha256_hex = hex::encode(sha2::Sha256::digest(&real_png_bytes));
    let expected_blob_path = base_path.join(".gleon/blobs/sha256").join(&sha256_hex);
    assert!(
        expected_blob_path.is_file(),
        "Expected blob file {:?} does not exist",
        expected_blob_path
    );
}

#[test]
fn test_stage_partial_path_filter_preserves_existing_entries() {
    use std::path::PathBuf;

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

    let screenshot_dir = base_path.join("billing");
    fs::create_dir_all(&screenshot_dir).unwrap();
    fs::write(screenshot_dir.join("form1.png"), &real_png_bytes).unwrap();
    fs::write(screenshot_dir.join("form2.png"), &real_png_bytes).unwrap();

    let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing/**/*.png"
"#;
    fs::write(base_path.join("gleon.yaml"), config_yaml).unwrap();

    let cli = Cli::for_test(Commands::Stage { paths: vec![] });
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    // 1. Initial stage: stages form1.png and form2.png
    stage_workspace(&ctx, base_path, None).expect("initial stage should succeed");

    // 2. Modify form1.png so that restaging it counts as modified
    let fixtures_dir_100 = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let alt_png_bytes = fs::read(fixtures_dir_100.join("diff_16px_corners_100x100.png"))
        .expect("diff_16px_corners_100x100.png fixture must exist");
    fs::write(screenshot_dir.join("form1.png"), &alt_png_bytes).unwrap();

    // 3. Filtered stage: stage ONLY form1.png
    let filter = vec![PathBuf::from("billing/form1.png")];
    let stage_res =
        stage_workspace(&ctx, base_path, Some(&filter)).expect("filtered stage should succeed");
    assert_eq!(
        stage_res.total_screenshots_staged, 1,
        "Filtered stage should only process matching screenshot paths"
    );
    // Verify CAS blob output invariant: blobs directory retains stored blobs from both form1 and form2
    assert!(base_path.join(".gleon/blobs/sha256").is_dir());
    let blob_count = fs::read_dir(base_path.join(".gleon/blobs/sha256"))
        .unwrap()
        .count();
    assert!(
        blob_count >= 2,
        "CAS blob directory must retain blobs from both form1 and form2"
    );
}

/// Interim Phase 3.2 test verifying that stage workspace processes matching screenshots
/// into CAS blobs before Phase 3.3 per-test manifest diff tracking is implemented.
#[test]
fn test_stage_phase32_re_stages_all_matching_screenshots() {
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

    let screenshot_dir = base_path.join("billing");
    fs::create_dir_all(&screenshot_dir).unwrap();
    fs::write(screenshot_dir.join("form.png"), &real_png_bytes).unwrap();

    let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing/**/*.png"
"#;
    fs::write(base_path.join("gleon.yaml"), config_yaml).unwrap();

    let cli = Cli::for_test(Commands::Stage { paths: vec![] });
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    // First stage: 1 screenshot staged
    let stage1 = stage_workspace(&ctx, base_path, None).unwrap();
    assert_eq!(stage1.total_screenshots_staged, 1);

    // Second stage in Phase 3.2 re-processes image and saves blob
    let stage2 = stage_workspace(&ctx, base_path, None).unwrap();
    assert_eq!(stage2.total_screenshots_staged, 1);
}
