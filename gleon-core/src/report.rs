use crate::engine::MismatchDetail;
use crate::scanner::{TestCaseResult, TestImageResult};
use minijinja::{Environment, context};
use serde::{
    Serialize, Serializer,
    ser::{SerializeSeq, SerializeStruct},
};
use std::sync::LazyLock;

static JINJA_ENV: LazyLock<Environment<'static>> = LazyLock::new(|| {
    let mut env = Environment::new();
    env.add_template("report.html", include_str!("templates/report.html"))
        .unwrap();
    env.add_template("junit.xml", include_str!("templates/junit.xml"))
        .unwrap();
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
}

/// Computes a relative path from `base` to `target`.
/// Precondition: `target` and `base` must share the same coordinate frame (both absolute or both relative).
/// If one path is absolute and the other is relative, returns `target` unchanged.
/// For example, if `target` is `.gleon/diffs/image.png` and `base` is `.gleon/reports`,
/// the result is `../diffs/image.png`.
pub fn make_relative_path(target: &std::path::Path, base: &std::path::Path) -> std::path::PathBuf {
    use std::path::{Component, PathBuf};

    if target.is_absolute() != base.is_absolute() {
        return target.to_path_buf();
    }

    let mut target_comps = target
        .components()
        .filter(|c| !matches!(c, Component::CurDir));
    let mut base_comps = base
        .components()
        .filter(|c| !matches!(c, Component::CurDir));

    if let (Some(Component::Prefix(p1)), Some(Component::Prefix(p2))) =
        (target_comps.clone().next(), base_comps.clone().next())
        && p1 != p2
    {
        return target.to_path_buf();
    }

    let mut target_comp = target_comps.next();
    let mut base_comp = base_comps.next();

    while let (Some(t), Some(b)) = (target_comp, base_comp) {
        if t == b {
            target_comp = target_comps.next();
            base_comp = base_comps.next();
        } else {
            break;
        }
    }

    let mut rel = PathBuf::new();

    if base_comp.is_some() {
        rel.push("..");
        for _ in base_comps {
            rel.push("..");
        }
    }

    if let Some(t) = target_comp {
        rel.push(t);
        for comp in target_comps {
            rel.push(comp);
        }
    }

    if rel.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        rel
    }
}

pub struct ReportGenerator;

impl ReportGenerator {}

// Zero-copy serialization wrapper for formatting a single path
struct FormattedPath<'a> {
    path: &'a std::path::Path,
    report_dir: Option<&'a std::path::Path>,
}

impl<'a> std::fmt::Display for FormattedPath<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use std::path::Component;
        let path_to_format = match self.report_dir {
            Some(base) => std::borrow::Cow::Owned(make_relative_path(self.path, base)),
            None => std::borrow::Cow::Borrowed(self.path),
        };

        let mut first = true;
        let mut last_was_slash = false;
        let mut has_output = false;

        for comp in path_to_format.components() {
            if !first && !last_was_slash {
                write!(f, "/")?;
            }
            first = false;
            match comp {
                Component::Normal(os_str) => {
                    write!(f, "{}", os_str.to_string_lossy())?;
                    last_was_slash = false;
                    has_output = true;
                }
                Component::ParentDir => {
                    write!(f, "..")?;
                    last_was_slash = false;
                    has_output = true;
                }
                Component::CurDir => {
                    write!(f, ".")?;
                    last_was_slash = false;
                    has_output = true;
                }
                Component::RootDir => {
                    write!(f, "/")?;
                    last_was_slash = true;
                    has_output = true;
                }
                Component::Prefix(prefix) => {
                    write!(f, "{}", prefix.as_os_str().to_string_lossy())?;
                    last_was_slash = false;
                    has_output = true;
                }
            }
        }
        if !has_output {
            write!(f, ".")?;
        }
        Ok(())
    }
}

impl<'a> Serialize for FormattedPath<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

// Lazy view for failures in HTML
struct HtmlFailureView<'a> {
    tc_name: &'a str,
    res: &'a TestImageResult,
    report_dir: Option<&'a std::path::Path>,
}

