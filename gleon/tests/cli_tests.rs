#![cfg(not(miri))]

use assert_cmd::Command;
use predicates::prelude::*;

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
fn test_status_linux_chrome() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("--config")
        .arg("tests/fixtures/platform/linux-chrome.yaml")
        .arg("status")
        .assert()
        .success()
        .stderr(predicates::str::contains("Platform resolved successfully"))
        .stdout(predicates::str::contains(
            "Key: 5:linux-6:x86_64-6:chrome-6:locale=5:en_us-5:theme=4:dark",
        ))
        .stdout(predicates::str::contains("OS: linux"));
    Ok(())
}

#[test]
fn test_status_macos_opaque() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("--config")
        .arg("tests/fixtures/platform/macos-opaque.yaml")
        .arg("status")
        .assert()
        .success()
        .stderr(predicates::str::contains("Platform resolved successfully"))
        .stdout(predicates::str::contains("Key: 13:macos-aarch64"));
    Ok(())
}

#[test]
fn test_status_minimal_with_overrides() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("--config")
        .arg("tests/fixtures/platform/minimal.yaml")
        .arg("--os")
        .arg("windows")
        .arg("--arch")
        .arg("x86_64")
        .arg("status")
        .assert()
        .success()
        .stderr(predicates::str::contains("Platform resolved successfully"))
        .stdout(predicates::str::contains("Key: 7:windows-6:x86_64"));
    Ok(())
}

#[test]
fn test_status_opaque_conflict_error() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("--config")
        .arg("tests/fixtures/platform/macos-opaque.yaml")
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
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("--config")
        .arg("tests/fixtures/platform/minimal.yaml")
        .arg("--platform")
        .arg("mac os")
        .arg("status")
        .assert()
        .failure()
        .stderr(predicates::str::contains("contains invalid characters"));
    Ok(())
}

#[test]
fn test_status_reserved_label_key_error() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("--config")
        .arg("tests/fixtures/platform/minimal.yaml")
        .arg("--label")
        .arg("os=linux")
        .arg("status")
        .assert()
        .failure()
        .stderr(predicates::str::contains("is reserved"));
    Ok(())
}

#[test]
fn test_status_missing_config_error() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("--config")
        .arg("non_existent_config_file.yaml")
        .arg("status")
        .assert()
        .failure()
        .stderr(predicates::str::contains("Configuration file not found"));
    Ok(())
}

#[test]
fn test_stage_placeholder() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("stage")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Subcommand stage is not fully implemented yet",
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
fn test_diff_placeholder() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("diff")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Subcommand diff is not fully implemented yet",
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
        .success()
        .stdout(predicates::str::contains(
            "Subcommand pull is not fully implemented yet",
        ));
    Ok(())
}

#[test]
fn test_push_placeholder() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("push")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Subcommand push is not fully implemented yet",
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
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("-v")
        .arg("--config")
        .arg("tests/fixtures/platform/minimal.yaml")
        .arg("status")
        .assert()
        .success()
        .stderr(predicates::str::contains("Platform resolved successfully"))
        .stderr(predicates::str::contains("INFO"))
        .stderr(predicates::str::contains("Gleon CLI starting up..."));
    Ok(())
}

#[test]
fn test_quiet_flag_coverage() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("-q")
        .arg("--config")
        .arg("tests/fixtures/platform/minimal.yaml")
        .arg("status")
        .assert()
        .success()
        .stderr(predicates::str::contains("Platform resolved successfully").not())
        .stderr(predicates::str::contains("Gleon CLI starting up...").not());
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
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.env("GLEON_OS", "linux")
        .env("GLEON_ARCH", "x86_64")
        .env("GLEON_RENDERER", "firefox")
        .env("GLEON_PLATFORM", "os=linux,arch=x86_64,renderer=firefox")
        .arg("status")
        .assert()
        .success()
        .stdout(predicates::str::contains("OS: linux"))
        .stdout(predicates::str::contains("Architecture: x86_64"))
        .stdout(predicates::str::contains("Renderer: firefox"))
        .stdout(predicates::str::contains("Key: 5:linux-6:x86_64-7:firefox"));
    Ok(())
}

#[test]
fn test_status_cli_platform_success() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("--platform")
        .arg("custom-opaque")
        .arg("status")
        .assert()
        .success()
        .stdout(predicates::str::contains("Key: 13:custom-opaque"))
        .stdout(predicates::str::contains("OS: custom-opaque"));
    Ok(())
}

#[test]
fn test_status_cli_platform_conflict() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("--platform")
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
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.env("GLEON_PLATFORM", "os=linux,arch=x86_64")
        .arg("--platform")
        .arg("custom-opaque")
        .arg("status")
        .assert()
        .failure()
        .stderr(predicates::str::contains("opaque platform configuration"));
    Ok(())
}

#[test]
fn test_status_with_real_git_repo() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("status")
        .assert()
        .success()
        .stderr(predicates::str::contains("Platform resolved successfully"))
        .stdout(predicates::str::contains("Branch:"))
        .stdout(predicates::str::contains("Key:"));
    Ok(())
}
