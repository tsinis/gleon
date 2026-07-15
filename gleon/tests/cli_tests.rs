#![cfg(not(miri))]

use assert_cmd::Command;

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
fn test_version() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::contains("gleon"));
    Ok(())
}

#[test]
fn test_status_placeholder() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("status")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Subcommand status is not fully implemented yet",
        ));
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
        .arg("status")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Subcommand status is not fully implemented yet",
        ));
    Ok(())
}

#[test]
fn test_quiet_flag_coverage() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("-q")
        .arg("status")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Subcommand status is not fully implemented yet",
        ));
    Ok(())
}
