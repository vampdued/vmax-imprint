//! Colour sampling and block grouping — port of `sample_colors` and
//! `group_lines_into_blocks` from `caption_replacer.py`.
//!
//! All thresholds are the Python ones, unchanged:
//! ring pad 14% of box height, ring subsampled to 1,500 px, boxed iff
//! mean channel std < 34.0, dark boxed backgrounds snap to [18,18,18],
//! text colour = median of inner pixels at/above the 92nd percentile of
//! Euclidean distance from the background colour (inner subsampled to
//! 2,000 px). Blocks merge when text colours are < 75 apart and lines are
//! on the same row or stacked; unboxed blocks under `min_chars` are dropped.

use crate::model::{Block, DroppedItem, RawLine, clamp_box, color_dist};
use image::RgbImage;

/// Sample background / text colours for one line bbox in an RGB image.
/// Returns `(bg_color, text_color, is_boxed)`.
///
/// The colours are sampled per block from the original image, so a rendered
/// caption keeps the meme's own palette — a yellow plate on top and a cyan
/// plate below is the original's doing, not a theme bug, and the plates must
/// never be normalised to one colour.
pub fn sample_colors(img_rgb: &RgbImage, bbox: [i32; 4]) -> ([u8; 3], [u8; 3], bool) {
    let (w, h) = (img_rgb.width() as i32, img_rgb.height() as i32);
    let [x1, y1, x2, y2] = clamp_box(bbox, w, h);
    if x2 <= x1 || y2 <= y1 {
        return ([18, 18, 18], [255, 255, 255], false);
    }
    let pad = 3.max(((y2 - y1) as f32 * 0.14) as i32);
    let [ox1, oy1, ox2, oy2] = clamp_box([x1 - pad, y1 - pad, x2 + pad, y2 + pad], w, h);

    // Outer ring = outer patch minus the inner box.
    let mut ring: Vec<[u8; 3]> = Vec::new();
    for y in oy1..oy2 {
        for x in ox1..ox2 {
            if x >= x1 && x < x2 && y >= y1 && y < y2 {
                continue;
            }
            let p = img_rgb.get_pixel(x as u32, y as u32);
            ring.push([p[0], p[1], p[2]]);
        }
    }
    if ring.is_empty() {
        for y in oy1..oy2 {
            for x in ox1..ox2 {
                let p = img_rgb.get_pixel(x as u32, y as u32);
                ring.push([p[0], p[1], p[2]]);
            }
        }
    }
    if ring.len() > 1500 {
        let step = (ring.len() / 1500).max(1);
        ring = ring.into_iter().step_by(step).collect();
    }

    let bg_color = median_rgb(&ring);
    // mean over channels of per-channel std (np.mean(np.std(ring, axis=0)))
    let n = ring.len() as f64;
    let mut bg_std = 0.0f64;
    for c in 0..3 {
        let mean = ring.iter().map(|p| p[c] as f64).sum::<f64>() / n;
        let var = ring.iter().map(|p| (p[c] as f64 - mean).powi(2)).sum::<f64>() / n;
        bg_std += var.sqrt();
    }
    bg_std /= 3.0;
    let is_boxed = bg_std < 34.0;

    let mut bg = bg_color;
    if is_boxed && bg.iter().max().copied().unwrap_or(0) < 55 {
        bg = [18, 18, 18]; // snap dark boxed bg to near-black: prevents photo bleed
    }

    // Inner pixels: distance from bg, 92nd percentile and above -> text colour.
    let mut inner: Vec<[u8; 3]> = Vec::new();
    for y in y1..y2 {
        for x in x1..x2 {
            let p = img_rgb.get_pixel(x as u32, y as u32);
            inner.push([p[0], p[1], p[2]]);
        }
    }
    let text_color = if inner.is_empty() {
        [255, 255, 255]
    } else {
        if inner.len() > 2000 {
            let step = (inner.len() / 2000).max(1);
            inner = inner.into_iter().step_by(step).collect();
        }
        let dists: Vec<f32> = inner
            .iter()
            .map(|p| {
                let d = [
                    p[0] as f32 - bg[0] as f32,
                    p[1] as f32 - bg[1] as f32,
                    p[2] as f32 - bg[2] as f32,
                ];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            })
            .collect();
        let mut sorted = dists.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = ((sorted.len() as f32 * 0.92) as usize).min(sorted.len().saturating_sub(1));
        let thresh = sorted[idx];
        let fg: Vec<[u8; 3]> = inner
            .iter()
            .zip(dists.iter())
            .filter(|(_, d)| **d >= thresh)
            .map(|(p, _)| *p)
            .collect();
        if fg.is_empty() {
            [255, 255, 255]
        } else {
            median_rgb(&fg)
        }
    };
    (bg, text_color, is_boxed)
}

