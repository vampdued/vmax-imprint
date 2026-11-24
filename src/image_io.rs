//! Image I/O — unicode-safe load/save (Python used `np.fromfile` +
//! `cv2.imdecode` precisely so non-ASCII paths work on Windows; plain
//! `image::open` already reads via `fs::read`, which is unicode-safe).

use anyhow::{Context, Result};
use image::{RgbImage, codecs::jpeg::JpegEncoder, codecs::png::PngEncoder};
use std::path::Path;

pub const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp"];

pub fn is_image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Load an image file as RGB (any supported format).
pub fn load_image(path: &Path) -> Result<RgbImage> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let img = image::load_from_memory(&bytes)
        .with_context(|| format!("could not decode {}", path.display()))?;
    Ok(img.to_rgb8())
}

/// Save an image, mirroring Python `save_image`:
/// JPEG quality 95, WebP quality 95, PNG default compression.
pub fn save_image(img: &RgbImage, path: &Path) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let file = std::fs::File::create(path)
        .with_context(|| format!("could not create {}", path.display()))?;
    let (w, h) = (img.width(), img.height());
    match ext.as_str() {
        "jpg" | "jpeg" => {
            let enc = JpegEncoder::new_with_quality(file, 95);
            use image::ImageEncoder;
            enc.write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
                .context("JPEG encode failed")?;
        }
        "webp" => {
            // image 0.25 only supports *lossless* WebP encoding (no q95
            // lossy path) — documented deviation from Python's q95 lossy.
            let enc = image::codecs::webp::WebPEncoder::new_lossless(file);
            use image::ImageEncoder;
            enc.write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
                .context("WebP encode failed")?;
        }
        _ => {
            let enc = PngEncoder::new(file);
            use image::ImageEncoder;
            enc.write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
                .context("PNG encode failed")?;
        }
    }
    Ok(())
}

/// Downscale an RGB image so its longest side is `max_side` (bilinear /
/// triangle filter — the OCR pre-scale; Python used INTER_AREA here, which
/// is a box filter; triangle is the closest `image`-crate equivalent and
/// differs only on very noisy downscales).
#[allow(dead_code)]
pub fn downscale_for_ocr(img: &RgbImage, max_side: u32) -> (RgbImage, f32) {
    let (w, h) = (img.width(), img.height());
    let m = w.max(h);
    if m <= max_side {
        return (img.clone(), 1.0);
    }
    let s = max_side as f32 / m as f32;
    let nw = (w as f32 * s).round().max(1.0) as u32;
    let nh = (h as f32 * s).round().max(1.0) as u32;
    (
        image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle),
        s,
    )
}
