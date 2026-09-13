//! Remote storage garbage collector orchestration for pruning unreferenced baseline blobs.

use std::collections::HashSet;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use tracing::{info, instrument, warn};

use crate::context::ResolvedContext;
use crate::manifest::ImageHash;
use crate::ops::common::{CoreError, ensure_initialized};
pub use crate::storage::RemoteBlobEntry;
use crate::storage::StorageError;
use crate::storage::adapter::{ObjectStoreAdapter, StorageConfig};

/// Errors that can occur during storage garbage collection.
#[derive(Debug, thiserror::Error)]
pub enum GcError {
    /// Remote storage operation failed.
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),

    /// Manifest or workspace operation failed.
    #[error(transparent)]
    Core(#[from] CoreError),

    /// Git repository discovery failed when remote storage is configured.
    #[error(
        "Remote storage garbage collection requires a Git repository to resolve active branch manifests. Run inside a Git repository or pass --force."
    )]
    GitRequired,

    /// Repository is a shallow clone lacking full history.
    #[error(
        "Git repository is a shallow clone; commit history is incomplete. Baseline blobs of other branches would be permanently deleted. Run 'git fetch --unshallow' (or configure 'fetch-depth: 0' in CI), or pass --force."
    )]
    ShallowClone,

    /// Repository contains too few references to guarantee safe branch discovery.
    #[error(
        "Repository contains only {0} tracked reference(s). Remote branches may not be fetched, risking baseline data loss. Fetch all branches or pass --force."
    )]
    InsufficientRefs(usize),

    /// Grace period specified is less than the 24-hour minimum.
    #[error(
        "Grace period must be at least 24 hours to prevent race conditions with concurrent uploads."
    )]
    GracePeriodTooShort,

    /// Git tree traversal encountered unrecoverable errors.
    #[error(
        "Git tree traversal encountered {failures} error(s). Halting garbage collection to prevent deleting valid baselines. Fix Git repository state or pass --force."
    )]
    UnsafeScan {
        /// Number of unreadable Git commits, trees, or test manifests.
        failures: usize,
    },

    /// Failed to resolve relative path in monorepo subfolder.
    #[error("Failed to resolve monorepo subfolder relative to repository root: '{0}'")]
    MonorepoResolutionFailed(String),
}

/// Operating mode of a completed garbage collection run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcMode {
    /// Local flat mode; remote storage is not configured so no remote operations occurred.
    LocalMode,
    /// Dry-run mode; orphan blobs were identified and reported without deletion.
    DryRun,
    /// Execution mode; orphan blobs were deleted from remote storage.
    Executed,
}

/// Options controlling storage garbage collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcOptions {
    /// When true, reports what would be deleted without actually deleting blobs.
    pub dry_run: bool,
    /// Minimum age required before an unreferenced blob is eligible for deletion.
    pub grace_period: Duration,
    /// When true, bypasses safety checks (shallow clone, single ref, 0-hour grace period, non-git).
    pub force: bool,
}

impl GcOptions {
    /// Creates a new `GcOptions` with `dry_run`, `grace_period_hours`, and `force`.
    #[must_use]
    pub fn new(dry_run: bool, grace_period_hours: u32, force: bool) -> Self {
        Self {
            dry_run,
            grace_period: Duration::hours(i64::from(grace_period_hours)),
            force,
        }
    }
}

impl Default for GcOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            grace_period: Duration::hours(24),
            force: false,
        }
    }
}

/// Results of a garbage collection operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcResult {
    /// Execution mode.
    pub mode: GcMode,
    /// Total number of blobs found in remote storage.
    pub total_remote_blobs: usize,
    /// Number of remote blobs actively referenced by at least one manifest.
    pub referenced_blobs: usize,
    /// Number of unreferenced blobs protected by the grace period window.
    pub protected_by_grace_period: usize,
    /// Number of orphan blobs deleted (or identified for deletion in dry-run mode).
    pub deleted_blobs: usize,
    /// Number of orphan blobs that failed deletion.
    pub failed_blobs: usize,
    /// Total bytes freed (or identified to be freed in dry-run mode).
    pub bytes_freed: u64,
    /// List of orphan blob entries identified.
    pub orphans: Vec<RemoteBlobEntry>,
}

impl Default for GcResult {
    fn default() -> Self {
        Self {
            mode: GcMode::LocalMode,
            total_remote_blobs: 0,
            referenced_blobs: 0,
            protected_by_grace_period: 0,
            deleted_blobs: 0,
            failed_blobs: 0,
            bytes_freed: 0,
            orphans: Vec::new(),
        }
    }
}

