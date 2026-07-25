use chrono::Utc;
use std::sync::Arc;
use tempfile::tempdir;

use gleon_core::manifest::{
    ImageHash, Manifest, ManifestEntry, ManifestIndexPointer, ManifestIndexRevision,
    SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION, SUPPORTED_MANIFEST_SCHEMA_VERSION, TestManifestState,
};
use gleon_core::storage::adapter::{ObjectStoreAdapter, StorageConfig};
use gleon_core::storage::sync::{SyncOptions, SyncOrchestrator};

#[tokio::test]
#[cfg(not(miri))]
async fn test_sync_orchestrator_push_and_pull() {
    let local_dir = tempdir().unwrap();
    let remote_dir = tempdir().unwrap();

    let local_root = local_dir.path().to_path_buf();
    let remote_url = url::Url::from_directory_path(remote_dir.path())
        .unwrap()
        .to_string();

    let config = StorageConfig {
        url: remote_url,
        aws_access_key_id: None,
        aws_secret_access_key: None,
        aws_region: None,
        aws_endpoint: None,
        r2_account_id: None,
        concurrency: 2,
    };

    let adapter = Arc::new(ObjectStoreAdapter::from_config(&config).unwrap());
    let orchestrator = SyncOrchestrator::new(adapter.clone(), local_root.clone());
    let options = SyncOptions::default();

    // 1. Create a local workspace with a manifest and blobs
    let blob_hash = "1111111111111111111111111111111111111111111111111111111111111111";
    let manifest_blob_hash = "2222222222222222222222222222222222222222222222222222222222222222";

    let blobs_dir = local_root.join(".gleon/blobs/sha256");
    std::fs::create_dir_all(&blobs_dir).unwrap();
    std::fs::write(blobs_dir.join(blob_hash), "fake png data").unwrap();

    let mut entries = std::collections::BTreeMap::new();
    entries.insert(
        "test.png".to_string(),
        ManifestEntry {
            hash: ImageHash::new("sha256", blob_hash).unwrap(),
            phash: ImageHash::new("dhash", "0000000000000000").unwrap(),
            width: 100,
            height: 100,
            created_at: Utc::now(),
            created_by: "test".to_string(),
            source_commit: "commit".to_string(),
        },
    );

    let test_manifest = Manifest {
        schema_version: SUPPORTED_MANIFEST_SCHEMA_VERSION,
        version: 1,
        hash_algo: "sha256".to_string(),
        pixel_format: "rgba".to_string(),
        generator_version: "1.0.0".to_string(),
        parent_hashes: Vec::new(),
        entries,
    };
    let test_manifest_json = serde_json::to_vec(&test_manifest).unwrap();
    std::fs::write(blobs_dir.join(manifest_blob_hash), test_manifest_json).unwrap();

    let revision_hash = "3333333333333333333333333333333333333333333333333333333333333333";
    let mut test_manifests = std::collections::BTreeMap::new();
    test_manifests.insert(
        "test".to_string(),
        TestManifestState::Present(ImageHash::new("sha256", manifest_blob_hash).unwrap()),
    );
    let revision = ManifestIndexRevision {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        parent_hashes: Vec::new(),
        test_manifests,
    };
    std::fs::write(
        blobs_dir.join(revision_hash),
        serde_json::to_vec(&revision).unwrap(),
    )
    .unwrap();

    let branches_dir = local_root.join(".gleon/branches/main/mac");
    std::fs::create_dir_all(&branches_dir).unwrap();
    ManifestIndexPointer {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        revision_hash: ImageHash::new("sha256", revision_hash).unwrap(),
    }
    .save(branches_dir.join("manifest_index.json"))
    .unwrap();

    // 2. Push to remote
    orchestrator.push("main", "mac", &options).await.unwrap();

    // Verify remote has the image, manifest, revision CAS blobs, and pointer.
    assert!(adapter.blob_exists(blob_hash).await.unwrap());
    assert!(adapter.blob_exists(manifest_blob_hash).await.unwrap());
    assert!(adapter.blob_exists(revision_hash).await.unwrap());
    let remote_pointer: ManifestIndexPointer =
        serde_json::from_slice(&adapter.download_manifest("main", "mac").await.unwrap()).unwrap();
    assert_eq!(remote_pointer.revision_hash.value(), revision_hash);

    // 3. Pull from remote into a fresh local workspace
    let fresh_local_dir = tempdir().unwrap();
    let fresh_local_root = fresh_local_dir.path().to_path_buf();

    let pull_orchestrator = SyncOrchestrator::new(adapter.clone(), fresh_local_root.clone());
    pull_orchestrator
        .pull("main", "mac", &options)
        .await
        .unwrap();

    // Verify the fresh workspace resolves pointer -> revision -> manifest/image CAS blobs.
    assert!(
        fresh_local_root
            .join(".gleon/blobs/sha256")
            .join(blob_hash)
            .exists()
    );
    assert!(
        fresh_local_root
            .join(".gleon/blobs/sha256")
            .join(manifest_blob_hash)
            .exists()
    );
    let fresh_pointer = ManifestIndexPointer::load(
        fresh_local_root.join(".gleon/branches/main/mac/manifest_index.json"),
    )
    .unwrap();
    assert_eq!(fresh_pointer.revision_hash.value(), revision_hash);
    let fresh_revision = ManifestIndexRevision::load(
        fresh_local_root
            .join(".gleon/blobs/sha256")
            .join(fresh_pointer.revision_hash.value()),
    )
    .unwrap();
    assert!(matches!(
        fresh_revision.test_manifests.get("test"),
        Some(TestManifestState::Present(hash)) if hash.value() == manifest_blob_hash
    ));
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_sync_orchestrator_pull_corrupt_manifest_fails() {
    let local_dir = tempdir().unwrap();
    let remote_dir = tempdir().unwrap();

    let local_root = local_dir.path().to_path_buf();
    let remote_url = url::Url::from_directory_path(remote_dir.path())
        .unwrap()
        .to_string();

    let config = StorageConfig {
        url: remote_url,
        aws_access_key_id: None,
        aws_secret_access_key: None,
        aws_region: None,
        aws_endpoint: None,
        r2_account_id: None,
        concurrency: 2,
    };

    let adapter = Arc::new(ObjectStoreAdapter::from_config(&config).unwrap());
    let orchestrator = SyncOrchestrator::new(adapter.clone(), local_root.clone());
    let options = SyncOptions::default();

    // Store a corrupt manifest blob on remote
    let manifest_blob_hash = "3333333333333333333333333333333333333333333333333333333333333333";
    let remote_blobs_dir = remote_dir.path().join("blobs/sha256");
    std::fs::create_dir_all(&remote_blobs_dir).unwrap();
    std::fs::write(
        remote_blobs_dir.join(manifest_blob_hash),
        "{ invalid json }",
    )
    .unwrap();

    let revision_hash = "4444444444444444444444444444444444444444444444444444444444444444";
    let mut test_manifests = std::collections::BTreeMap::new();
    test_manifests.insert(
        "test".to_string(),
        TestManifestState::Present(ImageHash::new("sha256", manifest_blob_hash).unwrap()),
    );
    let revision = ManifestIndexRevision {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        parent_hashes: Vec::new(),
        test_manifests,
    };
    std::fs::write(
        remote_blobs_dir.join(revision_hash),
        serde_json::to_vec(&revision).unwrap(),
    )
    .unwrap();

    let remote_index_dir = remote_dir.path().join("branches/main/mac");
    std::fs::create_dir_all(&remote_index_dir).unwrap();
    let pointer = ManifestIndexPointer {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        revision_hash: ImageHash::new("sha256", revision_hash).unwrap(),
    };
    std::fs::write(
        remote_index_dir.join("manifest_index.json"),
        serde_json::to_vec(&pointer).unwrap(),
    )
    .unwrap();

    // Pulling should fail because the manifest blob is corrupted
    let result = orchestrator.pull("main", "mac", &options).await;
    assert!(
        result.is_err(),
        "Expected error when pulling corrupted manifest blob"
    );
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_sync_orchestrator_push_missing_index_returns_ok() {
    let local_dir = tempdir().unwrap();
    let remote_dir = tempdir().unwrap();

    let local_root = local_dir.path().to_path_buf();
    let remote_url = format!("file://{}", remote_dir.path().display());

    let config = StorageConfig::new(remote_url);
    let adapter = Arc::new(ObjectStoreAdapter::from_config(&config).unwrap());
    let orchestrator = SyncOrchestrator::new(adapter.clone(), local_root);
    let options = SyncOptions::default();

    let result = orchestrator.push("main", "mac", &options).await;
    assert!(result.is_ok());
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_sync_orchestrator_push_missing_local_blob_fails() {
    let local_dir = tempdir().unwrap();
    let remote_dir = tempdir().unwrap();

    let local_root = local_dir.path().to_path_buf();
    let remote_url = format!("file://{}", remote_dir.path().display());

    let config = StorageConfig::new(remote_url);
    let adapter = Arc::new(ObjectStoreAdapter::from_config(&config).unwrap());
    let orchestrator = SyncOrchestrator::new(adapter.clone(), local_root.clone());
    let options = SyncOptions::default();

    // Create a local index that references a manifest
    let manifest_blob_hash = "2222222222222222222222222222222222222222222222222222222222222222";
    let blob_hash = "1111111111111111111111111111111111111111111111111111111111111111";

    let revision_hash = "3333333333333333333333333333333333333333333333333333333333333333";
    let mut test_manifests = std::collections::BTreeMap::new();
    test_manifests.insert(
        "test".to_string(),
        TestManifestState::Present(ImageHash::new("sha256", manifest_blob_hash).unwrap()),
    );
    let revision = ManifestIndexRevision {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        parent_hashes: Vec::new(),
        test_manifests,
    };

    let branches_dir = local_root.join(".gleon/branches/main/mac");
    std::fs::create_dir_all(&branches_dir).unwrap();
    ManifestIndexPointer {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        revision_hash: ImageHash::new("sha256", revision_hash).unwrap(),
    }
    .save(branches_dir.join("manifest_index.json"))
    .unwrap();

    // Create the manifest, but deliberately OMIT the referenced image blob from disk
    let mut entries = std::collections::BTreeMap::new();
    entries.insert(
        "test.png".to_string(),
        ManifestEntry {
            hash: ImageHash::new("sha256", blob_hash).unwrap(),
            phash: ImageHash::new("dhash", "0000000000000000").unwrap(),
            width: 100,
            height: 100,
            created_at: Utc::now(),
            created_by: "test".to_string(),
            source_commit: "commit".to_string(),
        },
    );

    let test_manifest = Manifest {
        schema_version: SUPPORTED_MANIFEST_SCHEMA_VERSION,
        version: 1,
        hash_algo: "sha256".to_string(),
        pixel_format: "rgba".to_string(),
        generator_version: "1.0.0".to_string(),
        parent_hashes: Vec::new(),
        entries,
    };
    let test_manifest_json = serde_json::to_vec(&test_manifest).unwrap();
    let blobs_dir = local_root.join(".gleon/blobs/sha256");
    std::fs::create_dir_all(&blobs_dir).unwrap();
    std::fs::write(blobs_dir.join(manifest_blob_hash), test_manifest_json).unwrap();
    std::fs::write(
        blobs_dir.join(revision_hash),
        serde_json::to_vec(&revision).unwrap(),
    )
    .unwrap();

    // DO NOT write `blob_hash` to disk!

    let result = orchestrator.push("main", "mac", &options).await;
    assert!(
        result.is_err(),
        "Push should fail because a locally referenced blob is missing from disk"
    );
}
