use chrono::Utc;
use std::sync::Arc;
use tempfile::tempdir;

fn store_revision(blob_dir: &std::path::Path, revision: &ManifestIndexRevision) -> ImageHash {
    use sha2::Digest;

    let bytes = serde_json::to_vec_pretty(revision).unwrap();
    let hash = ImageHash::new("sha256", hex::encode(sha2::Sha256::digest(&bytes))).unwrap();
    std::fs::create_dir_all(blob_dir).unwrap();
    std::fs::write(blob_dir.join(hash.value()), bytes).unwrap();
    hash
}

fn store_manifest(blob_dir: &std::path::Path, manifest: &Manifest) -> ImageHash {
    use sha2::Digest;

    let bytes = serde_json::to_vec_pretty(manifest).unwrap();
    let hash = ImageHash::new("sha256", hex::encode(sha2::Sha256::digest(&bytes))).unwrap();
    std::fs::create_dir_all(blob_dir).unwrap();
    std::fs::write(blob_dir.join(hash.value()), bytes).unwrap();
    hash
}

fn save_local_pointer(root: &std::path::Path, revision_hash: ImageHash) {
    let pointer_path = root.join(".gleon/branches/main/mac/manifest_index.json");
    std::fs::create_dir_all(pointer_path.parent().unwrap()).unwrap();
    ManifestIndexPointer {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        revision_hash,
    }
    .save(pointer_path)
    .unwrap();
}