/// Pure partitioning logic to divide remote blobs into orphans, protected, and referenced.
///
/// Every remote blob falls into exactly one category:
/// 1. Referenced: present in `referenced_hashes`.
/// 2. Protected by grace period: unreferenced, but `now - last_modified < grace_period`.
///    (Note: if `last_modified > now`, `now - last_modified` is negative, which is `< grace_period`,
///    so clock skew is naturally protected without special-casing).
/// 3. Orphan: unreferenced and older than `grace_period`.
#[must_use]
pub fn partition_remote_blobs<S: std::hash::BuildHasher>(
    remote_blobs: Vec<RemoteBlobEntry>,
    referenced_hashes: &HashSet<ImageHash, S>,
    now: DateTime<Utc>,
    grace_period: Duration,
) -> (Vec<RemoteBlobEntry>, usize, usize) {
    let mut orphans = Vec::new();
    let mut protected_by_grace_period = 0;
    let mut referenced_blobs = 0;

    for blob in remote_blobs {
        if referenced_hashes.contains(&blob.hash) {
            referenced_blobs += 1;
        } else {
            let age = now.signed_duration_since(blob.last_modified);
            if age < grace_period {
                protected_by_grace_period += 1;
            } else {
                orphans.push(blob);
            }
        }
    }

    (orphans, protected_by_grace_period, referenced_blobs)
}

/// Backward-compatible wrapper filtering borrowed slices of blobs.
#[must_use]
pub fn filter_orphans_to_delete<'a, S: std::hash::BuildHasher>(
    remote_blobs: &'a [RemoteBlobEntry],
    referenced_hashes: &HashSet<ImageHash, S>,
    now: DateTime<Utc>,
    grace_period: Duration,
) -> (Vec<&'a RemoteBlobEntry>, usize) {
    let mut to_delete = Vec::new();
    let mut protected_by_grace_period = 0;

    for blob in remote_blobs {
        if referenced_hashes.contains(&blob.hash) {
            continue;
        }

        let age = now.signed_duration_since(blob.last_modified);
        if age < grace_period {
            protected_by_grace_period += 1;
        } else {
            to_delete.push(blob);
        }
    }

    (to_delete, protected_by_grace_period)
}

#[derive(serde::Deserialize)]
struct ManifestHashOnly {
    hash: ImageHash,
}

/// Scans the manifest directory in the tree of the given commit, recording all
/// referenced `ImageHash` values.
fn scan_commit_manifests(
    repo: &gix::Repository,
    commit: &gix::Commit<'_>,
    manifests_path: &str,
    visited_trees: &mut HashSet<gix::ObjectId>,
    referenced_hashes: &mut HashSet<ImageHash>,
    scan_failures: &mut usize,
) {
    let Ok(tree) = commit.tree() else {
        *scan_failures += 1;
        return;
    };
    let Ok(entry_opt) = tree.lookup_entry_by_path(manifests_path) else {
        *scan_failures += 1;
        return;
    };
    let Some(entry) = entry_opt else {
        // Normal case: commit does not contain this manifest path (e.g. branch created before gleon)
        return;
    };
    let Ok(obj) = entry.object() else {
        *scan_failures += 1;
        return;
    };
    let Ok(manifest_tree) = obj.try_into_tree() else {
        *scan_failures += 1;
        return;
    };

    // Optimization: avoid re-scanning duplicate manifest trees across branches
    if !visited_trees.insert(manifest_tree.id) {
        return;
    }

    let mut recorder = gix::traverse::tree::Recorder::default();
    if manifest_tree
        .traverse()
        .breadthfirst(&mut recorder)
        .is_err()
    {
        *scan_failures += 1;
        return;
    }

    for record in recorder.records {
        if record.mode.is_blob() && record.filepath.ends_with(b".json") {
            if let Ok(blob) = repo.find_object(record.oid) {
                if let Ok(m) = serde_json::from_slice::<ManifestHashOnly>(&blob.data) {
                    referenced_hashes.insert(m.hash);
                } else {
                    *scan_failures += 1;
                }
            } else {
                *scan_failures += 1;
            }
        }
    }
}

