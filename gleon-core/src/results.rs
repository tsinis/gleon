//! Results of comparing a captured screenshot against its staged baseline.
//!
//! These types are produced by [`crate::ops::diff`] and [`crate::engine`], and consumed by
//! [`crate::report`] — they describe comparison outcomes, not files being scanned, so they
//! live apart from [`crate::scanner`].

use crate::engine::MismatchDetail;
use std::path::{Path, PathBuf};

/// Represents the result of running a test on a single screenshot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum TestImageResult {
    /// The actual image matches the baseline.
    Success {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
    },
    /// The screenshot file failed to decode.
    DecodeError {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
        /// The decoding error message.
        error: String,
    },
    /// The actual image has different dimensions than the baseline.
    DimensionMismatch {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
        /// Dimensions of the baseline image.
        baseline_size: (u32, u32),
        /// Dimensions of the actual image.
        actual_size: (u32, u32),
        /// Path to the baseline image on disk.
        baseline_path: PathBuf,
        /// Path to the actual image on disk.
        actual_path: PathBuf,
    },
    /// The screenshot failed the visual comparison threshold.
    Mismatch {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
        /// Specific detail about the comparison mismatch.
        detail: MismatchDetail,
        /// Path to the diff visualization image on disk.
        diff_path: PathBuf,
        /// Path to the baseline image on disk.
        baseline_path: PathBuf,
        /// Path to the actual image on disk.
        actual_path: PathBuf,
    },
    /// The baseline snapshot or blob is missing.
    MissingBaseline {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
        /// Reason/details for the missing baseline.
        reason: String,
    },
    /// An I/O error occurred while reading or saving the image.
    IoError {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
        /// The I/O error message.
        error: String,
    },
    /// An error occurred while encoding the diff image.
    EncodeError {
        /// Relative path of the screenshot file.
        relative_path: PathBuf,
        /// Path to the actual image on disk.
        actual_path: PathBuf,
        /// The encoding error message.
        error: String,
    },
}

impl TestImageResult {
    /// Returns the relative path of the screenshot file.
    #[must_use]
    pub fn relative_path(&self) -> &Path {
        match self {
            Self::Success { relative_path }
            | Self::DecodeError { relative_path, .. }
            | Self::MissingBaseline { relative_path, .. }
            | Self::DimensionMismatch { relative_path, .. }
            | Self::Mismatch { relative_path, .. }
            | Self::IoError { relative_path, .. }
            | Self::EncodeError { relative_path, .. } => relative_path,
        }
    }

    /// Returns the baseline image's local blob path, if this result has one.
    ///
    /// Only baselines are content-addressed and uploaded to remote storage, so this is the
    /// only image a report can link to remotely; `actual`/`diff` are produced per run on the
    /// machine executing the tests and never leave it.
    #[must_use]
    pub fn baseline_path(&self) -> Option<&Path> {
        match self {
            Self::Mismatch { baseline_path, .. }
            | Self::DimensionMismatch { baseline_path, .. } => Some(baseline_path),
            Self::Success { .. }
            | Self::DecodeError { .. }
            | Self::IoError { .. }
            | Self::EncodeError { .. }
            | Self::MissingBaseline { .. } => None,
        }
    }
}

/// Represents the final evaluation result of a complete test case.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct TestCaseResult {
    /// The test case name.
    pub name: String,
    /// Result of the single screenshot within the test case.
    pub result: TestImageResult,
}

impl TestCaseResult {
    /// Returns true if the screenshot result succeeded.
    #[must_use]
    pub const fn passed(&self) -> bool {
        matches!(self.result, TestImageResult::Success { .. })
    }
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

    #[test]
    fn test_baseline_path_only_for_results_that_have_one() {
        let mismatch = TestImageResult::Mismatch {
            relative_path: PathBuf::from("rel.png"),
            detail: MismatchDetail::Pixel { diff_count: 1 },
            diff_path: PathBuf::from("diff.png"),
            baseline_path: PathBuf::from("base.png"),
            actual_path: PathBuf::from("actual.png"),
        };
        assert_eq!(mismatch.baseline_path(), Some(Path::new("base.png")));

        let dim_mismatch = TestImageResult::DimensionMismatch {
            relative_path: PathBuf::from("rel.png"),
            baseline_size: (1, 1),
            actual_size: (2, 2),
            baseline_path: PathBuf::from("base.png"),
            actual_path: PathBuf::from("actual.png"),
        };
        assert_eq!(dim_mismatch.baseline_path(), Some(Path::new("base.png")));

        // No baseline was ever resolved for these, so there is nothing remote to link to.
        for without_baseline in [
            TestImageResult::EncodeError {
                relative_path: PathBuf::from("rel.png"),
                actual_path: PathBuf::from("actual.png"),
                error: "bad".to_string(),
            },
            TestImageResult::MissingBaseline {
                relative_path: PathBuf::from("rel.png"),
                reason: "none".to_string(),
            },
            TestImageResult::DecodeError {
                relative_path: PathBuf::from("rel.png"),
                error: "bad".to_string(),
            },
            TestImageResult::IoError {
                relative_path: PathBuf::from("rel.png"),
                error: "bad".to_string(),
            },
            TestImageResult::Success {
                relative_path: PathBuf::from("rel.png"),
            },
        ] {
            assert_eq!(without_baseline.baseline_path(), None);
        }
    }
}
