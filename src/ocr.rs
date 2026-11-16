//! OCR engine — RapidOCR / PP-OCRv4 pipeline on `ort` (ONNX Runtime 1.31).
//!
//! The detection model (`ch_PP-OCRv4_det_infer.onnx`), recognition model
//! (`ch_PP-OCRv4_rec_infer.onnx`) and character dictionary
//! (`ppocr_keys_v1.txt`) are embedded with `include_bytes!()` / `include_str!()`
//! and the sessions are built from memory, so the exe carries no model
//! files and no DLLs at runtime. Sessions are pre-warmed in a background
//! thread at app launch (`OcrEngine::new`).
//!
//! Pipeline (RapidOCR-equivalent, matching the Python app's behaviour):
//! 1. If the longest side exceeds 1600px, downscale proportionally
//!    (bilinear) for inference; detected coordinates are mapped back to
//!    native resolution.
//! 2. Detection: DBNet — resize so the longest side is <= 960px, pad dims
//!    to multiples of 32, normalise (ImageNet mean/std), run the det model,
//!    threshold the probability map at 0.3, label connected components,
//!    take the minimum-area rectangle of each, score it by the mean
//!    probability inside the box (fast mode), drop boxes scoring < 0.6,
//!    then unclip the polygon (Vatti-style offset, ratio 1.5).
//! 3. Recognition: rectify each box with a perspective warp to height 48,
//!    run the rec model, CTC greedy-decode with the dictionary
//!    (index 0 = blank, character `i` = `dict[i-1]`).
//! 4. All raw detections with `score >= 0.10` are kept in `raw_lines` so
//!    threshold changes re-filter instantly without re-running inference.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, anyhow};
use image::{Rgb, RgbImage};
use ort::session::Session;
use ort::value::{Shape, Tensor};

use crate::layout::sample_colors;
use crate::model::{RawLine, clamp_box};

pub const MAX_OCR_DIM: u32 = 1600;
/// Detection probability-map threshold (RapidOCR `det_db_thresh`).
const DET_THRESH: f32 = 0.3;
/// Detection box score threshold (RapidOCR `det_db_box_thresh`).
const DET_BOX_THRESH: f32 = 0.6;
/// Detection unclip ratio (RapidOCR `det_db_unclip_ratio`).
const DET_UNCLIP_RATIO: f32 = 1.5;
/// Raw detections below this score are discarded entirely (Python: 0.10).
const RAW_MIN_SCORE: f32 = 0.10;

/// Debug logging for the OCR pipeline, enabled with
/// `CAPTION_OCR_DEBUG=1`. Kept behind an env var (not a log crate) so the
/// release exe stays dependency-free.
fn ocr_debug() -> bool {
    std::env::var_os("CAPTION_OCR_DEBUG").is_some()
}
/// Recognition input height (RapidOCR `rec_image_shape`).
const REC_H: usize = 48;
/// Recognition input max width.
const REC_MAX_W: usize = 320;

static DET_MODEL: &[u8] = include_bytes!("../assets/models/ch_PP-OCRv4_det_infer.onnx");
static REC_MODEL: &[u8] = include_bytes!("../assets/models/ch_PP-OCRv4_rec_infer.onnx");
static DICT_TXT: &str = include_str!("../assets/models/ppocr_keys_v1.txt");

/// The single ONNX Runtime environment for the process (ort allows only
/// one). Kept alive forever; sessions are built from it.
static ENV: OnceLock<ort::environment::Environment> = OnceLock::new();

fn environment() -> &'static ort::environment::Environment {
    ENV.get_or_init(|| {
        ort::init()
            .with_name("caption-studio")
            .build()
            .expect("failed to initialise ONNX Runtime")
    })
}

pub struct OcrEngine {
    det: Mutex<Session>,
    rec: Mutex<Session>,
    det_input: String,
    det_output: String,
    rec_input: String,
    rec_output: String,
    dict: Vec<String>,
}

