//! Pre-signs remote storage URLs for the images referenced by failed test cases, so
//! `render_pr_comment` can link directly to signed URLs instead of falling back to
//! `base_image_url`-relative links.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;

use crate::results::TestCaseResult;
use crate::scanner::FileScanner;
use crate::storage::ObjectStoreAdapter;

impl super::ReportGenerator {
    /// Signs remote storage URLs for the images referenced by up to
    /// [`Self::MAX_MARKDOWN_DIFF_ROWS`] failed test cases (the same cap `render_pr_comment`
    /// truncates its table to), running signing requests concurrently up to
    /// `adapter.concurrency()`.
    ///
    /// Best-effort: a path whose signing fails, panics, or is cancelled is simply absent from
    /// the returned map — callers fall back to `base_image_url`-relative linking (or `N/A`) for
    /// anything missing, so a partial result is fine.
    ///
    /// # Panics
    ///
    /// Never panics in this function's own stack: each spawned signing task acquires a
    /// semaphore permit via `.expect(...)`, which is safe because the semaphore is owned by
    /// this function's `JoinSet` and never closed while permits are outstanding. If a spawned
    /// task were to panic anyway, `join_next()` reports it as an `Err` (logged and skipped),
    /// not an unwind here.
    pub async fn sign_image_urls(
        adapter: &ObjectStoreAdapter,
        test_cases: &[TestCaseResult],
        expires_in: Duration,
    ) -> HashMap<PathBuf, String> {
        let to_sign = test_cases
            .iter()
            .filter(|tc| !tc.passed())
            .take(Self::MAX_MARKDOWN_DIFF_ROWS);

        let mut unique_paths = HashSet::new();
        for tc in to_sign {
            unique_paths.extend(tc.result.signable_paths());
        }

        let mut join_set = tokio::task::JoinSet::new();
        let semaphore = Arc::new(Semaphore::new(adapter.concurrency()));

        for p in unique_paths {
            let normalized_key = FileScanner::normalize_path_str(p).to_string();
            let path_buf = p.to_path_buf();
            let adapter = adapter.clone();
            let sem = Arc::clone(&semaphore);
            join_set.spawn(async move {
                // `sem` is owned by this task set and never closed while permits are
                // outstanding, so `acquire_owned` cannot fail here.
                #[allow(clippy::expect_used)]
                let _permit = sem
                    .acquire_owned()
                    .await
                    .expect("semaphore never closed while permits are outstanding");
                adapter
                    .sign_blob_url(&normalized_key, expires_in)
                    .await
                    .map(|signed| (path_buf, signed))
            });
        }

        let mut signed_urls = HashMap::new();
        while let Some(res) = join_set.join_next().await {
            match res {
                Ok(Some((p, signed))) => {
                    let _ = signed_urls.insert(p, signed);
                }
                Ok(None) => {
                    tracing::warn!("Failed to generate pre-signed URL for blob path");
                }
                Err(e) => {
                    tracing::warn!("URL signing task panicked or was cancelled: {}", e);
                }
            }
        }

        signed_urls
    }
}

#[cfg(all(test, not(miri)))]
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
    use crate::report::ReportGenerator;
    use crate::results::TestImageResult;
    use crate::storage::StorageConfig;

    #[tokio::test]
    async fn test_sign_image_urls_signs_referenced_paths_only() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = StorageConfig::new(format!("file://{}", temp.path().display()));
        let adapter = ObjectStoreAdapter::from_config(&cfg).unwrap();

        let test_cases = vec![
            TestCaseResult {
                name: "passing".to_string(),
                result: TestImageResult::Success {
                    relative_path: "pass.png".into(),
                },
            },
            TestCaseResult {
                name: "mismatch".to_string(),
                result: TestImageResult::Mismatch {
                    relative_path: "rel.png".into(),
                    detail: crate::engine::MismatchDetail::Pixel { diff_count: 1 },
                    diff_path: "diff.png".into(),
                    baseline_path: "baseline.png".into(),
                    actual_path: "actual.png".into(),
                },
            },
        ];

        // `file://` scheme adapters don't implement a signer, so every path resolves to `None`
        // and is simply absent from the result — exercises the "best-effort" `Ok(None)` path.
        let signed =
            ReportGenerator::sign_image_urls(&adapter, &test_cases, Duration::from_secs(60)).await;
        assert!(signed.is_empty());
    }

    #[tokio::test]
    async fn test_sign_image_urls_respects_max_rows_cap() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = StorageConfig::new(format!("file://{}", temp.path().display()));
        let adapter = ObjectStoreAdapter::from_config(&cfg).unwrap();

        let test_cases: Vec<_> = (0..(ReportGenerator::MAX_MARKDOWN_DIFF_ROWS + 5))
            .map(|i| TestCaseResult {
                name: format!("mismatch_{i}"),
                result: TestImageResult::DecodeError {
                    relative_path: format!("{i}.png").into(),
                    error: "bad".to_string(),
                },
            })
            .collect();

        // Doesn't panic or hang when there are more failures than the row cap.
        let signed =
            ReportGenerator::sign_image_urls(&adapter, &test_cases, Duration::from_secs(60)).await;
        assert!(signed.is_empty());
    }
}
