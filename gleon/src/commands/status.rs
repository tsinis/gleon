//! Implementation of the `gleon status` subcommand.

use std::io::Write;

use gleon_core::context::ResolvedContext;

use crate::{commands::report_failure, exit_code::ExitCode};

/// Errors that can occur when serializing or writing status reports to stdout.
#[derive(Debug)]
pub enum StatusOutputError {
    Serialize(serde_json::Error),
    Io(std::io::Error),
}

impl std::fmt::Display for StatusOutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialize(e) => write!(f, "{e}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for StatusOutputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(e) => Some(e),
            Self::Io(e) => Some(e),
        }
    }
}

impl From<serde_json::Error> for StatusOutputError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialize(e)
    }
}

impl From<std::io::Error> for StatusOutputError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub fn write_status(
    report: &gleon_core::ops::StatusReport,
    json: bool,
    writer: &mut impl Write,
) -> Result<(), StatusOutputError> {
    if json {
        let rendered = report.format_json()?;
        writeln!(writer, "{rendered}")?;
    } else {
        let text = report.format_text();
        write!(writer, "{text}")?;
    }
    Ok(())
}

/// Runs the `gleon status` subcommand, printing the report to stdout.
pub fn run_status(ctx: &ResolvedContext, json: bool) -> ExitCode {
    let report = match gleon_core::ops::check_status(ctx) {
        Ok(report) => report,
        Err(e) => return report_failure("Error checking workspace status", &e),
    };

    let mut stdout = std::io::stdout().lock();
    if let Err(e) = write_status(&report, json, &mut stdout) {
        return match e {
            StatusOutputError::Serialize(err) => {
                report_failure("Error serializing status report", &err)
            }
            StatusOutputError::Io(err) => report_failure("Error writing status report", &err),
        };
    }
    ExitCode::Success
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::pedantic,
    clippy::nursery,
    reason = "test code: panics are assertions, and pedantic/nursery style lints are not enforced in tests"
)]
mod tests {
    use std::path::PathBuf;

    use gleon_core::ops::StatusReport;

    use super::*;

    #[test]
    fn test_write_status_text() {
        let mut report = StatusReport::default();
        report.added.push(PathBuf::from("login.png"));
        let mut buf = Vec::new();
        write_status(&report, false, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Added"));
        assert!(output.contains("login.png"));
    }

    #[test]
    fn test_write_status_json() {
        let mut report = StatusReport::default();
        report.added.push(PathBuf::from("login.png"));
        let mut buf = Vec::new();
        write_status(&report, true, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\"added\""));
        assert!(output.contains("\"login.png\""));
    }

    #[test]
    fn test_write_status_io_error_propagation() {
        struct FailingWriter;
        impl Write for FailingWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "broken pipe",
                ))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "broken pipe",
                ))
            }
        }

        let report = StatusReport::default();
        let err = write_status(&report, false, &mut FailingWriter).unwrap_err();
        assert!(matches!(err, StatusOutputError::Io(_)));
        assert!(err.to_string().contains("broken pipe"));
        assert!(std::error::Error::source(&err).is_some());

        let err_json = write_status(&report, true, &mut FailingWriter).unwrap_err();
        assert!(matches!(err_json, StatusOutputError::Io(_)));

        let json_err: serde_json::Error = serde_json::from_str::<String>("invalid").unwrap_err();
        let ser_err = StatusOutputError::from(json_err);
        assert!(!ser_err.to_string().is_empty());
        assert!(std::error::Error::source(&ser_err).is_some());
    }
}