impl OcrEngine {
    /// Build both sessions from the embedded models. This is the pre-warm
    /// step — call it once in a background thread at launch.
    pub fn new() -> Result<Self> {
        let env = environment();
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(8);
        let det = Session::builder(env)?
            .with_intra_threads(threads)
            .map_err(|e| anyhow!("{e}"))?
            .commit_from_memory(DET_MODEL)
            .map_err(|e| anyhow!("{e}"))
            .context("failed to load embedded detection model")?;
        let rec = Session::builder(env)?
            .with_intra_threads(threads)
            .map_err(|e| anyhow!("{e}"))?
            .commit_from_memory(REC_MODEL)
            .map_err(|e| anyhow!("{e}"))
            .context("failed to load embedded recognition model")?;
        let det_input = det.inputs().first().map(|o| o.name().to_string()).unwrap_or_else(|| "x".into());
        let det_output = det.outputs().first().map(|o| o.name().to_string()).unwrap_or_default();
        let rec_input = rec.inputs().first().map(|o| o.name().to_string()).unwrap_or_else(|| "x".into());
        let rec_output = rec.outputs().first().map(|o| o.name().to_string()).unwrap_or_default();
        let dict: Vec<String> = DICT_TXT.lines().map(|l| l.to_string()).collect();
        if dict.is_empty() {
            return Err(anyhow!("embedded character dictionary is empty"));
        }
        Ok(OcrEngine {
            det: Mutex::new(det),
            rec: Mutex::new(rec),
            det_input,
            det_output,
            rec_input,
            rec_output,
            dict,
        })
    }

    pub fn dict_len(&self) -> usize {
        self.dict.len()
    }

