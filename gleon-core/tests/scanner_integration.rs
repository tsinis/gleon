#![cfg(not(miri))]

use gleon_core::config::GlobPattern;
use gleon_core::scanner::{FileScanner, TestCase};
use std::path::Path;

#[test]
fn test_scanner_with_real_fixture() {
    // The tests/fixtures directory contains a real 200x100.png file and a corrupt.png file.
    let base_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");

    let fixture_path = base_dir.join("200x100.png");
    let corrupt_path = base_dir.join("corrupt.png");
    assert!(fixture_path.exists(), "The 200x100.png fixture is missing!");
    assert!(corrupt_path.exists(), "The corrupt.png fixture is missing!");

    let include = vec![GlobPattern::new("**/*.png").unwrap()];
    let exclude = vec![];

    let cases: Vec<TestCase> = FileScanner::scan_files(&include, &exclude, &base_dir)
        .expect("Scanning files should succeed");

    // Both files are in the base_dir, so they both fall under the test case name "."
    let test_case = cases
        .iter()
        .find(|c| c.name == ".")
        .expect("Should find test case for '.'");

    // Check we found exactly 2 files
    assert_eq!(test_case.images.len(), 2);

    // Verify 200x100.png
    let image_200x100 = test_case
        .images
        .iter()
        .find(|i| i.relative_path.to_str() == Some("200x100.png"))
        .expect("Should find 200x100.png in the scanned images");
    assert_eq!(image_200x100.absolute_path, fixture_path);
    let img = image_200x100
        .image
        .as_ref()
        .expect("Failed to decode the real PNG fixture");
    assert_eq!(img.width(), 200);
    assert_eq!(img.height(), 100);

    // Verify corrupt.png
    let corrupt_image = test_case
        .images
        .iter()
        .find(|i| i.relative_path.to_str() == Some("corrupt.png"))
        .expect("Should find corrupt.png in the scanned images");
    assert_eq!(corrupt_image.absolute_path, corrupt_path);
    assert!(
        corrupt_image.image.is_err(),
        "corrupt.png should fail to decode"
    );

    // Verify aggregate counts
    let ok_count = test_case.images.iter().filter(|i| i.image.is_ok()).count();
    let err_count = test_case.images.iter().filter(|i| i.image.is_err()).count();
    assert_eq!(ok_count, 1);
    assert_eq!(err_count, 1);
}
