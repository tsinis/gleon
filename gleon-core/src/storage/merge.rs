use crate::manifest::{Manifest, ManifestError, ManifestIndexRevision};

/// Errors that can occur while merging manifests.
#[derive(Debug, thiserror::Error)]
pub enum ManifestMergeError {
    /// The manifests use different algorithms for their content hashes.
    #[error(
        "Cannot merge manifests with different hash algorithms: remote '{remote}', local '{local}'"
    )]
    IncompatibleHashAlgorithm {
        /// Hash algorithm from the remote manifest.
        remote: String,
        /// Hash algorithm from the local manifest.
        local: String,
    },

    /// The manifests use different pixel formats.
    #[error(
        "Cannot merge manifests with different pixel formats: remote '{remote}', local '{local}'"
    )]
    IncompatiblePixelFormat {
        /// Pixel format from the remote manifest.
        remote: String,
        /// Pixel format from the local manifest.
        local: String,
    },

    /// The resulting manifest violates a manifest invariant.
    #[error("Merged manifest is invalid: {source}")]
    InvalidManifest {
        /// Validation error from the merged manifest.
        #[source]
        source: ManifestError,
    },
}

/// Merges local and remote manifests, producing a unified output.
pub struct ManifestMerger;

impl ManifestMerger {
    /// Merges the `local` manifest into the `remote` manifest.
    ///
    /// Rules:
    /// - "Local Wins": If an entry exists in both, the local entry overwrites the remote.
    /// - `version` is incremented by 1 relative to `remote.version`.
    /// - `hash_algo` and `pixel_format` must match between both manifests.
    /// - The result is validated before it is returned.
    ///
    /// # Errors
    /// Returns [`ManifestMergeError`] when metadata is incompatible or the merged manifest is
    /// invalid.
    pub fn merge_manifests(
        remote: &Manifest,
        local: &Manifest,
    ) -> Result<Manifest, ManifestMergeError> {
        if !remote.hash_algo.eq_ignore_ascii_case(&local.hash_algo) {
            return Err(ManifestMergeError::IncompatibleHashAlgorithm {
                remote: remote.hash_algo.clone(),
                local: local.hash_algo.clone(),
            });
        }
        if remote.pixel_format != local.pixel_format {
            return Err(ManifestMergeError::IncompatiblePixelFormat {
                remote: remote.pixel_format.clone(),
                local: local.pixel_format.clone(),
            });
        }

        let mut merged = remote.clone();
        merged.version = remote.version.saturating_add(1);
        merged.entries.extend(local.entries.clone());
        merged
            .validate()
            .map_err(|source| ManifestMergeError::InvalidManifest { source })?;

        Ok(merged)
    }

    /// Merges the `local` index revision into the `remote` index revision.
    ///
    /// Rules:
    /// - "Local Wins": if a test exists in both revisions, the local
    ///   [`TestManifestState`](crate::manifest::TestManifestState)
    ///   overwrites the remote state. In particular, a local `Deleted` tombstone replaces a
    ///   remote `Present` manifest and prevents it from being resurrected.
    /// - The result retains the remote revision metadata, including its parent hashes. This helper
    ///   receives revision contents rather than their content hashes, so it cannot truthfully add
    ///   either input revision as a direct parent of the new revision.
    pub fn merge_index_revisions(
        remote: &ManifestIndexRevision,
        local: &ManifestIndexRevision,
    ) -> ManifestIndexRevision {
        let mut merged = remote.clone();
        for (test_path, state) in &local.test_manifests {
            merged
                .test_manifests
                .insert(test_path.clone(), state.clone());
        }
        merged
    }
}
