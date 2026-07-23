#![cfg(not(miri))]

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

fn init_temp_dir() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::cargo_bin("gleon").unwrap();
    cmd.current_dir(dir.path()).arg("init").assert().success();
    dir
}

#[test]
fn test_help() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Universal visual regression testing CLI",
        ));
    Ok(())
}

#[test]
fn test_no_arguments_shows_help() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.assert()
        .failure() // clap exits with 2 when required subcommand is missing
        .stderr(predicates::str::contains("Usage:"))
        .stderr(predicates::str::contains("Commands:"));
    Ok(())
}

#[test]
fn test_version() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::contains("gleon"));
    Ok(())
}

#[test]
fn test_init_command() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicates::str::contains("Initialized gleon workspace"));

    assert!(dir.path().join(".gleon").is_dir());
    assert!(dir.path().join("gleon.yaml").is_file());
    Ok(())
}

#[test]
fn test_status_linux_chrome() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_config = manifest_dir.join("tests/fixtures/platform/linux-chrome.yaml");

    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .arg("--config")
        .arg(&fixture_config)
        .arg("status")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Nothing to report. Workspace is up to date.",
        ));
    Ok(())
}

#[test]
fn test_status_macos_opaque() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_config = manifest_dir.join("tests/fixtures/platform/macos-opaque.yaml");

    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .arg("--config")
        .arg(&fixture_config)
        .arg("status")
        .assert()
        .success();
    Ok(())
}

#[test]
fn test_status_minimal_with_overrides() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_config = manifest_dir.join("tests/fixtures/platform/minimal.yaml");

    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .arg("--config")
        .arg(&fixture_config)
        .arg("--os")
        .arg("windows")
        .arg("--arch")
        .arg("x86_64")
        .arg("status")
        .assert()
        .success();
    Ok(())
}

#[test]
fn test_status_opaque_conflict_error() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_config = manifest_dir.join("tests/fixtures/platform/macos-opaque.yaml");

    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .arg("--config")
        .arg(&fixture_config)
        .arg("--os")
        .arg("linux")
        .arg("status")
        .assert()
        .failure()
        .stderr(predicates::str::contains("opaque platform configuration"));
    Ok(())
}

#[test]
fn test_status_invalid_segment_error() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .arg("--os")
        .arg("mac os")
        .arg("status")
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "Invalid character or pattern in platform segment",
        ));
    Ok(())
}

#[test]
fn test_status_reserved_label_key_error() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .arg("--label")
        .arg("os=linux")
        .arg("status")
        .assert()
        .failure()
        .stderr(predicates::str::contains("Label key 'os' is reserved"));
    Ok(())
}

#[test]
fn test_stage_command() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .arg("stage")
        .assert()
        .success()
        .stdout(predicates::str::contains("Already up to date."));
    Ok(())
}

#[test]
fn test_diff_command() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .arg("diff")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Ran 0 test(s). Passed: 0, Failed: 0.",
        ));
    Ok(())
}

#[test]
fn test_merge_placeholder() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("merge")
        .arg("test-branch")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Subcommand merge for branch 'test-branch' is not fully implemented yet",
        ));
    Ok(())
}

#[test]
fn test_test_placeholder() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("test")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Subcommand test is not fully implemented yet",
        ));
    Ok(())
}

#[test]
fn test_pull_placeholder() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("pull")
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "No storage configured via GLEON_STORAGE_URL. Nothing to pull.",
        ));
    Ok(())
}

#[test]
fn test_push_placeholder() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("push")
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "No storage configured via GLEON_STORAGE_URL. Nothing to push.",
        ));
    Ok(())
}

#[test]
fn test_gc_placeholder() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("gc")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Subcommand gc is not fully implemented yet",
        ));
    Ok(())
}

#[test]
fn test_invalid_subcommand() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("invalid-command").assert().failure();
    Ok(())
}

#[test]
fn test_verbose_flag_coverage() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_config = manifest_dir.join("tests/fixtures/platform/minimal.yaml");

    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .arg("-v")
        .arg("--config")
        .arg(&fixture_config)
        .arg("status")
        .assert()
        .success()
        .stderr(predicates::str::contains("INFO"))
        .stderr(predicates::str::contains("gleon CLI starting up..."));
    Ok(())
}

