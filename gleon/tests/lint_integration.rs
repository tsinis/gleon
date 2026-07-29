//! Integration tests for `gleon lint-manifests`.

#![cfg(not(miri))]

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn test_lint_manifests_clean_pass() {
    let temp = tempdir().unwrap();
    let base_dir = temp.path();

    // Initialize workspace
    let mut cmd = Command::cargo_bin("gleon").unwrap();
    cmd.current_dir(base_dir).arg("init").assert().success();

    // Write valid manifest
    let manifest_dir = base_dir
        .join(".gleon")
        .join("manifests")
        .join("macos-aarch64")
        .join("auth");
    std::fs::create_dir_all(&manifest_dir).unwrap();

    std::fs::copy(
        "../gleon-core/tests/fixtures/valid_manifest.json",
        manifest_dir.join("login.json"),
    )
    .unwrap();

    // Run gleon lint-manifests
    let mut lint_cmd = Command::cargo_bin("gleon").unwrap();
    lint_cmd
        .current_dir(base_dir)
        .arg("lint-manifests")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "All manifest files passed linting.",
        ));
}

#[test]
fn test_lint_manifests_fails_on_conflict_markers() {
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

    // Run gleon lint-manifests (should fail with exit code 1)
    let mut lint_cmd = Command::cargo_bin("gleon").unwrap();
    lint_cmd
        .current_dir(base_dir)
        .arg("lint-manifests")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("Git merge conflict markers"));
}
