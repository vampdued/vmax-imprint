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
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use crate::fonts::{attrs_for, font_system, load_custom_font};
use crate::inpaint::inpaint_masked;
use crate::model::{ImageEntry, clamp_box};

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
        let t = run.text.get(start..end).unwrap_or("").trim_end().to_string();
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
        return FittedText { lines, size, line_step: step };
    }
    if fits(start_size) {
        let (lines, size, step) = eval(start_size);
        return FittedText { lines, size, line_step: step };
    }
    let mut lo = 13i32;
    let mut hi = start_size - 1;
    let mut best: Option<FittedText> = None;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        if fits(mid) {
            let (lines, size, step) = eval(mid);
            best = Some(FittedText { lines, size, line_step: step });
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    if let Some(b) = best {
        return b;
    }
    let (lines, size, step) = eval(12);
    FittedText { lines, size, line_step: step }
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
        let y1 = if i == 0 { b[1] } else { merged[merged.len() - 1][3] };
        let y2 = b[3];
        if !merged.is_empty() && (x2 - merged[merged.len() - 1][2]).abs() < (radius as f32 * 0.7) as i32 {
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
        let v1 = (p_prev.0 as f32 - p_curr.0 as f32, p_prev.1 as f32 - p_curr.1 as f32);
        let v2 = (p_next.0 as f32 - p_curr.0 as f32, p_next.1 as f32 - p_curr.1 as f32);
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
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
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
pub fn render_entry(img: &RgbImage, entry: &ImageEntry, custom_font: Option<&str>) -> RgbImage {
    let custom_family = custom_font.and_then(load_custom_font);
    let (w, h) = (img.width(), img.height());
    let (wi, hi) = (w as i32, h as i32);
    let mut work = img.clone();
    let mut inpaint_mask = vec![0u8; (w * h) as usize];
    let mut jobs: Vec<RenderJob> = Vec::new();

    for blk in &entry.blocks {
        let new_text = blk.new_text.trim();
        if new_text.is_empty() {
            continue;
        }
        let [x1, y1, x2, y2] = blk.bbox;
        let line_bboxes = if blk.line_bboxes.is_empty() {
            vec![blk.bbox]
        } else {
            blk.line_bboxes.clone()
        };
        let avg_h = line_bboxes.iter().map(|b| (b[3] - b[1]) as f32).sum::<f32>()
            / line_bboxes.len() as f32;

        let start_size = 16.max((avg_h * 1.05) as i32);
        let raw_max_w = if (x2 - x1) as f32 > w as f32 * 0.6 {
            (w as f32 * 0.86) as i32
        } else {
            x2 - x1
        }
        .max(x2 - x1);
        let max_w = (w as i32 - 56).min(raw_max_w);
        let max_h = ((y2 - y1) as f32 * 1.3).max(avg_h * 1.5) as i32;

        let fitted = fit_text_to_box(new_text, max_w, max_h, start_size, custom_family);
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
                [start_x - pad_x, cur_y - pad_y, start_x + lw + pad_x, cur_y + single_h + pad_y],
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
                let ox1 = if lx1 - old_pad_x < 15 { 0 } else { lx1 - old_pad_x };
                let oy1 = if ly1 - old_pad_y < 15 { 0 } else { ly1 - old_pad_y };
                let ox2 = if lx2 + old_pad_x > wi - 15 { wi } else { lx2 + old_pad_x };
                let oy2 = if ly2 + old_pad_y > hi - 15 { hi } else { ly2 + old_pad_y };
                let [ox1, oy1, ox2, oy2] = clamp_box([ox1, oy1, ox2, oy2], wi, hi);
                for y in oy1..oy2 {
                    for x in ox1..ox2 {
                        inpaint_mask[y as usize * w as usize + x as usize] = 255;
                    }
                }
            }
        } else {
            for [lx1, ly1, lx2, ly2] in &line_bboxes {
                let [lx1, ly1, lx2, ly2] =
                    clamp_box([lx1 - 3, ly1 - 3, lx2 + 3, ly2 + 3], wi, hi);
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

/// OCR diagnostic overlay — Python `render_ocr_diagnostic`.
/// Kept blocks in green, dropped lines in orange, with `#id · conf · Boxed/Unboxed`
/// tags. Returns an RGB image.
pub fn render_ocr_diagnostic(img: &RgbImage, entry: &ImageEntry) -> RgbImage {
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

    let rect_outline = |pixmap: &mut Pixmap, b: [i32; 4], color: [u8; 4], width: f32, fill: Option<[u8; 4]>| {
        let [x1, y1, x2, y2] = clamp_box(b, wi, hi);
        if x2 <= x1 || y2 <= y1 {
            return;
        }
        if let Some(f) = fill {
            let mut paint = Paint::default();
            paint.set_color_rgba8(f[0], f[1], f[2], f[3]);
            paint.anti_alias = true;
            if let Some(rect) = Rect::from_xywh(x1 as f32, y1 as f32, (x2 - x1) as f32, (y2 - y1) as f32) {
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            }
        }
        let mut paint = Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
        paint.anti_alias = true;
        let stroke = Stroke { width, ..Default::default() };
        if let Some(rect) = Rect::from_xywh(x1 as f32, y1 as f32, (x2 - x1) as f32, (y2 - y1) as f32) {
            let path = tiny_skia::PathBuilder::from_rect(rect);
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    };

    let tag = |pixmap: &mut Pixmap, x: i32, y: i32, text: &str, bg: [u8; 4]| {
        let tw = line_width(text, badge_size, None) as i32 + 10;
        let th = badge_size + 4;
        let ty = 0.max(y - th);
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
            [255, 255, 255],
            255,
            x + 5,
            ty + 1,
            tw,
            None,
            None,
        );
    };

    for drop in &entry.dropped {
        let [x1, y1, _, _] = clamp_box(drop.bbox, wi, hi);
        rect_outline(
            &mut pixmap,
            drop.bbox,
            [249, 115, 22, 235],
            stroke_w,
            Some([249, 115, 22, 35]),
        );
        tag(
            &mut pixmap,
            x1,
            y1,
            &format!("DROP ({})", drop.reason),
            [194, 65, 12, 230],
        );
    }
    for blk in &entry.blocks {
        let [x1, y1, _, _] = clamp_box(blk.bbox, wi, hi);
        rect_outline(
            &mut pixmap,
            blk.bbox,
            [34, 197, 94, 255],
            stroke_w + 1.0,
            Some([34, 197, 94, 30]),
        );
        let kind = if blk.is_boxed { "Boxed" } else { "Unboxed" };
        tag(
            &mut pixmap,
            x1,
            y1,
            &format!("#{} · {:.2} · {}", blk.id, blk.confidence, kind),
            [21, 128, 61, 240],
        );
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
        let out = render_entry(&img, &entry, None);
        assert_eq!(out.dimensions(), (400, 200));
        // The plate area should be dark-ish (bg 18,18,18 at alpha 232 over
        // the inpainted background), not the original mid-grey.
        let p = out.get_pixel(30, 30);
        assert!(p[0] < 120, "plate pixel too bright: {p:?}");
    }

    #[test]
    fn diagnostic_draws_without_panic() {
        let img = RgbImage::new(200, 100);
        let entry = test_entry();
        let out = render_ocr_diagnostic(&img, &entry);
        assert_eq!(out.dimensions(), (200, 100));
    }
}
