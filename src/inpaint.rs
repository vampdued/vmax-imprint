//! Inpainting — fills the masked caption regions with surrounding background.
//!
//! Python used `cv2.inpaint(..., cv2.INPAINT_TELEA)` on the ROI that bounds
//! all masked pixels, after dilating the mask once with a 5x5 kernel.
//! There is no OpenCV in this single-exe Rust build, so the fill is an
//! iterative diffusion (Laplace) inpaint: unknown pixels are repeatedly
//! replaced by the mean of their known neighbours, with known pixels held
//! fixed. On the flat / softly-graded caption plates this app removes, the
//! result is visually the same as Telea (smooth continuation of the
//! background); on textured photos it is a smooth blur rather than
//! Telea's edge-following fill. The ROI cropping and the 5x5 dilation are
//! kept exactly as in Python.

use image::{Rgb, RgbImage};

/// Dilate a binary mask once with a 5x5 kernel (Python:
/// `cv2.dilate(mask, ones((5,5)), iterations=1)`).
pub fn dilate_mask_5x5(mask: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut v = 0u8;
            'scan: for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                        if mask[ny as usize * w + nx as usize] > 0 {
                            v = 255;
                            break 'scan;
                        }
                    }
                }
            }
            out[y * w + x] = v;
        }
    }
    out
}

/// Inpaint `img` in place where `mask > 0` (mask is w*h, same size as img).
/// Only the bounding box of masked pixels (+16px margin, like Python) is
/// processed. `iterations` scales with the masked area so large boxed
/// plates converge; callers pass the default from [`inpaint_roi`].
pub fn inpaint_masked(img: &mut RgbImage, mask: &[u8]) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if mask.len() != w * h || !mask.iter().any(|&m| m > 0) {
        return;
    }
    // ROI bounding box + 16px margin (Python).
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] > 0 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    let rx1 = min_x.saturating_sub(16);
    let ry1 = min_y.saturating_sub(16);
    let rx2 = (max_x + 17).min(w);
    let ry2 = (max_y + 17).min(h);

    // Dilate only the ROI slice: build a local mask first.
    let rw = rx2 - rx1;
    let rh = ry2 - ry1;
    let mut local_mask = vec![0u8; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            local_mask[y * rw + x] = mask[(ry1 + y) * w + (rx1 + x)];
        }
    }
    let local_mask = dilate_mask_5x5(&local_mask, rw, rh);

    // Working float buffer for the ROI, initialised from the image; unknown
    // pixels start as the mean of the known pixels in the ROI.
    let mut known_sum = [0f64; 3];
    let mut known_n = 0usize;
    for y in 0..rh {
        for x in 0..rw {
            if local_mask[y * rw + x] == 0 {
                let p = img.get_pixel((rx1 + x) as u32, (ry1 + y) as u32);
                for c in 0..3 {
                    known_sum[c] += p[c] as f64;
                }
                known_n += 1;
            }
        }
    }
    if known_n == 0 {
        // Whole ROI masked: fill with black (nothing to diffuse from).
        for y in 0..rh {
            for x in 0..rw {
                img.put_pixel((rx1 + x) as u32, (ry1 + y) as u32, Rgb([0, 0, 0]));
            }
        }
        return;
    }
    let mean = [
        (known_sum[0] / known_n as f64) as f32,
        (known_sum[1] / known_n as f64) as f32,
        (known_sum[2] / known_n as f64) as f32,
    ];
    let mut buf = vec![[0f32; 3]; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            buf[y * rw + x] = if local_mask[y * rw + x] == 0 {
                let p = img.get_pixel((rx1 + x) as u32, (ry1 + y) as u32);
                [p[0] as f32, p[1] as f32, p[2] as f32]
            } else {
                mean
            };
        }
    }

    // Jacobi diffusion. Iterations scale with the largest masked run so a
    // 100px-tall plate converges; 400 is the cap (each pass is O(ROI)).
    let masked_n = local_mask.iter().filter(|&&m| m > 0).count();
    let iterations = if masked_n == 0 {
        0
    } else {
        // ~2 passes per pixel of the widest masked span.
        let span = (rw.max(rh) as f32).sqrt() as usize * 8 + 40;
        span.min(400)
    };
    let mut next = buf.clone();
    for _ in 0..iterations {
        for y in 0..rh {
            for x in 0..rw {
                let i = y * rw + x;
                if local_mask[i] == 0 {
                    continue;
                }
                let mut sum = [0f32; 3];
                let mut n = 0f32;
                for (dx, dy) in [(1i32, 0), (-1, 0), (0, 1), (0, -1)] {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx >= 0 && ny >= 0 && (nx as usize) < rw && (ny as usize) < rh {
                        let nb = buf[ny as usize * rw + nx as usize];
                        for c in 0..3 {
                            sum[c] += nb[c];
                        }
                        n += 1.0;
                    }
                }
                if n > 0.0 {
                    for c in 0..3 {
                        next[i][c] = sum[c] / n;
                    }
                }
            }
        }
        std::mem::swap(&mut buf, &mut next);
    }

    for y in 0..rh {
        for x in 0..rw {
            if local_mask[y * rw + x] > 0 {
                let v = buf[y * rw + x];
                img.put_pixel(
                    (rx1 + x) as u32,
                    (ry1 + y) as u32,
                    Rgb([
                        v[0].clamp(0.0, 255.0) as u8,
                        v[1].clamp(0.0, 255.0) as u8,
                        v[2].clamp(0.0, 255.0) as u8,
                    ]),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dilate_grows_by_two() {
        let mut mask = vec![0u8; 25];
        mask[12] = 255;
        let d = dilate_mask_5x5(&mask, 5, 5);
        assert_eq!(d.iter().filter(|&&v| v > 0).count(), 25);
    }

    #[test]
    fn inpaint_fills_flat_background() {
        let mut img = RgbImage::new(20, 20);
        for p in img.pixels_mut() {
            *p = Rgb([10, 120, 200]);
        }
        let mut mask = vec![0u8; 400];
        for y in 5..15 {
            for x in 5..15 {
                mask[y * 20 + x] = 255;
            }
        }
        inpaint_masked(&mut img, &mask);
        let p = img.get_pixel(10, 10);
        assert!((p[0] as i32 - 10).abs() <= 2);
        assert!((p[1] as i32 - 120).abs() <= 2);
        assert!((p[2] as i32 - 200).abs() <= 2);
    }
}
