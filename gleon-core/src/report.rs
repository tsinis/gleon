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
            crate::io::IoError::Io(e) => ReportError::Io(e),
            crate::io::IoError::JsonParse(e) => ReportError::JsonParse(e),
        }
    }
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

pub type ImageUrlResolver<'a> = dyn Fn(&std::path::Path) -> Option<String> + Sync + 'a;

#[derive(Default)]
pub struct MarkdownReportOptions<'a> {
    pub base_image_url: Option<&'a str>,
    pub html_artifact_url: Option<&'a str>,
    pub image_url_resolver: Option<&'a ImageUrlResolver<'a>>,
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
            if !first
                && !last_was_slash
                && !matches!(comp, Component::RootDir | Component::Prefix(_))
            {
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
                    first = true;
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

struct FormattedDimensions(u32, u32);

impl std::fmt::Display for FormattedDimensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.0, self.1)
    }
}

impl Serialize for FormattedDimensions {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

struct HtmlMismatchMessageView<'a>(&'a MismatchDetail);

impl<'a> std::fmt::Display for HtmlMismatchMessageView<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            MismatchDetail::Pixel { diff_count } => {
                write!(f, "Visual mismatch ({} pixels)", diff_count)
            }
            MismatchDetail::Ssim { ssim_score } => {
                write!(f, "Visual mismatch (SSIM: {:.4})", ssim_score)
            }
            MismatchDetail::SsimFallback { diff_count } => {
                write!(f, "Visual mismatch (SSIM Fallback: {} pixels)", diff_count)
            }
        }
    }
}

impl<'a> Serialize for HtmlMismatchMessageView<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

struct XmlDecodeErrorView<'a>(&'a str);

impl<'a> std::fmt::Display for XmlDecodeErrorView<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Decode error: {}", self.0)
    }
}

impl<'a> Serialize for XmlDecodeErrorView<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

struct XmlIoErrorView<'a>(&'a str);

impl<'a> std::fmt::Display for XmlIoErrorView<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IO error: {}", self.0)
    }
}

impl<'a> Serialize for XmlIoErrorView<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

struct XmlMissingBaselineView<'a>(&'a str);
impl<'a> Serialize for XmlMissingBaselineView<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(&format_args!("Missing baseline: {}", self.0))
    }
}

// Lazy view for XML encode error message
struct XmlEncodeErrorView<'a>(&'a str);
impl<'a> Serialize for XmlEncodeErrorView<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(&format_args!("Encode error: {}", self.0))
    }
}

struct XmlDimensionMismatchView((u32, u32), (u32, u32));

impl std::fmt::Display for XmlDimensionMismatchView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Dimension mismatch (Baseline: {}x{}, Actual: {}x{})",
            (self.0).0,
            (self.0).1,
            (self.1).0,
            (self.1).1
        )
    }
}

impl Serialize for XmlDimensionMismatchView {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

struct XmlMismatchMessageView<'a>(&'a MismatchDetail);

impl<'a> std::fmt::Display for XmlMismatchMessageView<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            MismatchDetail::Pixel { diff_count } => {
                write!(f, "Visual mismatch detected ({} pixels)", diff_count)
            }
            MismatchDetail::Ssim { ssim_score } => {
                write!(
                    f,
                    "Visual mismatch detected (SSIM score: {:.4})",
                    ssim_score
                )
            }
            MismatchDetail::SsimFallback { diff_count } => write!(
                f,
                "Visual mismatch detected (SSIM Fallback: {} pixels)",
                diff_count
            ),
        }
    }
}