#[test]
fn test_quiet_flag_coverage() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_config = manifest_dir.join("tests/fixtures/platform/minimal.yaml");

    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .arg("-q")
        .arg("--config")
        .arg(&fixture_config)
        .arg("status")
        .assert()
        .success()
        .stderr(predicates::str::contains("gleon CLI starting up...").not());
    Ok(())
}

#[test]
fn test_conflicting_verbose_and_quiet() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("-v").arg("-q").arg("status").assert().failure();
    Ok(())
}

#[test]
fn test_status_with_env_vars() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .env("GLEON_OS", "linux")
        .env("GLEON_ARCH", "x86_64")
        .env("GLEON_RENDERER", "firefox")
        .env("GLEON_PLATFORM", "os=linux,arch=x86_64,renderer=firefox")
        .arg("status")
        .assert()
        .success();
    Ok(())
}

#[test]
fn test_status_cli_platform_success() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .arg("--platform")
        .arg("custom-opaque")
        .arg("status")
        .assert()
        .success();
    Ok(())
}

#[test]
fn test_status_cli_platform_conflict() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .arg("--platform")
        .arg("custom-opaque")
        .arg("--arch")
        .arg("x86_64")
        .arg("status")
        .assert()
        .failure()
        .stderr(predicates::str::contains("structured overrides"));
    Ok(())
}

#[test]
fn test_status_cli_platform_conflict_with_env_platform() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.current_dir(dir.path())
        .env("GLEON_PLATFORM", "os=linux,arch=x86_64")
        .arg("--platform")
        .arg("custom-opaque")
        .arg("status")
        .assert()
        .failure()
        .stderr(predicates::str::contains("opaque platform configuration"));
    Ok(())
}

#[test]
fn test_cli_diff_exit_code_on_match_and_mismatch() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let core_fixtures = manifest_dir
        .parent()
        .ok_or("No parent dir")?
        .join("gleon-core/tests/fixtures");

    // 1. gleon init
    let mut cmd_init = Command::cargo_bin("gleon")?;
    cmd_init
        .current_dir(dir.path())
        .arg("init")
        .assert()
        .success();

    // 2. Add fixture image and gleon.yaml
    let img_200 = std::fs::read(core_fixtures.join("200x100.png"))?;
    let img_100 = std::fs::read(core_fixtures.join("diff_16px_corners_100x100.png"))?;

    let billing_dir = dir.path().join("billing");
    std::fs::create_dir_all(&billing_dir)?;
    std::fs::write(billing_dir.join("form.png"), &img_200)?;

    let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing/**/*.png"
"#;
    std::fs::write(dir.path().join("gleon.yaml"), config_yaml)?;

    // 3. gleon stage
    let mut cmd_stage = Command::cargo_bin("gleon")?;
    cmd_stage
        .current_dir(dir.path())
        .arg("stage")
        .assert()
        .success();

    // 4. gleon diff -> exit code 0 (match)
    let mut cmd_diff_match = Command::cargo_bin("gleon")?;
    cmd_diff_match
        .current_dir(dir.path())
        .arg("diff")
        .assert()
        .code(0);

    // 5. Overwrite screenshot with different image
    std::fs::write(billing_dir.join("form.png"), &img_100)?;

    // 6. gleon diff -> exit code 1 (mismatch)
    let mut cmd_diff_mismatch = Command::cargo_bin("gleon")?;
    cmd_diff_mismatch
        .current_dir(dir.path())
        .arg("diff")
        .assert()
        .code(1);

    Ok(())
}

#[test]
fn test_stage_already_up_to_date_message() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let core_fixtures = manifest_dir
        .parent()
        .ok_or("No parent dir")?
        .join("gleon-core/tests/fixtures");

    let mut cmd_init = Command::cargo_bin("gleon")?;
    cmd_init
        .current_dir(dir.path())
        .arg("init")
        .assert()
        .success();

    let img_200 = std::fs::read(core_fixtures.join("200x100.png"))?;
    let billing_dir = dir.path().join("billing");
    std::fs::create_dir_all(&billing_dir)?;
    std::fs::write(billing_dir.join("form.png"), &img_200)?;

    let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing/**/*.png"
