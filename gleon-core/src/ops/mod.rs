//! Core CLI operations library module.

pub mod diff;
pub mod init;
pub mod stage;
pub mod status;

const MAX_IMAGE_PROCESSING_THREADS: usize = 4;

/// Creates an immutable directory for one diff execution.
pub(crate) fn create_run_directory(
    runs_root: &std::path::Path,
) -> Result<std::path::PathBuf, std::io::Error> {
    use std::sync::atomic::{AtomicU64, Ordering};

    static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

    std::fs::create_dir_all(runs_root)?;
    for _ in 0..100 {
        let sequence = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(std::io::Error::other)?
            .as_nanos();
        let run_dir = runs_root.join(format!("{timestamp}-{}-{sequence}", std::process::id()));
        match std::fs::create_dir(&run_dir) {
            Ok(()) => return Ok(run_dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "unable to allocate a unique run directory",
    ))
}

/// Atomically records the completed run directory as the latest run.
pub(crate) fn publish_latest_run(
    runs_root: &std::path::Path,
    run_dir: &std::path::Path,
) -> Result<(), std::io::Error> {
    let run_name = run_dir.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "run directory has no final path component",
        )
    })?;
    let latest_path = runs_root.join("latest");
    if latest_path.is_dir() {
        std::fs::remove_dir_all(&latest_path)?;
    }
    crate::io::save_file_atomically(&latest_path, run_name.as_encoded_bytes())
        .map_err(std::io::Error::other)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_publish_latest_run_replaces_existing_directory() {
        let dir = tempdir().unwrap();
        let runs_root = dir.path();
        let latest_dir = runs_root.join("latest");
        std::fs::create_dir_all(&latest_dir).unwrap();
        std::fs::write(latest_dir.join("junk.txt"), b"junk").unwrap();

        let run_dir = runs_root.join("20260725_120000_1234");
        std::fs::create_dir_all(&run_dir).unwrap();

        publish_latest_run(runs_root, &run_dir).unwrap();

        let content = std::fs::read_to_string(runs_root.join("latest")).unwrap();
        assert_eq!(content, "20260725_120000_1234");
    }
}