impl<'a> Serialize for XmlMismatchMessageView<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
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
            TestImageResult::IoError {
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
                state.serialize_field("type", "IoError")?;
                state.serialize_field("error", error)?;
                state.serialize_field("actual_path", &None::<String>)?;
                state.serialize_field("baseline_path", &None::<String>)?;
                state.serialize_field("diff_path", &None::<String>)?;
                state.serialize_field("diff_count", &None::<u64>)?;
                state.serialize_field("actual_size", &None::<String>)?;
                state.serialize_field("baseline_size", &None::<String>)?;
            }
            TestImageResult::EncodeError {
                relative_path,
                actual_path,
                error,
            } => {
                state.serialize_field(
                    "image",
                    &FormattedPath {
                        path: relative_path,
                        report_dir: None,
                    },
                )?;
                state.serialize_field("type", "EncodeError")?;
                state.serialize_field("error", error)?;
                state.serialize_field(
                    "actual_path",
                    &FormattedPath {
                        path: actual_path,
                        report_dir: self.report_dir,
                    },
                )?;
                state.serialize_field("baseline_path", &None::<String>)?;
                state.serialize_field("diff_path", &None::<String>)?;
                state.serialize_field("diff_count", &None::<u64>)?;
                state.serialize_field("actual_size", &None::<String>)?;
                state.serialize_field("baseline_size", &None::<String>)?;
            }
            TestImageResult::MissingBaseline {
                relative_path,
                reason,
            } => {
                state.serialize_field(
                    "image",
                    &FormattedPath {
                        path: relative_path,
                        report_dir: None,
                    },
                )?;
                state.serialize_field("type", "MissingBaseline")?;
                state.serialize_field("error", reason)?;
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
                    &FormattedDimensions(actual_size.0, actual_size.1),
                )?;
                state.serialize_field(
                    "baseline_size",
                    &FormattedDimensions(baseline_size.0, baseline_size.1),
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

                let diff_count = match detail {
                    MismatchDetail::Pixel { diff_count }
                    | MismatchDetail::SsimFallback { diff_count } => Some(*diff_count),
                    MismatchDetail::Ssim { .. } => None,
                };

                state.serialize_field("error", &HtmlMismatchMessageView(detail))?;
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
            let res = &tc.result;
            if !matches!(res, TestImageResult::Success { .. }) {
                seq.serialize_element(&HtmlFailureView {
                    tc_name: &tc.name,
                    res,
                    report_dir: self.report_dir,
                })?;
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
                state.serialize_field("failure_message", &Some(XmlDecodeErrorView(error)))?;
            }
            TestImageResult::IoError { error, .. } => {
                state.serialize_field("status", "IoError")?;
                state.serialize_field("failure_message", &Some(XmlIoErrorView(error)))?;
            }
            TestImageResult::EncodeError { error, .. } => {
                state.serialize_field("status", "EncodeError")?;
                state.serialize_field("failure_message", &Some(XmlEncodeErrorView(error)))?;
            }
            TestImageResult::MissingBaseline { reason, .. } => {
                state.serialize_field("status", "MissingBaseline")?;
                state.serialize_field("failure_message", &Some(XmlMissingBaselineView(reason)))?;
            }
            TestImageResult::DimensionMismatch {
                baseline_size,
                actual_size,
                ..
            } => {
                state.serialize_field("status", "DimensionMismatch")?;
                state.serialize_field(
                    "failure_message",
                    &Some(XmlDimensionMismatchView(*baseline_size, *actual_size)),
                )?;
            }
            TestImageResult::Mismatch { detail, .. } => {
                state.serialize_field("status", "Mismatch")?;
                state.serialize_field("failure_message", &Some(XmlMismatchMessageView(detail)))?;
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

        // For JUnit compatibility, we serialize the single result as a 1-element list
        struct ResultsSeq<'a>(&'a TestImageResult);
        impl<'a> Serialize for ResultsSeq<'a> {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut seq = serializer.serialize_seq(Some(1))?;
                seq.serialize_element(&XmlTestImageResultView(self.0))?;
                seq.end()
            }
        }

        state.serialize_field("results", &ResultsSeq(&self.0.result))?;

        let failures = if matches!(self.0.result, TestImageResult::Success { .. }) {
            0
        } else {
            1
        };
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

pub struct PosixPathFormatter<'a>(pub &'a std::path::Path);
impl<'a> std::fmt::Display for PosixPathFormatter<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use std::fmt::Write;
        if self.0.as_os_str().is_empty() {
            return f.write_str(".");
        }
        let mut first = true;
        for comp in self.0.components() {
            match comp {
                std::path::Component::Normal(s) => {
                    if !first {
                        f.write_char('/')?;
                    }
                    f.write_str(&s.to_string_lossy())?;
                    first = false;
                }
                std::path::Component::ParentDir => {
                    if !first {
                        f.write_char('/')?;
                    }
                    f.write_str("..")?;
                    first = false;
                }
                std::path::Component::CurDir => {}
                std::path::Component::RootDir => {
                    f.write_char('/')?;
                    first = true;
                }
                std::path::Component::Prefix(prefix) => {
                    f.write_str(&prefix.as_os_str().to_string_lossy())?;
                    first = false;
                }
            }
        }
        Ok(())
    }
}

pub struct MarkdownEscape<'a>(pub &'a str);
impl<'a> std::fmt::Display for MarkdownEscape<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use std::fmt::Write;
        for c in self.0.chars() {
            match c {
                '|' => f.write_str("\\|")?,
                '\n' | '\r' => f.write_char(' ')?,
                '\\' => f.write_str("\\\\")?,
                '`' => f.write_str("\\`")?,
                '[' => f.write_str("\\[")?,
                ']' => f.write_str("\\]")?,
                _ => f.write_char(c)?,
            }
        }
        Ok(())
    }
}

