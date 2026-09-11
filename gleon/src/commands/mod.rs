//! Subcommand handlers for gleon CLI.

pub mod approve;
pub mod clean;
pub mod lint;
pub mod pull;
pub mod push;
pub mod report;
pub mod resolve;

use crate::exit_code::ExitCode;

/// Renders `context` plus `err` and every error in its `source()` chain as one line.
///
/// Walking the chain explicitly matters because a `thiserror` variant whose `#[error(...)]`
/// message does not interpolate its `#[source]` field (e.g. `ApproveError::ImageDecode`) would
/// otherwise report only "Image decode error for 'x.png'" and drop the actual reason.
fn format_failure(context: &str, err: &dyn std::error::Error) -> String {
    use std::fmt::Write as _;

    let mut out = format!("{context}: {err}");
    let mut source = err.source();
    while let Some(cause) = source {
        let rendered = cause.to_string();
        // Skip causes the parent already interpolated itself (`#[error("IO error: {0}")]`),
        // so only genuinely hidden ones get appended and nothing is echoed twice.
        if !out.contains(&rendered) {
            // Writing to a `String` via `fmt::Write` never fails.
            #[allow(clippy::expect_used)]
            write!(out, ": {rendered}").expect("write! to a String cannot fail");
        }
        source = cause.source();
    }
    out
}

/// Logs `context` plus `err`'s full cause chain via `tracing::error!` and returns
/// [`ExitCode::Failure`], so every subcommand reports an operation failure identically
/// regardless of its own error type (a `thiserror` enum from `gleon-core`, or an
/// `anyhow::Error` chain built up locally).
pub fn report_failure(context: &str, err: &dyn std::error::Error) -> ExitCode {
    tracing::error!("{}", format_failure(context, err));
    ExitCode::Failure
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

    #[derive(Debug)]
    struct Cause;
    impl std::fmt::Display for Cause {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the image format could not be determined")
        }
    }
    impl std::error::Error for Cause {}

    /// Mirrors `ApproveError::ImageDecode`: carries a `#[source]` that its own `Display`
    /// deliberately does not interpolate.
    #[derive(Debug)]
    struct HidesItsSource(Cause);
    impl std::fmt::Display for HidesItsSource {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("Image decode error for 'broken.png'")
        }
    }
    impl std::error::Error for HidesItsSource {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn test_format_failure_includes_hidden_source_chain() {
        let msg = format_failure("Error approving screenshots", &HidesItsSource(Cause));
        assert!(
            msg.contains("Image decode error for 'broken.png'"),
            "top-level message missing: {msg}"
        );
        assert!(
            msg.contains("the image format could not be determined"),
            "underlying cause must not be swallowed: {msg}"
        );
    }

    /// Mirrors `CoreError::Io`: `#[error("IO error: {0}")]` already interpolates its source.
    #[derive(Debug)]
    struct InterpolatesItsSource(Cause);
    impl std::fmt::Display for InterpolatesItsSource {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "IO error: {}", self.0)
        }
    }
    impl std::error::Error for InterpolatesItsSource {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn test_format_failure_does_not_repeat_already_interpolated_source() {
        // Many `thiserror` variants render their own source (`#[error("IO error: {0}")]`).
        // Walking the chain must not echo it a second time.
        let msg = format_failure(
            "Error initializing workspace",
            &InterpolatesItsSource(Cause),
        );
        assert_eq!(
            msg,
            "Error initializing workspace: IO error: the image format could not be determined"
        );
        assert_eq!(
            msg.matches("the image format could not be determined")
                .count(),
            1,
            "cause must appear exactly once: {msg}"
        );
    }

    #[test]
    fn test_format_failure_without_source_is_plain() {
        assert_eq!(
            format_failure("Error pushing baseline blobs", &Cause),
            "Error pushing baseline blobs: the image format could not be determined"
        );
    }

    #[test]
    fn test_report_failure_returns_failure_exit_code() {
        assert_eq!(report_failure("ctx", &Cause), ExitCode::Failure);
        assert_eq!(i32::from(ExitCode::Failure), 1);
        assert_eq!(i32::from(ExitCode::Success), 0);
    }
}
