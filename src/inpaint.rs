//! Inpainting — fills the masked caption regions with surrounding background.
//!
//! Python used `cv2.inpaint(..., cv2.INPAINT_TELEA)` on the ROI that bounds
//! all masked pixels, after dilating the mask once with a 5x5 kernel.
//! There is no OpenCV in this single-exe Rust build, so the fill is a
//! front-propagation (Telea-style) inpaint: unknown pixels are filled in
//! order of distance from the nearest known pixel, each from the mean of
//! its already-known neighbours, then a few Jacobi smoothing passes soften
//! the seams. This propagates local colours inward (the wall's beige
//! gradient, a sharp dark/light edge), where the earlier pure-Jacobi
//! diffusion never converged on wide masked bands and left a flat band of
//! the ROI mean colour. The ROI cropping and the 5x5 dilation are kept
//! exactly as in Python.

use image::{Rgb, RgbImage};

/// Dilate a binary mask once with a 5x5 kernel (Python:
/// `cv2.dilate(mask, ones((5,5)), iterations=1)`).
pub fn dilate_mask_5x5(mask: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut horiz = vec![0u8; w * h];
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            let x0 = x.saturating_sub(2);
            let x1 = (x + 2).min(w - 1);
            if mask[row + x0..=row + x1].iter().any(|&m| m > 0) {
                horiz[row + x] = 255;
            }
        }
    }
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        let y0 = y.saturating_sub(2);
        let y1 = (y + 2).min(h - 1);
        for x in 0..w {
            for ny in y0..=y1 {
                if horiz[ny * w + x] > 0 {
                    out[y * w + x] = 255;
                    break;
                }
            }
        }
    }
    out
}

/// Inpaint `img` in place where `mask > 0` (mask is w*h, same size as img).
/// Only the bounding box of masked pixels (+16px margin, like Python) is
/// processed. `iterations` scales with the masked area so large boxed
/// plates converge; callers pass the default from [`inpaint_roi`].
#[allow(dead_code)]
pub fn inpaint_masked(img: &mut RgbImage, mask: &[u8]) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if mask.len() != w * h || !mask.iter().any(|&m| m > 0) {
        return;
    }
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
    inpaint_masked_roi(img, mask, [min_x, min_y, max_x + 1, max_y + 1]);
}

/// Inpaint only within a single block's tight bounding box (+16 px margin) so distant
/// blocks never share one global ROI mean or streak colors across unrelated regions.
pub fn inpaint_masked_roi(img: &mut RgbImage, mask: &[u8], bounds: [usize; 4]) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if mask.len() != w * h {
        return;
    }
    let bx1 = bounds[0].min(w);
    let by1 = bounds[1].min(h);
    let bx2 = bounds[2].min(w);
    let by2 = bounds[3].min(h);
    if bx2 <= bx1 || by2 <= by1 {
        return;
    }
    let mut has_any = false;
    for y in by1..by2 {
        if mask[y * w + bx1..y * w + bx2].iter().any(|&m| m > 0) {
            has_any = true;
            break;
        }
    }
    if !has_any {
        return;
    }
    let rx1 = bx1.saturating_sub(16);
    let ry1 = by1.saturating_sub(16);
    let rx2 = (bx2 + 16).min(w);
    let ry2 = (by2 + 16).min(h);

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

    // Front-propagation fill, Telea-style: unknown pixels are filled in
    // order of distance from the nearest known pixel, each from the mean
    // of its already-known neighbours. This propagates LOCAL colours
    // inward (the wall's beige gradient, the door's dark red), where the
    // old pure-Jacobi diffusion never converged on wide masked bands and
    // left a flat band of the ROI mean colour. Each pixel is filled once
    // (a queue, not repeated full-ROI passes), then a few Jacobi passes
    // below smooth the seams.
    let mut filled = vec![false; rw * rh];
    for y in 0..rh {
        for x in 0..rw {
            filled[y * rw + x] = local_mask[y * rw + x] == 0;
        }
    }
    let mut queue: std::collections::VecDeque<(usize, usize)> = std::collections::VecDeque::new();
    for y in 0..rh {
        for x in 0..rw {
            if filled[y * rw + x] {
                continue;
            }
            let adjacent_known = [(1i32, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                nx >= 0
                    && ny >= 0
                    && (nx as usize) < rw
                    && (ny as usize) < rh
                    && filled[ny as usize * rw + nx as usize]
            });
            if adjacent_known {
                queue.push_back((x, y));
            }
        }
    }
    while let Some((x, y)) = queue.pop_front() {
        let i = y * rw + x;
        if filled[i] {
            continue;
        }
        let mut sum = [0f32; 3];
        let mut n = 0f32;
        for (dx, dy) in [(1i32, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx >= 0 && ny >= 0 && (nx as usize) < rw && (ny as usize) < rh {
                let ni = ny as usize * rw + nx as usize;
                if filled[ni] {
                    let nb = buf[ni];
                    for c in 0..3 {
                        sum[c] += nb[c];
                    }
                    n += 1.0;
                }
            }
        }
        if n > 0.0 {
            for c in 0..3 {
                buf[i][c] = sum[c] / n;
            }
            filled[i] = true;
            for (dx, dy) in [(1i32, 0), (-1, 0), (0, 1), (0, -1)] {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx >= 0 && ny >= 0 && (nx as usize) < rw && (ny as usize) < rh {
                    let ni = ny as usize * rw + nx as usize;
                    if !filled[ni] {
                        queue.push_back((nx as usize, ny as usize));
                    }
                }
            }
        }
    }

    // A few Jacobi smoothing passes over the filled pixels only, to soften
    // the fill seams; known (original) pixels stay fixed. The front fill
    // above already converged, so this is polish, not the fill itself.
    let iterations = 30;
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

    #[test]
    fn inpaint_preserves_local_colours_across_a_band() {
        // Two-tone image, sharp black/white edge at x=30; mask a band
        // straddling the edge. The fill must keep the edge (left side
        // dark, right side bright), not smear the ROI mean grey across
        // the whole band the way the old unconverged diffusion did.
        let mut img = RgbImage::new(60, 20);
        for y in 0..20 {
            for x in 0..60 {
                let v = if x < 30 { 0u8 } else { 255u8 };
                img.put_pixel(x, y, Rgb([v, v, v]));
            }
        }
        let mut mask = vec![0u8; 60 * 20];
        for y in 0..20 {
            for x in 25..35 {
                mask[y as usize * 60 + x as usize] = 255;
            }
        }
        inpaint_masked(&mut img, &mask);
        let left = img.get_pixel(27, 10);
        let right = img.get_pixel(33, 10);
        assert!(left[0] < 60, "left of edge went grey: {left:?}");
        assert!(right[0] > 195, "right of edge went grey: {right:?}");
    }
}
