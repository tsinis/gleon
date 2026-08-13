#![cfg(not(miri))]

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn test_cli_report_supports_all_formats() {
    let temp = tempdir().unwrap();
    let report_path = temp.path().join("report.json");

    let test_results = vec![gleon_core::scanner::TestCaseResult {
        name: "auth/login".to_string(),
        result: gleon_core::scanner::TestImageResult::MissingBaseline {
            relative_path: std::path::PathBuf::from("auth/login.png"),
            reason: "No baseline found".to_string(),
        },
    }];
    gleon_core::io::save_json_atomically(&report_path, &test_results).unwrap();

    // Test markdown format
    let mut cmd_md = Command::cargo_bin("gleon").unwrap();
    cmd_md
        .arg("report")
        .arg("markdown")
        .arg("--report")
        .arg(&report_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("Gleon Visual Regression Failure"))
        .stdout(predicate::str::contains("auth/login"));

    // Test html format with output file
    let html_out = temp.path().join("report.html");
    let mut cmd_html = Command::cargo_bin("gleon").unwrap();
    cmd_html
        .arg("report")
        .arg("html")
        .arg("--report")
        .arg(&report_path)
        .arg("-o")
        .arg(&html_out)
        .assert()
        .success();
    assert!(html_out.exists());

    // Test junit format
    let mut cmd_junit = Command::cargo_bin("gleon").unwrap();
    cmd_junit
        .arg("report")
        .arg("junit")
        .arg("--report")
        .arg(&report_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("<testsuite"))
        .stdout(predicate::str::contains("auth&#x2f;login"));

    // Test json format
    let mut cmd_json = Command::cargo_bin("gleon").unwrap();
    cmd_json
        .arg("report")
        .arg("json")
        .arg("--report")
        .arg(&report_path)
        .assert()
        .success()
        .stdout(predicate::str::contains("auth/login"));
}
