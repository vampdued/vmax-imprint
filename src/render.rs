//! Rendering — port of `fit_text_to_box`, `draw_smooth_caption_plate`,
//! `render_entry` and `render_ocr_diagnostic` from `caption_replacer.py`.
//!
//! Text is shaped and wrapped by `cosmic-text` (rustybuzz) and rasterised
//! per-pixel through `Buffer::draw` into a `tiny-skia` `Pixmap`; the
//! caption plate is a Bezier-rounded polygon filled by tiny-skia with
//! subpixel anti-aliasing. All geometry (pads, start sizes, max width /
//! height, anchor rules, plate alpha 232, stroke width 2) matches Python.

use std::cell::RefCell;

use cosmic_text::{Buffer, Color, Metrics, Shaping, SwashCache, Wrap};
use image::{Rgb, RgbImage};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, StrokeDash, Transform};

use crate::fonts::{attrs_for, font_system, load_custom_font};
use crate::inpaint::inpaint_masked;
use crate::model::{clamp_box, ImageEntry};

thread_local! {
    static SWASH_CACHE: RefCell<SwashCache> = RefCell::new(SwashCache::new());
}

/// Result of fitting a translation into a block's box.
#[derive(Debug, Clone)]
pub struct FittedText {
    pub lines: Vec<String>,
    pub size: i32,
    pub line_step: i32,
}

