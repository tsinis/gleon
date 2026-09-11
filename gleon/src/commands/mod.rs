//! Subcommand handlers for gleon CLI.

pub mod approve;
pub mod clean;
pub mod lint;
pub mod pull;
pub mod push;
pub mod report;
pub mod resolve;

use crate::exit_code::ExitCode;

/// Logs `context` plus `err`'s message via `tracing::error!` and returns [`ExitCode::Failure`],
/// so every subcommand reports an operation failure identically regardless of its own error type
/// (a `thiserror` enum from `gleon-core`, or an `anyhow::Error` chain built up locally).
pub fn report_failure(context: &str, err: impl std::fmt::Display) -> ExitCode {
    tracing::error!("{context}: {err}");
    ExitCode::Failure
}