pub struct CodeSpanEscape<'a>(pub &'a str);
impl<'a> std::fmt::Display for CodeSpanEscape<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use std::fmt::Write;
        for c in self.0.chars() {
            match c {
                '|' => f.write_str("\\|")?,
                '`' => f.write_char('\'')?,
                '\n' | '\r' => f.write_char(' ')?,
                _ => f.write_char(c)?,
            }
        }
        Ok(())
    }
}

impl ReportGenerator {
    /// Generates a single self-contained HTML report string linking images via relative paths.
    /// Skips generation entirely if 100% of tests passed by returning None.
    pub fn generate_html(
        test_cases: &[TestCaseResult],
        report_dir: Option<&std::path::Path>,
    ) -> Result<Option<String>, ReportError> {
        let total_tests = test_cases.len();
        let failed_tests = test_cases
            .iter()
            .filter(|tc| !matches!(tc.result, TestImageResult::Success { .. }))
            .count();

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
        let total_tests = test_cases.len();
        let failed_tests = test_cases
            .iter()
            .filter(|tc| !matches!(tc.result, TestImageResult::Success { .. }))
            .count();

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

    /// Maximum number of failure rows rendered in a PR comment table.
    pub const MAX_MARKDOWN_DIFF_ROWS: usize = 10;

    /// Renders a GitHub PR comment in Markdown from the failed test cases.
    /// Truncates the table to `MAX_MARKDOWN_DIFF_ROWS` rows.
    pub fn render_pr_comment(
        test_cases: &[TestCaseResult],
        options: &MarkdownReportOptions,
    ) -> String {
        use std::fmt::Write;

        let failed_tests: Vec<_> = test_cases.iter().filter(|tc| !tc.passed()).collect();
        let total_failed = failed_tests.len();

        if total_failed == 0 {
            return "### ✅ Gleon Visual Regression: All tests passed!\n".to_string();
        }

        let mut out = String::new();
        writeln!(
            out,
            "### ❌ Gleon Visual Regression Failure ({} diffs)\n",
            total_failed
        )
        .expect("write infallible");

        let has_image_urls =
            options.base_image_url.is_some() || options.image_url_resolver.is_some();

        if has_image_urls {
            out.push_str("| Test Name | Expected | Actual | Diff | Delta |\n");
            out.push_str("| :--- | :---: | :---: | :---: | :---: |\n");
        } else {
            out.push_str("| Test Name | Status | Error |\n");
            out.push_str("| :--- | :--- | :--- |\n");
        }

        struct ImgLinkFormatter<'a> {
            base_url: Option<&'a str>,
            path: Option<&'a std::path::Path>,
            resolver: Option<&'a ImageUrlResolver<'a>>,
        }
        impl<'a> std::fmt::Display for ImgLinkFormatter<'a> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                if let Some(p) = self.path {
                    if let Some(signed_url) = self.resolver.and_then(|res_fn| res_fn(p)) {
                        return write!(f, "[Image]({})", signed_url);
                    }
                    if let Some(base) = self.base_url {
                        let base = base.trim_end_matches('/');
                        return write!(f, "[Image]({}/{})", base, PosixPathFormatter(p));
                    }
                }
                f.write_str("N/A")
            }
        }

        struct DeltaFormatter<'a>(&'a MismatchDetail);
        impl<'a> std::fmt::Display for DeltaFormatter<'a> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self.0 {
                    MismatchDetail::Pixel { diff_count } => write!(f, "{} px", diff_count),
                    MismatchDetail::Ssim { ssim_score } => write!(f, "{:.4} SSIM", ssim_score),
                    MismatchDetail::SsimFallback { diff_count } => {
                        write!(f, "{} px (fb)", diff_count)
                    }
                }
            }
        }

        for tc in failed_tests.iter().take(Self::MAX_MARKDOWN_DIFF_ROWS) {
            let res = &tc.result;
            let name = &tc.name;

            if has_image_urls {
                match res {
                    TestImageResult::Mismatch {
                        detail,
                        diff_path,
                        baseline_path,
                        actual_path,
                        ..
                    } => {
                        writeln!(
                            out,
                            "| `{}` | {} | {} | {} | `{}` |",
                            CodeSpanEscape(name),
                            ImgLinkFormatter {
                                base_url: options.base_image_url,
                                path: Some(baseline_path),
                                resolver: options.image_url_resolver,
                            },
                            ImgLinkFormatter {
                                base_url: options.base_image_url,
                                path: Some(actual_path),
                                resolver: options.image_url_resolver,
                            },
                            ImgLinkFormatter {
                                base_url: options.base_image_url,
                                path: Some(diff_path),
                                resolver: options.image_url_resolver,
                            },
                            DeltaFormatter(detail)
                        )
                        .expect("write infallible");
                    }
                    TestImageResult::DimensionMismatch {
                        baseline_path,
                        actual_path,
                        ..
                    } => {
                        writeln!(
                            out,
                            "| `{}` | {} | {} | {} | `Dim` |",
                            CodeSpanEscape(name),
                            ImgLinkFormatter {
                                base_url: options.base_image_url,
                                path: Some(baseline_path),
                                resolver: options.image_url_resolver,
                            },
                            ImgLinkFormatter {
                                base_url: options.base_image_url,
                                path: Some(actual_path),
                                resolver: options.image_url_resolver,
                            },
                            ImgLinkFormatter {
                                base_url: None,
                                path: None,
                                resolver: None,
                            },
                        )
                        .expect("write infallible");
                    }
                    TestImageResult::MissingBaseline { .. } => {
                        writeln!(
                            out,
                            "| `{}` | {} | {} | {} | `Missing` |",
                            CodeSpanEscape(name),
                            ImgLinkFormatter {
                                base_url: None,
                                path: None,
                                resolver: None,
                            },
                            ImgLinkFormatter {
                                base_url: options.base_image_url,
                                path: Some(res.relative_path()),
                                resolver: options.image_url_resolver,
                            },
                            ImgLinkFormatter {
                                base_url: None,
                                path: None,
                                resolver: None,
                            },
                        )
                        .expect("write infallible");
                    }
                    TestImageResult::DecodeError { .. } => {
                        writeln!(
                            out,
                            "| `{}` | {} | {} | {} | `Decode Error` |",
                            CodeSpanEscape(name),
                            ImgLinkFormatter {
                                base_url: None,
                                path: None,
                                resolver: None,
                            },
                            ImgLinkFormatter {
                                base_url: options.base_image_url,
                                path: Some(res.relative_path()),
                                resolver: options.image_url_resolver,
                            },
                            ImgLinkFormatter {
                                base_url: None,
                                path: None,
                                resolver: None,
                            },
                        )
                        .expect("write infallible");
                    }
                    TestImageResult::IoError { .. } => {
                        writeln!(
                            out,
                            "| `{}` | {} | {} | {} | `IO Error` |",
                            CodeSpanEscape(name),
                            ImgLinkFormatter {
                                base_url: None,
                                path: None,
                                resolver: None,
                            },
                            ImgLinkFormatter {
                                base_url: None,
                                path: None,
                                resolver: None,
                            },
                            ImgLinkFormatter {
                                base_url: None,
                                path: None,
                                resolver: None,
                            },
                        )
                        .expect("write infallible");
                    }
                    TestImageResult::EncodeError { actual_path, .. } => {
                        writeln!(
                            out,
                            "| `{}` | {} | {} | {} | `Encode Error` |",
                            CodeSpanEscape(name),
                            ImgLinkFormatter {
                                base_url: None,
                                path: None,
                                resolver: None,
                            },
                            ImgLinkFormatter {
                                base_url: options.base_image_url,
                                path: Some(actual_path),
                                resolver: options.image_url_resolver,
                            },
                            ImgLinkFormatter {
                                base_url: None,
                                path: None,
                                resolver: None,
                            },
                        )
                        .expect("write infallible");
                    }
                    TestImageResult::Success { .. } => unreachable!(),
                }
            } else {
                match res {
                    TestImageResult::Mismatch { detail, .. } => {
                        writeln!(
                            out,
                            "| `{}` | Mismatch | {} |",
                            CodeSpanEscape(name),
                            DeltaFormatter(detail)
                        )
                        .expect("write infallible");
                    }
                    TestImageResult::DimensionMismatch { .. } => {
                        writeln!(
                            out,
                            "| `{}` | Dimension Mismatch | Dim mismatch |",
                            CodeSpanEscape(name)
                        )
                        .expect("write infallible");
                    }
                    TestImageResult::MissingBaseline { reason, .. } => {
                        writeln!(
                            out,
                            "| `{}` | Missing Baseline | {} |",
                            CodeSpanEscape(name),
                            MarkdownEscape(reason)
                        )
                        .expect("write infallible");
                    }
                    TestImageResult::DecodeError { error, .. } => {
                        writeln!(
                            out,
                            "| `{}` | Decode Error | {} |",
                            CodeSpanEscape(name),
                            MarkdownEscape(error)
                        )
                        .expect("write infallible");
                    }
                    TestImageResult::IoError { error, .. } => {
                        writeln!(
                            out,
                            "| `{}` | IO Error | {} |",
                            CodeSpanEscape(name),
                            MarkdownEscape(error)
                        )
                        .expect("write infallible");
                    }
                    TestImageResult::EncodeError { error, .. } => {
                        writeln!(
                            out,
                            "| `{}` | Encode Error | {} |",
                            CodeSpanEscape(name),
                            MarkdownEscape(error)
                        )
                        .expect("write infallible");
                    }
                    TestImageResult::Success { .. } => unreachable!(),
                }
            }
        }

        if total_failed > Self::MAX_MARKDOWN_DIFF_ROWS {
            let remaining = total_failed - Self::MAX_MARKDOWN_DIFF_ROWS;
            out.push_str("\n> ⚠️ **Truncated ");
            write!(out, "{}", remaining).expect("write infallible");
            out.push_str(" additional diffs.** ");
            match options.html_artifact_url {
                Some(url) => {
                    out.push_str("Download the full [Gleon HTML Report](");
                    out.push_str(url);
                    out.push_str(") to inspect.\n");
                }
                None => {
                    out.push_str(
                        "Download the full HTML Report from GitHub Action Artifacts to inspect.\n",
                    );
                }
            }
        }

        out.push_str(
            "\n---\n*Reply with `/gleon approve` to update baseline images for this PR.*\n",
        );
        out
    }

    /// Generates a simple Markdown report summary string.
    pub fn generate_markdown(test_cases: &[TestCaseResult]) -> String {
        use std::fmt::Write;

        let total = test_cases.len();
        let failed = test_cases.iter().filter(|tc| !tc.passed()).count();

        let mut out = String::new();
        writeln!(
            out,
            "# gleon Visual Regression Summary\n\n**Total Tests:** {}\n**Failed:** {}\n",
            total, failed
        )
        .expect("write infallible");

        out.push_str("| Test Case | Screenshot | Status |\n|---|---|---|\n");

        for tc in test_cases {
            let res = &tc.result;
            let status = match res {
                TestImageResult::Success { .. } => "✅ Pass",
                TestImageResult::DecodeError { .. } => "❌ Decode Error",
                TestImageResult::IoError { .. } => "❌ IO Error",
                TestImageResult::EncodeError { .. } => "❌ Encode Error",
                TestImageResult::MissingBaseline { .. } => "❌ Missing Baseline",
                TestImageResult::DimensionMismatch { .. } => "❌ Dimension Mismatch",
                TestImageResult::Mismatch { .. } => "❌ Mismatch",
            };

            let path_fmt = PosixPathFormatter(res.relative_path());
            let path_str = path_fmt.to_string();
            writeln!(
                out,
                "| {} | {} | {} |",
                MarkdownEscape(&tc.name),
                MarkdownEscape(&path_str),
                status
            )
            .expect("fmt::Write on String is infallible");
        }

        out
    }

    /// Generates markdown, JUnit XML, HTML, and JSON report files inside `runs_dir`.
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
    fn test_generate_html_skips_on_success() {
        let tc = TestCaseResult {
            name: "billing".to_string(),
            result: TestImageResult::Success {
                relative_path: PathBuf::from("form.png"),
            },
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
            result: TestImageResult::Mismatch {
                relative_path: PathBuf::from("form.png"),
                detail: MismatchDetail::Pixel { diff_count: 5 },
                diff_path: PathBuf::from(".gleon/diffs/diff.png"),
                baseline_path: PathBuf::from("baseline.png"),
                actual_path: PathBuf::from(".gleon/actual/actual.png"),
            },
        };
        let report_dir = PathBuf::from(".gleon/reports");
        let html = ReportGenerator::generate_html(&[tc], Some(&report_dir))
            .expect("Render should succeed")
            .expect("Expected HTML output");
        assert!(html.contains("..&#x2f;actual&#x2f;actual.png"));
        assert!(html.contains("Visual mismatch (5 pixels)"));
    }

    #[test]
    fn test_render_pr_comment_with_base_url_and_fallback() {
        let tc = TestCaseResult {
            name: "login_button".to_string(),
            result: TestImageResult::Mismatch {
                relative_path: PathBuf::from("login.png"),
                detail: MismatchDetail::SsimFallback { diff_count: 12 },
                diff_path: PathBuf::from("diffs/login.png"),
                baseline_path: PathBuf::from("goldens/login.png"),
                actual_path: PathBuf::from("actual/login.png"),
            },
        };
        let options = MarkdownReportOptions {
            base_image_url: Some("https://storage.cdn.com/run-1"),
            html_artifact_url: Some("https://github.com/org/repo/actions/runs/1/artifacts/2"),
            image_url_resolver: None,
        };
        let comment = ReportGenerator::render_pr_comment(&[tc], &options);
        assert!(comment.contains("`login_button`"));
        assert!(comment.contains("[Image](https://storage.cdn.com/run-1/goldens/login.png)"));
        assert!(comment.contains("12 px (fb)"));
    }

    #[test]
    fn test_render_pr_comment_truncation() {
        let mut test_cases = Vec::new();
        for i in 0..15 {
            test_cases.push(TestCaseResult {
                name: format!("test_{}", i),
                result: TestImageResult::Mismatch {
                    relative_path: PathBuf::from(format!("{}.png", i)),
                    detail: MismatchDetail::Pixel { diff_count: i + 1 },
                    diff_path: PathBuf::from(format!("diff_{}.png", i)),
                    baseline_path: PathBuf::from(format!("base_{}.png", i)),
                    actual_path: PathBuf::from(format!("act_{}.png", i)),
                },
            });
        }
        let options = MarkdownReportOptions {
            base_image_url: None,
            html_artifact_url: Some("https://artifact.url/report.html"),
            image_url_resolver: None,
        };
        let comment = ReportGenerator::render_pr_comment(&test_cases, &options);
        assert!(comment.contains("Truncated 5 additional diffs"));
        assert!(comment.contains("https://artifact.url/report.html"));
    }

    #[test]
    fn test_render_pr_comment_all_variants_with_base_url() {
        let test_cases = vec![
            TestCaseResult {
                name: "tc1".to_string(),
                result: TestImageResult::DimensionMismatch {
                    relative_path: PathBuf::from("dim.png"),
                    actual_size: (100, 200),
                    baseline_size: (101, 200),
                    baseline_path: PathBuf::from("base.png"),
                    actual_path: PathBuf::from("act.png"),
                },
            },
            TestCaseResult {
                name: "tc2".to_string(),
                result: TestImageResult::MissingBaseline {
                    relative_path: PathBuf::from("miss.png"),
                    reason: "No baseline".to_string(),
                },
            },
            TestCaseResult {
                name: "tc3".to_string(),
                result: TestImageResult::DecodeError {
                    relative_path: PathBuf::from("err.png"),
                    error: "Corrupt".to_string(),
                },
            },
        ];
        let options = MarkdownReportOptions {
            base_image_url: Some("http://test.com"),
            html_artifact_url: None,
            image_url_resolver: None,
        };
        let out = ReportGenerator::render_pr_comment(&test_cases, &options);
        assert!(out.contains("`Dim`"));
        assert!(out.contains("`Missing`"));
        assert!(out.contains("`Decode Error`"));
    }

    #[test]
    fn test_render_pr_comment_all_variants_no_base_url() {
        let test_cases = vec![
            TestCaseResult {
                name: "tc1".to_string(),
                result: TestImageResult::DimensionMismatch {
                    relative_path: PathBuf::from("dim.png"),
                    actual_size: (100, 200),
                    baseline_size: (101, 200),
                    baseline_path: PathBuf::from("base.png"),
                    actual_path: PathBuf::from("act.png"),
                },
            },
            TestCaseResult {
                name: "tc2".to_string(),
                result: TestImageResult::MissingBaseline {
                    relative_path: PathBuf::from("miss.png"),
                    reason: "No baseline".to_string(),
                },
            },
            TestCaseResult {
                name: "tc3".to_string(),
                result: TestImageResult::DecodeError {
                    relative_path: PathBuf::from("err.png"),
                    error: "Corrupt".to_string(),
                },
            },
            TestCaseResult {
                name: "tc4".to_string(),
                result: TestImageResult::EncodeError {
                    relative_path: PathBuf::from("encode_err.png"),
                    actual_path: PathBuf::from("act.png"),
                    error: "io error".to_string(),
                },
            },
        ];
        let options = MarkdownReportOptions {
            base_image_url: None,
            html_artifact_url: None,
            image_url_resolver: None,
        };
        let out = ReportGenerator::render_pr_comment(&test_cases, &options);
        assert!(out.contains("Dimension Mismatch"));
        assert!(out.contains("Missing Baseline"));
        assert!(out.contains("Decode Error"));
    }

    #[test]
    fn test_markdown_escape_and_posix_branches() {
        let escaped = MarkdownEscape("a|b\\c\n\r`d`[e]").to_string();
        assert_eq!(escaped, "a\\|b\\\\c  \\`d\\`\\[e\\]");

        let p = PathBuf::from("foo/.././bar");
        assert_eq!(PosixPathFormatter(&p).to_string(), "foo/../bar");
    }

    #[test]
    fn test_posix_path_formatter_special_components() {
        let empty_path = PathBuf::from("");
        assert_eq!(PosixPathFormatter(&empty_path).to_string(), ".");

        let root_path = std::path::Path::new("/");
        assert_eq!(PosixPathFormatter(root_path).to_string(), "/");
    }

    #[test]
    fn test_render_pr_comment_pass_path() {
        let test_cases = vec![];
        let options = MarkdownReportOptions {
            base_image_url: None,
            html_artifact_url: None,
            image_url_resolver: None,
        };
        let comment = ReportGenerator::render_pr_comment(&test_cases, &options);
        assert!(comment.contains("All tests passed!"));
    }

    #[test]
    fn test_render_pr_comment_image_truncation_without_url() {
        let mut test_cases = Vec::new();
        for i in 0..15 {
            test_cases.push(TestCaseResult {
                name: format!("test_{}", i),
                result: TestImageResult::Mismatch {
                    relative_path: PathBuf::from(format!("{}.png", i)),
                    detail: MismatchDetail::Pixel { diff_count: i + 1 },
                    diff_path: PathBuf::from(format!("diff_{}.png", i)),
                    baseline_path: PathBuf::from(format!("base_{}.png", i)),
                    actual_path: PathBuf::from(format!("act_{}.png", i)),
                },
            });
        }
        let options = MarkdownReportOptions {
            base_image_url: Some("http://example.com"),
            html_artifact_url: None,
            image_url_resolver: None,
        };
        let comment = ReportGenerator::render_pr_comment(&test_cases, &options);
        assert!(comment.contains("Truncated 5 additional diffs"));
        assert!(
            comment
                .contains("Download the full HTML Report from GitHub Action Artifacts to inspect.")
        );
    }

    #[test]
    fn test_render_pr_comment_name_escaping_no_bracket_slashes() {
        let tc = TestCaseResult {
            name: "test`[foo]|bar".to_string(),
            result: TestImageResult::DecodeError {
                relative_path: PathBuf::from("err.png"),
                error: "Bad header".to_string(),
            },
        };
        let options = MarkdownReportOptions {
            base_image_url: None,
            html_artifact_url: None,
            image_url_resolver: None,
        };
        let comment = ReportGenerator::render_pr_comment(&[tc], &options);
        // Should contain `test'[foo]\|bar` (pipe escaped, brackets unescaped, backtick replaced)
        assert!(comment.contains("`test'[foo]\\|bar`"));
        assert!(!comment.contains("\\["));
    }

    #[test]
    fn test_render_pr_comment_with_image_url_resolver() {
        let tc = TestCaseResult {
            name: "login_btn".to_string(),
            result: TestImageResult::Mismatch {
                relative_path: PathBuf::from("login.png"),
                detail: MismatchDetail::Pixel { diff_count: 5 },
                diff_path: PathBuf::from("diffs/login.png"),
                baseline_path: PathBuf::from("goldens/login.png"),
                actual_path: PathBuf::from("actual/login.png"),
            },
        };
        let resolver = |p: &std::path::Path| {
            if p == std::path::Path::new("goldens/login.png") {
                Some("https://signed.com/golden.png?token=123".to_string())
            } else {
                None
            }
        };
        let options = MarkdownReportOptions {
            base_image_url: None,
            html_artifact_url: None,
            image_url_resolver: Some(&resolver),
        };
        let comment = ReportGenerator::render_pr_comment(&[tc], &options);
        assert!(comment.contains("[Image](https://signed.com/golden.png?token=123)"));
    }

    #[test]
    fn test_generate_junit_xml() {
        let tc1 = TestCaseResult {
            name: "billing".to_string(),
            result: TestImageResult::Mismatch {
                relative_path: PathBuf::from("form.png"),
                detail: MismatchDetail::Pixel { diff_count: 5 },
                diff_path: PathBuf::from("diff.png"),
                baseline_path: PathBuf::from("baseline.png"),
                actual_path: PathBuf::from("actual.png"),
            },
        };
        let tc2 = TestCaseResult {
            name: "billing".to_string(),
            result: TestImageResult::Mismatch {
                relative_path: PathBuf::from("ssim_form.png"),
                detail: MismatchDetail::Ssim { ssim_score: 0.9412 },
                diff_path: PathBuf::from("diff.png"),
                baseline_path: PathBuf::from("baseline.png"),
                actual_path: PathBuf::from("actual.png"),
            },
        };
        let tc3 = TestCaseResult {
            name: "billing".to_string(),
            result: TestImageResult::EncodeError {
                relative_path: PathBuf::from("encode_form.png"),
                actual_path: PathBuf::from("act.png"),
                error: "io error".to_string(),
            },
        };
        let xml =
            ReportGenerator::generate_junit_xml(&[tc1, tc2, tc3]).expect("Render should succeed");
        assert!(xml.contains("<failure message=\"Visual mismatch detected (5 pixels)\">Visual mismatch detected (5 pixels)</failure>"));
        assert!(xml.contains("<failure message=\"Visual mismatch detected (SSIM score: 0.9412)\">Visual mismatch detected (SSIM score: 0.9412)</failure>"));
        assert!(xml.contains(
            "<failure message=\"Encode error: io error\">Encode error: io error</failure>"
        ));
        assert!(xml.contains("classname=\"billing\""));
        assert!(xml.contains("name=\"form.png\""));
        assert!(xml.contains("name=\"encode_form.png\""));
    }

    #[test]
    fn test_generate_markdown() {
        let tc = TestCaseResult {
            name: "billing".to_string(),
            result: TestImageResult::DecodeError {
                relative_path: PathBuf::from("corrupt.png"),
                error: "bad data".to_string(),
            },
        };
        let md = ReportGenerator::generate_markdown(&[tc]);
        assert!(md.contains("# gleon Visual Regression Summary"));
        assert!(md.contains("❌ Decode Error"));
        assert!(md.contains("billing"));
    }

    #[test]
    fn test_formatted_path_display() {
        let path1 = std::path::Path::new("foo/bar/baz.png");
        assert_eq!(
            FormattedPath {
                path: path1,
                report_dir: None
            }
            .to_string(),
            "foo/bar/baz.png"
        );

        #[cfg(windows)]
        {
            let path2 = std::path::Path::new("C:\\foo\\bar.png");
            let formatted2 = FormattedPath {
                path: path2,
                report_dir: None,
            }
            .to_string();
            assert_eq!(formatted2, "C:/foo/bar.png");
        }
    }

    #[test]
    fn test_formatted_path_all_components() {
        let root_path = std::path::Path::new("/a/.././b");
        let formatted = FormattedPath {
            path: root_path,
            report_dir: None,
        }
        .to_string();
        assert!(formatted.contains("a"));

        let empty_path = std::path::Path::new("");
        let formatted_empty = FormattedPath {
            path: empty_path,
            report_dir: None,
        }
        .to_string();
        assert_eq!(formatted_empty, ".");
    }

    #[test]
    fn test_posix_path_formatter_parent_and_curdir() {
        let p = std::path::Path::new("../goldens/./login.png");
        assert_eq!(PosixPathFormatter(p).to_string(), "../goldens/login.png");
    }

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

    #[test]
    fn test_render_pr_comment_missing_and_decode_error() {
        let mut tests = Vec::new();
        tests.push(TestCaseResult {
            name: "missing".to_string(),
            result: TestImageResult::MissingBaseline {
                relative_path: PathBuf::from("missing.png"),
                reason: "not found".to_string(),
            },
        });
        tests.push(TestCaseResult {
            name: "corrupt".to_string(),
            result: TestImageResult::DecodeError {
                relative_path: PathBuf::from("corrupt.png"),
                error: "bad data".to_string(),
            },
        });
        tests.push(TestCaseResult {
            name: "ssim_fb".to_string(),
            result: TestImageResult::Mismatch {
                relative_path: PathBuf::from("fb.png"),
                detail: MismatchDetail::SsimFallback { diff_count: 10 },
                diff_path: PathBuf::from("diff.png"),
                baseline_path: PathBuf::from("base.png"),
                actual_path: PathBuf::from("actual.png"),
            },
        });
        for i in 0..10 {
            tests.push(TestCaseResult {
                name: format!("mismatch_{i}"),
                result: TestImageResult::Mismatch {
                    relative_path: PathBuf::from(format!("{i}.png")),
                    detail: MismatchDetail::Pixel { diff_count: i + 1 },
                    diff_path: PathBuf::from(format!("diff_{i}.png")),
                    baseline_path: PathBuf::from(format!("base_{i}.png")),
                    actual_path: PathBuf::from(format!("actual_{i}.png")),
                },
            });
        }

        let opts = MarkdownReportOptions {
            base_image_url: Some("https://storage.url"),
            html_artifact_url: Some("https://artifact.url"),
            image_url_resolver: None,
        };
        let md = ReportGenerator::render_pr_comment(&tests, &opts);
        assert!(md.contains("`Missing`"));
        assert!(md.contains("`Decode Error`"));
        assert!(md.contains("10 px (fb)"));
        assert!(md.contains("https://artifact.url"));
    }
}
