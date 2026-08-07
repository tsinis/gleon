use anyhow::{Context, Result, anyhow};
use gleon_core::context::ResolvedContext;
use gleon_core::io::load_json;
use gleon_core::report::{MarkdownReportOptions, ReportGenerator};
use gleon_core::scanner::TestCaseResult;

pub async fn run_report(
    _ctx: &ResolvedContext,
    env: &dyn gleon_core::git::EnvProvider,
    storage_cfg: Option<gleon_core::storage::StorageConfig>,
    format: &str,
    report_path: &str,
    _pr_number: Option<u64>,
    out: Option<&str>,
) -> Result<i32> {
    if format != "markdown" {
        return Err(anyhow!(
            "Unsupported report format: {}. Only 'markdown' is currently supported.",
            format
        ));
    }

    let report_data: Vec<TestCaseResult> = load_json(report_path)
        .with_context(|| format!("Failed to parse report JSON from '{}'", report_path))?;

    let mut base_image_url = None;
    if let Some(cfg) = &storage_cfg {
        // TODO: Implement true Presigned URLs (via AmazonS3 Signer or similar)
        // For now, if the configured URL is already HTTP/HTTPS (e.g. a public R2 bucket gateway), we use it.
        // Otherwise, we gracefully fallback to the HTML artifact (Tier C) instead of faking S3 URLs.
        if cfg.url.starts_with("http") {
            base_image_url = Some(cfg.url.as_str());
        }
    }

    let mut html_artifact_url = None;
    let artifact_url_env = env.get_var("GLEON_HTML_ARTIFACT_URL").unwrap_or_default();
    if !artifact_url_env.is_empty() {
        html_artifact_url = Some(artifact_url_env.as_str());
    }

    let options = MarkdownReportOptions {
        base_image_url,
        html_artifact_url,
    };

    let md_content = ReportGenerator::render_pr_comment(&report_data, &options);

    if let Some(out_path) = out {
        gleon_core::io::save_file_atomically(std::path::Path::new(out_path), md_content.as_bytes())
            .with_context(|| format!("Failed to write output to '{}'", out_path))?;
        tracing::info!("Generated markdown report at {}", out_path);
    } else {
        println!("{}", md_content);
    }

    Ok(0)
}
