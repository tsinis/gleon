//! Core CLI operations library module.

pub mod diff;
pub mod init;
pub mod stage;
pub mod status;

const MAX_IMAGE_PROCESSING_THREADS: usize = 4;

fn image_processing_pool() -> Result<rayon::ThreadPool, std::io::Error> {
    let worker_count = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .min(MAX_IMAGE_PROCESSING_THREADS);
    rayon::ThreadPoolBuilder::new()
        .num_threads(worker_count)
        .build()
        .map_err(std::io::Error::other)
}

pub use diff::{DiffOpError, DiffReportResult, run_diff};
pub use init::{InitError, InitResult, init_workspace};
pub use stage::{StageError, StageResult, stage_workspace};
pub use status::{StatusError, StatusReport, check_status};