impl<'a> Serialize for HtmlFailureView<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HtmlFailureContext", 10)?;
        state.serialize_field("name", self.tc_name)?;

        match self.res {
            TestImageResult::Success { .. } => unreachable!(),
            TestImageResult::DecodeError {
                relative_path,
                error,
            } => {
                state.serialize_field(
                    "image",
                    &FormattedPath {
                        path: relative_path,
                        report_dir: None,
                    },
                )?;
                state.serialize_field("type", "DecodeError")?;
                state.serialize_field("error", error)?;
                state.serialize_field("actual_path", &None::<String>)?;
                state.serialize_field("baseline_path", &None::<String>)?;
                state.serialize_field("diff_path", &None::<String>)?;
                state.serialize_field("diff_count", &None::<u64>)?;
                state.serialize_field("actual_size", &None::<String>)?;
                state.serialize_field("baseline_size", &None::<String>)?;
            }
            TestImageResult::DimensionMismatch {
                relative_path,
                baseline_size,
                actual_size,
                baseline_path,
                actual_path,
            } => {
                state.serialize_field(
                    "image",
                    &FormattedPath {
                        path: relative_path,
                        report_dir: None,
                    },
                )?;
                state.serialize_field("type", "DimensionMismatch")?;
                state.serialize_field("error", "Dimension mismatch")?;
                state.serialize_field(
                    "actual_path",
                    &FormattedPath {
                        path: actual_path,
                        report_dir: self.report_dir,
                    },
                )?;
                state.serialize_field(
                    "baseline_path",
                    &FormattedPath {
                        path: baseline_path,
                        report_dir: self.report_dir,
                    },
                )?;
                state.serialize_field("diff_path", &None::<String>)?;
                state.serialize_field("diff_count", &None::<u64>)?;
                state.serialize_field(
                    "actual_size",
                    &format!("{}x{}", actual_size.0, actual_size.1),
                )?;
                state.serialize_field(
                    "baseline_size",
                    &format!("{}x{}", baseline_size.0, baseline_size.1),
                )?;
            }
            TestImageResult::Mismatch {
                relative_path,
                detail,
                diff_path,
                baseline_path,
                actual_path,
            } => {
                state.serialize_field(
                    "image",
                    &FormattedPath {
                        path: relative_path,
                        report_dir: None,
                    },
                )?;
                state.serialize_field("type", "Mismatch")?;

                let (error_msg, diff_count) = match detail {
                    MismatchDetail::Pixel { diff_count } => (
                        format!("Visual mismatch ({} pixels)", diff_count),
                        Some(*diff_count),
                    ),
                    MismatchDetail::Ssim { ssim_score } => {
                        (format!("Visual mismatch (SSIM: {:.4})", ssim_score), None)
                    }
                    MismatchDetail::SsimFallback { diff_count } => (
                        format!("Visual mismatch (SSIM Fallback: {} pixels)", diff_count),
                        Some(*diff_count),
                    ),
                };

                state.serialize_field("error", &error_msg)?;
                state.serialize_field(
                    "actual_path",
                    &FormattedPath {
                        path: actual_path,
                        report_dir: self.report_dir,
                    },
                )?;
                state.serialize_field(
                    "baseline_path",
                    &FormattedPath {
                        path: baseline_path,
                        report_dir: self.report_dir,
                    },
                )?;
                state.serialize_field(
                    "diff_path",
                    &FormattedPath {
                        path: diff_path,
                        report_dir: self.report_dir,
                    },
                )?;
                state.serialize_field("diff_count", &diff_count)?;
                state.serialize_field("actual_size", &None::<String>)?;
                state.serialize_field("baseline_size", &None::<String>)?;
            }
        }
        state.end()
    }
}

struct HtmlReportFailuresView<'a> {
    test_cases: &'a [TestCaseResult],
    report_dir: Option<&'a std::path::Path>,
}

impl<'a> Serialize for HtmlReportFailuresView<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(None)?;
        for tc in self.test_cases {
            for res in &tc.results {
                if !matches!(res, TestImageResult::Success { .. }) {
                    seq.serialize_element(&HtmlFailureView {
                        tc_name: &tc.name,
                        res,
                        report_dir: self.report_dir,
                    })?;
                }
            }
        }
        seq.end()
    }
}

// Lazy view for XML image result
struct XmlTestImageResultView<'a>(&'a TestImageResult);

impl<'a> Serialize for XmlTestImageResultView<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("XmlTestImageResult", 3)?;
        state.serialize_field(
            "name",
            &FormattedPath {
                path: self.0.relative_path(),
                report_dir: None,
            },
        )?;

        match self.0 {
            TestImageResult::Success { .. } => {
                state.serialize_field("status", "Success")?;
                state.serialize_field("failure_message", &None::<String>)?;
            }
            TestImageResult::DecodeError { error, .. } => {
                state.serialize_field("status", "DecodeError")?;
                state.serialize_field(
                    "failure_message",
                    &Some(format!("Decode error: {}", error)),
                )?;
            }
            TestImageResult::DimensionMismatch {
                baseline_size,
                actual_size,
                ..
            } => {
                state.serialize_field("status", "DimensionMismatch")?;
                state.serialize_field(
                    "failure_message",
                    &Some(format!(
                        "Dimension mismatch (Baseline: {}x{}, Actual: {}x{})",
                        baseline_size.0, baseline_size.1, actual_size.0, actual_size.1
                    )),
                )?;
            }
            TestImageResult::Mismatch { detail, .. } => {
                state.serialize_field("status", "Mismatch")?;
                let msg = match detail {
                    MismatchDetail::Pixel { diff_count } => {
                        format!("Visual mismatch detected ({} pixels)", diff_count)
                    }
                    MismatchDetail::Ssim { ssim_score } => {
                        format!("Visual mismatch detected (SSIM score: {:.4})", ssim_score)
                    }
                    MismatchDetail::SsimFallback { diff_count } => format!(
                        "Visual mismatch detected (SSIM Fallback: {} pixels)",
                        diff_count
                    ),
                };
                state.serialize_field("failure_message", &Some(msg))?;
            }
        }
        state.end()
    }
}