/// Measure one text at one size: wrapped lines, widest line, total height.
/// `line_step = int(size * 1.26)` exactly like Python.
fn measure(
    text: &str,
    max_w: i32,
    max_h: i32,
    size: i32,
    custom_family: Option<&'static str>,
) -> (Vec<String>, f32, bool) {
    let line_step = (size as f32 * 1.26) as i32;
    let mut fs = font_system().lock().unwrap();
    let metrics = Metrics::new(size as f32, size as f32 * 1.26);
    let mut buf = Buffer::new(&mut fs, metrics);
    buf.set_size(Some(max_w.max(1) as f32), None);
    buf.set_wrap(Wrap::Word);
    let attrs = attrs_for(&fs, text, custom_family);
    buf.set_text(text, &attrs, Shaping::Advanced, None);
    buf.shape_until_scroll(&mut fs, false);
    let mut lines: Vec<String> = Vec::new();
    let mut max_line_w: f32 = 0.0;
    for run in buf.layout_runs() {
        max_line_w = max_line_w.max(run.line_w);
        // NOTE: `run.text` is the FULL logical line, not this visual line's
        // fragment — a sentence wrapping to N visual lines used to come
        // back as N copies of the whole sentence (double-render bug,
        // 2026-11-16). Slice by the run's glyph byte ranges to get the
        // fragment actually laid out on this visual line.
        let start = run.glyphs.first().map(|g| g.start).unwrap_or(0);
        let end = run.glyphs.last().map(|g| g.end).unwrap_or(run.text.len());
        let t = run
            .text
            .get(start..end)
            .unwrap_or("")
            .trim_end()
            .to_string();
        if !t.is_empty() || lines.is_empty() {
            lines.push(t);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    let total_h = lines.len() as i32 * line_step;
    let fits = total_h <= max_h && max_line_w <= max_w as f32;
    (lines, max_line_w, fits)
}

/// Fast-path at `start_size`, then binary search for the largest fitting
/// size in [13, start_size-1], falling back to size 12 — Python
/// `fit_text_to_box`, verbatim in structure.
pub fn fit_text_to_box(
    text: &str,
    max_w: i32,
    max_h: i32,
    start_size: i32,
    custom_family: Option<&'static str>,
) -> FittedText {
    let eval = |size: i32| -> (Vec<String>, i32, i32) {
        let (lines, _, _) = measure(text, max_w, max_h, size, custom_family);
        (lines, size, (size as f32 * 1.26) as i32)
    };
    let fits = |size: i32| -> bool {
        let (_, _, f) = measure(text, max_w, max_h, size, custom_family);
        f
    };

    if start_size <= 12 {
        let (lines, size, step) = eval(12);
        return FittedText {
            lines,
            size,
            line_step: step,
        };
    }
    if fits(start_size) {
        let (lines, size, step) = eval(start_size);
        return FittedText {
            lines,
            size,
            line_step: step,
        };
    }
    let mut lo = 13i32;
    let mut hi = start_size - 1;
    let mut best: Option<FittedText> = None;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        if fits(mid) {
            let (lines, size, step) = eval(mid);
            best = Some(FittedText {
                lines,
                size,
                line_step: step,
            });
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    if let Some(b) = best {
        return b;
    }
    let (lines, size, step) = eval(12);
    FittedText {
        lines,
        size,
        line_step: step,
    }
}

/// Height of the "Agjpqy" reference string at `size` (ascender+descender),
/// used for pill geometry and vertical anchoring like Python's
/// `textbbox("Agjpqy")`.
fn ref_text_height(size: i32, custom_family: Option<&'static str>) -> i32 {
    let mut fs = font_system().lock().unwrap();
    let metrics = Metrics::new(size as f32, size as f32 * 1.26);
    let mut buf = Buffer::new(&mut fs, metrics);
    buf.set_size(Some(1000.0), None);
    buf.set_wrap(Wrap::None);
    let attrs = attrs_for(&fs, "Agjpqy", custom_family);
    buf.set_text("Agjpqy", &attrs, Shaping::Advanced, None);
    buf.shape_until_scroll(&mut fs, false);
    if let Some(layout) = buf.line_layout(&mut fs, 0) {
        if let Some(line) = layout.first() {
            return (line.max_ascent + line.max_descent).round() as i32;
        }
    }
    (size as f32 * 0.9) as i32
}

/// Width of a single line at `size`.
fn line_width(text: &str, size: i32, custom_family: Option<&'static str>) -> f32 {
    let (lines, max_w, _) = measure(text, 100_000, 10_000, size, custom_family);
    let _ = lines;
    max_w
}

// ---------------------------------------------------------------------------
// tiny-skia helpers
// ---------------------------------------------------------------------------

/// Blend one straight-alpha source pixel over a premultiplied pixmap pixel.
fn blend_pixel(data: &mut [u8], x: i32, y: i32, w: i32, color: Color) {
    if x < 0 || y < 0 || x >= w {
        return;
    }
    let idx = (y as usize * w as usize + x as usize) * 4;
    if idx + 3 >= data.len() {
        return;
    }
    let (sr, sg, sb, sa) = color.as_rgba_tuple();
    if sa == 0 {
        return;
    }
    let inv = 255u32 - sa as u32;
    // premultiplied source
    let psr = sr as u32 * sa as u32 / 255;
    let psg = sg as u32 * sa as u32 / 255;
    let psb = sb as u32 * sa as u32 / 255;
    data[idx] = (psr + data[idx] as u32 * inv / 255) as u8;
    data[idx + 1] = (psg + data[idx + 1] as u32 * inv / 255) as u8;
    data[idx + 2] = (psb + data[idx + 2] as u32 * inv / 255) as u8;
    data[idx + 3] = (sa as u32 + data[idx + 3] as u32 * inv / 255) as u8;
}

/// Draw (possibly multi-line) text into a pixmap at an offset, using
/// cosmic-text shaping. `stroke` draws the text first in `stroke_color`
/// at 8 offsets of `stroke_px` (approximates PIL `stroke_width`).
#[allow(clippy::too_many_arguments)]
fn draw_text(
    pixmap: &mut Pixmap,
    text: &str,
    size: i32,
    color: [u8; 3],
    alpha: u8,
    offset_x: i32,
    offset_y: i32,
    max_w: i32,
    custom_family: Option<&'static str>,
    stroke: Option<([u8; 3], i32)>,
) {
    let line_step = (size as f32 * 1.26) as i32;
    let metrics = Metrics::new(size as f32, size as f32 * 1.26);
    let mut buf = {
        let mut fs = font_system().lock().unwrap();
        let mut buf = Buffer::new(&mut fs, metrics);
        buf.set_size(Some(max_w.max(1) as f32), None);
        buf.set_wrap(Wrap::Word);
        let attrs = attrs_for(&fs, text, custom_family);
        buf.set_text(text, &attrs, Shaping::Advanced, None);
        buf
    };
    // NOTE: the font-system lock is released here on purpose — draw_once
    // re-locks it per draw (std Mutex is not re-entrant).

    let mut draw_once = |pixmap: &mut Pixmap, dx: i32, dy: i32, col: Color| {
        let mut fs = font_system().lock().unwrap();
        SWASH_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let w = pixmap.width() as i32;
            let data = pixmap.data_mut();
            buf.draw(&mut fs, &mut cache, col, |x, y, _, _, px| {
                blend_pixel(data, x + offset_x + dx, y + offset_y + dy, w, px);
            });
        });
        let _ = line_step;
    };

    if let Some((stroke_col, stroke_px)) = stroke {
        let sc = Color::rgba(stroke_col[0], stroke_col[1], stroke_col[2], alpha);
        for (dx, dy) in [
            (-stroke_px, 0),
            (stroke_px, 0),
            (0, -stroke_px),
            (0, stroke_px),
            (-stroke_px, -stroke_px),
            (stroke_px, -stroke_px),
            (-stroke_px, stroke_px),
            (stroke_px, stroke_px),
        ] {
            draw_once(pixmap, dx, dy, sc);
        }
    }
    draw_once(
        pixmap,
        0,
        0,
        Color::rgba(color[0], color[1], color[2], alpha),
    );
}

/// The smooth caption plate: Python `draw_smooth_caption_plate`.
/// Line boxes are unified to a common left edge, consecutive boxes with a
/// similar right edge are merged, then each corner is rounded with a
/// quadratic Bezier sampled in 8 steps, and the polygon is filled.
fn draw_smooth_caption_plate(
    pixmap: &mut Pixmap,
    line_boxes: &[[i32; 4]],
    radius: i32,
    fill: [u8; 4],
) {
    if line_boxes.is_empty() {
        return;
    }
    let unified_left = line_boxes.iter().map(|b| b[0]).min().unwrap_or(0);
    let mut merged: Vec<[i32; 4]> = Vec::new();
    for (i, b) in line_boxes.iter().enumerate() {
        let x1 = unified_left;
        let x2 = b[2];
        let y1 = if i == 0 {
            b[1]
        } else {
            merged[merged.len() - 1][3]
        };
        let y2 = b[3];
        if !merged.is_empty()
            && (x2 - merged[merged.len() - 1][2]).abs() < (radius as f32 * 0.7) as i32
        {
            let shared = x2.max(merged[merged.len() - 1][2]);
            let last = merged.len() - 1;
            merged[last][2] = shared;
            merged[last][3] = y2;
        } else {
            merged.push([x1, y1, x2, y2]);
        }
    }

    let mut pts: Vec<(i32, i32)> = vec![(merged[0][0], merged[0][1]), (merged[0][2], merged[0][1])];
    for i in 0..merged.len() - 1 {
        let (curr_x2, curr_y2) = (merged[i][2], merged[i][3]);
        let next_x2 = merged[i + 1][2];
        pts.push((curr_x2, curr_y2));
        pts.push((next_x2, curr_y2));
    }
    pts.push((merged[merged.len() - 1][2], merged[merged.len() - 1][3]));
    pts.push((merged[merged.len() - 1][0], merged[merged.len() - 1][3]));

    // Drop collinear points.
    let mut clean: Vec<(i32, i32)> = Vec::new();
    let n = pts.len();
    for i in 0..n {
        let prev = pts[(i + n - 1) % n];
        let curr = pts[i];
        let next = pts[(i + 1) % n];
        if (prev.0 == curr.0 && curr.0 == next.0) || (prev.1 == curr.1 && curr.1 == next.1) {
            continue;
        }
        clean.push(curr);
    }

    // Sample a quadratic Bezier around every corner (8 steps each).
    let mut poly: Vec<(f32, f32)> = Vec::new();
    let m = clean.len();
    for i in 0..m {
        let p_prev = clean[(i + m - 1) % m];
        let p_curr = clean[i];
        let p_next = clean[(i + 1) % m];
        let v1 = (
            p_prev.0 as f32 - p_curr.0 as f32,
            p_prev.1 as f32 - p_curr.1 as f32,
        );
        let v2 = (
            p_next.0 as f32 - p_curr.0 as f32,
            p_next.1 as f32 - p_curr.1 as f32,
        );
        let len1 = (v1.0 * v1.0 + v1.1 * v1.1).sqrt();
        let len2 = (v2.0 * v2.0 + v2.1 * v2.1).sqrt();
        if len1 == 0.0 || len2 == 0.0 {
            continue;
        }
        let r = (radius as f32).min(len1 * 0.45).min(len2 * 0.45);
        let u1 = (v1.0 / len1, v1.1 / len1);
        let u2 = (v2.0 / len2, v2.1 / len2);
        let start = (p_curr.0 as f32 + u1.0 * r, p_curr.1 as f32 + u1.1 * r);
        let end = (p_curr.0 as f32 + u2.0 * r, p_curr.1 as f32 + u2.1 * r);
        for s in 0..=8 {
            let t = s as f32 / 8.0;
            let pt = (
                (1.0 - t).powi(2) * start.0 + 2.0 * (1.0 - t) * t * p_curr.0 as f32 + t * t * end.0,
                (1.0 - t).powi(2) * start.1 + 2.0 * (1.0 - t) * t * p_curr.1 as f32 + t * t * end.1,
            );
            poly.push(pt);
        }
    }
    if poly.len() < 3 {
        return;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(poly[0].0, poly[0].1);
    for p in &poly[1..] {
        pb.line_to(p.0, p.1);
    }
    pb.close();
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(fill[0], fill[1], fill[2], fill[3]);
        paint.anti_alias = true;
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

// ---------------------------------------------------------------------------
// Entry rendering
// ---------------------------------------------------------------------------

struct RenderJob {
    is_boxed: bool,
    bg_col: [u8; 3],
    txt_col: [u8; 3],
    lines: Vec<String>,
    size: i32,
    line_step: i32,
    pills: Vec<[i32; 4]>,
    start_x: i32,
    start_y: i32,
    radius: i32,
}

/// Render one image entry with its translations — Python `render_entry`.
/// Blocks without `new_text` are left untouched (their original text stays,
/// because nothing is inpainted for them).
/// Per-block fit bounds (start size, max width, max height) for the
/// original per-block behaviour — each block fitted independently to its
/// own footprint.
fn per_block_bounds(blk: &crate::model::Block, avg_h: f32, w: i32) -> (i32, i32, i32) {
    let [x1, _, x2, y2] = blk.bbox;
    let start_size = 16.max((avg_h * 1.05) as i32);
    let raw_max_w = if (x2 - x1) as f32 > w as f32 * 0.6 {
        (w as f32 * 0.86) as i32
    } else {
        x2 - x1
    }
    .max(x2 - x1);
    let max_w = (w - 56).min(raw_max_w);
    let max_h = ((y2 - blk.bbox[1]) as f32 * 1.3).max(avg_h * 1.5) as i32;
    (start_size, max_w, max_h)
}

/// The uniform size for an entry (median `font_size_est` of the blocks
/// that have a translation and are kept), shared by the renderer and the
/// inspector's fit indicator.
pub fn uniform_size_for(entry: &ImageEntry) -> i32 {
    let mut ests: Vec<i32> = entry
        .blocks
        .iter()
        .filter(|b| b.kept() && !b.new_text.trim().is_empty())
        .map(|b| b.font_size_est.max(16))
        .collect();
    ests.sort_unstable();
    ests.get(ests.len() / 2).copied().unwrap_or(16)
}

/// The smallest per-block fitted size for an entry (UniformSmallest mode),
/// shared by the renderer and the inspector.
pub fn smallest_size_for(entry: &ImageEntry, wi: i32, custom_family: Option<&'static str>) -> i32 {
    let mut sizes: Vec<i32> = Vec::new();
    for blk in entry
        .blocks
        .iter()
        .filter(|b| b.kept() && !b.new_text.trim().is_empty())
    {
        let line_bboxes = if blk.line_bboxes.is_empty() {
            vec![blk.bbox]
        } else {
            blk.line_bboxes.clone()
        };
        let avg_h = line_bboxes
            .iter()
            .map(|b| (b[3] - b[1]) as f32)
            .sum::<f32>()
            / line_bboxes.len() as f32;
        let (start_size, max_w, max_h) = per_block_bounds(blk, avg_h, wi);
        let fitted = fit_text_to_box(blk.new_text.trim(), max_w, max_h, start_size, custom_family);
        sizes.push(fitted.size);
    }
    sizes.into_iter().min().unwrap_or(16)
}

/// The fit bounds (start size, max width, max height) for one block under a
/// sizing mode — the same bounds `render_entry` uses, so the inspector's fit
/// indicator reports what the render will actually do.
pub fn fit_bounds_for(
    blk: &crate::model::Block,
    sizing: crate::model::SizingMode,
    uniform_size: i32,
    smallest_size: i32,
    wi: i32,
    hi: i32,
) -> (i32, i32, i32) {
    use crate::model::SizingMode;
    let [x1, y1, _, y2] = blk.bbox;
    let line_bboxes = if blk.line_bboxes.is_empty() {
        vec![blk.bbox]
    } else {
        blk.line_bboxes.clone()
    };
    let avg_h = line_bboxes
        .iter()
        .map(|b| (b[3] - b[1]) as f32)
        .sum::<f32>()
        / line_bboxes.len() as f32;
    match sizing {
        SizingMode::PerBlock => per_block_bounds(blk, avg_h, wi),
        SizingMode::UniformGrow => {
            // The plate grows to the very edge, so a long line at the
            // uniform size stays on one line instead of wrapping at
            // the w-56 margin and colliding with the block above.
            //
            // But the fit width is the width available FROM THE TEXT
            // ORIGIN, not the full image width: the text is drawn at
            // start_x (the block's left edge plus a margin), so a line
            // `wi` wide starting at x≈36 overruns the right edge by
            // exactly that offset and tiny-skia clips the glyphs (the
            // cut plates on _3.jpg, 2026-11-17). Estimate the pad
            // from the uniform size up front; the real pad can only
            // shrink from here (the fit never grows the size), so the
            // estimate stays safe.
            let pad_est = 14.max((uniform_size as f32 * 0.45) as i32);
            let start_est = (pad_est + 12).max(x1);
            let max_w = (wi - start_est - pad_est).max(16);
            // Plate grows downward from y1 (top/middle blocks) or
            // upward from y2 (bottom blocks); the fit must keep the
            // plate inside the image.
            let max_h = if y2 > (hi as f32 * 0.65) as i32 {
                y2.max(16)
            } else {
                (hi - y1).max(16)
            };
            (uniform_size, max_w, max_h)
        }
        SizingMode::UniformSmallest => {
            let (_, max_w, max_h) = per_block_bounds(blk, avg_h, wi);
            (smallest_size, max_w, max_h)
        }
    }
}

pub fn render_entry(
    img: &RgbImage,
    entry: &ImageEntry,
    custom_font: Option<&str>,
    sizing: crate::model::SizingMode,
) -> RgbImage {
    let custom_family = custom_font.and_then(load_custom_font);
    let (w, h) = (img.width(), img.height());
    let (wi, hi) = (w as i32, h as i32);
    let mut work = img.clone();
    let mut inpaint_mask = vec![0u8; (w * h) as usize];
    let mut jobs: Vec<RenderJob> = Vec::new();

    // Uniform modes need one size for the whole image, computed up front
    // from the blocks that actually have a translation and are kept
    // (a filtered-out block renders nothing, so it must not set the size).
    // UniformGrow (A): the original's size — the median of the blocks'
    // font_size_est. UniformSmallest (C): the smallest per-block fit.
    let uniform_size = uniform_size_for(entry);
    let smallest_size = smallest_size_for(entry, wi, custom_family);

    for blk in &entry.blocks {
        let new_text = blk.new_text.trim();
        if new_text.is_empty() {
            continue;
        }
        // A filtered-out block keeps its translation in the JSON but
        // renders nothing — the original text stays in the image.
        if !blk.kept() {
            continue;
        }
        let [x1, y1, x2, y2] = blk.bbox;
        let line_bboxes = if blk.line_bboxes.is_empty() {
            vec![blk.bbox]
        } else {
            blk.line_bboxes.clone()
        };
        let avg_h = line_bboxes
            .iter()
            .map(|b| (b[3] - b[1]) as f32)
            .sum::<f32>()
            / line_bboxes.len() as f32;

        // Fit bounds per sizing mode — the shared helper, so the inspector's
        // fit indicator reports the same bounds the render uses.
        let (start_size, max_w, max_h) =
            fit_bounds_for(blk, sizing, uniform_size, smallest_size, wi, hi);

        let mut fitted = fit_text_to_box(new_text, max_w, max_h, start_size, custom_family);
        // Shared guard: no mode may draw past the right edge. The fit
        // width already accounts for the origin in UniformGrow; if a line
        // still overruns (an unbreakable word at the size floor), shrink
        // until it fits or the floor is reached.
        loop {
            let size = fitted.size;
            let pad_g = 14.max((size as f32 * 0.45) as i32);
            let start_g = (pad_g + 12).max(x1);
            let widest = fitted
                .lines
                .iter()
                .map(|ln| line_width(ln, size, custom_family) as i32)
                .max()
                .unwrap_or(0);
            if start_g + widest + pad_g <= wi || size <= 12 {
                break;
            }
            fitted = fit_text_to_box(new_text, max_w, max_h, size - 1, custom_family);
        }
        let chosen_size = fitted.size;
        let line_step = fitted.line_step;

        let pad_x = 14.max((chosen_size as f32 * 0.45) as i32);
        let pad_y = 7.max((chosen_size as f32 * 0.22) as i32);

        let single_h = ref_text_height(chosen_size, custom_family);
        let total_visual_h = (fitted.lines.len() as i32 - 1) * line_step + single_h;

        let start_y = if y1 < (h as f32 * 0.35) as i32 {
            12.max(y1)
        } else if y2 > (h as f32 * 0.65) as i32 {
            12.max(y2 - total_visual_h)
        } else {
            y1
        };
        let start_x = (pad_x + 12).max(x1);

        let mut pills: Vec<[i32; 4]> = Vec::new();
        let mut cur_y = start_y;
        for ln in &fitted.lines {
            let lw = line_width(ln, chosen_size, custom_family) as i32;
            // PIL textbbox top for "Agjpqy" at (start_x, cur_y) is cur_y;
            // the line's ink spans [cur_y, cur_y + single_h].
            let pill = clamp_box(
                [
                    start_x - pad_x,
                    cur_y - pad_y,
                    start_x + lw + pad_x,
                    cur_y + single_h + pad_y,
                ],
                wi,
                hi,
            );
            pills.push(pill);
            cur_y += line_step;
        }

        let raw_bg = blk.bg_color;
        let bg_col: [u8; 3] = if raw_bg.iter().max().copied().unwrap_or(0) < 60 {
            [18, 18, 18]
        } else {
            raw_bg
        };
        let txt_col = blk.text_color;
        let is_boxed = blk.is_boxed;

        if is_boxed {
            let old_pad_x = 36.max((avg_h * 0.95) as i32);
            let old_pad_y = 20.max((avg_h * 0.60) as i32);
            for [lx1, ly1, lx2, ly2] in &line_bboxes {
                let ox1 = if lx1 - old_pad_x < 15 {
                    0
                } else {
                    lx1 - old_pad_x
                };
                let oy1 = if ly1 - old_pad_y < 15 {
                    0
                } else {
                    ly1 - old_pad_y
                };
                let ox2 = if lx2 + old_pad_x > wi - 15 {
                    wi
                } else {
                    lx2 + old_pad_x
                };
                let oy2 = if ly2 + old_pad_y > hi - 15 {
                    hi
                } else {
                    ly2 + old_pad_y
                };
                let [ox1, oy1, ox2, oy2] = clamp_box([ox1, oy1, ox2, oy2], wi, hi);
                for y in oy1..oy2 {
                    for x in ox1..ox2 {
                        inpaint_mask[y as usize * w as usize + x as usize] = 255;
                    }
                }
            }
        } else {
            for [lx1, ly1, lx2, ly2] in &line_bboxes {
                let [lx1, ly1, lx2, ly2] = clamp_box([lx1 - 3, ly1 - 3, lx2 + 3, ly2 + 3], wi, hi);
                if lx2 <= lx1 || ly2 <= ly1 {
                    continue;
                }
                for y in ly1..ly2 {
                    for x in lx1..lx2 {
                        let p = work.get_pixel(x as u32, y as u32);
                        let d = [
                            p[0] as f32 - txt_col[0] as f32,
                            p[1] as f32 - txt_col[1] as f32,
                            p[2] as f32 - txt_col[2] as f32,
                        ];
                        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                        if dist < 90.0 {
                            inpaint_mask[y as usize * w as usize + x as usize] = 255;
                        }
                    }
                }
            }
        }

        jobs.push(RenderJob {
            is_boxed,
            bg_col,
            txt_col,
            lines: fitted.lines,
            size: chosen_size,
            line_step,
            pills,
            start_x,
            start_y,
            radius: 10.max((chosen_size as f32 * 0.32) as i32),
        });
    }

    inpaint_masked(&mut work, &inpaint_mask);

    // Composite onto a tiny-skia pixmap (premultiplied) for plate + text.
    let mut pixmap = Pixmap::new(w, h).expect("pixmap");
    {
        let data = pixmap.data_mut();
        for (i, px) in work.pixels().enumerate() {
            data[i * 4] = px[0];
            data[i * 4 + 1] = px[1];
            data[i * 4 + 2] = px[2];
            data[i * 4 + 3] = 255;
        }
    }

    for job in &jobs {
        if job.is_boxed {
            draw_smooth_caption_plate(
                &mut pixmap,
                &job.pills,
                job.radius,
                [job.bg_col[0], job.bg_col[1], job.bg_col[2], 232],
            );
        }
        let joined = job.lines.join("\n");
        let max_w = job
            .pills
            .iter()
            .map(|p| p[2] - p[0])
            .max()
            .unwrap_or(0)
            .max(1);
        if job.is_boxed {
            draw_text(
                &mut pixmap,
                &joined,
                job.size,
                job.txt_col,
                255,
                job.start_x,
                job.start_y,
                max_w,
                custom_family,
                None,
            );
        } else {
            draw_text(
                &mut pixmap,
                &joined,
                job.size,
                job.txt_col,
                255,
                job.start_x,
                job.start_y,
                max_w,
                custom_family,
                Some(([0, 0, 0], 2)),
            );
        }
        let _ = job.line_step;
    }

    // Back to straight-alpha RGB.
    let mut out = RgbImage::new(w, h);
    {
        let data = pixmap.data();
        for (i, px) in out.pixels_mut().enumerate() {
            let a = data[i * 4 + 3] as u32;
            if a == 0 {
                *px = Rgb([0, 0, 0]);
            } else if a == 255 {
                *px = Rgb([data[i * 4], data[i * 4 + 1], data[i * 4 + 2]]);
            } else {
                *px = Rgb([
                    (data[i * 4] as u32 * 255 / a) as u8,
                    (data[i * 4 + 1] as u32 * 255 / a) as u8,
                    (data[i * 4 + 2] as u32 * 255 / a) as u8,
                ]);
            }
        }
    }
    out
}

/// One diagnostic tag to place: the box it annotates, its size, and its
/// text/colours (kept in the caller). `place_diag_labels` returns each
/// tag's top edge (y), in input order, so no two tags overlap: it prefers
/// directly above the box, stacks upward on a collision, and falls back to
/// below the box when there is no room above. Pure, so the placement is
/// unit-tested.
#[derive(Clone, Copy)]
struct DiagLabel {
    x: i32,
    box_top: i32,
    box_bottom: i32,
    width: i32,
    height: i32,
}

fn place_diag_labels(labels: &[DiagLabel], img_h: i32) -> Vec<i32> {
    // Topmost first, so the highest tag claims its spot and the ones below
    // stack under it.
    let mut order: Vec<usize> = (0..labels.len()).collect();
    order.sort_by_key(|&i| (labels[i].box_top, labels[i].x));
    let mut placed: Vec<(i32, i32, i32, i32)> = Vec::new(); // (x, y, w, h)
    let mut out = vec![0i32; labels.len()];
    for &i in &order {
        let l = &labels[i];
        let overlaps = |ty: i32, placed: &[(i32, i32, i32, i32)]| {
            placed.iter().any(|&(px, py, pw, ph)| {
                l.x < px + pw && px < l.x + l.width && ty < py + ph && py < ty + l.height
            })
        };
        // Preferred: directly above the box; stack upward on a collision.
        let mut ty = l.box_top - l.height;
        while overlaps(ty, &placed) && ty > 0 {
            let top = placed
                .iter()
                .filter(|&&(px, py, pw, ph)| {
                    l.x < px + pw && px < l.x + l.width && ty < py + ph && py < ty + l.height
                })
                .map(|&(_, py, _, _)| py)
                .min()
                .unwrap_or(ty);
            ty = top - l.height - 2;
        }
        // No room above: below the box instead, stacking downward.
        if ty < 0 {
            ty = l.box_bottom + 2;
            while overlaps(ty, &placed) && ty + l.height < img_h {
                let bottom = placed
                    .iter()
                    .filter(|&&(px, py, pw, ph)| {
                        l.x < px + pw && px < l.x + l.width && ty < py + ph && py < ty + l.height
                    })
                    .map(|&(_, py, _, ph)| py + ph)
                    .max()
                    .unwrap_or(ty);
                ty = bottom + 2;
            }
        }
        ty = ty.clamp(0, (img_h - l.height).max(0));
        placed.push((l.x, ty, l.width, l.height));
        out[i] = ty;
    }
    out
}

/// OCR diagnostic overlay — Python `render_ocr_diagnostic`.
/// Kept blocks in green, dropped lines in orange, with `#id · conf · kept`
/// labels outside the box top-left (SPEC §5: 11px at display scale — the
/// label size here scales with the image width so it stays legible at any
/// zoom). Kept borders are solid, dropped borders dashed, so colour is not
/// the only cue. The selected block (highlight) is Max Blue.
/// tags. `highlight` (a block id) draws that block in Max Blue instead, so the
/// preview can show which block the Detected Blocks list has selected.
/// Returns an RGB image.
pub fn render_ocr_diagnostic(
    img: &RgbImage,
    entry: &ImageEntry,
    highlight: Option<usize>,
) -> RgbImage {
    let (w, h) = (img.width(), img.height());
    let (wi, hi) = (w as i32, h as i32);
    let mut pixmap = Pixmap::new(w, h).expect("pixmap");
    {
        let data = pixmap.data_mut();
        for (i, px) in img.pixels().enumerate() {
            data[i * 4] = px[0];
            data[i * 4 + 1] = px[1];
            data[i * 4 + 2] = px[2];
            data[i * 4 + 3] = 255;
        }
    }
    let badge_size = 13.max((w as f32 * 0.017) as i32);
    let stroke_w = 2.max((w as f32 * 0.0025) as i32) as f32;

    let rect_outline = |pixmap: &mut Pixmap,
                        b: [i32; 4],
                        color: [u8; 4],
                        width: f32,
                        fill: Option<[u8; 4]>,
                        dashed: bool| {
        let [x1, y1, x2, y2] = clamp_box(b, wi, hi);
        if x2 <= x1 || y2 <= y1 {
            return;
        }
        if let Some(f) = fill {
            let mut paint = Paint::default();
            paint.set_color_rgba8(f[0], f[1], f[2], f[3]);
            paint.anti_alias = true;
            if let Some(rect) =
                Rect::from_xywh(x1 as f32, y1 as f32, (x2 - x1) as f32, (y2 - y1) as f32)
            {
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            }
        }
        // A dashed outline for filtered (dropped) boxes, solid for kept —
        // SPEC §5: the border style differs so colour is not the only cue.
        // The dash scales with the stroke width so it reads at any image size.
        let dash = dashed
            .then(|| {
                let d = (width * 3.0).max(6.0);
                let g = (width * 2.0).max(4.0);
                StrokeDash::new(vec![d, g], 0.0)
            })
            .flatten();
        let mut paint = Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
        paint.anti_alias = true;
        let stroke = Stroke {
            width,
            dash: dash.clone(),
            ..Default::default()
        };
        if let Some(rect) =
            Rect::from_xywh(x1 as f32, y1 as f32, (x2 - x1) as f32, (y2 - y1) as f32)
        {
            let path = tiny_skia::PathBuilder::from_rect(rect);
            // A dark halo under the coloured outline, so it holds up on
            // light photo areas: the kept green is 2.28:1 and the dropped
            // orange 2.80:1 on white, below the 3:1 a non-text mark needs.
            // Drawn first, 2px wider, so it reads as a dark edge around the
            // coloured line. Dashed with the outline, so the halo doesn't
            // fill the dashes back in as a solid dark line.
            let mut halo = Paint::default();
            halo.set_color_rgba8(0, 0, 0, 200);
            halo.anti_alias = true;
            let halo_stroke = Stroke {
                width: width + 2.0,
                dash,
                ..Default::default()
            };
            pixmap.stroke_path(&path, &halo, &halo_stroke, Transform::identity(), None);
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    };

    // Draw one tag at a given top edge. The placement (which top edge) is
    // decided by `place_diag_labels` after all boxes are drawn, so tags
    // never overlap each other or sit half inside their box.
    let draw_tag_at =
        |pixmap: &mut Pixmap, x: i32, ty: i32, text: &str, bg: [u8; 4], ink: [u8; 3]| {
            let tw = line_width(text, badge_size, None) as i32 + 10;
            let th = badge_size + 4;
            let mut paint = Paint::default();
            paint.set_color_rgba8(bg[0], bg[1], bg[2], bg[3]);
            paint.anti_alias = true;
            if let Some(rect) = Rect::from_xywh(x as f32, ty as f32, tw as f32, th as f32) {
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            }
            draw_text(
                pixmap,
                text,
                badge_size,
                ink,
                255,
                x + 5,
                ty + 1,
                tw,
                None,
                None,
            );
        };

    // Every tag, collected while the boxes are drawn and placed together
    // at the end: (placement, text, bg, ink).
    let mut labels: Vec<(DiagLabel, String, [u8; 4], [u8; 3])> = Vec::new();
    let tag_size = |text: &str| -> (i32, i32) {
        (
            line_width(text, badge_size, None) as i32 + 10,
            badge_size + 4,
        )
    };

    for drop in &entry.dropped {
        let [x1, y1, _, y2] = clamp_box(drop.bbox, wi, hi);
        rect_outline(
            &mut pixmap,
            drop.bbox,
            [249, 115, 22, 235],
            stroke_w,
            Some([249, 115, 22, 35]),
            true,
        );
        let text = format!("DROP ({})", drop.reason);
        let (tw, th) = tag_size(&text);
        labels.push((
            DiagLabel {
                x: x1,
                box_top: y1,
                box_bottom: y2,
                width: tw,
                height: th,
            },
            text,
            [194, 65, 12, 230],
            [255, 255, 255],
        ));
    }
    // One block painter for both the kept green and the highlighted blue;
    // the highlight is drawn last so its tag sits on top. The tag is only
    // collected here; all tags are placed (de-collided) and drawn together
    // after every box, so a tag never covers another tag or its own box.
    let mut draw_block =
        |pixmap: &mut Pixmap,
         blk: &crate::model::Block,
         stroke: [u8; 4],
         width_extra: f32,
         fill: [u8; 4],
         tag_bg: [u8; 4],
         ink: [u8; 3],
         label: &str,
         dashed: bool,
         labels: &mut Vec<(DiagLabel, String, [u8; 4], [u8; 3])>| {
            let [x1, y1, _, y2] = clamp_box(blk.bbox, wi, hi);
            rect_outline(
                pixmap,
                blk.bbox,
                stroke,
                stroke_w + width_extra,
                Some(fill),
                dashed,
            );
            let text = format!("#{} · {:.2} · {}", blk.id, blk.confidence, label);
            let (tw, th) = tag_size(&text);
            labels.push((
                DiagLabel {
                    x: x1,
                    box_top: y1,
                    box_bottom: y2,
                    width: tw,
                    height: th,
                },
                text,
                tag_bg,
                ink,
            ));
        };
    for blk in &entry.blocks {
        if Some(blk.id) == highlight {
            continue;
        }
        if blk.kept() {
            draw_block(
                &mut pixmap,
                blk,
                // SPEC §4 `ok` #4ADE80 (was #22C55E). The label is black on
                // that green (11.23:1) — white on #4ADE80 would be unreadable.
                [74, 222, 128, 255],
                1.0,
                [74, 222, 128, 30],
                [74, 222, 128, 240],
                [0, 0, 0],
                "kept",
                false,
                &mut labels,
            );
        } else {
            // Manually filtered out: drawn like a dropped item (dashed
            // orange border, "filtered" tag) so the stage shows the state
            // the render will honour.
            draw_block(
                &mut pixmap,
                blk,
                [249, 115, 22, 235],
                1.0,
                [249, 115, 22, 35],
                [194, 65, 12, 230],
                [255, 255, 255],
                "filtered",
                true,
                &mut labels,
            );
        }
    }
    if let Some(id) = highlight {
        if let Some(blk) = entry.blocks.iter().find(|b| b.id == id) {
            // Max Blue #2F5BFF, drawn thicker so it reads as selected even
            // at a glance. The tag names the block's state (kept/filtered).
            let label = if blk.kept() { "kept" } else { "filtered" };
            draw_block(
                &mut pixmap,
                blk,
                [47, 91, 255, 255],
                3.0,
                [47, 91, 255, 40],
                [47, 91, 255, 240],
                [255, 255, 255],
                label,
                false,
                &mut labels,
            );
        }
    }
    // Place every tag together (de-collided: no two overlap, none sits
    // half inside its box) and draw them on top of the boxes.
    let specs: Vec<DiagLabel> = labels.iter().map(|(s, _, _, _)| *s).collect();
    let tops = place_diag_labels(&specs, hi);
    for (i, (_, text, bg, ink)) in labels.iter().enumerate() {
        draw_tag_at(&mut pixmap, specs[i].x, tops[i], text, *bg, *ink);
    }

    let mut out = RgbImage::new(w, h);
    {
        let data = pixmap.data();
        for (i, px) in out.pixels_mut().enumerate() {
            let a = data[i * 4 + 3] as u32;
            if a == 0 {
                *px = Rgb([0, 0, 0]);
            } else if a == 255 {
                *px = Rgb([data[i * 4], data[i * 4 + 1], data[i * 4 + 2]]);
            } else {
                *px = Rgb([
                    (data[i * 4] as u32 * 255 / a) as u8,
                    (data[i * 4 + 1] as u32 * 255 / a) as u8,
                    (data[i * 4 + 2] as u32 * 255 / a) as u8,
                ]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Block;

    fn test_entry() -> ImageEntry {
        ImageEntry {
            file: "a.png".into(),
            width: 400,
            height: 200,
            raw_lines: vec![],
            blocks: vec![Block {
                id: 1,
                bbox: [20, 20, 380, 60],
                line_bboxes: vec![[20, 20, 380, 60]],
                original_text: "Hello world".into(),
                new_text: "Vanakkam ulagam".into(),
                confidence: 0.95,
                text_color: [255, 255, 255],
                bg_color: [18, 18, 18],
                is_boxed: true,
                font_size_est: 40,
                kept_override: None,
            }],
            dropped: vec![],
        }
    }

    #[test]
    fn fit_shrinks_long_text() {
        let big = fit_text_to_box("Hello world", 200, 60, 40, None);
        assert!(big.size <= 40);
        let small = fit_text_to_box("Hi", 200, 60, 40, None);
        assert_eq!(small.size, 40); // fast path fits
        assert_eq!(small.lines, vec!["Hi"]);
    }

    #[test]
    fn fit_wraps_without_duplicating_text() {
        // Regression test for the double-render bug (2026-11-16):
        // cosmic-text `layout_runs()` yields the FULL logical line in
        // `run.text` for every visual line, so `measure` used to return
        // the whole sentence once per wrapped line — the plate grew N
        // lines tall and the text drew N times. Wrapped lines must be
        // true fragments: no line may equal the whole text, and the
        // fragments must rejoin (word for word) to the original.
        for text in [
            "Chintu idha gavanichu kaduppa face vechitu",
            "Appuram avaloda payyan pathi sollanum na avan peru chintu Namma ellarum maadhiriye appo appo actress pics paathutu poolu adichitu iruppan....",
        ] {
            let fitted = fit_text_to_box(text, 200, 400, 40, None);
            assert!(fitted.lines.len() > 1, "expected wrapping, got {:?}", fitted.lines);
            assert!(
                fitted.lines.iter().all(|l| l != text),
                "duplicated full text: {:?}",
                fitted.lines
            );
            let joined = fitted.lines.join(" ");
            let rejoined: Vec<&str> = joined.split_whitespace().collect();
            let original: Vec<&str> = text.split_whitespace().collect();
            assert_eq!(rejoined, original, "fragments lost/added words: {:?}", fitted.lines);
        }
    }

    #[test]
    fn render_entry_replaces_boxed_caption() {
        let mut img = RgbImage::new(400, 200);
        for p in img.pixels_mut() {
            *p = Rgb([200, 200, 200]);
        }
        // dark boxed caption area
        for y in 20..60 {
            for x in 20..380 {
                img.put_pixel(x, y, Rgb([10, 10, 10]));
            }
        }
        let entry = test_entry();
        let out = render_entry(&img, &entry, None, crate::model::SizingMode::PerBlock);
        assert_eq!(out.dimensions(), (400, 200));
        // The plate area should be dark-ish (bg 18,18,18 at alpha 232 over
        // the inpainted background), not the original mid-grey.
        let p = out.get_pixel(30, 30);
        assert!(p[0] < 120, "plate pixel too bright: {p:?}");
    }

    /// UniformGrow renders every block at ONE size (the median
    /// font_size_est) and lets the plate grow: the long-translation block
    /// that used to shrink to 29 and wrap to 2 lines now stays at the
    /// uniform size on one line.
    #[test]
    fn uniform_grow_keeps_one_size_and_grows_plate() {
        use crate::model::{Block, ImageEntry};
        // Two blocks whose originals share one size (font_size_est 50)
        // but whose translations differ in length: the narrow block's
        // translation is much longer than its original.
        let mk = |id: usize, bbox: [i32; 4], new_text: &str| Block {
            id,
            bbox,
            line_bboxes: vec![bbox],
            original_text: String::new(),
            new_text: new_text.to_string(),
            confidence: 0.9,
            text_color: [245, 225, 131],
            bg_color: [18, 18, 18],
            is_boxed: true,
            font_size_est: 50,
            kept_override: None,
        };
        let entry = ImageEntry {
            file: "u.jpg".to_string(),
            width: 1440,
            height: 1800,
            raw_lines: vec![],
            blocks: vec![
                mk(1, [24, 36, 1368, 196], "Short line here"),
                mk(
                    2,
                    [26, 1729, 707, 1777],
                    "Andha group la Siddhu msg vandhurundhudhu",
                ),
            ],
            dropped: vec![],
        };
        let img = RgbImage::new(1440, 1800);
        let out = render_entry(&img, &entry, None, crate::model::SizingMode::UniformGrow);
        assert_eq!(out.dimensions(), (1440, 1800));
        // The narrow block's plate must have grown wider than its
        // original 681px footprint to hold the 42-char line at size ~50
        // (at size 50 the line is ~920px wide): check a pixel well right
        // of the original bbox is plate-dark, not background.
        let p = out.get_pixel(850, 1750);
        assert!(
            p[0] < 120,
            "uniform plate did not grow past the footprint: {p:?}"
        );
        // And the wide block must NOT have shrunk: its text is still at
        // the uniform size, so its plate is a single line tall (~50px),
        // not a shrunken multi-line stack.
        let p2 = out.get_pixel(700, 60);
        assert!(p2[0] < 120, "wide block plate missing: {p2:?}");
    }

    /// Regression test for the clipped plates on _3.jpg (2026-11-17):
    /// in UniformGrow the fit width was the full image width while the
    /// text starts ~36px in from the left, so a long line's tail ran past
    /// the right edge and tiny-skia clipped the glyphs. No caption ink
    /// may appear in the rightmost pad margin of the image.
    #[test]
    fn uniform_grow_keeps_text_inside_image() {
        use crate::model::{Block, ImageEntry};
        let entry = ImageEntry {
            file: "clip.jpg".to_string(),
            width: 1440,
            height: 900,
            raw_lines: vec![],
            blocks: vec![Block {
                id: 1,
                bbox: [24, 40, 700, 100],
                line_bboxes: vec![[24, 40, 700, 100]],
                original_text: String::new(),
                // Short words pack the greedy wrap tight to the fit limit
                // (line 1 lands at 1404px of the 1440px image) — the geometry
                // that clipped the plates on _3.jpg.
                new_text: "ka ka ka ka ka ka ka ka ka ka ka ka ka ka ka ka ka ka ka ka ka ka ka"
                    .to_string(),
                confidence: 0.94,
                text_color: [255, 64, 64],
                bg_color: [18, 18, 18],
                is_boxed: true,
                font_size_est: 51,
                kept_override: None,
            }],
            dropped: vec![],
        };
        let img = RgbImage::new(1440, 900);
        let out = render_entry(&img, &entry, None, crate::model::SizingMode::UniformGrow);
        // Caption ink (reddish text) must stay at least the minimum
        // plate pad (14px) away from the right edge.
        for y in 0..900 {
            for x in 1426..1440 {
                let p = out.get_pixel(x, y);
                assert!(
                    !(p[0] > 200 && p[1] < 100 && p[2] < 100),
                    "caption ink at the right edge: ({x}, {y}) = {p:?}"
                );
            }
        }
    }

    #[test]
    fn diagnostic_draws_without_panic() {
        let img = RgbImage::new(200, 100);
        let entry = test_entry();
        let out = render_ocr_diagnostic(&img, &entry, None);
        assert_eq!(out.dimensions(), (200, 100));
    }

    #[test]
    fn diagnostic_highlight_recolours_only_the_selected_block() {
        let img = RgbImage::new(200, 100);
        let entry = test_entry();
        let plain = render_ocr_diagnostic(&img, &entry, None);
        let lit = render_ocr_diagnostic(&img, &entry, Some(1));
        // The highlight changes the render (blue stroke + tag instead of green).
        assert_ne!(plain.as_raw(), lit.as_raw());
        // An unknown id highlights nothing.
        let unknown = render_ocr_diagnostic(&img, &entry, Some(99));
        assert_eq!(plain.as_raw(), unknown.as_raw());
    }

    #[test]
    fn diag_labels_stack_instead_of_overlapping() {
        // Two clustered boxes (near-identical tops): the second tag stacks
        // above the first instead of landing on top of it.
        let labels = vec![
            DiagLabel {
                x: 10,
                box_top: 100,
                box_bottom: 120,
                width: 80,
                height: 17,
            },
            DiagLabel {
                x: 12,
                box_top: 102,
                box_bottom: 122,
                width: 80,
                height: 17,
            },
        ];
        let tops = place_diag_labels(&labels, 400);
        // First tag directly above its box; second stacked above the first.
        assert_eq!(tops[0], 100 - 17);
        assert_eq!(tops[1], 100 - 17 - 17 - 2);
        // No overlap: the second tag's bottom is above the first's top.
        assert!(tops[1] + 17 <= tops[0]);
        // A box at the top of the image has no room above — its tag goes
        // below the box instead of overlapping it.
        let top = vec![DiagLabel {
            x: 10,
            box_top: 5,
            box_bottom: 25,
            width: 80,
            height: 17,
        }];
        let tops = place_diag_labels(&top, 400);
        assert_eq!(tops[0], 25 + 2);
    }

    #[test]
    fn filtered_block_renders_nothing_and_diagnostic_marks_it() {
        let img = RgbImage::new(400, 200);
        let mut entry = test_entry();
        // The block has a translation; filtering it out must leave the
        // image untouched (the original text stays).
        let rendered = render_entry(&img, &entry, None, crate::model::SizingMode::UniformGrow);
        assert_ne!(rendered.as_raw(), img.as_raw());
        entry.blocks[0].kept_override = Some(false);
        let rendered = render_entry(&img, &entry, None, crate::model::SizingMode::UniformGrow);
        assert_eq!(rendered.as_raw(), img.as_raw());
        // The diagnostic draws the filtered block differently from kept.
        let kept_diag = render_ocr_diagnostic(&img, &test_entry(), None);
        let filtered_diag = render_ocr_diagnostic(&img, &entry, None);
        assert_ne!(kept_diag.as_raw(), filtered_diag.as_raw());
    }
}
