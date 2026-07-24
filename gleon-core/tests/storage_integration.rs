//! Integration test for ObjectStoreAdapter using file:// backend with local disk & tempdir.

#![cfg(not(miri))]

use std::fs;
use std::path::PathBuf;

use gleon_core::storage::{ObjectStoreAdapter, StorageConfig};
use tempfile::tempdir;

fn fixture_path(filename: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(filename)
}

#[tokio::test]
async fn test_file_scheme_storage_integration() {
    let remote_dir = tempdir().expect("remote tempdir creation");
    let file_url = url::Url::from_directory_path(remote_dir.path())
        .expect("canonical file URL")
        .to_string();

    let config = StorageConfig::new(file_url);
    let adapter = ObjectStoreAdapter::from_config(&config).expect("valid file url adapter");

    let real_png = fixture_path("baseline_100x100.png");
    assert!(real_png.exists(), "baseline PNG fixture must exist");

    let sha256_hash = "a1b2c3d4e5f67890123456789abcdef0123456789abcdef0123456789abcdef0";

    // 1. Upload real PNG blob to file:// store
    adapter
        .upload_blob(sha256_hash, &real_png)
        .await
        .expect("upload real PNG blob ok");

    // Verify file exists on remote disk under blobs/sha256/
    let expected_remote_path = remote_dir
        .path()
        .join("blobs")
        .join("sha256")
        .join(sha256_hash);
    assert!(expected_remote_path.exists());

    // 2. Download blob to local client workspace
    let local_dest_dir = tempdir().expect("local dest tempdir");
    let downloaded_path = local_dest_dir.path().join("downloaded.png");

    adapter
        .download_blob(sha256_hash, &downloaded_path)
        .await
        .expect("download blob ok");

    let original_bytes = fs::read(&real_png).expect("read original png");
    let downloaded_bytes = fs::read(&downloaded_path).expect("read downloaded png");
    assert_eq!(original_bytes, downloaded_bytes);

    // 3. Upload & Download manifest index
    let manifest_fixture = fixture_path("default_manifest_index.json");
    let manifest_bytes = fs::read(&manifest_fixture).expect("read manifest index fixture");

    adapter
        .upload_manifest("main", "macos-arm64", &manifest_bytes)
        .await
        .expect("upload manifest ok");

    let downloaded_manifest = adapter
        .download_manifest("main", "macos-arm64")
        .await
        .expect("download manifest ok");
    assert_eq!(manifest_bytes, downloaded_manifest);
}

#[tokio::test]
async fn test_r2_credentials_config_resolution() {
    let mut config = StorageConfig::new("s3://test-bucket/baselines");
    config.r2_account_id = Some("acc123456789".to_string());
    config.aws_access_key_id = Some("R2_KEY_ID".to_string());
    config.aws_secret_access_key = Some("R2_SECRET".to_string());

    let adapter_res = ObjectStoreAdapter::from_config(&config);
    assert!(adapter_res.is_ok(), "R2 URL config parsing must succeed");
}