// Lazy view for XML Test Case
struct XmlTestCaseView<'a>(&'a TestCaseResult);

impl<'a> Serialize for XmlTestCaseView<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("XmlTestCase", 3)?;
        state.serialize_field("name", &self.0.name)?;

        struct ResultsSeq<'a>(&'a [TestImageResult]);
        impl<'a> Serialize for ResultsSeq<'a> {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
                for res in self.0 {
                    seq.serialize_element(&XmlTestImageResultView(res))?;
                }
                seq.end()
            }
        }

        state.serialize_field("results", &ResultsSeq(&self.0.results))?;

        let failures = self
            .0
            .results
            .iter()
            .filter(|r| !matches!(r, TestImageResult::Success { .. }))
            .count();
        state.serialize_field("failures", &failures)?;

        state.end()
    }
}

// Lazy view for all test cases
struct XmlTestCasesView<'a>(&'a [TestCaseResult]);

impl<'a> Serialize for XmlTestCasesView<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for tc in self.0 {
            seq.serialize_element(&XmlTestCaseView(tc))?;
        }
        seq.end()
    }
}

impl ReportGenerator {
    /// Generates a single self-contained HTML report string linking images via relative paths.
    /// Skips generation entirely if 100% of tests passed by returning None.
    pub fn generate_html(
        test_cases: &[TestCaseResult],
        report_dir: Option<&std::path::Path>,
    ) -> Result<Option<String>, ReportError> {
        let mut total_tests = 0;
        let mut failed_tests = 0;

        for tc in test_cases {
            for res in &tc.results {
                total_tests += 1;
                if !matches!(res, TestImageResult::Success { .. }) {
                    failed_tests += 1;
                }
            }
        }

        if failed_tests == 0 {
            return Ok(None);
        }

        let tmpl = JINJA_ENV.get_template("report.html").unwrap();

        let ctx = context! {
            total_tests => total_tests,
            failed_tests => failed_tests,
            failures => HtmlReportFailuresView { test_cases, report_dir },
        };

        tmpl.render(ctx).map(Some).map_err(|e| ReportError::Render {
            template: "report.html",
            source: e,
        })
    }

    /// Generates raw junit.xml file bytes mapping failures and decode/dimension errors to <failure> nodes.
    pub fn generate_junit_xml(test_cases: &[TestCaseResult]) -> Result<String, ReportError> {
        let mut total_tests = 0;
        let mut failed_tests = 0;

        for tc in test_cases {
            for res in &tc.results {
                total_tests += 1;
                if !matches!(res, TestImageResult::Success { .. }) {
                    failed_tests += 1;
                }
            }
        }

        let tmpl = JINJA_ENV.get_template("junit.xml").unwrap();

        let ctx = context! {
            total_tests => total_tests,
            failed_tests => failed_tests,
            test_cases => XmlTestCasesView(test_cases),
        };

        tmpl.render(ctx).map_err(|e| ReportError::Render {
            template: "junit.xml",
            source: e,
        })
    }