fn resolve_manifests_git_path(base_dir: &Path, repo: &gix::Repository) -> Result<String, GcError> {
    repo.workdir().map_or_else(
        || Ok(".gleon/manifests".to_string()),
        |workdir| {
            let canonical_base = base_dir
                .canonicalize()
                .unwrap_or_else(|_| base_dir.to_path_buf());
            let canonical_work = workdir
                .canonicalize()
                .unwrap_or_else(|_| workdir.to_path_buf());
            let rel_base = match canonical_base.strip_prefix(&canonical_work) {
                Ok(p) => p,
                Err(_) => match base_dir.strip_prefix(workdir) {
                    Ok(p) => p,
                    Err(e) => {
                        return Err(GcError::MonorepoResolutionFailed(e.to_string()));
                    }
                },
            };
            let rel_str = rel_base.to_string_lossy();
            let normalized_rel = crate::naming::normalize_path_separators(&rel_str);
            if normalized_rel.is_empty() {
                Ok(".gleon/manifests".to_string())
            } else {
                Ok(format!("{normalized_rel}/.gleon/manifests"))
            }
        },
    )
}

fn scan_all_tracked_refs(
    repo: &gix::Repository,
    manifests_git_path: &str,
    visited_trees: &mut HashSet<gix::ObjectId>,
    referenced_hashes: &mut HashSet<ImageHash>,
    scan_failures: &mut usize,
) -> usize {
    let mut tracked_refs_count = 0;
    let Ok(platform) = repo.references() else {
        *scan_failures += 1;
        return 0;
    };
    let Ok(all_refs) = platform.all() else {
        *scan_failures += 1;
        return 0;
    };

    for reference_res in all_refs {
        let Ok(reference) = reference_res else {
            *scan_failures += 1;
            continue;
        };
        let name = reference.name().as_bstr();
        let is_tracked = name.starts_with(b"refs/heads/")
            || name.starts_with(b"refs/remotes/")
            || name.starts_with(b"refs/tags/")
            || name.starts_with(b"refs/pull/");
        if !is_tracked {
            continue;
        }
        tracked_refs_count += 1;

        if let Ok(id) = reference.into_fully_peeled_id() {
            let commit_opt = repo.find_commit(id).ok().or_else(|| {
                repo.find_object(id)
                    .ok()
                    .and_then(|obj| obj.peel_to_commit().ok())
            });
            if let Some(commit) = commit_opt {
                scan_commit_manifests(
                    repo,
                    &commit,
                    manifests_git_path,
                    visited_trees,
                    referenced_hashes,
                    scan_failures,
                );
            }
        }
    }
    tracked_refs_count
}

/// Collects all image hashes referenced across local workspace manifests
/// AND all discoverable Git branch/tag commits (refs/heads/*, refs/remotes/*, refs/tags/*, refs/pull/*, and HEAD).
///
/// # Errors
/// Returns [`GcError`] if Git repository discovery, branch iteration, or manifest parsing fails.
#[instrument(skip(base_dir, options), level = "debug")]
pub fn collect_all_referenced_hashes(
    base_dir: &Path,
    options: &GcOptions,
) -> Result<HashSet<ImageHash>, GcError> {
    let manifests_root = base_dir.join(".gleon").join("manifests");
    let local_platform_dirs =
        crate::ops::sync::list_platform_dirs(&manifests_root).map_err(CoreError::Io)?;
    let local_referenced = crate::ops::sync::collect_referenced_hashes(&local_platform_dirs)?;
    let mut referenced_hashes: HashSet<ImageHash> = local_referenced.into_keys().collect();

    let Ok(repo) = gix::discover(base_dir) else {
        if !options.force && !options.dry_run {
            return Err(GcError::GitRequired);
        }
        warn!(
            location = %base_dir.display(),
            "No Git repository discovered; operating with local workspace manifests only"
        );
        return Ok(referenced_hashes);
    };

    let is_shallow = repo.shallow_file().exists();
    if is_shallow {
        if !options.force && !options.dry_run {
            return Err(GcError::ShallowClone);
        }
        warn!(
            "Git repository is a shallow clone; commit history is incomplete. \
             Remote baseline blobs referenced only by older commits or un-fetched branches \
             may be collected as orphans. Consider configuring 'fetch-depth: 0' in CI."
        );
    }

    let manifests_git_path = resolve_manifests_git_path(base_dir, &repo)?;

    let mut visited_trees = HashSet::new();
    let mut scan_failures = 0;

    if let Ok(head_commit) = repo.head_commit() {
        scan_commit_manifests(
            &repo,
            &head_commit,
            &manifests_git_path,
            &mut visited_trees,
            &mut referenced_hashes,
            &mut scan_failures,
        );
    }

    let tracked_refs_count = scan_all_tracked_refs(
        &repo,
        &manifests_git_path,
        &mut visited_trees,
        &mut referenced_hashes,
        &mut scan_failures,
    );

    if tracked_refs_count <= 1 && !options.force && !options.dry_run {
        return Err(GcError::InsufficientRefs(tracked_refs_count));
    }

    if scan_failures > 0 {
        if !options.force && !options.dry_run {
            return Err(GcError::UnsafeScan {
                failures: scan_failures,
            });
        }
        warn!(
            failures = scan_failures,
            "Git tree traversal encountered errors; continuing due to --force or --dry-run"
        );
    }

    info!(
        total_referenced = referenced_hashes.len(),
        scanned_trees = visited_trees.len(),
        tracked_refs = tracked_refs_count,
        "Collected referenced blob hashes from workspace and Git branches"
    );

    Ok(referenced_hashes)
}