fn median_rgb(pixels: &[[u8; 3]]) -> [u8; 3] {
    if pixels.is_empty() {
        return [0, 0, 0];
    }
    let mut out = [0u8; 3];
    for c in 0..3 {
        let mut vals: Vec<u8> = pixels.iter().map(|p| p[c]).collect();
        vals.sort_unstable();
        let mid = vals.len() / 2;
        out[c] = if vals.len() % 2 == 1 {
            vals[mid]
        } else {
            ((vals[mid - 1] as u16 + vals[mid] as u16) / 2) as u8
        };
    }
    out
}

/// Group raw lines into blocks, filtering by confidence first.
/// Returns `(blocks, dropped)` — dropped carries the Python reason strings
/// (`"conf X.XX < Y.YY"` / `"unboxed N < M chars"`).
pub fn group_lines_into_blocks(
    raw_lines: &[RawLine],
    min_conf: f32,
    min_chars: usize,
) -> (Vec<Block>, Vec<DroppedItem>) {
    let mut kept: Vec<&RawLine> = Vec::new();
    let mut dropped: Vec<DroppedItem> = Vec::new();

    for line in raw_lines {
        let score = line.score;
        if score < min_conf {
            dropped.push(DroppedItem {
                bbox: line.bbox,
                text: line.text.clone(),
                reason: format!("confidence {score:.2}, needed {min_conf:.2}"),
            });
        } else {
            kept.push(line);
        }
    }
    if kept.is_empty() {
        return (Vec::new(), dropped);
    }

    kept.sort_by_key(|l| (l.bbox[1], l.bbox[0]));

    // blocks as Vec<Vec<&RawLine>>; compare against the last line of each
    // block in reverse order, like Python's `for blk in reversed(blocks)`.
    let mut blocks: Vec<Vec<&RawLine>> = Vec::new();
    for line in kept {
        let [lx1, ly1, lx2, ly2] = line.bbox;
        let lh = (ly2 - ly1).max(1) as f32;
        let mut placed = false;
        for blk in blocks.iter_mut().rev() {
            let last = blk[blk.len() - 1];
            let [px1, py1, px2, py2] = last.bbox;
            let ph = (py2 - py1).max(1) as f32;
            let avg_h = (lh + ph) / 2.0;

            let v_overlap = (ly2.min(py2) - ly1.max(py1)) as f32;
            let same_row = v_overlap > avg_h * 0.45 && ((lx1 - px2) as f32).abs() < avg_h * 3.0;
            let v_gap = (ly1 - py2) as f32;
            let h_overlap = (lx2.min(px2) - lx1.max(px1)) as f32;
            let stacked = (-avg_h * 0.3..=avg_h * 0.85).contains(&v_gap)
                && h_overlap > -avg_h * 1.5;
            let similar_color = color_dist(line.text_color, last.text_color) < 75.0;

            if (same_row || stacked) && similar_color {
                blk.push(line);
                placed = true;
                break;
            }
        }
        if !placed {
            blocks.push(vec![line]);
        }
    }

    let mut result: Vec<Block> = Vec::new();
    let mut next_id = 1usize;
    for blk in blocks {
        let mut ls: Vec<&RawLine> = blk;
        let min_x = ls.iter().map(|l| l.bbox[0]).min().unwrap_or(0);
        let min_y = ls.iter().map(|l| l.bbox[1]).min().unwrap_or(0);
        let max_x = ls.iter().map(|l| l.bbox[2]).max().unwrap_or(0);
        let max_y = ls.iter().map(|l| l.bbox[3]).max().unwrap_or(0);
        let avg_line_h = ls.iter().map(|l| l.bbox[3] - l.bbox[1]).sum::<i32>() as f32
            / ls.len() as f32;
        let avg_line_h_i = avg_line_h as i32;

        ls.sort_by_key(|l| {
            (
                (l.bbox[1] as f32 / (avg_line_h * 0.5).max(1.0)).round() as i32,
                l.bbox[0],
            )
        });
        let combined_text = ls
            .iter()
            .map(|l| l.text.trim())
            .collect::<Vec<_>>()
            .join(" ");
        let avg_conf = (ls.iter().map(|l| l.score).sum::<f32>() / ls.len() as f32 * 100.0)
            .round()
            / 100.0;

        let bg_color = median_rgb(
            &ls.iter().map(|l| l.bg_color).collect::<Vec<_>>(),
        );
        let text_color = median_rgb(
            &ls.iter().map(|l| l.text_color).collect::<Vec<_>>(),
        );
        let is_boxed =
            ls.iter().filter(|l| l.is_boxed).count() as f32 >= ls.len() as f32 / 2.0;

        let n_chars = combined_text.chars().count();
        if !is_boxed && n_chars < min_chars {
            dropped.push(DroppedItem {
                bbox: [min_x, min_y, max_x, max_y],
                text: combined_text,
                reason: format!("{n_chars} chars, needs {min_chars}"),
            });
            continue;
        }

        result.push(Block {
            id: next_id,
            bbox: [min_x, min_y, max_x, max_y],
            line_bboxes: ls.iter().map(|l| l.bbox).collect(),
            original_text: combined_text,
            new_text: String::new(),
            confidence: avg_conf,
            text_color,
            bg_color,
            is_boxed,
            font_size_est: 16.max((avg_line_h * 1.05) as i32),
        });
        next_id += 1;
        let _ = avg_line_h_i;
    }

    (result, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RawLine;

    fn line(bbox: [i32; 4], text: &str, score: f32) -> RawLine {
        RawLine {
            bbox,
            text: text.into(),
            score,
            bg_color: [0, 0, 0],
            text_color: [255, 255, 255],
            is_boxed: true,
        }
    }

    #[test]
    fn conf_filter_records_reason() {
        let raw = vec![line([0, 0, 10, 10], "hi", 0.05), line([0, 20, 10, 30], "hello world hello", 0.9)];
        let (blocks, dropped) = group_lines_into_blocks(&raw, 0.6, 12);
        assert_eq!(blocks.len(), 1);
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].reason, "confidence 0.05, needed 0.60");
    }

    #[test]
    fn stacked_lines_merge_and_unboxed_short_drops() {
        let mut a = line([0, 0, 100, 20], "first line here", 0.9);
        a.is_boxed = false;
        let mut b = line([0, 22, 100, 42], "second line here", 0.9);
        b.is_boxed = false;
        let (blocks, _) = group_lines_into_blocks(&[a, b], 0.6, 12);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].original_text, "first line here second line here");

        let short = line([0, 0, 50, 20], "tiny", 0.9);
        let mut short = short;
        short.is_boxed = false;
        let (blocks, dropped) = group_lines_into_blocks(&[short], 0.6, 12);
        assert!(blocks.is_empty());
        assert!(dropped[0].reason.contains("4 chars, needs 12"));
    }

    #[test]
    fn sample_colors_boxed_dark_snaps() {
        // 40x40 image, black plate in the middle, white text pixels inside.
        // The bbox is the *text* bbox (as OCR returns), so the 14% ring
        // around it samples the black plate, not the grey photo outside.
        let mut img = RgbImage::new(40, 40);
        for p in img.pixels_mut() {
            *p = image::Rgb([200, 200, 200]);
        }
        for y in 10..30 {
            for x in 10..30 {
                img.put_pixel(x, y, image::Rgb([10, 10, 10]));
            }
        }
        for y in 16..24 {
            for x in 14..26 {
                img.put_pixel(x, y, image::Rgb([255, 255, 255]));
            }
        }
        let (bg, fg, boxed) = sample_colors(&img, [14, 16, 26, 24]);
        assert!(boxed);
        assert_eq!(bg, [18, 18, 18]); // snapped
        assert!(fg[0] > 200);
    }
}
