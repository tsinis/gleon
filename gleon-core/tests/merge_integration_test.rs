use chrono::Utc;
use gleon_core::manifest::{ImageHash, Manifest, ManifestEntry, SUPPORTED_MANIFEST_SCHEMA_VERSION};
use gleon_core::storage::merge::ManifestMerger;
use std::collections::BTreeMap;

fn create_mock_manifest(version: u64, entries_data: Vec<(&str, &str)>) -> Manifest {
    let mut entries = BTreeMap::new();
    for (path, hash_val) in entries_data {
        entries.insert(
            path.to_string(),
            ManifestEntry {
                hash: ImageHash::new("sha256", hash_val).unwrap(),
                phash: ImageHash::new("dhash", "0000000000000000").unwrap(),
                width: 100,
                height: 100,
                created_at: Utc::now(),
                created_by: "test".to_string(),
                source_commit: "commit".to_string(),
            },
        );
    }
    Manifest {
        schema_version: SUPPORTED_MANIFEST_SCHEMA_VERSION,
        version,
        hash_algo: "sha256".to_string(),
        pixel_format: "rgba".to_string(),
        generator_version: "1.0.0".to_string(),
        entries,
    }
}

#[test]
fn test_manifest_merge_local_wins() {
    let remote = create_mock_manifest(
        10,
        vec![
            (
                "existing_file.png",
                "1111111111111111111111111111111111111111111111111111111111111111",
            ),
            (
                "remote_only.png",
                "2222222222222222222222222222222222222222222222222222222222222222",
            ),
        ],
    );

    let local = create_mock_manifest(
        10,
        vec![
            (
                "existing_file.png",
                "3333333333333333333333333333333333333333333333333333333333333333",
            ), // Changed locally
            (
                "local_only.png",
                "4444444444444444444444444444444444444444444444444444444444444444",
            ),
        ],
    );

    let merged = ManifestMerger::merge_manifests(&remote, &local);

    // Version should be remote.version + 1
    assert_eq!(merged.version, 11);

    // All distinct entries from both should be present
    assert_eq!(merged.entries.len(), 3);

    // "Local Wins": existing_file.png should have local's hash
    assert_eq!(
        merged
            .entries
            .get("existing_file.png")
            .unwrap()
            .hash
            .value(),
        "3333333333333333333333333333333333333333333333333333333333333333"
    );

    // remote_only.png should be preserved
    assert_eq!(
        merged.entries.get("remote_only.png").unwrap().hash.value(),
        "2222222222222222222222222222222222222222222222222222222222222222"
    );

    // local_only.png should be added
    assert_eq!(
        merged.entries.get("local_only.png").unwrap().hash.value(),
        "4444444444444444444444444444444444444444444444444444444444444444"
    );
}

#[test]
fn test_manifest_merge_empty() {
    let remote = create_mock_manifest(1, vec![]);
    let local = create_mock_manifest(1, vec![]);

    let merged = ManifestMerger::merge_manifests(&remote, &local);

    assert_eq!(merged.version, 2);
    assert_eq!(merged.entries.len(), 0);
}