"#;
    std::fs::write(dir.path().join("gleon.yaml"), config_yaml)?;

    // First stage
    let mut cmd_stage1 = Command::cargo_bin("gleon")?;
    cmd_stage1
        .current_dir(dir.path())
        .arg("stage")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Staged 1 screenshot(s) across 1 test case(s).",
        ));

    // Second stage (no-op) -> asserts "Already up to date."
    let mut cmd_stage2 = Command::cargo_bin("gleon")?;
    cmd_stage2
        .current_dir(dir.path())
        .arg("stage")
        .assert()
        .success()
        .stdout(predicates::str::contains("Already up to date."));

    Ok(())
}

#[test]
fn test_pull_and_push_no_storage_configured() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();

    // Pull without GLEON_STORAGE_URL
    let mut cmd_pull = Command::cargo_bin("gleon")?;
    cmd_pull
        .current_dir(dir.path())
        .env_remove("GLEON_STORAGE_URL")
        .arg("pull")
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "No storage configured via GLEON_STORAGE_URL. Nothing to pull.",
        ));

    // Push without GLEON_STORAGE_URL
    let mut cmd_push = Command::cargo_bin("gleon")?;
    cmd_push
        .current_dir(dir.path())
        .env_remove("GLEON_STORAGE_URL")
        .arg("push")
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "No storage configured via GLEON_STORAGE_URL. Nothing to push.",
        ));

    // Diff --auto-pull without GLEON_STORAGE_URL
    let mut cmd_diff = Command::cargo_bin("gleon")?;
    cmd_diff
        .current_dir(dir.path())
        .env_remove("GLEON_STORAGE_URL")
        .arg("diff")
        .arg("--auto-pull")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "No storage configured via GLEON_STORAGE_URL. Skipping auto-pull.",
        ));

    Ok(())
}

#[test]
fn test_pull_and_push_with_file_storage_url() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();
    let remote_dir = tempfile::tempdir()?;
    let remote_url = url::Url::from_directory_path(remote_dir.path())
        .unwrap()
        .to_string();

    // 1. Stage a screenshot first
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let core_fixtures = manifest_dir
        .parent()
        .ok_or("No parent dir")?
        .join("gleon-core/tests/fixtures");

    let img_200 = std::fs::read(core_fixtures.join("200x100.png"))?;
    let billing_dir = dir.path().join("billing");
    std::fs::create_dir_all(&billing_dir)?;
    std::fs::write(billing_dir.join("form.png"), &img_200)?;

    let config_yaml = r#"
required_version: ">=0.1.0"
screenshots:
  - include: "billing/**/*.png"
"#;
    std::fs::write(dir.path().join("gleon.yaml"), config_yaml)?;

    let mut cmd_stage = Command::cargo_bin("gleon")?;
    cmd_stage
        .current_dir(dir.path())
        .arg("stage")
        .assert()
        .success();

    // 2. Push with storage URL
    let mut cmd_push = Command::cargo_bin("gleon")?;
    cmd_push
        .current_dir(dir.path())
        .env("GLEON_STORAGE_URL", &remote_url)
        .arg("push")
        .assert()
        .success()
        .stdout(predicates::str::contains("Push complete."));

    // 3. Pull in clean workspace
    let fresh_dir = tempfile::tempdir()?;
    let mut cmd_init2 = Command::cargo_bin("gleon")?;
    cmd_init2
        .current_dir(fresh_dir.path())
        .arg("init")
        .assert()
        .success();

    let mut cmd_pull = Command::cargo_bin("gleon")?;
    cmd_pull
        .current_dir(fresh_dir.path())
        .env("GLEON_STORAGE_URL", &remote_url)
        .arg("pull")
        .assert()
        .success()
        .stdout(predicates::str::contains("Pull complete."));

    Ok(())
}

#[test]
fn test_unimplemented_subcommands() -> Result<(), Box<dyn std::error::Error>> {
    let dir = init_temp_dir();

    let mut cmd_test = Command::cargo_bin("gleon")?;
    cmd_test
        .current_dir(dir.path())
        .arg("test")
        .assert()
        .success()
        .stdout(predicates::str::contains("not fully implemented yet"));

    let mut cmd_merge = Command::cargo_bin("gleon")?;
    cmd_merge
        .current_dir(dir.path())
        .arg("merge")
        .arg("main")
        .assert()
        .success()
        .stdout(predicates::str::contains("not fully implemented yet"));

    let mut cmd_gc = Command::cargo_bin("gleon")?;
    cmd_gc
        .current_dir(dir.path())
        .arg("gc")
        .assert()
        .success()
        .stdout(predicates::str::contains("not fully implemented yet"));

    Ok(())
}
