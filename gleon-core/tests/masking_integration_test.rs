#![cfg(not(miri))]

use gleon_core::config::{Dimension, Zone};
use gleon_core::masking;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn test_masking_integration() {
    // 1. Resolve base directory and locate the 200x100.png fixture
    let base_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let src_fixture_path = base_dir.join("200x100.png");
    assert!(
        src_fixture_path.exists(),
        "Source fixture 200x100.png must exist"
    );

    // 2. Load the source image
    let src_img = image::open(&src_fixture_path)
        .expect("Failed to open source 200x100.png fixture")
        .to_rgba8();

    assert_eq!(src_img.width(), 200);
    assert_eq!(src_img.height(), 100);

    // 3. Crop to 100x100 to prepare the baseline_100x100.png
    let cropped_img = image::imageops::crop_imm(&src_img, 0, 0, 100, 100).to_image();
    assert_eq!(cropped_img.width(), 100);
    assert_eq!(cropped_img.height(), 100);

    // 4. Save to a temporary directory to adhere to "Load baseline_100x100.png"
    let tmp_dir = tempdir().expect("Failed to create temporary directory");
    let baseline_path = tmp_dir.path().join("baseline_100x100.png");

    cropped_img
        .save(&baseline_path)
        .expect("Failed to save baseline_100x100.png");

    // 5. Load the baseline_100x100.png
    let mut img = image::open(&baseline_path)
        .expect("Failed to load baseline_100x100.png")
        .to_rgba8();

    let original_img = img.clone();

    // 6. Define an out-of-bounds mask zone (x: 90, width: 20px, height: 100%)
    // Since the image width is 100px, x + width = 110px, which is out of bounds.
    let zones = vec![Zone {
        x: 90,
        y: 0,
        width: Dimension::Pixels(20),
        height: Dimension::Percent(100.0),
    }];

    // 7. Apply masks — clamping is expected due to x:90 + width:20 > 100
    masking::apply_masks(&mut img, &zones);

    // 8. Assert that exactly the bounding pixels [90..99] are mutated to black,
    // and pixels [0..89] remain unchanged.
    for y in 0..100 {
        for x in 0..100 {
            let pixel = img.get_pixel(x, y);
            if x >= 90 {
                // Should be mutated to black
                assert_eq!(
                    *pixel,
                    image::Rgba([0, 0, 0, 255]),
                    "Pixel at ({x}, {y}) should be black"
                );
            } else {
                // Should remain original
                let original_pixel = original_img.get_pixel(x, y);
                assert_eq!(
                    *pixel, *original_pixel,
                    "Pixel at ({x}, {y}) should remain unchanged"
                );
            }
        }
    }
}
