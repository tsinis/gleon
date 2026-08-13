use anyhow::{Context, Result, anyhow};
use gleon_core::io::load_json;
use gleon_core::report::{MarkdownReportOptions, ReportGenerator};
use gleon_core::scanner::TestCaseResult;

pub async fn run_report(
    env: &dyn gleon_core::git::EnvProvider,
    storage_cfg: Option<gleon_core::storage::StorageConfig>,
    format: &str,
    report_path: &std::path::Path,
    pr_number: Option<u64>,
    out: Option<&std::path::Path>,
) -> Result<i32> {
    if let Some(pr) = pr_number {
        if pr == 0 {
            return Err(anyhow!("PR number must be greater than 0"));
        }
        tracing::info!("Report target PR: #{}", pr);
    }

    tracing::debug!("Generating report in '{}' format", format);

    let report_data: Vec<TestCaseResult> = load_json(report_path).with_context(|| {
        format!(
            "Failed to parse report JSON from '{}'",
            report_path.display()
        )
    })?;

    let mut base_image_url = None;
    let mut signed_urls = std::collections::HashMap::new();

    if let Some(cfg) = &storage_cfg {
        if cfg.url.starts_with("https://") || cfg.url.starts_with("http://") {
            base_image_url = Some(cfg.url.as_str());
        }

        if let Ok(adapter) = gleon_core::storage::ObjectStoreAdapter::from_config(cfg) {
            let expires_in = std::time::Duration::from_secs(7 * 24 * 3600);

            let failed_tests: Vec<_> = report_data.iter().filter(|tc| !tc.passed()).collect();
            let limit = ReportGenerator::MAX_MARKDOWN_DIFF_ROWS;
            let to_sign: Vec<_> = failed_tests.into_iter().take(limit).collect();

            let mut join_set = tokio::task::JoinSet::new();
            let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(adapter.concurrency()));

            for tc in to_sign {
                let paths: Vec<&std::path::Path> = match &tc.result {
                    gleon_core::scanner::TestImageResult::Mismatch {
                        baseline_path,
                        actual_path,
                        diff_path,
                        ..
                    } => {
                        vec![
                            baseline_path.as_path(),
                            actual_path.as_path(),
                            diff_path.as_path(),
                        ]
                    }
                    gleon_core::scanner::TestImageResult::DimensionMismatch {
                        baseline_path,
                        actual_path,
                        ..
                    } => {
                        vec![baseline_path.as_path(), actual_path.as_path()]
                    }
                    gleon_core::scanner::TestImageResult::EncodeError { actual_path, .. } => {
                        vec![actual_path.as_path()]
                    }
                    gleon_core::scanner::TestImageResult::MissingBaseline {
                        relative_path, ..
                    }
                    | gleon_core::scanner::TestImageResult::DecodeError { relative_path, .. }
                    | gleon_core::scanner::TestImageResult::IoError { relative_path, .. } => {
                        vec![relative_path.as_path()]
                    }
                    _ => vec![],
                };

                for p in paths {
                    let normalized_key =
                        gleon_core::scanner::FileScanner::normalize_path_str(p).to_string();
                    let path_buf = p.to_path_buf();
                    let adapter = adapter.clone();
                    let sem = semaphore.clone();
                    join_set.spawn(async move {
                        let _permit = sem.acquire_owned().await.expect("Semaphore closed");
                        adapter
                            .sign_blob_url(&normalized_key, expires_in)
                            .await
                            .map(|signed| (path_buf, signed))
                    });
                }
            }

            while let Some(res) = join_set.join_next().await {
                match res {
                    Ok(Some((p, signed))) => {
                        let _ = signed_urls.insert(p, signed);
                    }
                    Ok(None) => {
                        tracing::warn!("Failed to generate pre-signed URL for blob path");
                    }
                    Err(e) => {
                        tracing::warn!("URL signing task panicked or was cancelled: {}", e);
                    }
                }
            }
        }
    }

    let artifact_env = env.get_var("GLEON_HTML_ARTIFACT_URL");
    let html_artifact_url = artifact_env.as_deref().filter(|s| !s.is_empty());

    let is_ci = env.get_var("GITHUB_ACTIONS").is_some() || pr_number.is_some();
    let context = if is_ci {
        gleon_core::report::ExecutionContext::GitHubActions
    } else {
        gleon_core::report::ExecutionContext::LocalTerminal
    };

    let has_signed_urls = !signed_urls.is_empty();
    let resolver = |p: &std::path::Path| signed_urls.get(p).cloned();
    let options = MarkdownReportOptions {
        context,
        base_image_url,
        html_artifact_url,
        image_url_resolver: if has_signed_urls {
            Some(&resolver)
        } else {
            None
        },
    };

    let report_content =
        if format.eq_ignore_ascii_case("markdown") || format.eq_ignore_ascii_case("comment") {
            ReportGenerator::render_pr_comment(&report_data, &options)
        } else if format.eq_ignore_ascii_case("html") {
            let report_dir = out.and_then(|p| p.parent());
            ReportGenerator::generate_html(&report_data, report_dir)
                .with_context(|| "Failed to generate HTML report")?
                .unwrap_or_else(|| "<html><body>All tests passed!</body></html>".to_string())
        } else if format.eq_ignore_ascii_case("junit")
            || format.eq_ignore_ascii_case("junit.xml")
            || format.eq_ignore_ascii_case("xml")
        {
            // TODO: support options in generate_junit_xml
            ReportGenerator::generate_junit_xml(&report_data)
                .with_context(|| "Failed to generate JUnit XML report")?
        } else if format.eq_ignore_ascii_case("json") {
            // TODO: support options in json
            serde_json::to_string_pretty(&report_data)
                .with_context(|| "Failed to serialize report to JSON")?
        } else {
            return Err(anyhow!("Unsupported report format: '{}'", format));
        };

    if let Some(out_path) = out {
        let parent = out_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create parent directory for report output '{}'",
                parent.display()
            )
        })?;
        gleon_core::io::save_file_atomically(out_path, report_content.as_bytes())
            .with_context(|| format!("Failed to write output to '{}'", out_path.display()))?;
        tracing::info!("Generated {} report at {}", format, out_path.display());
    } else {
        println!("{}", report_content);
    }

    Ok(0)
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_report_creates_nested_parent_dir() {
        let temp = tempfile::tempdir().unwrap();
        let report_json_path = temp.path().join("report.json");
        std::fs::write(&report_json_path, "[]").unwrap();

        let nested_out = temp.path().join("nested").join("sub").join("output.md");

        struct DummyEnv;
        impl gleon_core::git::EnvProvider for DummyEnv {
            fn get_var(&self, _key: &str) -> Option<String> {
                None
            }
        }

        let res = run_report(
            &DummyEnv,
            None,
            "markdown",
            &report_json_path,
            None,
            Some(&nested_out),
        )
        .await;

        assert!(res.is_ok());
        assert!(nested_out.is_file());
    }

    #[tokio::test]
    async fn test_run_report_with_encode_error() {
        let temp = tempfile::tempdir().unwrap();
        let report_json_path = temp.path().join("report.json");
        let tc = TestCaseResult {
            name: "test_enc".to_string(),
            result: gleon_core::scanner::TestImageResult::EncodeError {
                relative_path: std::path::PathBuf::from("enc.png"),
                actual_path: std::path::PathBuf::from("actual_enc.png"),
                error: "Encode failure".to_string(),
            },
        };
        gleon_core::io::save_json_atomically(&report_json_path, &vec![tc]).unwrap();

        let out_path = temp.path().join("output.md");

        struct DummyEnv;
        impl gleon_core::git::EnvProvider for DummyEnv {
            fn get_var(&self, _key: &str) -> Option<String> {
                None
            }
        }

        let storage_cfg = gleon_core::storage::StorageConfig::new("https://signed.com");
        let res = run_report(
            &DummyEnv,
            Some(storage_cfg),
            "markdown",
            &report_json_path,
            None,
            Some(&out_path),
        )
        .await;

        assert!(res.is_ok());
        let md = std::fs::read_to_string(&out_path).unwrap();
        assert!(md.contains("Encode Error"));
        assert!(md.contains("actual_enc.png"));
    }

    #[tokio::test]
    async fn test_run_report_formats() {
        let temp = tempfile::tempdir().unwrap();
        let report_json_path = temp.path().join("report.json");
        let tc = TestCaseResult {
            name: "test_fmt".to_string(),
            result: gleon_core::scanner::TestImageResult::EncodeError {
                relative_path: std::path::PathBuf::from("enc.png"),
                actual_path: std::path::PathBuf::from("actual_enc.png"),
                error: "Encode failure".to_string(),
            },
        };
        gleon_core::io::save_json_atomically(&report_json_path, &vec![tc]).unwrap();

        struct DummyEnv;
        impl gleon_core::git::EnvProvider for DummyEnv {
            fn get_var(&self, _key: &str) -> Option<String> {
                None
            }
        }

        // Test JUnit format
        let junit_out = temp.path().join("output.xml");
        let res_junit = run_report(
            &DummyEnv,
            None,
            "junit",
            &report_json_path,
            None,
            Some(&junit_out),
        )
        .await;
        assert!(res_junit.is_ok());
        let xml = std::fs::read_to_string(&junit_out).unwrap();
        assert!(xml.contains("<testsuites") || xml.contains("<testsuite"));

        // Test HTML format
        let html_out = temp.path().join("output.html");
        let res_html = run_report(
            &DummyEnv,
            None,
            "html",
            &report_json_path,
            None,
            Some(&html_out),
        )
        .await;
        assert!(res_html.is_ok());
        let html = std::fs::read_to_string(&html_out).unwrap();
        assert!(html.contains("<!DOCTYPE html>") || html.contains("<html"));

        // Test unsupported format
        let res_unsupported = run_report(
            &DummyEnv,
            None,
            "invalid_fmt",
            &report_json_path,
            None,
            None,
        )
        .await;
        assert!(res_unsupported.is_err());
    }

    #[tokio::test]
    async fn test_run_report_invalid_json_and_html_custom_dir() {
        let temp = tempfile::tempdir().unwrap();
        let corrupt_report_path = temp.path().join("corrupt.json");
        std::fs::write(&corrupt_report_path, "not json data").unwrap();

        struct DummyEnv;
        impl gleon_core::git::EnvProvider for DummyEnv {
            fn get_var(&self, _key: &str) -> Option<String> {
                None
            }
        }

        // Corrupt report JSON returns error
        let res_err = run_report(
            &DummyEnv,
            None,
            "markdown",
            &corrupt_report_path,
            None,
            None,
        )
        .await;
        assert!(res_err.is_err());

        // Test HTML format written to custom nested directory
        let valid_report = temp.path().join("valid.json");
        let tc = TestCaseResult {
            name: "test_html_dir".to_string(),
            result: gleon_core::scanner::TestImageResult::MissingBaseline {
                relative_path: std::path::PathBuf::from("sub/missing.png"),
                reason: "no baseline".to_string(),
            },
        };
        gleon_core::io::save_json_atomically(&valid_report, &vec![tc]).unwrap();

        let nested_html_out = temp.path().join("nested").join("dir").join("report.html");
        let res_html = run_report(
            &DummyEnv,
            None,
            "html",
            &valid_report,
            None,
            Some(&nested_html_out),
        )
        .await;
        assert!(res_html.is_ok());
        assert!(nested_html_out.exists());
    }
}