fn save_remote_pointer(root: &std::path::Path, revision_hash: ImageHash) {
    let pointer_path = root.join("branches/main/mac/manifest_index.json");
    std::fs::create_dir_all(pointer_path.parent().unwrap()).unwrap();
    std::fs::write(
        pointer_path,
        serde_json::to_vec_pretty(&ManifestIndexPointer {
            schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
            revision_hash,
        })
        .unwrap(),
    )
    .unwrap();
}

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
async fn test_sync_orchestrator_pull_fast_forwards_to_remote_deleted_descendant() {
    let local_dir = tempdir().unwrap();
    let remote_dir = tempdir().unwrap();
    let local_root = local_dir.path();
    let remote_root = remote_dir.path();

    let present_manifest = ImageHash::new("sha256", "1".repeat(64)).unwrap();
    let mut base_manifests = std::collections::BTreeMap::new();
    base_manifests.insert(
        "removed-test".to_string(),
        TestManifestState::Present(present_manifest),
    );
    let base = ManifestIndexRevision {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        parent_hashes: Vec::new(),
        test_manifests: base_manifests,
    };
    let base_hash = store_revision(&local_root.join(".gleon/blobs/sha256"), &base);
    store_revision(&remote_root.join("blobs/sha256"), &base);
    save_local_pointer(local_root, base_hash.clone());

    let mut descendant_manifests = std::collections::BTreeMap::new();
    descendant_manifests.insert("removed-test".to_string(), TestManifestState::Deleted);
    let descendant = ManifestIndexRevision {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        parent_hashes: vec![base_hash],
        test_manifests: descendant_manifests,
    };
    let descendant_hash = store_revision(&remote_root.join("blobs/sha256"), &descendant);
    save_remote_pointer(remote_root, descendant_hash.clone());

    let adapter = Arc::new(
        ObjectStoreAdapter::from_config(&StorageConfig::new(format!(
            "file://{}",
            remote_root.display()
        )))
        .unwrap(),
    );
    SyncOrchestrator::new(adapter, local_root.to_path_buf())
        .pull("main", "mac", &SyncOptions::default())
        .await
        .unwrap();

    let pointer =
        ManifestIndexPointer::load(local_root.join(".gleon/branches/main/mac/manifest_index.json"))
            .unwrap();
    assert_eq!(pointer.revision_hash, descendant_hash);
    let revision = ManifestIndexRevision::load(
        local_root
            .join(".gleon/blobs/sha256")
            .join(pointer.revision_hash.value()),
    )
    .unwrap();
    assert!(matches!(
        revision.test_manifests.get("removed-test"),
        Some(TestManifestState::Deleted)
    ));
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_sync_orchestrator_push_retries_competing_occ_writer() {
    let base = ManifestIndexRevision {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        parent_hashes: Vec::new(),
        test_manifests: std::collections::BTreeMap::new(),
    };
    let first_dir = tempdir().unwrap();
    let second_dir = tempdir().unwrap();
    let first_root = first_dir.path();
    let second_root = second_dir.path();
    let base_hash = store_revision(&first_root.join(".gleon/blobs/sha256"), &base);
    store_revision(&second_root.join(".gleon/blobs/sha256"), &base);
    let first_revision = ManifestIndexRevision {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        parent_hashes: vec![base_hash.clone()],
        test_manifests: std::collections::BTreeMap::from([(
            "first".to_string(),
            TestManifestState::Deleted,
        )]),
    };
    let second_revision = ManifestIndexRevision {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        parent_hashes: vec![base_hash.clone()],
        test_manifests: std::collections::BTreeMap::from([(
            "second".to_string(),
            TestManifestState::Deleted,
        )]),
    };
    let first_hash = store_revision(&first_root.join(".gleon/blobs/sha256"), &first_revision);
    let second_hash = store_revision(&second_root.join(".gleon/blobs/sha256"), &second_revision);
    save_local_pointer(first_root, first_hash.clone());
    save_local_pointer(second_root, second_hash.clone());

    let adapter =
        Arc::new(ObjectStoreAdapter::from_config(&StorageConfig::new("memory://")).unwrap());
    adapter
        .upload_blob(
            base_hash.value(),
            &first_root
                .join(".gleon/blobs/sha256")
                .join(base_hash.value()),
        )
        .await
        .unwrap();
    adapter
        .create_manifest(
            "main",
            "mac",
            &serde_json::to_vec_pretty(&ManifestIndexPointer {
                schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
                revision_hash: base_hash,
            })
            .unwrap(),
        )
        .await
        .unwrap();

    let first = SyncOrchestrator::new(adapter.clone(), first_root.to_path_buf());
    let second = SyncOrchestrator::new(adapter.clone(), second_root.to_path_buf());
    let options = SyncOptions {
        retries: 3,
        ..SyncOptions::default()
    };

    let (first_result, second_result) = tokio::join!(
        first.push("main", "mac", &options),
        second.push("main", "mac", &options)
    );
    first_result.unwrap();
    second_result.unwrap();

    let pointer: ManifestIndexPointer =
        serde_json::from_slice(&adapter.download_manifest("main", "mac").await.unwrap()).unwrap();
    let downloaded = tempdir().unwrap();
    let revision_path = downloaded.path().join("merged-revision.json");
    adapter
        .download_blob(pointer.revision_hash.value(), &revision_path)
        .await
        .unwrap();
    let revision = ManifestIndexRevision::load(revision_path).unwrap();
    assert_eq!(revision.parent_hashes.len(), 2);
    assert!(revision.parent_hashes.contains(&first_hash));
    assert!(revision.parent_hashes.contains(&second_hash));
}

#[tokio::test]
#[cfg(not(miri))]
async fn test_sync_orchestrator_pull_three_way_merges_sibling_manifests() {
    let local_dir = tempdir().unwrap();
    let remote_dir = tempdir().unwrap();
    let local_root = local_dir.path();
    let remote_root = remote_dir.path();
    let local_blobs = local_root.join(".gleon/blobs/sha256");
    let remote_blobs = remote_root.join("blobs/sha256");

    let deleted_image = ImageHash::new("sha256", "1".repeat(64)).unwrap();
    let remote_image = ImageHash::new("sha256", "2".repeat(64)).unwrap();
    let entry = |hash: ImageHash| ManifestEntry {
        hash,
        phash: ImageHash::new("dhash", "0000000000000000").unwrap(),
        width: 100,
        height: 100,
        created_at: Utc::now(),
        created_by: "test".to_string(),
        source_commit: "commit".to_string(),
    };
    let base_manifest = Manifest {
        schema_version: SUPPORTED_MANIFEST_SCHEMA_VERSION,
        version: 1,
        hash_algo: "sha256".to_string(),
        pixel_format: "rgba".to_string(),
        generator_version: "1.0.0".to_string(),
        parent_hashes: Vec::new(),
        entries: std::collections::BTreeMap::from([(
            "deleted-locally.png".to_string(),
            entry(deleted_image),
        )]),
    };
    let base_manifest_hash = store_manifest(&local_blobs, &base_manifest);
    store_manifest(&remote_blobs, &base_manifest);

    let mut local_manifest = base_manifest.clone();
    local_manifest.version = 2;
    local_manifest.parent_hashes = vec![base_manifest_hash.clone()];
    local_manifest.entries.remove("deleted-locally.png");
    let local_manifest_hash = store_manifest(&local_blobs, &local_manifest);
    // The LCA is available only from remote CAS, so resolution must download it.
    std::fs::remove_file(local_blobs.join(base_manifest_hash.value())).unwrap();

    let mut remote_manifest = base_manifest.clone();
    remote_manifest.version = 2;
    remote_manifest.parent_hashes = vec![base_manifest_hash.clone()];
    remote_manifest.entries.insert(
        "added-remotely.png".to_string(),
        entry(remote_image.clone()),
    );
    let remote_manifest_hash = store_manifest(&remote_blobs, &remote_manifest);
    std::fs::write(remote_blobs.join(remote_image.value()), "remote image").unwrap();

    let base_revision = ManifestIndexRevision {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        parent_hashes: Vec::new(),
        test_manifests: std::collections::BTreeMap::from([(
            "screenshots".to_string(),
            TestManifestState::Present(base_manifest_hash),
        )]),
    };
    let base_revision_hash = store_revision(&local_blobs, &base_revision);
    store_revision(&remote_blobs, &base_revision);
    let local_revision = ManifestIndexRevision {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        parent_hashes: vec![base_revision_hash.clone()],
        test_manifests: std::collections::BTreeMap::from([(
            "screenshots".to_string(),
            TestManifestState::Present(local_manifest_hash.clone()),
        )]),
    };
    let remote_revision = ManifestIndexRevision {
        schema_version: SUPPORTED_MANIFEST_INDEX_SCHEMA_VERSION,
        parent_hashes: vec![base_revision_hash],
        test_manifests: std::collections::BTreeMap::from([(
            "screenshots".to_string(),
            TestManifestState::Present(remote_manifest_hash.clone()),
        )]),
    };
    let local_revision_hash = store_revision(&local_blobs, &local_revision);
    let remote_revision_hash = store_revision(&remote_blobs, &remote_revision);
    save_local_pointer(local_root, local_revision_hash.clone());
    save_remote_pointer(remote_root, remote_revision_hash.clone());

    let adapter = Arc::new(
        ObjectStoreAdapter::from_config(&StorageConfig::new(format!(
            "file://{}",
            remote_root.display()
        )))
        .unwrap(),
    );
    SyncOrchestrator::new(adapter, local_root.to_path_buf())
        .pull("main", "mac", &SyncOptions::default())
        .await
        .unwrap();

    let pointer =
        ManifestIndexPointer::load(local_root.join(".gleon/branches/main/mac/manifest_index.json"))
            .unwrap();
    let merged_revision =
        ManifestIndexRevision::load(local_blobs.join(pointer.revision_hash.value())).unwrap();
    assert_eq!(merged_revision.parent_hashes.len(), 2);
    assert!(merged_revision.parent_hashes.contains(&local_revision_hash));
    assert!(
        merged_revision
            .parent_hashes
            .contains(&remote_revision_hash)
    );
    let TestManifestState::Present(merged_manifest_hash) =
        merged_revision.test_manifests.get("screenshots").unwrap()
    else {
        panic!("merged screenshot manifest must be present");
    };
    let merged_manifest = Manifest::load(local_blobs.join(merged_manifest_hash.value())).unwrap();
    assert!(!merged_manifest.entries.contains_key("deleted-locally.png"));
    assert!(merged_manifest.entries.contains_key("added-remotely.png"));
    assert_eq!(merged_manifest.parent_hashes.len(), 2);
    assert!(merged_manifest.parent_hashes.contains(&local_manifest_hash));
    assert!(
        merged_manifest
            .parent_hashes
            .contains(&remote_manifest_hash)
    );
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
    let options = SyncOptions {
        fail_fast: false,
        retries: 0,
        ..SyncOptions::default()
    };

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
    assert!(matches!(
        adapter.download_manifest("main", "mac").await,
        Err(gleon_core::storage::StorageError::BlobNotFound(_))
    ));
}
