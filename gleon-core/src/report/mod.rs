//! Generates HTML, `JUnit` XML, and Markdown reports from test results.
//!
//! Split by output format: [`html`] (`report.html`), [`xml`] (`junit.xml`), and [`markdown`]
//! (PR-comment/summary Markdown), plus [`presign`] (signing remote storage URLs for a Markdown
//! PR comment's images), sharing the path-formatting helpers in [`format`] and the [`JINJA_ENV`]
//! template registry defined here. Each submodule contributes its generator methods via its own
//! `impl ReportGenerator` block.

mod format;
mod html;
mod markdown;
mod presign;
mod xml;

use crate::results::TestCaseResult;
use std::sync::LazyLock;

static JINJA_ENV: LazyLock<minijinja::Environment<'static>> = LazyLock::new(|| {
    let mut env = minijinja::Environment::new();
    // Bundled templates are compiled into the binary and validated by the test
    // suite; a syntax error here would be a build-time bug caught immediately,
    // not a runtime condition callers need to handle.
    #[allow(clippy::expect_used)]
    env.add_template("report.html", include_str!("../templates/report.html"))
        .expect("bundled report.html template is valid minijinja syntax");
    #[allow(clippy::expect_used)]
    env.add_template("junit.xml", include_str!("../templates/junit.xml"))
        .expect("bundled junit.xml template is valid minijinja syntax");
    #[allow(clippy::expect_used)]
    env.add_template("pr_comment.md", include_str!("../templates/pr_comment.md"))
        .expect("bundled pr_comment.md template is valid minijinja syntax");
    env
});

/// Errors that can occur during report generation.
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    /// Template rendering failed.
    #[error("Template rendering failed for '{template}'")]
    Render {
        /// Name of the template that failed.
        template: &'static str,
        /// The underlying minijinja error message.
        #[source]
        source: minijinja::Error,
    },

    /// Error deserializing JSON.
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<crate::io::IoError> for ReportError {
    fn from(err: crate::io::IoError) -> Self {
        match err {
            crate::io::IoError::Io(e) => Self::Io(e),
            crate::io::IoError::JsonParse(e) => Self::JsonParse(e),
        }
    }
}

/// Resolves a screenshot path to a signed/absolute URL for embedding in a PR comment.
///
/// Returns `None` to fall back to `base_image_url`-relative linking (or `N/A` if
/// that is also unset).
pub type ImageUrlResolver<'a> = dyn Fn(&std::path::Path) -> Option<String> + Sync + 'a;

/// Where the PR comment is being rendered, used to pick an appropriate footer.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderTarget {
    /// Rendered for a local terminal invocation (e.g. `gleon diff`).
    #[default]
    LocalTerminal,
    /// Rendered for a GitHub Actions workflow run.
    GitHubActions,
}

/// Options controlling how [`ReportGenerator::render_pr_comment`] links images and
/// which footer it appends.
#[derive(Default)]
pub struct MarkdownReportOptions<'a> {
    /// Base URL prepended to relative image paths when no `image_url_resolver` applies.
    pub base_image_url: Option<&'a str>,
    /// URL of the full HTML report artifact, linked when rows are truncated.
    pub html_artifact_url: Option<&'a str>,
    /// Optional per-path resolver for signed/absolute image URLs, tried before `base_image_url`.
    pub image_url_resolver: Option<&'a ImageUrlResolver<'a>>,
    /// Where the comment is being rendered, selecting the footer text.
    pub context: RenderTarget,
}

/// Generates HTML, `JUnit` XML, and Markdown reports from test results.
pub struct ReportGenerator;

impl ReportGenerator {
    /// Footer appended to PR comments rendered for GitHub Actions.
    pub const FOOTER_GITHUB_ACTIONS: &'static str = "\n---\n*Reply with `/gleon approve` to update baseline images for this PR (see repository README for workflow setup instructions).*\n";
    /// Footer appended to PR comments rendered for a local terminal.
    pub const FOOTER_LOCAL_TERMINAL: &'static str =
        "\n---\n*Run `gleon approve` to accept failed screenshots as new baselines locally.*\n";

    /// Generates markdown, `JUnit` XML, HTML, and JSON report files inside `runs_dir`.
    ///
    /// # Errors
    ///
    /// Returns `ReportError` if any of the underlying report generation steps
    /// fail (template rendering) or if writing a report file to `runs_dir`
    /// fails (I/O or JSON serialization).
    pub fn generate_all(
        runs_dir: &std::path::Path,
        test_cases: &[TestCaseResult],
    ) -> Result<(), ReportError> {
        let md = Self::generate_markdown(test_cases);
        let md_path = runs_dir.join("report.md");
        crate::io::save_file_atomically(&md_path, md.as_bytes())?;

        let xml = Self::generate_junit_xml(test_cases)?;
        let xml_path = runs_dir.join("junit.xml");
        crate::io::save_file_atomically(&xml_path, xml.as_bytes())?;

        if let Some(html) = Self::generate_html(test_cases, Some(runs_dir))? {
            let html_path = runs_dir.join("report.html");
            crate::io::save_file_atomically(&html_path, html.as_bytes())?;
        }

        let json_path = runs_dir.join("gleon-report.json");
        crate::io::save_json_atomically(&json_path, test_cases).map_err(ReportError::from)?;

        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::pedantic,
    clippy::nursery
)]
mod tests {
    use super::*;

    #[test]
    fn test_report_error_from_io_error() {
        let io_err = crate::io::IoError::Io(std::io::Error::other("test io"));
        let report_err: ReportError = io_err.into();
        assert!(matches!(report_err, ReportError::Io(_)));

        let json_err: serde_json::Error = serde_json::from_str::<String>("invalid").unwrap_err();
        let io_json_err = crate::io::IoError::JsonParse(json_err);
        let report_json_err: ReportError = io_json_err.into();
        assert!(matches!(report_json_err, ReportError::JsonParse(_)));

        assert_eq!(report_err.to_string(), "IO error: test io");
    }
}
