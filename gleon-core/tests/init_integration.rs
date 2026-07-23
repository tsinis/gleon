#![cfg(not(miri))]

use gleon_core::cli::{Cli, Commands};
use gleon_core::config::GleonConfig;
use gleon_core::context::ResolvedContext;
use gleon_core::ops::init_workspace;
use std::fs;

#[test]
fn test_init_workspace_creates_real_structure_and_valid_config() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli = Cli::for_test(Commands::Init);
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    let result = init_workspace(&ctx, base_path).expect("init_workspace should succeed");

    assert_eq!(result.gleon_dir, base_path.join(".gleon"));
    let config_path = result.config_created.expect("gleon.yaml should be created");
    assert_eq!(config_path, base_path.join("gleon.yaml"));

    // Verify directory structure exists on disk
    assert!(base_path.join(".gleon/blobs/sha256").is_dir());
    assert!(base_path.join(".gleon/branches").is_dir());
    assert!(base_path.join(".gleon/runs/latest").is_dir());
    assert!(config_path.is_file());

    // Verify created config is a valid GleonConfig that can be parsed
    let loaded_config =
        GleonConfig::load_from_file(&config_path).expect("Created config should be valid YAML");
    assert_eq!(loaded_config, GleonConfig::default());
}

#[test]
fn test_init_workspace_idempotent_preserves_custom_config() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let config_path = base_path.join("gleon.yaml");
    let custom_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "custom/**/*.png"
"#;
    fs::write(&config_path, custom_yaml).unwrap();

    let cli = Cli::for_test(Commands::Init);
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    let result = init_workspace(&ctx, base_path).expect("Second init should succeed");

    assert_eq!(result.config_created, None);
    let loaded_config = GleonConfig::load_from_file(&config_path).unwrap();
    assert_eq!(
        loaded_config.screenshots[0].include[0].as_str(),
        "custom/**/*.png"
    );
}

#[test]
fn test_init_workspace_honors_cli_overrides() {
    let temp_dir = tempfile::tempdir().unwrap();
    let base_path = temp_dir.path();

    let cli = Cli {
        branch: Some("feature/login".to_string()),
        os: Some("custom-os".to_string()),
        arch: Some("custom-arch".to_string()),
        renderer: None,
        labels: vec![("theme".to_string(), "dark".to_string())],
        platform: None,
        verbose: false,
        quiet: false,
        config: None,
        target_branch: "main".to_string(),
        command: Commands::Init,
    };
    let ctx = ResolvedContext::from_cli(&cli, base_path).unwrap();

    init_workspace(&ctx, base_path).expect("init_workspace should succeed");

    let platform_key = ctx.platform.to_key().unwrap();
    assert_eq!(platform_key, "9:custom-os-11:custom-arch-5:theme=4:dark");

    let index_path = base_path
        .join(".gleon/branches/feature/login")
        .join(&platform_key)
        .join("manifest_index.json");

    assert!(
        index_path.is_file(),
        "manifest_index.json must be created under correct branch/platform"
    );
}