    /// Run OCR on an RGB image and return raw lines in native coordinates.
    pub fn recognize_image(&self, img_rgb: &RgbImage) -> Result<Vec<RawLine>> {
        let (w, h) = (img_rgb.width(), img_rgb.height());
        if w == 0 || h == 0 {
            return Ok(Vec::new());
        }
        // 1. Downscale for inference (Python MAX_OCR_DIM = 1600).
        let max_side = w.max(h);
        let (ocr_img, scale) = if max_side > MAX_OCR_DIM {
            let s = MAX_OCR_DIM as f32 / max_side as f32;
            let nw = (w as f32 * s).round().max(1.0) as u32;
            let nh = (h as f32 * s).round().max(1.0) as u32;
            (
                image::imageops::resize(img_rgb, nw, nh, image::imageops::FilterType::Triangle),
                s,
            )
        } else {
            (img_rgb.clone(), 1.0)
        };
        let inv_scale = 1.0 / scale;

        // 2. Detection on the (possibly downscaled) image.
        let boxes = self.detect(&ocr_img)?; // boxes in ocr_img coordinates
        if ocr_debug() {
            eprintln!("[ocr-debug] detect returned {} quads", boxes.len());
        }

        // 3. Recognition + colour sampling per box.
        let mut raw_lines: Vec<RawLine> = Vec::new();
        for quad in boxes {
            let (text, score) = self.recognize_box(&ocr_img, &quad)?;
            if ocr_debug() {
                eprintln!("[ocr-debug] rec quad {quad:?} -> {score:.3} {text:?}");
            }
            if score < RAW_MIN_SCORE {
                continue;
            }
            let xs: Vec<f32> = quad.iter().map(|p| p.0 * inv_scale).collect();
            let ys: Vec<f32> = quad.iter().map(|p| p.1 * inv_scale).collect();
            let bbox = clamp_box(
                [
                    xs.iter().cloned().fold(f32::INFINITY, f32::min) as i32,
                    ys.iter().cloned().fold(f32::INFINITY, f32::min) as i32,
                    xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max) as i32,
                    ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max) as i32,
                ],
                w as i32,
                h as i32,
            );
            let (bg_color, text_color, is_boxed) = sample_colors(img_rgb, bbox);
            raw_lines.push(RawLine {
                bbox,
                text,
                score: (score * 1000.0).round() / 1000.0,
                bg_color,
                text_color,
                is_boxed,
            });
        }
        raw_lines.sort_by_key(|l| (l.bbox[1], l.bbox[0]));
        Ok(raw_lines)
    }

    // ------------------------------------------------------------------
    // Detection (DBNet)
    // ------------------------------------------------------------------

    /// Detect text boxes. Returns quads in the input image's coordinates.
    fn detect(&self, img: &RgbImage) -> Result<Vec<[(f32, f32); 4]>> {
        let (w, h) = (img.width() as usize, img.height() as usize);
        // RapidOCR det preprocess: longest side <= 960, dims -> multiple of 32.
        let max_side = w.max(h);
        let ratio = if max_side > 960 { 960.0 / max_side as f32 } else { 1.0 };
        let mut dw = ((w as f32 * ratio).round() as usize).max(32);
        let mut dh = ((h as f32 * ratio).round() as usize).max(32);
        dw = dw.div_ceil(32) * 32;
        dh = dh.div_ceil(32) * 32;
        let resized = image::imageops::resize(
            img,
            dw as u32,
            dh as u32,
            image::imageops::FilterType::Triangle,
        );
        let scale_x = dw as f32 / w as f32;
        let scale_y = dh as f32 / h as f32;

        // Normalise to ImageNet mean/std, CHW.
        let mean = [0.485f32, 0.456, 0.406];
        let std = [0.229f32, 0.224, 0.225];
        let mut input = vec![0f32; 3 * dh * dw];
        for y in 0..dh {
            for x in 0..dw {
                let p = resized.get_pixel(x as u32, y as u32);
                for c in 0..3 {
                    input[(c * dh + y) * dw + x] =
                        (p[c] as f32 / 255.0 - mean[c]) / std[c];
                }
            }
        }
        let tensor = Tensor::<f32>::from_array((Shape::new([1, 3, dh as i64, dw as i64]), input))?;
        // SessionOutputs borrow the session (and the MutexGuard), so the
        // tensor data is copied to owned Vecs inside the lock scope.
        let (shape, prob): (Vec<i64>, Vec<f32>) = {
            let mut det = self.det.lock().map_err(|_| anyhow!("det session poisoned"))?;
            let outputs = det.run(ort::inputs![self.det_input.as_str() => &tensor])?;
            let (shape, prob) = outputs[0].try_extract_tensor::<f32>()?;
            (
                shape.iter().map(|d| *d as i64).collect(),
                prob.to_vec(),
            )
        };
        if shape.len() != 4 {
            return Err(anyhow!("unexpected det output shape {shape:?}"));
        }
        let (ph, pw) = (shape[2] as usize, shape[3] as usize);
        // Prob-map resolution is derived from the output shape, NOT assumed
        // 1/4: the SWHL export returns a FULL-resolution map (verified
        // 2026-11-16 with Python onnxruntime: (1,1,512,928) for a 512x928
        // input). A classic Paddle export would be 1/4 — both work here.
        let map_scale_x = dw as f32 / pw as f32;
        let map_scale_y = dh as f32 / ph as f32;

        // Binary map + connected components.
        let binary: Vec<bool> = prob.iter().map(|&p| p > DET_THRESH).collect();
        if ocr_debug() {
            let mx = prob.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            eprintln!(
                "[ocr-debug] det out shape {shape:?} max_prob {mx:.3} px>0.3 {}",
                binary.iter().filter(|b| **b).count()
            );
        }
        let labels = label_components(&binary, pw, ph);

        let mut quads: Vec<[(f32, f32); 4]> = Vec::new();
        let mut max_label = 0usize;
        for &l in &labels {
            max_label = max_label.max(l);
        }
        for label in 1..=max_label {
            // Collect component pixels (in prob-map coordinates).
            let mut pts: Vec<(f32, f32)> = Vec::new();
            for y in 0..ph {
                for x in 0..pw {
                    if labels[y * pw + x] == label {
                        pts.push((x as f32, y as f32));
                    }
                }
            }
            if pts.len() < 9 {
                continue;
            }
            let hull = convex_hull(&pts);
            if hull.len() < 3 {
                continue;
            }
            let rect = min_area_rect(&hull); // in prob-map coords
            // Score: mean probability inside the rect (fast mode).
            let score = box_score(&prob, pw, ph, &rect);
            if score < DET_BOX_THRESH {
                continue;
            }
            // Unclip: offset the rect outward.
            let quad = unclip_polygon(&rect, DET_UNCLIP_RATIO);
            // Map back: prob-map coords -> det-input coords (map_scale),
            // then det-input -> OCR-image coords (1/scale).
            let quad: [(f32, f32); 4] = quad.map(|(x, y)| {
                (
                    x * map_scale_x / scale_x,
                    y * map_scale_y / scale_y,
                )
            });
            // Clip to the OCR image.
            let quad: [(f32, f32); 4] = quad.map(|(x, y)| {
                (x.clamp(0.0, w as f32 - 1.0), y.clamp(0.0, h as f32 - 1.0))
            });
            quads.push(quad);
        }
        if ocr_debug() {
            eprintln!("[ocr-debug] detect kept {} quads", quads.len());
        }
        Ok(quads)
    }

    // ------------------------------------------------------------------
    // Recognition (CTC)
    // ------------------------------------------------------------------

    /// Recognise one detection quad. Returns (text, mean char probability).
    fn recognize_box(&self, img: &RgbImage, quad: &[(f32, f32); 4]) -> Result<(String, f32)> {
        let ordered = order_points(quad);
        let (tl, tr, br, bl) = (ordered[0], ordered[1], ordered[2], ordered[3]);
        let width_top = dist(tl, tr);
        let width_bot = dist(bl, br);
        let height_left = dist(tl, bl);
        let height_right = dist(tr, br);
        let crop_w = width_top.max(width_bot).max(8.0) as usize;
        let crop_h = height_left.max(height_right).max(8.0) as usize;

        // Perspective warp to an upright rectangle (bilinear sampling).
        let mut crop = RgbImage::new(crop_w as u32, crop_h as u32);
        let src = [tl, tr, br, bl];
        let dst = [
            (0.0f32, 0.0f32),
            (crop_w as f32 - 1.0, 0.0),
            (crop_w as f32 - 1.0, crop_h as f32 - 1.0),
            (0.0, crop_h as f32 - 1.0),
        ];
        let hom = homography(&dst, &src).unwrap_or_else(|| {
            // Fall back to an affine-ish identity mapping.
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        });
        for y in 0..crop_h {
            for x in 0..crop_w {
                let (sx, sy) = apply_homography(&hom, x as f32, y as f32);
                crop.put_pixel(x as u32, y as u32, sample_bilinear(img, sx, sy));
            }
        }

        // Resize to height 48, width proportional (capped at 320).
        let aspect = crop_w as f32 / crop_h as f32;
        let rec_w = ((REC_H as f32 * aspect).round() as usize).clamp(8, REC_MAX_W);
        let rec_img = image::imageops::resize(
            &crop,
            rec_w as u32,
            REC_H as u32,
            image::imageops::FilterType::Triangle,
        );
        let mut input = vec![0f32; 3 * REC_H * rec_w];
        for y in 0..REC_H {
            for x in 0..rec_w {
                let p = rec_img.get_pixel(x as u32, y as u32);
                for c in 0..3 {
                    input[(c * REC_H + y) * rec_w + x] = (p[c] as f32 / 255.0 - 0.5) / 0.5;
                }
            }
        }
        let tensor = Tensor::<f32>::from_array((Shape::new([1, 3, REC_H as i64, rec_w as i64]), input))?;
        let (shape, logits): (Vec<i64>, Vec<f32>) = {
            let mut rec = self.rec.lock().map_err(|_| anyhow!("rec session poisoned"))?;
            let outputs = rec.run(ort::inputs![self.rec_input.as_str() => &tensor])?;
            let (shape, logits) = outputs[0].try_extract_tensor::<f32>()?;
            (
                shape.iter().map(|d| *d as i64).collect(),
                logits.to_vec(),
            )
        };
        if shape.len() != 3 {
            return Err(anyhow!("unexpected rec output shape {shape:?}"));
        }
        let (t, classes) = (shape[1] as usize, shape[2] as usize);

        // CTC greedy decode: argmax per step, collapse repeats, drop blank(0).
        // The SWHL rec model outputs 6,625 classes and the rows are ALREADY
        // softmax probabilities (verified 2026-11-16: rows sum to 1.0), so
        // the argmax probability is the row max — no second softmax.
        // Class layout: 0 = blank, 1..=6623 = dict[i-1], 6624 = space.
        let mut text = String::new();
        let mut probs: Vec<f32> = Vec::new();
        let mut prev = 0usize;
        for step in 0..t {
            let row = &logits[step * classes..(step + 1) * classes];
            let (idx, max_prob) = row
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, v)| (i, *v))
                .unwrap_or((0, 0.0));
            let prob = max_prob.clamp(0.0, 1.0);
            if idx != 0 && idx != prev {
                if idx == self.dict.len() + 1 {
                    text.push(' ');
                    probs.push(prob);
                } else if idx - 1 < self.dict.len() {
                    text.push_str(&self.dict[idx - 1]);
                    probs.push(prob);
                }
            }
            prev = idx;
        }
        let score = if probs.is_empty() {
            0.0
        } else {
            probs.iter().sum::<f32>() / probs.len() as f32
        };
        Ok((text, score))
    }
}

