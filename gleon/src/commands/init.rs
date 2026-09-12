//! Implementation of the `gleon init` subcommand.

use gleon_core::context::ResolvedContext;
use tracing::info;

use crate::commands::report_failure;
use crate::exit_code::ExitCode;

/// Runs the `gleon init` subcommand.
pub fn run_init(ctx: &ResolvedContext) -> ExitCode {
    let res = match gleon_core::ops::init_workspace(ctx) {
        Ok(res) => res,
        Err(e) => return report_failure("Error initializing workspace", &e),
    };

    info!("Initialized gleon workspace at {}", res.gleon_dir.display());
    if let Some(ref config_path) = res.config_created {
        info!(
            "Created default configuration file at {}",
            config_path.display()
        );
    }
    ExitCode::Success
}
