#![cfg(not(miri))]

use gleon_core::config::GlobPattern;
use gleon_core::scanner::{FileScanner, TestCase};
use std::path::Path;

#[test]
fn test_scanner_with_real_fixture() {
    // The tests/fixtures directory contains a real 200x100.png file.
    let base_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");

    // Make sure the fixture actually exists
    let fixture_path = base_dir.join("200x100.png");
    assert!(fixture_path.exists(), "The 200x100.png fixture is missing!");

    let include = vec![GlobPattern::new("**/*.png").unwrap()];
    let exclude = vec![];

    let cases: Vec<TestCase> = FileScanner::scan_files(&include, &exclude, &base_dir)
        .expect("Scanning files should succeed");

    // The image is directly in tests/fixtures, so the relative parent directory is "."
    // which resolves to the test name "."
    let test_case = cases
        .iter()
        .find(|c| c.name == ".")
        .expect("Should find test case for '.'");

    // Find the specific fixture image in the results
    let image_result = test_case
        .images
        .iter()
        .find(|i| i.relative_path.to_str() == Some("200x100.png"))
        .expect("Should find 200x100.png in the scanned images");

    // Assert the absolute path is correct
    assert_eq!(image_result.absolute_path, fixture_path);

    // Verify it decoded successfully and dimensions match the real file
    let img = image_result
        .image
        .as_ref()
        .expect("Failed to decode the real PNG fixture");
    assert_eq!(img.width(), 200);
    assert_eq!(img.height(), 100);
}
