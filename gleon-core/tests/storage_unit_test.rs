#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::pedantic,
    clippy::nursery,
    missing_docs
)]
//! Unit tests for `ObjectStoreAdapter` using memory:// storage backend.

#![cfg(not(miri))]

use gleon_core::storage::{BlobMetadata, ObjectStoreAdapter, StorageConfig, StorageError};
use object_store::Attribute;
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

    let blob_hash = gleon_core::manifest::ImageHash::new(
        "sha256",
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    )
    .unwrap();

    // 1. Upload Blob
    adapter
        .upload_blob(&blob_hash, &src_file)
        .await
        .expect("upload blob ok");

    // 2. List Blobs
    let list = adapter.list_blobs("sha256").await.expect("list blobs ok");
    assert_eq!(list, vec![blob_hash.value().to_string()]);

    // 3. Download Blob
    let dest_file = dir.path().join("downloaded_blob.png");
    adapter
        .download_blob(&blob_hash, &dest_file)
        .await
        .expect("download blob ok");

    let downloaded_bytes = std::fs::read(&dest_file).expect("read downloaded file");
    assert_eq!(downloaded_bytes, b"png_file_bytes");

    // 4. Download non-existent blob -> BlobNotFound
    let missing_hash = gleon_core::manifest::ImageHash::new(
        "sha256",
        "0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    let not_found = adapter.download_blob(&missing_hash, &dest_file).await;

    assert!(matches!(not_found, Err(StorageError::BlobNotFound(_))));

    // 5. Check blob_exists
    assert!(
        adapter
            .blob_exists(&blob_hash)
            .await
            .expect("blob_exists ok")
    );
    assert!(
        !adapter
            .blob_exists(&missing_hash)
            .await
            .expect("blob_exists false ok")
    );

    // 6. Upload missing local file -> StorageError::Io
    let missing_local = dir.path().join("non_existent_file.png");
    let upload_err = adapter.upload_blob(&blob_hash, &missing_local).await;
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

    let blob_hash = gleon_core::manifest::ImageHash::new(
        "sha256",
        "1111111111111111111111111111111111111111111111111111111111111111",
    )
    .unwrap();
    adapter.upload_blob(&blob_hash, &src_file).await.unwrap();

    let file_as_dir = dir.path().join("regular_file.txt");
    std::fs::write(&file_as_dir, b"not a directory").expect("write file as dir");

    let dest_file = file_as_dir.join("downloaded.png");
    let err = adapter.download_blob(&blob_hash, &dest_file).await;

    assert!(
        matches!(err, Err(StorageError::Io { .. })),
        "Expected Io error: {err:?}"
    );
}

#[tokio::test]
async fn test_adapter_list_blobs() {
    let config = StorageConfig::new("memory://");
    let adapter = ObjectStoreAdapter::from_config(&config).unwrap();

    let dir = tempdir().expect("tempdir creation");
    let src_file = dir.path().join("blob.png");
    std::fs::write(&src_file, b"sample bytes").unwrap();

    let blob_hash = gleon_core::manifest::ImageHash::new(
        "sha256",
        "2222222222222222222222222222222222222222222222222222222222222222",
    )
    .unwrap();
    adapter.upload_blob(&blob_hash, &src_file).await.unwrap();

    let blobs = adapter.list_blobs("sha256").await.unwrap();
    assert_eq!(blobs, vec![blob_hash.value().to_string()]);
}

#[test]
fn test_aws_options_coverage() {
    let mut config = StorageConfig::new("memory://");
    config.aws_access_key_id = Some("abc".to_string());
    config.aws_secret_access_key = Some("def".to_string());
    config.aws_region = Some("us-east-1".to_string());
    config.aws_endpoint = Some("http://localhost:9000".to_string());

    // We expect this to not panic and return Ok, even though memory:// ignores AWS options.
    let adapter = ObjectStoreAdapter::from_config(&config).unwrap();
    assert_eq!(adapter.concurrency(), 8);

    // Test the R2 fallback path
    let mut config_r2 = StorageConfig::new("memory://");
    config_r2.r2_account_id = Some("123456789".to_string());
    let adapter_r2 = ObjectStoreAdapter::from_config(&config_r2).unwrap();
    assert_eq!(adapter_r2.concurrency(), 8);

    // Test invalid url parse error
    let config_bad = StorageConfig::new("http://[:::1]"); // Invalid IPv6
    let err = ObjectStoreAdapter::from_config(&config_bad);
    assert!(matches!(err, Err(StorageError::InvalidUrl { .. })));
}

