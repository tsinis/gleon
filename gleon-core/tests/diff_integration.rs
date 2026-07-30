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
        command: Commands::Diff {
            auto_pull: false,
            resolve: false,
        },
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
        command: Commands::Diff {
            auto_pull: false,
            resolve: false,
        },
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

    // 6. Run diff against modified image -> should report failure
    let report_mismatch = run_diff(&ctx, base_path).expect("run_diff should succeed");
    assert!(!report_mismatch.passed);
    assert_eq!(report_mismatch.failed_tests, 1);

    // 7. Verify generated report artifacts on disk
    let runs_dir = base_path.join(".gleon/runs/latest");
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

    let cli = Cli::for_test(Commands::Diff {
        auto_pull: false,
        resolve: false,
    });
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

    let cli = Cli::for_test(Commands::Diff {
        auto_pull: false,
        resolve: false,
    });
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    // Stage baseline
    stage_workspace(&ctx, base_path, None).expect("stage_workspace should succeed");

    // Explicitly verify backslash-to-forward-slash path key normalization
    let backslash_path = Path::new("billing\\form.png");
    let normalized = gleon_core::scanner::FileScanner::normalize_path_str(backslash_path);
    assert_eq!(normalized, "billing/form.png");

    // Run diff -> should handle backslash manifest keys cross-platform!
    let report = run_diff(&ctx, base_path).expect("run_diff should handle backslash manifest keys");
    assert!(report.passed);
    assert_eq!(report.total_tests, 1);
    assert_eq!(report.failed_tests, 0);
}

#[test]
fn test_diff_missing_baseline_returns_missing_baseline() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli_init = Cli::for_test(Commands::Init);
    let ctx_init = ResolvedContext::from_cli(&cli_init, base_path).unwrap();
    init_workspace(&ctx_init, base_path).expect("init_workspace should succeed");

    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let baseline_png_bytes = fs::read(fixtures_dir.join("200x100.png")).unwrap();

    let screenshot_dir = base_path.join("billing");
    fs::create_dir_all(&screenshot_dir).unwrap();
    fs::write(screenshot_dir.join("unstaged.png"), &baseline_png_bytes).unwrap();

    let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing/**/*.png"
"#;
    fs::write(base_path.join("gleon.yaml"), config_yaml).unwrap();

    let cli = Cli::for_test(Commands::Diff {
        auto_pull: false,
        resolve: false,
    });
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    // Do NOT stage unstaged.png
    let report = run_diff(&ctx, base_path).expect("run_diff should run");
    assert!(!report.passed);
    assert_eq!(report.total_tests, 1);
    assert_eq!(report.failed_tests, 1);

    let md = fs::read_to_string(report.runs_dir.join("report.md")).unwrap();
    assert!(md.contains("Missing Baseline"));
}

#[test]
fn test_diff_missing_blob_file_and_corrupt_images() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli_init = Cli::for_test(Commands::Init);
    let ctx_init = ResolvedContext::from_cli(&cli_init, base_path).unwrap();
    init_workspace(&ctx_init, base_path).expect("init_workspace should succeed");

    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let baseline_png_bytes = fs::read(fixtures_dir.join("baseline_100x100.png")).unwrap();

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

    let cli = Cli::for_test(Commands::Diff {
        auto_pull: false,
        resolve: false,
    });
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    // Stage baseline
    stage_workspace(&ctx, base_path, None).expect("stage_workspace should succeed");

    // 1. Remove blob file manually from .gleon/blobs/sha256
    let blobs_dir = base_path.join(".gleon/blobs/sha256");
    for entry in fs::read_dir(&blobs_dir).unwrap() {
        let path = entry.unwrap().path();
        let _ = fs::remove_file(path);
    }

    let report_missing_blob = run_diff(&ctx, base_path).unwrap();
    assert!(!report_missing_blob.passed);
    let md_missing = fs::read_to_string(report_missing_blob.runs_dir.join("report.md")).unwrap();
    assert!(md_missing.contains("Missing Baseline"));

    // 2. Write corrupt baseline blob file back
    let mut blob_digest = String::new();
    for entry in fs::read_dir(base_path.join(".gleon/manifests")).unwrap() {
        // Find platform dir
        let p_dir = entry.unwrap().path();
        if p_dir.is_dir() {
            let manifest_path = p_dir.join("billing/form.json");
            if manifest_path.is_file() {
                let manifest_json = fs::read_to_string(&manifest_path).unwrap();
                if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&manifest_json) {
                    let hash_str = manifest["hash"].as_str().unwrap();
                    blob_digest = hash_str.split_once(':').unwrap().1.to_string();
                    fs::write(blobs_dir.join(&blob_digest), b"not a png").unwrap();
                }
            }
        }
    }

    let report_corrupt_blob = run_diff(&ctx, base_path).unwrap();
    assert!(!report_corrupt_blob.passed);
    let md_corrupt_blob =
        fs::read_to_string(report_corrupt_blob.runs_dir.join("report.md")).unwrap();
    assert!(md_corrupt_blob.to_lowercase().contains("decode"));

    // Restore valid baseline blob so run_diff decodes the baseline and tests corrupt actual screenshot
    fs::write(blobs_dir.join(&blob_digest), &baseline_png_bytes).unwrap();

    // 3. Write corrupt actual screenshot file
    fs::write(&screenshot_file, b"not a png").unwrap();
    let report_corrupt_actual = run_diff(&ctx, base_path).unwrap();
    assert!(!report_corrupt_actual.passed);
    let md_corrupt_actual =
        fs::read_to_string(report_corrupt_actual.runs_dir.join("report.md")).unwrap();
    assert!(md_corrupt_actual.to_lowercase().contains("decode"));
}