    /// Generates a simple Markdown report summary string.
    pub fn generate_markdown(test_cases: &[TestCaseResult]) -> String {
        use std::fmt::Write;

        let mut total = 0;
        let mut failed = 0;
        let mut table = String::from("| Test Case | Screenshot | Status |\n|---|---|---|\n");

        for tc in test_cases {
            for res in &tc.results {
                total += 1;
                let status = match res {
                    TestImageResult::Success { .. } => "✅ Pass",
                    TestImageResult::DecodeError { .. } => {
                        failed += 1;
                        "❌ Decode Error"
                    }
                    TestImageResult::DimensionMismatch { .. } => {
                        failed += 1;
                        "❌ Dimension Mismatch"
                    }
                    TestImageResult::Mismatch { .. } => {
                        failed += 1;
                        "❌ Mismatch"
                    }
                };

                let sanitize_cell = |s: &str| -> String {
                    s.replace('\\', "\\\\")
                        .replace('|', "\\|")
                        .replace('\n', " ")
                        .replace('\r', "")
                };
                let safe_name = sanitize_cell(&tc.name);
                let path_str = FormattedPath {
                    path: res.relative_path(),
                    report_dir: None,
                }
                .to_string();
                let safe_path = sanitize_cell(&path_str);
                writeln!(table, "| {} | {} | {} |", safe_name, safe_path, status)
                    .expect("fmt::Write on String is infallible");
            }
        }

        format!(
            "# Gleon Visual Regression Summary\n\n**Total Tests:** {}\n**Failed:** {}\n\n{}",
            total, failed, table
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_make_relative_path() {
        let target = PathBuf::from(".gleon/diffs/billing/form.png");
        let base = PathBuf::from(".gleon/reports");
        let rel = make_relative_path(&target, &base);
        assert_eq!(rel, PathBuf::from("../diffs/billing/form.png"));
    }

    #[test]
    fn test_make_relative_path_with_dots_and_curdir() {
        let target = PathBuf::from("./.gleon/diffs/billing/form.png");
        let base = PathBuf::from(".gleon/reports");
        let rel = make_relative_path(&target, &base);
        assert_eq!(rel, PathBuf::from("../diffs/billing/form.png"));
    }

    #[test]
    fn test_generate_html_skips_on_success() {
        let tc = TestCaseResult {
            name: "billing".to_string(),
            results: vec![TestImageResult::Success {
                relative_path: PathBuf::from("form.png"),
            }],
        };
        assert!(
            ReportGenerator::generate_html(&[tc], None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_generate_html_on_failure() {
        let tc = TestCaseResult {
            name: "billing".to_string(),
            results: vec![TestImageResult::Mismatch {
                relative_path: PathBuf::from("form.png"),
                detail: MismatchDetail::Pixel { diff_count: 5 },
                diff_path: PathBuf::from(".gleon/diffs/diff.png"),
                baseline_path: PathBuf::from("baseline.png"),
                actual_path: PathBuf::from(".gleon/actual/actual.png"),
            }],
        };
        let report_dir = PathBuf::from(".gleon/reports");
        let html = ReportGenerator::generate_html(&[tc], Some(&report_dir))
            .expect("Render should succeed")
            .expect("Expected HTML output");
        assert!(html.contains("..&#x2f;actual&#x2f;actual.png"));
        assert!(html.contains("Visual mismatch (5 pixels)"));
    }

    #[test]
    fn test_generate_junit_xml() {
        let tc = TestCaseResult {
            name: "billing".to_string(),
            results: vec![
                TestImageResult::Mismatch {
                    relative_path: PathBuf::from("form.png"),
                    detail: MismatchDetail::Pixel { diff_count: 5 },
                    diff_path: PathBuf::from("diff.png"),
                    baseline_path: PathBuf::from("baseline.png"),
                    actual_path: PathBuf::from("actual.png"),
                },
                TestImageResult::Mismatch {
                    relative_path: PathBuf::from("ssim_form.png"),
                    detail: MismatchDetail::Ssim { ssim_score: 0.9412 },
                    diff_path: PathBuf::from("diff.png"),
                    baseline_path: PathBuf::from("baseline.png"),
                    actual_path: PathBuf::from("actual.png"),
                },
            ],
        };
        let xml = ReportGenerator::generate_junit_xml(&[tc]).expect("Render should succeed");
        assert!(xml.contains("<failure message=\"Visual mismatch detected (5 pixels)\">Visual mismatch detected (5 pixels)</failure>"));
        assert!(xml.contains("<failure message=\"Visual mismatch detected (SSIM score: 0.9412)\">Visual mismatch detected (SSIM score: 0.9412)</failure>"));
        assert!(xml.contains("classname=\"billing\""));
        assert!(xml.contains("name=\"form.png\""));
    }

    #[test]
    fn test_generate_markdown() {
        let tc = TestCaseResult {
            name: "billing".to_string(),
            results: vec![TestImageResult::DecodeError {
                relative_path: PathBuf::from("corrupt.png"),
                error: "Bad header".to_string(),
            }],
        };
        let md = ReportGenerator::generate_markdown(&[tc]);
        assert!(md.contains("# Gleon Visual Regression Summary"));
        assert!(md.contains("❌ Decode Error"));
        assert!(md.contains("billing"));
    }

    #[test]
    fn test_generate_markdown_sanitization() {
        let tc = TestCaseResult {
            name: "billing \\| feature\nline".to_string(),
            results: vec![TestImageResult::DecodeError {
                relative_path: PathBuf::from("corrupt | file.png"),
                error: "Bad header".to_string(),
            }],
        };
        let md = ReportGenerator::generate_markdown(&[tc]);
        assert!(md.contains("billing \\\\\\| feature line"));
        assert!(md.contains("corrupt \\| file.png"));
    }
}
