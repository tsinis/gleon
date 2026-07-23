//! Unit tests for ObjectStoreAdapter using memory:// storage backend.

#![cfg(not(miri))]

use gleon_core::storage::{ObjectStoreAdapter, StorageConfig, StorageError};
use tempfile::tempdir;

#[test]
fn test_storage_config_secret_masking() {
    let mut config =
        StorageConfig::new("https://myuser:mysecretpassword@s3.amazonaws.com/mybucket");
    config.aws_access_key_id = Some("AKIAIOSFODNN7EXAMPLE".to_string());
    config.aws_secret_access_key = Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string());

    let debug_str = format!("{config:?}");
    assert!(!debug_str.contains("wJalrXUtnFEMI"));
    assert!(!debug_str.contains("mysecretpassword"));
    assert!(debug_str.contains("[REDACTED]"));
    assert!(debug_str.contains("[PRESENT]"));
}

#[tokio::test]
async fn test_memory_store_blob_and_manifest_lifecycle() {
    let config = StorageConfig::new("memory://");
    let adapter = ObjectStoreAdapter::from_config(&config).expect("valid memory url");

    let dir = tempdir().expect("tempdir creation");
    let src_file = dir.path().join("sample_blob.png");
    std::fs::write(&src_file, b"png_file_bytes").expect("write src file");

    let blob_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    // 1. Upload Blob
    adapter
        .upload_blob(blob_hash, &src_file)
        .await
        .expect("upload blob ok");

    // 2. List Blobs
    let list = adapter.list_blobs().await.expect("list blobs ok");
    assert_eq!(list, vec![blob_hash.to_string()]);

    // 3. Download Blob
    let dest_file = dir.path().join("downloaded_blob.png");
    adapter
        .download_blob(blob_hash, &dest_file)
        .await
        .expect("download blob ok");

    let downloaded_bytes = std::fs::read(&dest_file).expect("read downloaded file");
    assert_eq!(downloaded_bytes, b"png_file_bytes");

    // 4. Download non-existent blob -> BlobNotFound
    let not_found = adapter
        .download_blob(
            "0000000000000000000000000000000000000000000000000000000000000000",
            &dest_file,
        )
        .await;

    assert!(matches!(not_found, Err(StorageError::BlobNotFound(_))));

    // 5. Upload & Download Manifest
    let manifest_content = b"{\"schema_version\":1,\"entries\":{}}";
    adapter
        .upload_manifest("feature-1", "macos-arm64", manifest_content)
        .await
        .expect("upload manifest ok");

    let downloaded_manifest = adapter
        .download_manifest("feature-1", "macos-arm64")
        .await
        .expect("download manifest ok");

    assert_eq!(downloaded_manifest, manifest_content);

    // 6. Download non-existent manifest -> BlobNotFound
    let manifest_not_found = adapter.download_manifest("main", "linux-x64").await;
    assert!(matches!(
        manifest_not_found,
        Err(StorageError::BlobNotFound(_))
    ));

    // 7. Check blob_exists
    assert!(
        adapter
            .blob_exists(blob_hash)
            .await
            .expect("blob_exists ok")
    );
    assert!(
        !adapter
            .blob_exists("0000000000000000000000000000000000000000000000000000000000000000")
            .await
            .expect("blob_exists false ok")
    );

    // 8. Upload missing local file -> StorageError::Io
    let missing_local = dir.path().join("non_existent_file.png");
    let upload_err = adapter.upload_blob(blob_hash, &missing_local).await;
    assert!(matches!(upload_err, Err(StorageError::Io { .. })));
}

#[test]
fn test_concurrency_clamp_to_one() {
    let mut config = StorageConfig::new("memory://");
    config.concurrency = 0;
    let adapter = ObjectStoreAdapter::from_config(&config).unwrap();
    assert_eq!(adapter.concurrency(), 1);
}

#[tokio::test]
async fn test_adapter_download_io_errors() {
    let config = StorageConfig::new("memory://");
    let adapter = ObjectStoreAdapter::from_config(&config).unwrap();

    let dir = tempdir().expect("tempdir creation");
    let src_file = dir.path().join("sample_blob.png");
    std::fs::write(&src_file, b"png_file_bytes").expect("write src file");

    let blob_hash = "1111111111111111111111111111111111111111111111111111111111111111";
    adapter.upload_blob(blob_hash, &src_file).await.unwrap();

    let file_as_dir = dir.path().join("regular_file.txt");
    std::fs::write(&file_as_dir, b"not a directory").expect("write file as dir");

    let dest_file = file_as_dir.join("downloaded.png");
    let err = adapter.download_blob(blob_hash, &dest_file).await;

    assert!(
        matches!(err, Err(StorageError::Io { .. })),
        "Expected Io error: {:?}",
        err
    );
}

#[tokio::test]
async fn test_adapter_list_blobs() {
    let config = StorageConfig::new("memory://");
    let adapter = ObjectStoreAdapter::from_config(&config).unwrap();

    let dir = tempdir().expect("tempdir creation");
    let src_file = dir.path().join("blob.png");
    std::fs::write(&src_file, b"sample bytes").unwrap();

    let blob_hash = "2222222222222222222222222222222222222222222222222222222222222222";
    adapter.upload_blob(blob_hash, &src_file).await.unwrap();

    let blobs = adapter.list_blobs().await.unwrap();
    assert_eq!(blobs, vec![blob_hash.to_string()]);
}
