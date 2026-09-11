//! Implementation of the `gleon status` subcommand.

use gleon_core::context::ResolvedContext;

use crate::commands::report_failure;
use crate::exit_code::ExitCode;

/// Runs the `gleon status` subcommand, printing the report to stdout.
pub fn run_status(ctx: &ResolvedContext, json: bool) -> ExitCode {
    let report = match gleon_core::ops::check_status(ctx) {
        Ok(report) => report,
        Err(e) => return report_failure("Error checking workspace status", &e),
    };

    if json {
        match report.format_json() {
            Ok(rendered) => println!("{rendered}"),
            Err(e) => return report_failure("Error serializing status report", &e),
        }
    } else {
        print!("{}", report.format_text());
    }
    ExitCode::Success
}