#[test]
fn test_blob_metadata_creation() {
    let meta = BlobMetadata::new("auth/login", "macos-arm64");
    assert_eq!(meta.test_name.as_deref(), Some("auth/login"));
    assert_eq!(meta.platform.as_deref(), Some("macos-arm64"));
    assert_eq!(meta.path.as_deref(), Some("macos-arm64/auth/login.png"));
    assert_eq!(meta.content_type.as_deref(), Some("image/png"));

    let default_meta = BlobMetadata::default();
    assert_eq!(default_meta.test_name, None);
    assert_eq!(default_meta.platform, None);
    assert_eq!(default_meta.path, None);
    assert_eq!(default_meta.content_type, None);
}

#[tokio::test]
async fn test_upload_blob_with_metadata_attributes() {
    let config = StorageConfig::new("memory://");
    let adapter = ObjectStoreAdapter::from_config(&config).unwrap();

    let dir = tempdir().unwrap();
    let src_file = dir.path().join("blob_with_meta.png");
    std::fs::write(&src_file, b"sample_png_bytes").unwrap();

    let hash = gleon_core::manifest::ImageHash::new(
        "sha256",
        "4444444444444444444444444444444444444444444444444444444444444444",
    )
    .unwrap();

    let meta = BlobMetadata::new("auth/login_screen", "macos-arm64");
    adapter
        .upload_blob_with_metadata(&hash, &src_file, Some(&meta))
        .await
        .unwrap();

    // Verify stored attributes using encapsulated get_blob_attributes
    let attributes = adapter
        .get_blob_attributes(&hash)
        .await
        .unwrap()
        .expect("attributes present");

    let content_type = attributes
        .get(&Attribute::ContentType)
        .expect("ContentType attribute present");
    assert_eq!(content_type.as_ref(), "image/png");

    let test_attr = attributes
        .get(&Attribute::Metadata(std::borrow::Cow::Borrowed("test")))
        .expect("test metadata present");
    assert_eq!(test_attr.as_ref(), "auth/login_screen");

    let platform_attr = attributes
        .get(&Attribute::Metadata(std::borrow::Cow::Borrowed("platform")))
        .expect("platform metadata present");
    assert_eq!(platform_attr.as_ref(), "macos-arm64");

    let path_attr = attributes
        .get(&Attribute::Metadata(std::borrow::Cow::Borrowed("path")))
        .expect("path metadata present");
    assert_eq!(path_attr.as_ref(), "macos-arm64/auth/login_screen.png");

    // Query non-existent hash returns None
    let missing_hash = gleon_core::manifest::ImageHash::new(
        "sha256",
        "8888888888888888888888888888888888888888888888888888888888888888",
    )
    .unwrap();
    assert!(
        adapter
            .get_blob_attributes(&missing_hash)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn test_local_file_system_skips_attributes_for_file_scheme() {
    let dir = tempdir().unwrap();
    // file:// URL creates an adapter with supports_attributes = false
    let url = format!("file://{}", dir.path().display());
    let config = StorageConfig::new(url);
    let adapter = ObjectStoreAdapter::from_config(&config).unwrap();

    let src_file = dir.path().join("local_sample.png");
    std::fs::write(&src_file, b"sample_file_data").unwrap();

    let hash = gleon_core::manifest::ImageHash::new(
        "sha256",
        "5555555555555555555555555555555555555555555555555555555555555555",
    )
    .unwrap();

    let meta = BlobMetadata::new("home/dashboard", "linux-x86_64");
    adapter
        .upload_blob_with_metadata(&hash, &src_file, Some(&meta))
        .await
        .expect("upload on LocalFileSystem should succeed directly via put");

    assert!(adapter.blob_exists(&hash).await.unwrap());
    let attrs = adapter.get_blob_attributes(&hash).await.unwrap().unwrap();
    assert!(attrs.is_empty());
}

#[tokio::test]
async fn test_upload_blob_with_custom_content_type_and_sanitization() {
    let config = StorageConfig::new("memory://");
    let adapter = ObjectStoreAdapter::from_config(&config).unwrap();

    let dir = tempdir().unwrap();
    let src_file = dir.path().join("blob_custom_meta.png");
    std::fs::write(&src_file, b"sample_custom_bytes").unwrap();

    let hash = gleon_core::manifest::ImageHash::new(
        "sha256",
        "7777777777777777777777777777777777777777777777777777777777777777",
    )
    .unwrap();

    // Pass custom content_type and invalid (control / newline) characters in metadata
    let mut meta = BlobMetadata::new("auth/login", "macos-arm64");
    meta.content_type = Some("image/webp".to_string());
    meta.test_name = Some("auth\nmalicious".to_string()); // Invalid CRLF should be skipped
    meta.platform = Some("macos\r\ninjection".to_string()); // Invalid CRLF should be skipped
    meta.path = Some("macos-arm64/auth/login.png".to_string()); // Valid, should be kept

    adapter
        .upload_blob_with_metadata(&hash, &src_file, Some(&meta))
        .await
        .unwrap();

    let attrs = adapter
        .get_blob_attributes(&hash)
        .await
        .unwrap()
        .expect("attributes present");

    assert_eq!(
        attrs.get(&Attribute::ContentType).unwrap().as_ref(),
        "image/webp"
    );
    // Malformed headers were skipped safely
    assert!(
        attrs
            .get(&Attribute::Metadata(std::borrow::Cow::Borrowed("test")))
            .is_none()
    );
    assert!(
        attrs
            .get(&Attribute::Metadata(std::borrow::Cow::Borrowed("platform")))
            .is_none()
    );
    // Valid header was retained
    assert_eq!(
        attrs
            .get(&Attribute::Metadata(std::borrow::Cow::Borrowed("path")))
            .unwrap()
            .as_ref(),
        "macos-arm64/auth/login.png"
    );

    // Second upload: invalid path and invalid content_type are also skipped safely
    let hash2 = gleon_core::manifest::ImageHash::new(
        "sha256",
        "9999999999999999999999999999999999999999999999999999999999999999",
    )
    .unwrap();

    let meta2 = BlobMetadata {
        content_type: Some("invalid\ncontent/type".to_string()),
        path: Some("invalid\npath/screen.png".to_string()),
        ..Default::default()
    };

    adapter
        .upload_blob_with_metadata(&hash2, &src_file, Some(&meta2))
        .await
        .unwrap();

    let attrs2 = adapter
        .get_blob_attributes(&hash2)
        .await
        .unwrap()
        .expect("attributes present");

    assert!(attrs2.get(&Attribute::ContentType).is_none());
    assert!(
        attrs2
            .get(&Attribute::Metadata(std::borrow::Cow::Borrowed("path")))
            .is_none()
    );

    // Third upload: metadata with None fields falls back to default Content-Type and sets no headers
    let hash3 = gleon_core::manifest::ImageHash::new(
        "sha256",
        "1111111111111111111111111111111111111111111111111111111111111111",
    )
    .unwrap();

    let meta3 = BlobMetadata::default();
    adapter
        .upload_blob_with_metadata(&hash3, &src_file, Some(&meta3))
        .await
        .unwrap();

    let attrs3 = adapter
        .get_blob_attributes(&hash3)
        .await
        .unwrap()
        .expect("attributes present");

    assert_eq!(
        attrs3.get(&Attribute::ContentType).unwrap().as_ref(),
        "image/png"
    );
    assert!(
        attrs3
            .get(&Attribute::Metadata(std::borrow::Cow::Borrowed("test")))
            .is_none()
    );
    assert!(
        attrs3
            .get(&Attribute::Metadata(std::borrow::Cow::Borrowed("platform")))
            .is_none()
    );
    assert!(
        attrs3
            .get(&Attribute::Metadata(std::borrow::Cow::Borrowed("path")))
            .is_none()
    );
}
