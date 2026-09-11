//! Shared process exit code for CLI subcommand handlers.

/// Process exit code a subcommand handler reports once its operation has run to completion —
/// as opposed to an unexpected error, which every handler now reports identically via
/// [`crate::commands::report_failure`] instead of letting it bubble in its own format.
///
/// Distinguishes "the operation ran fine and determined an unsuccessful outcome" (a failed
/// diff, a bad lint report, a non-interactive terminal, ...) from "the operation itself broke".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitCode {
    /// The operation completed and succeeded.
    Success = 0,
    /// The operation completed but determined a failure.
    Failure = 1,
}

impl From<ExitCode> for i32 {
    fn from(code: ExitCode) -> Self {
        code as Self
    }
}
