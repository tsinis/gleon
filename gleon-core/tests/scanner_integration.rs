#![cfg(not(miri))]

use gleon_core::config::{DiffConfig, GlobPattern, Mode, ScreenshotRule};
use gleon_core::scanner::{FileScanner, TestCase};
use std::path::Path;
use std::sync::Arc;

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
    let exclude = vec![GlobPattern::new("**/report_output/**").unwrap()];
    let rule = Arc::new(ScreenshotRule {
        include: include.clone(),
        mode: Mode::Pixel,
        diff: DiffConfig::default(),
        masks: vec![],
    });

    let cases: Vec<TestCase> = FileScanner::scan_files(&include, &exclude, &base_dir, rule)
        .expect("Scanning files should succeed");

    // Both files are in the base_dir, so they both fall under the test case name "."
    let test_case = cases
        .iter()
        .find(|c| c.name == ".")
        .expect("Should find test case for '.'");

    // Check we found all the expected PNG files (9 files total in the fixtures dir)
    assert_eq!(test_case.images.len(), 9);

    // Verify 200x100.png
    let image_200x100 = test_case
        .images
        .iter()
        .find(|i| i.relative_path.to_str() == Some("200x100.png"))
        .expect("Should find 200x100.png in the scanned images");
    assert_eq!(image_200x100.absolute_path, fixture_path);
    let img =
        image::open(&image_200x100.absolute_path).expect("Failed to decode the real PNG fixture");
    assert_eq!(img.width(), 200);
    assert_eq!(img.height(), 100);

    // Verify baseline_100x100.png
    let image_100x100 = test_case
        .images
        .iter()
        .find(|i| i.relative_path.to_str() == Some("baseline_100x100.png"))
        .expect("Should find baseline_100x100.png in the scanned images");
    assert_eq!(
        image_100x100.absolute_path,
        base_dir.join("baseline_100x100.png")
    );
    let img_100 = image::open(&image_100x100.absolute_path)
        .expect("Failed to decode the baseline 100x100 PNG fixture");
    assert_eq!(img_100.width(), 100);
    assert_eq!(img_100.height(), 100);

    // Verify corrupt.png
    let corrupt_image = test_case
        .images
        .iter()
        .find(|i| i.relative_path.to_str() == Some("corrupt.png"))
        .expect("Should find corrupt.png in the scanned images");
    assert_eq!(corrupt_image.absolute_path, corrupt_path);
    assert!(
        image::open(&corrupt_image.absolute_path).is_err(),
        "corrupt.png should fail to decode"
    );
}
