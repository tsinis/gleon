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
fn test_invalid_subcommand() -> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = Command::cargo_bin("gleon")?;
    cmd.arg("invalid-command").assert().failure();
    Ok(())
}
