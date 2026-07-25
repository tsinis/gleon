use chrono::Utc;
use gleon_core::manifest::{ImageHash, Manifest, ManifestEntry, SUPPORTED_MANIFEST_SCHEMA_VERSION};
use gleon_core::storage::merge::{ManifestMergeError, ManifestMerger};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

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
        parent_hashes: Vec::new(),
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

    let merged =
        ManifestMerger::merge_manifests(&remote, &local).expect("compatible manifests must merge");

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

    let merged =
        ManifestMerger::merge_manifests(&remote, &local).expect("compatible manifests must merge");

    assert_eq!(merged.version, 2);
    assert_eq!(merged.entries.len(), 0);
}

#[test]
fn test_manifest_merge_local_empty_preserves_remote() {
    let remote = create_mock_manifest(
        7,
        vec![(
            "remote_only.png",
            "1111111111111111111111111111111111111111111111111111111111111111",
        )],
    );
    let local = create_mock_manifest(2, vec![]);

    let merged =
        ManifestMerger::merge_manifests(&remote, &local).expect("compatible manifests must merge");

    assert_eq!(merged.version, 8);
    assert_eq!(merged.entries, remote.entries);
}

#[test]
fn test_manifest_merge_remote_empty_adds_local() {
    let remote = create_mock_manifest(7, vec![]);
    let local = create_mock_manifest(
        2,
        vec![(
            "local_only.png",
            "1111111111111111111111111111111111111111111111111111111111111111",
        )],
    );

    let merged =
        ManifestMerger::merge_manifests(&remote, &local).expect("compatible manifests must merge");

    assert_eq!(merged.version, 8);
    assert_eq!(merged.entries, local.entries);
}

#[test]
fn test_manifest_merge_version_saturates_at_u64_max() {
    let remote = create_mock_manifest(u64::MAX, vec![]);
    let local = create_mock_manifest(0, vec![]);

    let merged =
        ManifestMerger::merge_manifests(&remote, &local).expect("compatible manifests must merge");

    assert_eq!(merged.version, u64::MAX);
}

#[test]
fn test_manifest_merge_from_json_fixtures() {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let remote = Manifest::load(fixtures_dir.join("merge_remote_manifest.json"))
        .expect("remote manifest fixture must be valid");
    let local = Manifest::load(fixtures_dir.join("merge_local_manifest.json"))
        .expect("local manifest fixture must be valid");

    let merged = ManifestMerger::merge_manifests(&remote, &local)
        .expect("compatible fixture manifests must merge");
    let actual = serde_json::to_value(merged).expect("merged manifest must serialize");
    let expected: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(fixtures_dir.join("merge_expected_manifest.json"))
            .expect("expected manifest fixture must exist"),
    )
    .expect("expected manifest fixture must contain valid JSON");

    assert_eq!(actual, expected);
}

#[test]
fn test_manifest_merge_rejects_incompatible_hash_algorithms() {
    let remote = create_mock_manifest(1, vec![]);
    let mut local = create_mock_manifest(1, vec![]);
    local.hash_algo = "blake3".to_string();

    let result = ManifestMerger::merge_manifests(&remote, &local);

    assert!(matches!(
        result,
        Err(ManifestMergeError::IncompatibleHashAlgorithm { .. })
    ));
}

#[test]
fn test_manifest_merge_rejects_incompatible_pixel_formats() {
    let remote = create_mock_manifest(1, vec![]);
    let mut local = create_mock_manifest(1, vec![]);
    local.pixel_format = "rgb".to_string();

    let result = ManifestMerger::merge_manifests(&remote, &local);

    assert!(matches!(
        result,
        Err(ManifestMergeError::IncompatiblePixelFormat { .. })
    ));
}

#[test]
fn test_manifest_merge_rejects_invalid_merged_manifest() {
    let remote = create_mock_manifest(1, vec![]);
    let mut local = create_mock_manifest(1, vec![]);
    local.entries.insert(
        "invalid.png".to_string(),
        ManifestEntry {
            hash: ImageHash::new("dhash", "0000000000000000").unwrap(),
            phash: ImageHash::new("dhash", "0000000000000000").unwrap(),
            width: 100,
            height: 100,
            created_at: Utc::now(),
            created_by: "test".to_string(),
            source_commit: "commit".to_string(),
        },
    );

    let result = ManifestMerger::merge_manifests(&remote, &local);

    assert!(matches!(
        result,
        Err(ManifestMergeError::InvalidManifest { .. })
    ));
}

#[test]
fn test_manifest_index_revision_merge_local_wins() {
    use gleon_core::manifest::{
        ManifestIndexRevision, SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION, TestManifestState,
    };

    let mut remote_manifests = BTreeMap::new();
    remote_manifests.insert(
        "test1".to_string(),
        TestManifestState::Present(
            ImageHash::new(
                "sha256",
                "1111111111111111111111111111111111111111111111111111111111111111",
            )
            .unwrap(),
        ),
    );
    remote_manifests.insert(
        "remote_only".to_string(),
        TestManifestState::Present(
            ImageHash::new(
                "sha256",
                "2222222222222222222222222222222222222222222222222222222222222222",
            )
            .unwrap(),
        ),
    );
    let remote_revision = ManifestIndexRevision {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        parent_hashes: Vec::new(),
        test_manifests: remote_manifests,
    };

    let mut local_manifests = BTreeMap::new();
    local_manifests.insert(
        "test1".to_string(),
        TestManifestState::Present(
            ImageHash::new(
                "sha256",
                "3333333333333333333333333333333333333333333333333333333333333333",
            )
            .unwrap(),
        ),
    );
    local_manifests.insert(
        "local_only".to_string(),
        TestManifestState::Present(
            ImageHash::new(
                "sha256",
                "4444444444444444444444444444444444444444444444444444444444444444",
            )
            .unwrap(),
        ),
    );
    let local_revision = ManifestIndexRevision {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        parent_hashes: Vec::new(),
        test_manifests: local_manifests,
    };

    let merged_revision = ManifestMerger::merge_index_revisions(&remote_revision, &local_revision);

    assert_eq!(merged_revision.test_manifests.len(), 3);
    assert_eq!(
        merged_revision.test_manifests.get("test1"),
        Some(&TestManifestState::Present(
            ImageHash::new(
                "sha256",
                "3333333333333333333333333333333333333333333333333333333333333333",
            )
            .unwrap(),
        ))
    );
    assert_eq!(
        merged_revision.test_manifests.get("remote_only"),
        Some(&TestManifestState::Present(
            ImageHash::new(
                "sha256",
                "2222222222222222222222222222222222222222222222222222222222222222",
            )
            .unwrap(),
        ))
    );
    assert_eq!(
        merged_revision.test_manifests.get("local_only"),
        Some(&TestManifestState::Present(
            ImageHash::new(
                "sha256",
                "4444444444444444444444444444444444444444444444444444444444444444",
            )
            .unwrap(),
        ))
    );
}
