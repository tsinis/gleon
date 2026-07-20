//! Pixel-by-pixel image comparison.

use image::RgbaImage;

/// Compares two images of the same dimensions pixel-by-pixel.
/// Returns the number of mismatched pixels and a composite diff image
/// where matching areas are darkened and mismatched areas are painted magenta.
pub fn compare_pixels(baseline: &RgbaImage, actual: &RgbaImage) -> (u32, RgbaImage) {
    let width = baseline.width();
    let height = baseline.height();

    let baseline_raw = baseline.as_raw();
    let actual_raw = actual.as_raw();

    let mut diff_raw = vec![0u8; baseline_raw.len()];
    let mut diff_count = 0;

    let b_chunks = baseline_raw.chunks_exact(4);
    let a_chunks = actual_raw.chunks_exact(4);
    let d_chunks = diff_raw.chunks_exact_mut(4);

    for ((b_chunk, a_chunk), d_chunk) in b_chunks.zip(a_chunks).zip(d_chunks) {
        if b_chunk != a_chunk {
            diff_count += 1;
            // Magenta: [255, 0, 255, 255]
            d_chunk.copy_from_slice(&[255, 0, 255, 255]);
        } else {
            // Darken matching pixel: divide R, G, B by 2, keep A
            d_chunk[0] = b_chunk[0] / 2;
            d_chunk[1] = b_chunk[1] / 2;
            d_chunk[2] = b_chunk[2] / 2;
            d_chunk[3] = b_chunk[3];
        }
    }

    let diff_image = RgbaImage::from_raw(width, height, diff_raw)
        .expect("invariant: diff_raw length must be exactly width * height * 4");

    (diff_count, diff_image)
}

/// Counts the number of mismatched pixels without allocating a diff image.
pub fn count_mismatched_pixels(baseline: &RgbaImage, actual: &RgbaImage) -> u32 {
    let baseline_raw = baseline.as_raw();
    let actual_raw = actual.as_raw();

    // Fast path: reinterpret the byte slices as u32 for cheaper, word-sized
    // equality checks. RgbaImage guarantees the raw length is a multiple of 4.
    // `try_cast_slice` never panics on misaligned input; it simply returns
    // Err, in which case we fall back to the byte-chunk comparison below.
    if let (Ok(b_u32), Ok(a_u32)) = (
        bytemuck::try_cast_slice::<u8, u32>(baseline_raw),
        bytemuck::try_cast_slice::<u8, u32>(actual_raw),
    ) {
        b_u32
            .iter()
            .zip(a_u32.iter())
            .filter(|(b, a)| b != a)
            .count() as u32
    } else {
        // Fallback for weirdly aligned data (e.g. from FFI)
        let b_chunks = baseline_raw.chunks_exact(4);
        let a_chunks = actual_raw.chunks_exact(4);
        b_chunks.zip(a_chunks).filter(|(b, a)| b != a).count() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn test_compare_pixels_identical() {
        let img1 = ImageBuffer::from_pixel(10, 10, Rgba([255, 0, 0, 255]));
        let img2 = ImageBuffer::from_pixel(10, 10, Rgba([255, 0, 0, 255]));

        let (diff_count, diff_img) = compare_pixels(&img1, &img2);
        assert_eq!(diff_count, 0);
        // Matching pixels should be darkened: 255 / 2 = 127
        assert_eq!(*diff_img.get_pixel(0, 0), Rgba([127, 0, 0, 255]));
    }

    #[test]
    fn test_compare_pixels_mismatch() {
        let img1 = ImageBuffer::from_pixel(10, 10, Rgba([255, 0, 0, 255]));
        let mut img2 = ImageBuffer::from_pixel(10, 10, Rgba([255, 0, 0, 255]));
        img2.put_pixel(5, 5, Rgba([0, 255, 0, 255]));

        let (diff_count, diff_img) = compare_pixels(&img1, &img2);
        assert_eq!(diff_count, 1);
        // The mismatched pixel should be magenta
        assert_eq!(*diff_img.get_pixel(5, 5), Rgba([255, 0, 255, 255]));
        // The matching pixel should be darkened
        assert_eq!(*diff_img.get_pixel(0, 0), Rgba([127, 0, 0, 255]));
    }
}