#[test]
fn test_diff_with_mask_rules_ignores_masked_differences() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli_init = Cli::for_test(Commands::Init);
    let ctx_init = ResolvedContext::from_cli(&cli_init, base_path).unwrap();
    init_workspace(&ctx_init, base_path).expect("init_workspace should succeed");

    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let baseline_png_bytes = fs::read(fixtures_dir.join("baseline_gradient_100x100.png")).unwrap();

    let screenshot_dir = base_path.join("masked_app");
    fs::create_dir_all(&screenshot_dir).unwrap();
    let screenshot_file = screenshot_dir.join("screen.png");
    fs::write(&screenshot_file, &baseline_png_bytes).unwrap();

    // Config mask covering pixel (50, 50)
    let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "masked_app/**/*.png"
    masks:
      - path: "**/*.png"
        zones:
          - x: 50
            y: 50
            width: 1
            height: 1
"#;
    fs::write(base_path.join("gleon.yaml"), config_yaml).unwrap();

    let cli = Cli::for_test(Commands::Diff {
        auto_pull: false,
        resolve: false,
    });
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    // 1. Stage baseline (applies mask to baseline blob and saves it)
    stage_workspace(&ctx, base_path, None).expect("stage_workspace should succeed");

    // 2. Replace actual screenshot with image modified ONLY at (50, 50)
    let modified_png_bytes =
        fs::read(fixtures_dir.join("diff_1px_black_center_100x100.png")).unwrap();
    fs::write(&screenshot_file, &modified_png_bytes).unwrap();

    // 3. Run diff -> Mask on actual screenshot masks out the modified pixel (50, 50),
    // baseline is already masked. Comparison must PASS!
    let report = run_diff(&ctx, base_path).expect("run_diff should succeed");
    assert!(report.passed);
    assert_eq!(report.total_tests, 1);
    assert_eq!(report.failed_tests, 0);
}

#[test]
fn test_diff_baseline_staged_before_mask_configuration() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli_init = Cli::for_test(Commands::Init);
    let ctx_init = ResolvedContext::from_cli(&cli_init, base_path).unwrap();
    init_workspace(&ctx_init, base_path).expect("init_workspace should succeed");

    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let baseline_png_bytes = fs::read(fixtures_dir.join("baseline_gradient_100x100.png")).unwrap();

    let screenshot_dir = base_path.join("unmasked_app");
    fs::create_dir_all(&screenshot_dir).unwrap();
    let screenshot_file = screenshot_dir.join("screen.png");
    fs::write(&screenshot_file, &baseline_png_bytes).unwrap();

    // 1. Initial config WITHOUT masks
    let initial_config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "unmasked_app/**/*.png"
"#;
    fs::write(base_path.join("gleon.yaml"), initial_config_yaml).unwrap();

    let cli = Cli::for_test(Commands::Diff {
        auto_pull: false,
        resolve: false,
    });
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    // 2. Stage baseline BEFORE configuring mask (blob on disk is UNMASKED)
    stage_workspace(&ctx, base_path, None).expect("stage_workspace should succeed");

    // 3. Update config AFTER staging to ADD mask covering pixel (50, 50)
    let masked_config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "unmasked_app/**/*.png"
    masks:
      - path: "**/*.png"
        zones:
          - x: 50
            y: 50
            width: 1
            height: 1
"#;
    fs::write(base_path.join("gleon.yaml"), masked_config_yaml).unwrap();

    // 4. Modify actual screenshot at (50, 50)
    let modified_png_bytes =
        fs::read(fixtures_dir.join("diff_1px_black_center_100x100.png")).unwrap();
    fs::write(&screenshot_file, &modified_png_bytes).unwrap();

    // Re-resolve context with updated config
    let ctx_masked = ResolvedContext::from_cli(&cli, base_path).unwrap();

    // 5. Run diff -> baseline blob on disk was unmasked, but run_diff applies the new mask
    // to BOTH baseline_rgba and actual_rgba on the fly. Comparison MUST PASS!
    let report = run_diff(&ctx_masked, base_path).expect("run_diff should succeed");
    assert!(report.passed);
    assert_eq!(report.total_tests, 1);
    assert_eq!(report.failed_tests, 0);
}
