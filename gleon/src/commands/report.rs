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
                    if let Some(path_str) = p.to_str() {
                        let path_buf = p.to_path_buf();
                        let path_string = path_str.to_string();
                        let adapter = adapter.clone();
                        let sem = semaphore.clone();
                        join_set.spawn(async move {
                            let _permit = sem.acquire_owned().await.expect("Semaphore closed");
                            adapter
                                .sign_blob_url(&path_string, expires_in)
                                .await
                                .map(|signed| (path_buf, signed))
                        });
                    }
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

    let has_signed_urls = !signed_urls.is_empty();
    let resolver = |p: &std::path::Path| signed_urls.get(p).cloned();
    let options = MarkdownReportOptions {
        base_image_url,
        html_artifact_url,
        image_url_resolver: if has_signed_urls {
            Some(&resolver)
        } else {
            None
        },
    };

    let md_content = ReportGenerator::render_pr_comment(&report_data, &options);

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
        gleon_core::io::save_file_atomically(out_path, md_content.as_bytes())
            .with_context(|| format!("Failed to write output to '{}'", out_path.display()))?;
        tracing::info!("Generated markdown report at {}", out_path.display());
    } else {
        println!("{}", md_content);
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
}
