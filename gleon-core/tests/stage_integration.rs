#![cfg(not(miri))]

use gleon_core::cli::{Cli, Commands};
use gleon_core::context::ResolvedContext;
use gleon_core::manifest::ManifestIndex;
use gleon_core::ops::{StageError, check_status, init_workspace, stage_workspace};
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

    // 5. Verify manifest_index.json exists and is valid
    let platform_key = ctx.platform.to_key().unwrap();
    let index_path = base_path
        .join(".gleon/branches/main")
        .join(&platform_key)
        .join("manifest_index.json");
    assert!(index_path.is_file());

    let index = ManifestIndex::load(&index_path).expect("manifest_index.json should be valid");
    assert!(index.test_manifests.contains_key("billing"));

    // 6. After staging: status reports CLEAN!
    let status_after = check_status(&ctx, base_path).unwrap();
    assert!(
        status_after.is_clean(),
        "Workspace should be clean after staging"
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

    // 4. Load manifest from index and verify BOTH form1.png AND form2.png remain in manifest entries
    let platform_key = ctx.platform.to_key().unwrap();
    let index_path = base_path
        .join(".gleon/branches/main")
        .join(&platform_key)
        .join("manifest_index.json");

    let index = ManifestIndex::load(&index_path).unwrap();
    let manifest_hash = index
        .test_manifests
        .get("billing")
        .expect("billing manifest must exist");

    let manifest_path = base_path
        .join(".gleon/blobs")
        .join(manifest_hash.scheme())
        .join(manifest_hash.value());

    let manifest =
        gleon_core::manifest::Manifest::load(manifest_path).expect("manifest should load");

    assert!(
        manifest.entries.contains_key("billing/form1.png"),
        "form1.png should exist in manifest"
    );
    assert!(
        manifest.entries.contains_key("billing/form2.png"),
        "form2.png MUST NOT be deleted when staging form1.png partially"
    );
}

#[test]
fn test_stage_noop_when_unchanged() {
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

    // Second stage without modifying files: 0 screenshots staged (no-op!)
    let stage2 = stage_workspace(&ctx, base_path, None).unwrap();
    assert_eq!(stage2.total_screenshots_staged, 0);
}