// ---------------------------------------------------------------------------
// Geometry helpers (DBNet post-processing)
// ---------------------------------------------------------------------------

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// 4-point connected-component labelling (BFS) on a binary map.
fn label_components(binary: &[bool], w: usize, h: usize) -> Vec<usize> {
    let mut labels = vec![0usize; w * h];
    let mut next = 0usize;
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
    for y in 0..h {
        for x in 0..w {
            if !binary[y * w + x] || labels[y * w + x] != 0 {
                continue;
            }
            next += 1;
            labels[y * w + x] = next;
            queue.push_back((x, y));
            while let Some((cx, cy)) = queue.pop_front() {
                for (nx, ny) in [
                    (cx.wrapping_sub(1), cy),
                    (cx + 1, cy),
                    (cx, cy.wrapping_sub(1)),
                    (cx, cy + 1),
                ] {
                    if nx < w && ny < h && binary[ny * w + nx] && labels[ny * w + nx] == 0 {
                        labels[ny * w + nx] = next;
                        queue.push_back((nx, ny));
                    }
                }
            }
        }
    }
    labels
}

/// Andrew monotone-chain convex hull (CCW, no duplicate last point).
fn convex_hull(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let mut pts = points.to_vec();
    pts.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    pts.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: (f32, f32), a: (f32, f32), b: (f32, f32)| {
        (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
    };
    let mut lower: Vec<(f32, f32)> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<(f32, f32)> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Minimum-area enclosing rectangle of a convex polygon (rotating calipers).
/// Returns 4 corners in order.
fn min_area_rect(hull: &[(f32, f32)]) -> [(f32, f32); 4] {
    let n = hull.len();
    let mut best_area = f32::INFINITY;
    let mut best: [(f32, f32); 4] = [hull[0], hull[0], hull[0], hull[0]];
    for i in 0..n {
        let p0 = hull[i];
        let p1 = hull[(i + 1) % n];
        let ex = p1.0 - p0.0;
        let ey = p1.1 - p0.1;
        let len = (ex * ex + ey * ey).sqrt().max(1e-9);
        let (ux, uy) = (ex / len, ey / len);
        let (vx, vy) = (-uy, ux);
        let mut min_u = f32::INFINITY;
        let mut max_u = f32::NEG_INFINITY;
        let mut min_v = f32::INFINITY;
        let mut max_v = f32::NEG_INFINITY;
        for &p in hull {
            let u = p.0 * ux + p.1 * uy;
            let v = p.0 * vx + p.1 * vy;
            min_u = min_u.min(u);
            max_u = max_u.max(u);
            min_v = min_v.min(v);
            max_v = max_v.max(v);
        }
        let area = (max_u - min_u) * (max_v - min_v);
        if area < best_area {
            best_area = area;
            best = [
                (min_u * ux + min_v * vx, min_u * uy + min_v * vy),
                (max_u * ux + min_v * vx, max_u * uy + min_v * vy),
                (max_u * ux + max_v * vx, max_u * uy + max_v * vy),
                (min_u * ux + max_v * vx, min_u * uy + max_v * vy),
            ];
        }
    }
    best
}

/// Mean probability inside a quad (fast score mode): sample the prob map
/// at integer pixels whose centres fall inside the quad.
fn box_score(prob: &[f32], w: usize, h: usize, quad: &[(f32, f32); 4]) -> f32 {
    let min_x = quad.iter().map(|p| p.0).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize;
    let max_x = quad.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max).ceil().min(w as f32 - 1.0) as usize;
    let min_y = quad.iter().map(|p| p.1).fold(f32::INFINITY, f32::min).floor().max(0.0) as usize;
    let max_y = quad.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max).ceil().min(h as f32 - 1.0) as usize;
    let mut sum = 0f32;
    let mut count = 0usize;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if point_in_quad((x as f32 + 0.5, y as f32 + 0.5), quad) {
                sum += prob[y * w + x];
                count += 1;
            }
        }
    }
    if count == 0 {
        0.0
    } else {
        sum / count as f32
    }
}

