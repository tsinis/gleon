use crate::manifest::Manifest;

/// Merges local and remote manifests, producing a unified output.
pub struct ManifestMerger;

impl ManifestMerger {
    /// Merges the `local` manifest into the `remote` manifest.
    ///
    /// Rules:
    /// - "Local Wins": If an entry exists in both, the local entry overwrites the remote.
    /// - `version` is incremented by 1 relative to `remote.version`.
    /// - All other metadata (schema version, algo, format) is preserved from `remote`,
    ///   assuming the sync process has already validated compatibility.
    pub fn merge_manifests(remote: &Manifest, local: &Manifest) -> Manifest {
        let mut merged = remote.clone();

        merged.version = remote.version.saturating_add(1);

        for (path, entry) in &local.entries {
            merged.entries.insert(path.clone(), entry.clone());
        }

        merged
    }

    /// Merges the `local` manifest index into the `remote` manifest index.
    ///
    /// Rules:
    /// - "Local Wins": If a test exists in both, the local hash overwrites the remote.
    pub fn merge_indexes(
        remote: &crate::manifest::ManifestIndex,
        local: &crate::manifest::ManifestIndex,
    ) -> crate::manifest::ManifestIndex {
        let mut merged = remote.clone();
        for (path, hash) in &local.test_manifests {
            merged.test_manifests.insert(path.clone(), hash.clone());
        }
        merged
    }
}
