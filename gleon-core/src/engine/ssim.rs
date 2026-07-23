//! Structural Similarity Index (SSIM) image comparison.

use image::RgbaImage;

/// Errors that can occur during SSIM comparison.
#[derive(Debug, thiserror::Error)]
pub enum SsimError {
    /// The underlying ssim comparison failed.
    #[error("SSIM comparison failed")]
    Compare(#[from] image_compare::CompareError),
}

/// Compares two images of the same dimensions using SSIM (Structural Similarity).
/// Under the hood, this uses `image_compare::rgba_hybrid_compare`.
///
/// Returns the similarity score (where 1.0 is perfect similarity) and a diff image.
pub fn compare_ssim(
    baseline: &RgbaImage,
    actual: &RgbaImage,
) -> Result<(f64, RgbaImage), SsimError> {
    let result = image_compare::rgba_hybrid_compare(baseline, actual)?;
    let ssim_score = result.score;
    let diff_image = result.image.to_color_map().into_rgba8();
    Ok((ssim_score, diff_image))
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn test_compare_ssim_identical() {
        let img1 = ImageBuffer::from_pixel(100, 100, Rgba([255, 0, 0, 255]));
        let img2 = ImageBuffer::from_pixel(100, 100, Rgba([255, 0, 0, 255]));

        let (score, _diff_img) = compare_ssim(&img1, &img2).unwrap();
        // Identical images should have an SSIM score of 1.0
        assert!((score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_compare_ssim_mismatch() {
        let img1 = ImageBuffer::from_pixel(100, 100, Rgba([255, 0, 0, 255]));
        let mut img2 = ImageBuffer::from_pixel(100, 100, Rgba([255, 0, 0, 255]));
        // Make half of img2 green
        for y in 0..50 {
            for x in 0..100 {
                img2.put_pixel(x, y, Rgba([0, 255, 0, 255]));
            }
        }

        let (score, _diff_img) = compare_ssim(&img1, &img2).unwrap();
        // Different images should have an SSIM score strictly less than 1.0
        assert!(score < 0.99);
    }

    #[test]
    fn test_ssim_error_source() {
        use std::error::Error;
        let img1 = ImageBuffer::from_pixel(10, 10, Rgba([255, 0, 0, 255]));
        let img2 = ImageBuffer::from_pixel(20, 20, Rgba([255, 0, 0, 255]));
        let err = compare_ssim(&img1, &img2).unwrap_err();
        assert!(err.source().is_some());
    }
}
