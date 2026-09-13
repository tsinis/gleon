#![cfg(not(miri))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::pedantic,
    clippy::nursery,
    missing_docs
)]

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn test_cli_dashboard_end_to_end() {
    let temp = tempdir().unwrap();
    let workspace = temp.path();

    // 1. Uninitialized workspace fails
    let mut cmd_uninit = Command::cargo_bin("gleon").unwrap();
    cmd_uninit
        .current_dir(workspace)
        .arg("dashboard")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Workspace not initialized"));

    // 2. Initialize workspace
    let mut cmd_init = Command::cargo_bin("gleon").unwrap();
    cmd_init
        .current_dir(workspace)
        .arg("init")
        .assert()
        .success();

    // Copy static fixture report
    let runs_dir = workspace.join(".gleon").join("runs").join("latest");
    std::fs::create_dir_all(&runs_dir).unwrap();
    let report_path = runs_dir.join("gleon-report.json");

    let fixture_report = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("gleon-core")
        .join("tests")
        .join("fixtures")
        .join("sample_report.json");
    std::fs::copy(&fixture_report, &report_path).unwrap();

    // 3. Run dashboard command locally and verify stdout has path and stderr has diagnostic
    let mut cmd_dash = Command::cargo_bin("gleon").unwrap();
    cmd_dash
        .current_dir(workspace)
        .arg("dashboard")
        .assert()
        .success()
        .stdout(predicate::str::contains("dashboard.html"))
        .stderr(predicate::str::contains("Dashboard compiled successfully"));

    let history_file = workspace.join(".gleon").join("history.json");
    let dashboard_file = workspace.join(".gleon").join("dashboard.html");
    assert!(history_file.is_file());
    assert!(dashboard_file.is_file());

    let html_content = std::fs::read_to_string(&dashboard_file).unwrap();
    assert!(html_content.contains("Gleon Regression History"));
    assert!(html_content.contains("auth&#x2f;login"));

    // 4. Test --truncate-history 0 fails validation
    let mut cmd_zero = Command::cargo_bin("gleon").unwrap();
    cmd_zero
        .current_dir(workspace)
        .arg("dashboard")
        .arg("--truncate-history")
        .arg("0")
        .assert()
        .failure()
        .stderr(predicate::str::contains("zero"));

    // 5. Test --truncate-history 2 with multiple runs
    for _ in 0..3 {
        let mut cmd = Command::cargo_bin("gleon").unwrap();
        cmd.current_dir(workspace)
            .arg("dashboard")
            .arg("--truncate-history")
            .arg("2")
            .assert()
            .success();
    }

    let history_data: gleon_core::dashboard::DashboardHistory =
        gleon_core::io::load_json(&history_file).unwrap();
    assert_eq!(history_data.runs.len(), 2);

    // 6. Test --push fails fast without storage configuration
    let mut cmd_push_fail = Command::cargo_bin("gleon").unwrap();
    cmd_push_fail
        .current_dir(workspace)
        .env_remove("GLEON_STORAGE_URL")
        .arg("dashboard")
        .arg("--push")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Storage not configured"));

    // 7. Test --push with remote storage
    let remote_dir = temp.path().join("remote_bucket");
    std::fs::create_dir_all(&remote_dir).unwrap();

    let mut cmd_push = Command::cargo_bin("gleon").unwrap();
    cmd_push
        .current_dir(workspace)
        .env(
            "GLEON_STORAGE_URL",
            format!("file://{}", remote_dir.display()),
        )
        .arg("dashboard")
        .arg("--push")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Successfully uploaded history.json and dashboard.html",
        ));

    assert!(remote_dir.join("history.json").is_file());
    assert!(remote_dir.join("dashboard.html").is_file());

    // 8. Test --report pointing to non-existent file fails fast
    let mut cmd_bad_report = Command::cargo_bin("gleon").unwrap();
    cmd_bad_report
        .current_dir(workspace)
        .arg("dashboard")
        .arg("--report")
        .arg("non_existent_report.json")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Report file not found"));

    // 9. Test running dashboard from a nested subdirectory resolves workspace report correctly
    let subfolder = workspace.join("packages").join("app");
    std::fs::create_dir_all(&subfolder).unwrap();

    let mut cmd_sub = Command::cargo_bin("gleon").unwrap();
    cmd_sub
        .current_dir(&subfolder)
        .arg("dashboard")
        .assert()
        .success()
        .stdout(predicate::str::contains("dashboard.html"))
        .stderr(predicate::str::contains("Dashboard compiled successfully"));
}

#[test]
fn test_cli_dashboard_large_history_merge_and_truncate() {
    let temp = tempdir().unwrap();
    let workspace = temp.path();

    let mut cmd_init = Command::cargo_bin("gleon").unwrap();
    cmd_init
        .current_dir(workspace)
        .arg("init")
        .assert()
        .success();

    // Setup history and report
    let runs_dir = workspace.join(".gleon").join("runs").join("latest");
    std::fs::create_dir_all(&runs_dir).unwrap();

    let base_fixtures = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("gleon-core")
        .join("tests")
        .join("fixtures");

    std::fs::copy(
        base_fixtures.join("sample_report.json"),
        runs_dir.join("gleon-report.json"),
    )
    .unwrap();

    let history_file = workspace.join(".gleon").join("history.json");
    std::fs::copy(base_fixtures.join("sample_history.json"), &history_file).unwrap();

    // Verify initial count is 150 from fixture
    let initial_data: gleon_core::dashboard::DashboardHistory =
        gleon_core::io::load_json(&history_file).unwrap();
    assert_eq!(initial_data.runs.len(), 150);

    // Run dashboard with truncate to 50
    let mut cmd = Command::cargo_bin("gleon").unwrap();
    cmd.current_dir(workspace)
        .arg("dashboard")
        .arg("--truncate-history")
        .arg("50")
        .assert()
        .success();

    let truncated_data: gleon_core::dashboard::DashboardHistory =
        gleon_core::io::load_json(&history_file).unwrap();
    // It should have exactly 50 runs: it merged the 150, appended 1 new run (151 total), then truncated down to 50.
    assert_eq!(truncated_data.runs.len(), 50);
}
