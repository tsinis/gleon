//! Integration tests for `gleon resolve`.

#![cfg(not(miri))]

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn test_resolve_non_interactive_fails() {
    let temp = tempdir().unwrap();
    let base_dir = temp.path();

    // Initialize workspace
    let mut cmd = Command::cargo_bin("gleon").unwrap();
    cmd.current_dir(base_dir).arg("init").assert().success();

    // Write conflicted manifest
    let manifest_dir = base_dir
        .join(".gleon")
        .join("manifests")
        .join("macos-aarch64")
        .join("auth");
    std::fs::create_dir_all(&manifest_dir).unwrap();

    std::fs::copy(
        "../gleon-core/tests/fixtures/conflict_2way.json",
        manifest_dir.join("login.json"),
    )
    .unwrap();

    // Run gleon resolve in non-interactive subprocess (should exit with error code 1)
    let mut resolve_cmd = Command::cargo_bin("gleon").unwrap();
    resolve_cmd
        .current_dir(base_dir)
        .arg("resolve")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("Terminal is non-interactive"));
}

#[test]
fn test_resolve_no_conflicts() {
    let temp = tempdir().unwrap();
    let base_dir = temp.path();

    // Initialize workspace
    let mut cmd = Command::cargo_bin("gleon").unwrap();
    cmd.current_dir(base_dir).arg("init").assert().success();

    let mut resolve_cmd = Command::cargo_bin("gleon").unwrap();
    resolve_cmd
        .current_dir(base_dir)
        .arg("resolve")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "No conflicted manifest files found.",
        ));
}
