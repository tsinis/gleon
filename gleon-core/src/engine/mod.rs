//! Unified visual regression diff engine.

pub mod phash;
pub mod pixel;
pub mod ssim;

pub use phash::{calculate_hamming_distance, compute_phash};
pub use pixel::compare_pixels;
pub use ssim::{SsimError, compare_ssim};

use crate::config::{DiffConfig, Mode};
use image::RgbaImage;

/// Detailed breakdown of a mismatch between baseline and actual images.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum MismatchDetail {
    /// Pixel difference count.
    Pixel {
        /// Number of mismatched pixels.
        diff_count: u32,
    },
    /// Structural Similarity (SSIM) score.
    Ssim {
        /// Computed SSIM score (1.0 is identical).
        ssim_score: f64,
    },
    /// SSIM calculation failed and fell back to pixel difference.
    SsimFallback {
        /// Number of mismatched pixels.
        diff_count: u32,
    },
}

/// The result of comparing a baseline and an actual image.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ComparisonResult {
    /// The images match within the configured tolerance thresholds.
    Match,
    /// The images differ.
    Mismatch {
        /// Detailed information about the mismatch.
        detail: MismatchDetail,
        /// The generated visualization diff image.
        diff_image: RgbaImage,
    },
    /// The images have different dimensions.
    DimensionMismatch {
        /// Dimensions of the baseline image.
        baseline_size: (u32, u32),
        /// Dimensions of the actual image.
        actual_size: (u32, u32),
    },
}

fn execute_pixel_comparison(
    baseline: &RgbaImage,
    actual: &RgbaImage,
    threshold: f64,
    is_fallback: bool,
) -> ComparisonResult {
    let total_pixels = baseline.width().saturating_mul(baseline.height());
    if total_pixels == 0 {
        return ComparisonResult::Match;
    }

    if threshold == 0.0 {
        // Single pass fast-path: count mismatches and build diff_image in a single iteration.
        let (diff_count, diff_image) = compare_pixels(baseline, actual);
        if diff_count == 0 {
            ComparisonResult::Match
        } else {
            ComparisonResult::Mismatch {
                detail: if is_fallback {
                    MismatchDetail::SsimFallback { diff_count }
                } else {
                    MismatchDetail::Pixel { diff_count }
                },
                diff_image,
            }
        }
    } else {
        // First, count mismatched pixels without allocating a diff image to save memory.
        let diff_count = pixel::count_mismatched_pixels(baseline, actual);
        let mismatch_ratio = diff_count as f64 / total_pixels as f64;
        if mismatch_ratio <= threshold {
            ComparisonResult::Match
        } else {
            // Only generate the diff image if there's actually a mismatch.
            let (_, diff_image) = compare_pixels(baseline, actual);
            ComparisonResult::Mismatch {
                detail: if is_fallback {
                    MismatchDetail::SsimFallback { diff_count }
                } else {
                    MismatchDetail::Pixel { diff_count }
                },
                diff_image,
            }
        }
    }
}

/// Compares a baseline and an actual image using the configured mode and thresholds.
///
/// If dimensions do not match, returns `ComparisonResult::DimensionMismatch`.
pub fn compare_images(
    baseline: &RgbaImage,
    actual: &RgbaImage,
    mode: Mode,
    config: &DiffConfig,
) -> ComparisonResult {
    let w1 = baseline.width();
    let h1 = baseline.height();
    let w2 = actual.width();
    let h2 = actual.height();

    if w1 != w2 || h1 != h2 {
        return ComparisonResult::DimensionMismatch {
            baseline_size: (w1, h1),
            actual_size: (w2, h2),
        };
    }

    match mode {
        Mode::Pixel => execute_pixel_comparison(baseline, actual, config.threshold, false),
        Mode::Ssim => match compare_ssim(baseline, actual) {
            Ok((ssim_score, diff_image)) => {
                if ssim_score >= config.min_similarity {
                    ComparisonResult::Match
                } else {
                    ComparisonResult::Mismatch {
                        detail: MismatchDetail::Ssim { ssim_score },
                        diff_image,
                    }
                }
            }
            Err(err) => {
                tracing::error!(
                    "SSIM calculation failed: {}. Falling back to pixel comparison.",
                    err
                );
                execute_pixel_comparison(baseline, actual, config.threshold, true)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DiffConfig;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn test_dimension_mismatch() {
        let img1 = ImageBuffer::from_pixel(100, 100, Rgba([255, 0, 0, 255]));
        let img2 = ImageBuffer::from_pixel(120, 100, Rgba([255, 0, 0, 255]));

        let config = DiffConfig::default();
        let result = compare_images(&img1, &img2, Mode::Pixel, &config);

        assert!(matches!(
            result,
            ComparisonResult::DimensionMismatch {
                baseline_size: (100, 100),
                actual_size: (120, 100)
            }
        ));
    }

    #[test]
    fn test_compare_images_pixel_match_with_tolerance() {
        let img1 = ImageBuffer::from_pixel(10, 10, Rgba([255, 0, 0, 255]));
        let mut img2 = ImageBuffer::from_pixel(10, 10, Rgba([255, 0, 0, 255]));
        // Make 5 pixels different out of 100 (5% difference)
        for i in 0..5 {
            img2.put_pixel(i, 0, Rgba([0, 255, 0, 255]));
        }

        // With 10% threshold, it should match
        let config = DiffConfig {
            threshold: 0.10,
            ..Default::default()
        };

        let result = compare_images(&img1, &img2, Mode::Pixel, &config);
        assert_eq!(result, ComparisonResult::Match);

        // With 2% threshold, it should mismatch
        let config2 = DiffConfig {
            threshold: 0.02,
            ..Default::default()
        };
        let result2 = compare_images(&img1, &img2, Mode::Pixel, &config2);
        assert!(matches!(
            result2,
            ComparisonResult::Mismatch {
                detail: MismatchDetail::Pixel { diff_count: 5 },
                ..
            }
        ));
    }

    #[test]
    #[cfg(not(miri))]
    fn test_compare_images_ssim_match() {
        let img1 = ImageBuffer::from_pixel(100, 100, Rgba([255, 0, 0, 255]));
        let mut img2 = ImageBuffer::from_pixel(100, 100, Rgba([255, 0, 0, 255]));
        // Make a small change
        img2.put_pixel(50, 50, Rgba([0, 255, 0, 255]));

        let config = DiffConfig {
            min_similarity: 0.95,
            ..Default::default()
        };

        let result = compare_images(&img1, &img2, Mode::Ssim, &config);
        assert_eq!(result, ComparisonResult::Match);

        // A large change should mismatch
        for y in 0..50 {
            for x in 0..100 {
                img2.put_pixel(x, y, Rgba([0, 255, 0, 255]));
            }
        }
        let result2 = compare_images(&img1, &img2, Mode::Ssim, &config);
        assert!(matches!(
            result2,
            ComparisonResult::Mismatch {
                detail: MismatchDetail::Ssim { .. },
                ..
            }
        ));
    }
}
