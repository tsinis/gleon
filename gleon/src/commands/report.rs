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
    if let Some(cfg) = &storage_cfg {
        // TODO: Implement true pre-signed URLs (via AmazonS3 Signer or similar)
        // For now, if the configured URL is already HTTP/HTTPS (e.g. a public R2 bucket gateway), we use it.
        // Otherwise, we gracefully fallback to the HTML artifact (Tier C) instead of faking S3 URLs.
        if cfg.url.starts_with("https://") || cfg.url.starts_with("http://") {
            base_image_url = Some(cfg.url.as_str());
        }
    }

    let artifact_env = env.get_var("GLEON_HTML_ARTIFACT_URL");
    let html_artifact_url = artifact_env.as_deref().filter(|s| !s.is_empty());

    let options = MarkdownReportOptions {
        base_image_url,
        html_artifact_url,
    };

    let md_content = ReportGenerator::render_pr_comment(&report_data, &options);

    if let Some(out_path) = out {
        gleon_core::io::save_file_atomically(out_path, md_content.as_bytes())
            .with_context(|| format!("Failed to write output to '{}'", out_path.display()))?;
        tracing::info!("Generated markdown report at {}", out_path.display());
    } else {
        println!("{}", md_content);
    }

    Ok(0)
}
