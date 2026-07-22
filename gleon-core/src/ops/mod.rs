//! Core CLI operations library module.

pub mod diff;
pub mod init;
pub mod stage;
pub mod status;

pub use diff::{DiffOpError, DiffReportResult, run_diff};
pub use init::{InitError, InitResult, init_workspace};
pub use stage::{StageError, StageResult, stage_workspace};
pub use status::{StatusError, StatusReport, check_status};