fn point_in_quad(p: (f32, f32), quad: &[(f32, f32); 4]) -> bool {
    // Even-odd ray casting.
    let mut inside = false;
    for i in 0..4 {
        let a = quad[i];
        let b = quad[(i + 1) % 4];
        if (a.1 > p.1) != (b.1 > p.1) {
            let xinters = (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0;
            if p.0 < xinters {
                inside = !inside;
            }
        }
    }
    inside
}

fn polygon_area(poly: &[(f32, f32); 4]) -> f32 {
    let mut a = 0f32;
    for i in 0..4 {
        let p = poly[i];
        let q = poly[(i + 1) % 4];
        a += p.0 * q.1 - q.0 * p.1;
    }
    a / 2.0
}

fn polygon_perimeter(poly: &[(f32, f32); 4]) -> f32 {
    (0..4).map(|i| dist(poly[i], poly[(i + 1) % 4])).sum()
}

/// Vatti-style unclipping: offset a convex quad outward by
/// `area * ratio / perimeter` (RapidOCR `det_db_unclip_ratio` = 1.5),
/// by shifting each edge along its outward normal and intersecting
/// adjacent shifted lines.
fn unclip_polygon(quad: &[(f32, f32); 4], ratio: f32) -> [(f32, f32); 4] {
    let mut poly = *quad;
    let area = polygon_area(&poly).abs();
    let perim = polygon_perimeter(&poly);
    if area == 0.0 || perim == 0.0 {
        return poly;
    }
    // Ensure CCW (positive area) so outward normals are consistent.
    if polygon_area(&poly) < 0.0 {
        poly.reverse();
    }
    let d = area * ratio / perim;
    // Shifted lines: each edge as (point, direction); shifted point = p + n*d.
    let mut lines: [((f32, f32), (f32, f32)); 4] = [((0.0, 0.0), (0.0, 0.0)); 4];
    for i in 0..4 {
        let p = poly[i];
        let q = poly[(i + 1) % 4];
        let (ex, ey) = (q.0 - p.0, q.1 - p.1);
        let len = (ex * ex + ey * ey).sqrt().max(1e-9);
        // Outward normal for a CCW polygon: (ey, -ex) / len.
        let n = (ey / len, -ex / len);
        lines[i] = ((p.0 + n.0 * d, p.1 + n.1 * d), (ex / len, ey / len));
    }
    let mut out = [(0.0f32, 0.0f32); 4];
    for i in 0..4 {
        let prev = lines[(i + 3) % 4];
        let curr = lines[i];
        out[i] = line_intersection(prev, curr).unwrap_or(poly[i]);
    }
    out
}

fn line_intersection(
    l1: ((f32, f32), (f32, f32)),
    l2: ((f32, f32), (f32, f32)),
) -> Option<(f32, f32)> {
    let ((x1, y1), (dx1, dy1)) = l1;
    let ((x2, y2), (dx2, dy2)) = l2;
    let denom = dx1 * dy2 - dy1 * dx2;
    if denom.abs() < 1e-9 {
        return None;
    }
    let t = ((x2 - x1) * dy2 - (y2 - y1) * dx2) / denom;
    Some((x1 + t * dx1, y1 + t * dy1))
}

/// Order quad points as TL, TR, BR, BL (RapidOCR `order_points`).
fn order_points(quad: &[(f32, f32); 4]) -> [(f32, f32); 4] {
    let mut pts = *quad;
    // TL = min(x+y), BR = max(x+y). For the other two, diff = x - y:
    // top-right has large x and small y (MAX diff), bottom-left has
    // small x and large y (MIN diff). (The classic imutils recipe uses
    // diff = y - x, which flips this — using x - y with min/max swapped
    // warps the crop diagonally and recognition returns blank.)
    pts.sort_by(|a, b| {
        (a.0 + a.1)
            .partial_cmp(&(b.0 + b.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let tl = pts[0];
    let br = pts[3];
    let rest = [pts[1], pts[2]];
    let (tr, bl) = if rest[0].0 - rest[0].1 > rest[1].0 - rest[1].1 {
        (rest[0], rest[1])
    } else {
        (rest[1], rest[0])
    };
    [tl, tr, br, bl]
}

/// Homography mapping `src` quad -> `dst` quad (DLT, 8 unknowns).
fn homography(dst: &[(f32, f32); 4], src: &[(f32, f32); 4]) -> Option<[[f32; 3]; 3]> {
    // Solve for h (8 unknowns, h33 = 1) via Gaussian elimination.
    let mut a = [[0f64; 8]; 8];
    let mut b = [0f64; 8];
    for i in 0..4 {
        let (x, y) = (dst[i].0 as f64, dst[i].1 as f64);
        let (u, v) = (src[i].0 as f64, src[i].1 as f64);
        a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y];
        b[2 * i] = u;
        a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y];
        b[2 * i + 1] = v;
    }
    // Gaussian elimination with partial pivoting.
    for col in 0..8 {
        let mut piv = col;
        for r in col + 1..8 {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for r in 0..8 {
            if r != col {
                let f = a[r][col] / a[col][col];
                for c in col..8 {
                    a[r][c] -= f * a[col][c];
                }
                b[r] -= f * b[col];
            }
        }
    }
    let h: Vec<f32> = (0..8).map(|i| (b[i] / a[i][i]) as f32).collect();
    Some([
        [h[0], h[1], h[2]],
        [h[3], h[4], h[5]],
        [h[6], h[7], 1.0],
    ])
}

fn apply_homography(h: &[[f32; 3]; 3], x: f32, y: f32) -> (f32, f32) {
    let w = h[2][0] * x + h[2][1] * y + h[2][2];
    if w.abs() < 1e-9 {
        return (x, y);
    }
    (
        (h[0][0] * x + h[0][1] * y + h[0][2]) / w,
        (h[1][0] * x + h[1][1] * y + h[1][2]) / w,
    )
}

fn sample_bilinear(img: &RgbImage, x: f32, y: f32) -> Rgb<u8> {
    let (w, h) = (img.width() as i32, img.height() as i32);
    if x < 0.0 || y < 0.0 || x > (w - 1) as f32 || y > (h - 1) as f32 {
        return Rgb([0, 0, 0]);
    }
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let mut out = [0f32; 3];
    for (px, py, wx) in [
        (x0, y0, (1.0 - fx) * (1.0 - fy)),
        (x1, y0, fx * (1.0 - fy)),
        (x0, y1, (1.0 - fx) * fy),
        (x1, y1, fx * fy),
    ] {
        let p = img.get_pixel(px as u32, py as u32);
        for c in 0..3 {
            out[c] += p[c] as f32 * wx;
        }
    }
    Rgb([out[0] as u8, out[1] as u8, out[2] as u8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_and_rect() {
        let pts = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0), (5.0, 2.0)];
        let hull = convex_hull(&pts);
        assert_eq!(hull.len(), 4);
        let rect = min_area_rect(&hull);
        let area = polygon_area(&rect).abs();
        assert!((area - 50.0).abs() < 1.0, "area {area}");
    }

    #[test]
    fn unclip_grows_quad() {
        let quad = [(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)];
        let out = unclip_polygon(&quad, 1.5);
        assert!(polygon_area(&out).abs() > 50.0);
    }

    #[test]
    fn order_points_tl_first() {
        let quad = [(10.0, 0.0), (0.0, 5.0), (10.0, 5.0), (0.0, 0.0)];
        let o = order_points(&quad);
        assert_eq!(o[0], (0.0, 0.0)); // TL
        assert_eq!(o[1], (10.0, 0.0)); // TR (max x-y)
        assert_eq!(o[2], (10.0, 5.0)); // BR
        assert_eq!(o[3], (0.0, 5.0)); // BL (min x-y)
    }

    #[test]
    fn dict_loads() {
        assert_eq!(DICT_TXT.lines().count(), 6623);
    }
}
