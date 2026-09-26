//! Integration tests for Dashboard Compiler and History Tracker.

#![cfg(not(miri))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::pedantic,
    clippy::nursery,
    missing_docs,
    reason = "test code: panics are assertions, and pedantic/nursery style lints are not enforced in tests"
)]

use std::{num::NonZeroUsize, path::PathBuf};

use gleon_core::{
    context::{ContextOptions, ResolvedContext},
    dashboard::{DashboardCompiler, DashboardError, DashboardHistory, DashboardOptions},
    paths::GleonPaths,
    storage::{ObjectStoreAdapter, StorageConfig},
};

#[tokio::test]
#[cfg(not(miri))]
async fn test_dashboard_compilation_and_history_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let base_dir = temp.path();
    let paths = GleonPaths::new(base_dir);

    // Copy static fixture report with both success and failure results
    let fixture_report = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample_report.json");
    let report_path = base_dir.join("test-report.json");
    std::fs::copy(&fixture_report, &report_path).unwrap();

    let remote_dir = temp.path().join("remote_store");
    std::fs::create_dir_all(&remote_dir).unwrap();
    let storage_cfg = StorageConfig::new(format!("file://{}", remote_dir.display()));

    let options = ContextOptions {
        branch: Some("feature/checkout".to_string()),
        ..Default::default()
    };
    let ctx = ResolvedContext::from_options(&options, base_dir).unwrap();

    // Run 1: Compile and push to remote storage with truncate limit 2
    let dash_opts_1 = DashboardOptions {
        truncate_limit: NonZeroUsize::new(2),
        push_to_storage: true,
        ..Default::default()
    };
    let res1 =
        DashboardCompiler::execute(&paths, &ctx, &report_path, &dash_opts_1, Some(&storage_cfg))
            .await
            .unwrap();

    assert_eq!(res1.total_runs, 1);
    assert!(res1.pushed);
    assert!(paths.history_file().is_file());
    assert!(paths.dashboard_file().is_file());

    // Verify dashboard HTML content and visual assets
    let html1 = std::fs::read_to_string(paths.dashboard_file()).unwrap();
    assert!(html1.contains("<!DOCTYPE html>"));
    assert!(html1.contains("Gleon Regression History"));
    assert!(html1.contains("<svg class=\"svg-chart\""));
    assert!(html1.contains("feature&#x2f;checkout"));
    assert!(html1.contains("auth&#x2f;login_screen"));
    assert!(html1.contains("checkout&#x2f;payment_modal"));
    assert!(html1.contains("Diff pixels: 128"));
    assert!(html1.contains("Run Pass Rate"));
    assert!(html1.contains("Test Pass Rate"));
    assert!(html1.contains("0.0%")); // Run pass rate (0 of 1 runs passed)
    assert!(html1.contains("50.0%")); // Test pass rate (1 of 2 tests passed)
    assert!(html1.contains("1</strong> / 2 passed"));

    // Verify history.json on disk
    let history_json1 = std::fs::read_to_string(paths.history_file()).unwrap();
    let history1 = DashboardHistory::parse_or_empty(&history_json1).unwrap();
    assert_eq!(history1.schema_version, 1);
    assert_eq!(history1.runs.len(), 1);
    assert_eq!(history1.runs[0].summary.total, 2);
    assert_eq!(history1.runs[0].summary.passed, 1);
    assert_eq!(history1.runs[0].summary.failed, 1);

    // Verify remote storage received both assets
    let adapter = ObjectStoreAdapter::from_config(&storage_cfg).unwrap();
    let remote_history_obj = adapter
        .get_object("history.json")
        .await
        .unwrap()
        .expect("remote history.json should exist");
    let remote_html_obj = adapter
        .get_object("dashboard.html")
        .await
        .unwrap()
        .expect("remote dashboard.html should exist");

    assert!(!remote_history_obj.bytes.is_empty());
    assert!(!remote_html_obj.bytes.is_empty());

    // Run 2: Second run on main branch
    let options_main = ContextOptions {
        branch: Some("main".to_string()),
        ..Default::default()
    };
    let ctx_main = ResolvedContext::from_options(&options_main, base_dir).unwrap();
    let dash_opts_2 = DashboardOptions {
        truncate_limit: NonZeroUsize::new(2),
        push_to_storage: true,
        ..Default::default()
    };
    let res2 = DashboardCompiler::execute(
        &paths,
        &ctx_main,
        &report_path,
        &dash_opts_2,
        Some(&storage_cfg),
    )
    .await
    .unwrap();

    assert_eq!(res2.total_runs, 2);

    // Run 3: Third run, which triggers truncation down to 2 runs
    let res3 = DashboardCompiler::execute(
        &paths,
        &ctx_main,
        &report_path,
        &dash_opts_2,
        Some(&storage_cfg),
    )
    .await
    .unwrap();

    assert_eq!(res3.total_runs, 2); // Truncated to 2

    let history_json3 = std::fs::read_to_string(paths.history_file()).unwrap();
    let history3 = DashboardHistory::parse_or_empty(&history_json3).unwrap();
    assert_eq!(history3.runs.len(), 2);
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_dashboard_custom_output_path() {
    let temp = tempfile::tempdir().unwrap();
    let base_dir = temp.path();
    let paths = GleonPaths::new(base_dir);

    let fixture_report = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample_report.json");
    let report_path = base_dir.join("report.json");
    std::fs::copy(&fixture_report, &report_path).unwrap();

    let custom_html_out = base_dir.join("custom_reports").join("dash.html");

    let options_test = ContextOptions {
        branch: Some("test".to_string()),
        ..Default::default()
    };
    let ctx = ResolvedContext::from_options(&options_test, base_dir).unwrap();

    let opts = DashboardOptions {
        out_html: Some(&custom_html_out),
        ..Default::default()
    };

    let res = DashboardCompiler::execute(&paths, &ctx, &report_path, &opts, None)
        .await
        .unwrap();

    assert_eq!(res.html_path, custom_html_out);
    assert!(custom_html_out.is_file());
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_dashboard_push_without_storage_fails_fast() {
    let temp = tempfile::tempdir().unwrap();
    let base_dir = temp.path();
    let paths = GleonPaths::new(base_dir);

    let fixture_report = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sample_report.json");
    let report_path = base_dir.join("report.json");
    std::fs::copy(&fixture_report, &report_path).unwrap();

    let ctx = ResolvedContext::default();
    let opts = DashboardOptions {
        push_to_storage: true,
        ..Default::default()
    };

    let err = DashboardCompiler::execute(&paths, &ctx, &report_path, &opts, None).await;
    assert!(matches!(err, Err(DashboardError::StorageNotConfigured)));
}