/// Orchestrates remote storage garbage collection.
///
/// 1. Verifies storage configuration (returns early with `mode = LocalMode` if not configured).
/// 2. Enforces safety guardrails (grace period >= 24h, uninitialized workspace check).
/// 3. Captures current timestamp `now` before querying remote storage.
/// 4. Lists all remote CAS blobs under `blobs/`.
/// 5. Collects referenced hashes across local workspace and Git branches.
/// 6. Partitions remote blobs into orphans, protected, and referenced.
/// 7. Batch deletes orphan blobs in parallel via `delete_stream` (unless `dry_run` is requested).
///
/// # Errors
/// Returns [`GcError`] if safety preconditions fail or remote storage operations error.
#[instrument(skip(context, storage_config), level = "info")]
pub async fn garbage_collect(
    context: &ResolvedContext,
    storage_config: Option<&StorageConfig>,
    options: &GcOptions,
) -> Result<GcResult, GcError> {
    let Some(storage_cfg) = crate::ops::sync::active_storage_config(storage_config) else {
        return Ok(GcResult {
            mode: GcMode::LocalMode,
            ..Default::default()
        });
    };

    // Fail fast if workspace is not initialized
    ensure_initialized(&context.base_dir)?;

    // Enforce grace period guardrail
    if options.grace_period < Duration::hours(24) {
        return Err(GcError::GracePeriodTooShort);
    }

    let adapter = ObjectStoreAdapter::from_config(storage_cfg).map_err(GcError::Storage)?;

    // Safe Ordering:
    // 1. Capture current timestamp before listing remote blobs
    let now = Utc::now();

    // 2. Query remote blobs
    let remote_blobs = adapter.list_all_blobs().await.map_err(GcError::Storage)?;
    let total_remote_blobs = remote_blobs.len();

    // 3. Collect all referenced hashes across local workspace & Git branches
    let referenced_hashes = collect_all_referenced_hashes(&context.base_dir, options)?;

    // 4. Partition remote blobs
    let (orphans, protected_by_grace_period, referenced_blobs) =
        partition_remote_blobs(remote_blobs, &referenced_hashes, now, options.grace_period);

    let orphan_bytes: u64 = orphans.iter().map(|b| b.size).sum();
    let orphan_count = orphans.len();

    if options.dry_run {
        info!(
            total = total_remote_blobs,
            referenced = referenced_blobs,
            protected = protected_by_grace_period,
            orphans = orphan_count,
            bytes = orphan_bytes,
            "[DRY RUN] Identified orphan blobs for deletion"
        );
        return Ok(GcResult {
            mode: GcMode::DryRun,
            total_remote_blobs,
            referenced_blobs,
            protected_by_grace_period,
            deleted_blobs: orphan_count,
            failed_blobs: 0,
            bytes_freed: orphan_bytes,
            orphans,
        });
    }

    // 5. Perform batch deletion via delete_blobs_with_progress
    let (deleted_count, failed_count, bytes_freed) = if orphans.is_empty() {
        (0, 0, 0)
    } else {
        let summary = adapter
            .delete_blobs_with_progress(orphans.iter().map(|b| &b.hash), |hash, success| {
                if !success {
                    warn!(hash = %hash, "Failed to delete orphan blob during batch deletion");
                }
            })
            .await
            .map_err(GcError::Storage)?;

        let bytes_freed: u64 = orphans
            .iter()
            .filter(|b| summary.deleted_hashes.contains(&b.hash))
            .map(|b| b.size)
            .sum();

        (summary.deleted, summary.failed, bytes_freed)
    };

    info!(
        total = total_remote_blobs,
        referenced = referenced_blobs,
        protected = protected_by_grace_period,
        deleted = deleted_count,
        failed = failed_count,
        bytes = bytes_freed,
        "Storage garbage collection completed"
    );

    Ok(GcResult {
        mode: GcMode::Executed,
        total_remote_blobs,
        referenced_blobs,
        protected_by_grace_period,
        deleted_blobs: deleted_count,
        failed_blobs: failed_count,
        bytes_freed,
        orphans,
    })
}
