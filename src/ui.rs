//! Dual-column GUI — port of `CaptionReplacerApp` (tkinter) to eframe/egui.
//!
//! Left column: folders & font, OCR threshold tuning (instant re-filter
//! from stored `raw_lines`, no re-inference), batch workflow, activity log.
//! Right column: tabbed workspace — Translation JSON editor and Live
//! Preview & OCR Check (Rendered / OCR Boxes / Original).
//!
//! OCR is pre-warmed in a background thread at launch. Scanning runs on a
//! worker thread; rendering runs on a worker thread with rayon inside.
//! The UI thread only ever receives progress/log/done messages over a
//! channel, so the interface stays responsive during a batch.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::SystemTime;

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender, unbounded};
use egui::{ColorImage, TextureHandle};

use crate::image_io::{is_image_file, load_image, save_image};
use crate::layout::group_lines_into_blocks;
use crate::model::{ExtractedData, Settings};
use crate::ocr::OcrEngine;
use crate::parser::{compact_map_from_data, parse_flexible_json_or_dict, resolve_data};
use crate::render::{render_entry, render_ocr_diagnostic};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Json,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewMode {
    Rendered,
    Ocr,
    Original,
}

enum WorkerMsg {
    Log(String),
    Status(String),
    Progress { done: usize, total: usize, label: String },
    OcrReady(Result<Arc<OcrEngine>, String>),
    ExtractDone { data: ExtractedData, cancelled: bool },
    RenderDone { rendered: usize, copied: usize, failed: usize, cancelled: bool },
}

pub struct CaptionStudioApp {
    // Settings (persisted to ~/.caption_replacer.json, same as Python).
    input_dir: String,
    output_dir: String,
    font_path: String,
    conf: f64,
    minchars: i32,

    // Data
    extracted: Option<ExtractedData>,
    editor_text: String,
    ocr: Option<Arc<OcrEngine>>,
    ocr_ready: bool,
    ocr_error: Option<String>,
    pending_extract: bool,

    // UI state
    tab: Tab,
    preview_file: String,
    preview_mode: PreviewMode,
    preview_key: String,
    preview_texture: Option<TextureHandle>,
    preview_size: [f32; 2],
    verify_text: String,
    log_lines: Vec<String>,
    status: String,
    progress: f32,
    busy: bool,
    cancel: Arc<AtomicBool>,
    confirm_copy_originals: bool,
    pending_render: Option<ExtractedData>,

    tx: Sender<WorkerMsg>,
    rx: Receiver<WorkerMsg>,
}

impl CaptionStudioApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let settings = Settings::load();
        let (tx, rx) = unbounded::<WorkerMsg>();
        let app = CaptionStudioApp {
            input_dir: settings.input,
            output_dir: settings.output,
            font_path: settings.font,
            conf: settings.conf,
            minchars: settings.minchars,
            extracted: None,
            editor_text: String::new(),
            ocr: None,
            ocr_ready: false,
            ocr_error: None,
            pending_extract: false,
            tab: Tab::Json,
            preview_file: String::new(),
            preview_mode: PreviewMode::Rendered,
            preview_key: String::new(),
            preview_texture: None,
            preview_size: [0.0, 0.0],
            verify_text: String::new(),
            log_lines: Vec::new(),
            status: "Choose an input folder, then click Scan & Extract Text.".to_string(),
            progress: 0.0,
            busy: false,
            cancel: Arc::new(AtomicBool::new(false)),
            confirm_copy_originals: false,
            pending_render: None,
            tx: tx.clone(),
            rx,
        };
        // Pre-warm the OCR engine in a background thread (Python did the
        // same with `threading.Thread(target=self._get_ocr_engine)`).
        let warm_tx = tx;
        std::thread::spawn(move || {
            let res = OcrEngine::new().map(Arc::new);
            let _ = warm_tx.send(WorkerMsg::OcrReady(res.map_err(|e| e.to_string())));
        });
        app
    }

    // ------------------------------------------------------------ helpers

    fn log(&mut self, msg: String) {
        self.log_lines.push(msg);
        if self.log_lines.len() > 400 {
            let excess = self.log_lines.len() - 400;
            self.log_lines.drain(..excess);
        }
    }

    fn save_settings(&self) {
        Settings {
            input: self.input_dir.clone(),
            output: self.output_dir.clone(),
            font: self.font_path.clone(),
            conf: self.conf.clamp(0.0, 1.0),
            minchars: self.minchars.max(0),
        }
        .save();
    }

    fn safe_conf(&self) -> f32 {
        self.conf.clamp(0.0, 1.0) as f32
    }

    fn safe_minchars(&self) -> usize {
        self.minchars.max(0) as usize
    }

    fn ocr_engine(&self) -> Option<Arc<OcrEngine>> {
        self.ocr.clone()
    }

    /// Resolve the editor text into renderable data (see parser.rs).
    fn resolve_editor(&self) -> Result<(ExtractedData, Vec<String>, Option<usize>)> {
        let raw = self.editor_text.trim();
        if raw.is_empty() {
            anyhow::bail!("Paste the translated JSON into the editor first.");
        }
        let input_dir = PathBuf::from(&self.input_dir);
        resolve_data(raw, &self.extracted, &input_dir)
    }

    // ------------------------------------------------------------ actions

    fn browse_input(&mut self) {
        if let Some(folder) = rfd::FileDialog::new().set_title("Select Input Images Folder").pick_folder() {
            self.input_dir = folder.to_string_lossy().to_string();
            self.output_dir = folder.join("translated_output").to_string_lossy().to_string();
            self.extracted = None;
            self.preview_texture = None;
            self.preview_key.clear();
            self.save_settings();
        }
    }

    fn browse_output(&mut self) {
        if let Some(folder) = rfd::FileDialog::new().set_title("Select Output Folder").pick_folder() {
            self.output_dir = folder.to_string_lossy().to_string();
            self.save_settings();
        }
    }

    fn browse_font(&mut self) {
        if let Some(file) = rfd::FileDialog::new()
            .set_title("Select Font File")
            .add_filter("Font files", &["ttf", "otf", "ttc"])
            .pick_file()
        {
            self.font_path = file.to_string_lossy().to_string();
            self.preview_key.clear();
            self.save_settings();
        }
    }

    /// Instant threshold re-apply from stored raw_lines (Python
    /// `_reapply_thresholds`). Translations already in the editor are
    /// preserved by matching on the old block's original text.
    fn reapply_thresholds(&mut self) {
        let Some(data) = self.extracted.clone() else {
            self.status = "Run 'Scan & Extract Text' first, then adjust thresholds anytime.".into();
            return;
        };
        if !data.images.iter().any(|img| !img.raw_lines.is_empty()) {
            self.status = "This captions.json predates raw OCR storage. Click 'Scan & Extract Text' once to enable instant tuning.".into();
            return;
        }
        // Preserve translations from the editor, keyed by original text.
        let mut existing: std::collections::HashMap<String, std::collections::HashMap<String, String>> =
            std::collections::HashMap::new();
        if let Ok(parsed) = parse_flexible_json_or_dict(self.editor_text.trim()) {
            for img in &data.images {
                if let Some(serde_json::Value::Object(tmap)) = parsed.get(&img.file) {
                    let mut lookup = std::collections::HashMap::new();
                    for old in &img.blocks {
                        if let Some(serde_json::Value::String(v)) = tmap.get(&old.id.to_string()) {
                            if v != &old.original_text {
                                lookup.insert(old.original_text.clone(), v.clone());
                            }
                        }
                    }
                    if !lookup.is_empty() {
                        existing.insert(img.file.clone(), lookup);
                    }
                }
            }
        }

        let min_conf = self.safe_conf();
        let min_chars = self.safe_minchars();
        let mut data = data;
        let mut total_kept = 0usize;
        let mut total_dropped = 0usize;
        for img in data.images.iter_mut() {
            let (blocks, dropped) = group_lines_into_blocks(&img.raw_lines, min_conf, min_chars);
            let mut blocks = blocks;
            if let Some(saved) = existing.get(&img.file) {
                for b in blocks.iter_mut() {
                    if let Some(v) = saved.get(&b.original_text) {
                        b.new_text = v.clone();
                    }
                }
            }
            total_kept += blocks.len();
            total_dropped += dropped.len();
            img.blocks = blocks;
            img.dropped = dropped;
        }
        self.extracted = Some(data);
        self.preview_key.clear();
        let compact = compact_map_from_data(self.extracted.as_ref().unwrap());
        self.editor_text =
            serde_json::to_string_pretty(&compact).unwrap_or_default();
        self.status = format!(
            "Thresholds applied (conf >= {min_conf:.2}, unboxed >= {min_chars} chars): {total_kept} kept, {total_dropped} filtered out."
        );
    }

    fn copy_compact_for_ai(&mut self) {
        if self.editor_text.trim().is_empty() {
            self.status = "Nothing to copy — scan & extract text first or load a JSON file.".into();
            return;
        }
        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(self.editor_text.clone())) {
            Ok(()) => self.status = "Copied JSON to clipboard — paste it into your AI translator.".into(),
            Err(e) => self.status = format!("Clipboard error: {e}"),
        }
    }

    fn paste_clipboard(&mut self) {
        match arboard::Clipboard::new().and_then(|mut cb| cb.get_text()) {
            Ok(clip) => {
                self.editor_text = clip;
                self.preview_key.clear();
                self.status = "Pasted translated JSON from clipboard. Switch to Live Preview or Render.".into();
            }
            Err(_) => self.status = "Clipboard empty — nothing found on the clipboard.".into(),
        }
    }

    fn save_full_json(&mut self) {
        let Some(data) = self.extracted.clone() else {
            self.status = "No data — run 'Scan & Extract Text' first.".into();
            return;
        };
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Save captions.json")
            .set_file_name("captions.json")
            .add_filter("JSON files", &["json"])
            .save_file()
        {
            match serde_json::to_string_pretty(&data)
                .map_err(anyhow::Error::from)
                .and_then(|s| std::fs::write(&path, s).map_err(anyhow::Error::from))
            {
                Ok(()) => {
                    self.status = format!("Saved metadata to {}", path.file_name().unwrap_or_default().to_string_lossy());
                }
                Err(e) => self.status = format!("Save failed: {e}"),
            }
        }
    }

    fn load_full_json(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Load captions.json")
            .add_filter("JSON files", &["json"])
            .pick_file()
        {
            match std::fs::read_to_string(&path)
                .map_err(anyhow::Error::from)
                .and_then(|s| serde_json::from_str::<ExtractedData>(&s).map_err(anyhow::Error::from))
            {
                Ok(data) => {
                    let n = data.images.len();
                    self.editor_text = serde_json::to_string_pretty(&compact_map_from_data(&data))
                        .unwrap_or_default();
                    self.extracted = Some(data);
                    self.preview_key.clear();
                    self.status = format!("Loaded {n} images from {}", path.file_name().unwrap_or_default().to_string_lossy());
                }
                Err(e) => self.status = format!("Could not load JSON: {e}"),
            }
        }
    }

    fn load_translated_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Select Translated JSON File")
            .add_filter("JSON files", &["json"])
            .pick_file()
        {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    self.editor_text = text;
                    self.preview_key.clear();
                    self.status = format!("Imported translations from {}", path.file_name().unwrap_or_default().to_string_lossy());
                }
                Err(e) => self.status = format!("Could not read file: {e}"),
            }
        }
    }

    fn open_output_folder(&mut self) {
        let out = self.output_dir.trim();
        if out.is_empty() || !Path::new(out).exists() {
            self.status = "Output folder does not exist yet.".into();
            return;
        }
        let res = if cfg!(target_os = "windows") {
            std::process::Command::new("explorer").arg(out).spawn()
        } else if cfg!(target_os = "macos") {
            std::process::Command::new("open").arg(out).spawn()
        } else {
            std::process::Command::new("xdg-open").arg(out).spawn()
        };
        if let Err(e) = res {
            self.status = format!("Could not open folder: {e}");
        }
    }

    // ------------------------------------------------------------ workers

    fn start_extract(&mut self) {
        let in_dir = self.input_dir.trim().to_string();
        if in_dir.is_empty() || !Path::new(&in_dir).is_dir() {
            self.status = "Please select a valid Input Folder.".into();
            return;
        }
        if !self.ocr_ready {
            self.pending_extract = true;
            self.status = "OCR engine still warming up — scan will start automatically.".into();
            return;
        }
        self.busy = true;
        self.cancel.store(false, Ordering::SeqCst);
        self.progress = 0.0;
        self.status = "Initializing OCR engine...".into();
        let tx = self.tx.clone();
        let cancel = self.cancel.clone();
        let min_conf = self.safe_conf();
        let min_chars = self.safe_minchars();
        let engine = self.ocr.clone().expect("OCR engine ready");
        std::thread::spawn(move || {
            extract_worker(PathBuf::from(in_dir), engine, min_conf, min_chars, tx, cancel);
        });
    }

    fn start_render(&mut self) {
        let in_dir = self.input_dir.trim().to_string();
        let out_dir = self.output_dir.trim().to_string();
        if in_dir.is_empty() || !Path::new(&in_dir).is_dir() {
            self.status = "Please select a valid Input Folder.".into();
            return;
        }
        if out_dir.is_empty() {
            self.status = "Please specify an Output Folder.".into();
            return;
        }
        if Path::new(&out_dir) == Path::new(&in_dir) {
            self.status = "Output folder must differ from Input folder.".into();
            return;
        }
        let (data, warnings, count) = match self.resolve_editor() {
            Ok(v) => v,
            Err(e) => {
                self.status = format!("Parse error: {e}");
                return;
            }
        };
        for w in &warnings {
            self.log(format!("WARN {w}"));
        }
        if count == Some(0) {
            // No caption differs from the original — ask, like Python.
            self.pending_render = Some(data);
            self.confirm_copy_originals = true;
            return;
        }
        self.launch_render(data);
    }

    fn launch_render(&mut self, data: ExtractedData) {
        self.busy = true;
        self.cancel.store(false, Ordering::SeqCst);
        self.progress = 0.0;
        let tx = self.tx.clone();
        let cancel = self.cancel.clone();
        let in_dir = PathBuf::from(self.input_dir.trim());
        let out_dir = PathBuf::from(self.output_dir.trim());
        let font = if self.font_path.trim().is_empty() {
            None
        } else {
            Some(self.font_path.trim().to_string())
        };
        std::thread::spawn(move || {
            render_worker(in_dir, out_dir, data, font, tx, cancel);
        });
    }

    // ------------------------------------------------------------ preview

    /// Resolve the current editor + extracted data, pick the selected file's
    /// entry, and rebuild the verify text + preview texture when the cache
    /// key changed (Python `_refresh_inline_preview`).
    fn refresh_preview(&mut self, ctx: &egui::Context) {
        let in_dir = self.input_dir.trim();
        if in_dir.is_empty() || !Path::new(in_dir).is_dir() {
            self.verify_text = "Select a valid Input Folder on the left to preview images.".into();
            return;
        }
        let data = if self.editor_text.trim().is_empty() {
            match &self.extracted {
                Some(d) => d.clone(),
                None => {
                    self.verify_text = "Scan & Extract text or paste translated JSON to preview.".into();
                    return;
                }
            }
        } else {
            match self.resolve_editor() {
                Ok((d, _, _)) => d,
                Err(e) => {
                    self.verify_text = format!("JSON Preview Error:\n{e}");
                    return;
                }
            }
        };
        if data.images.is_empty() {
            self.verify_text = "No images found in extracted metadata.".into();
            return;
        }
        if !data.images.iter().any(|e| e.file == self.preview_file) {
            let mut names: Vec<&str> = data.images.iter().map(|e| e.file.as_str()).collect();
            names.sort();
            if let Some(first) = names.first() {
                self.preview_file = first.to_string();
            }
        }
        let Some(entry) = data.entry(&self.preview_file).cloned() else {
            return;
        };
        self.verify_text = verify_text_for(&entry);

        let img_path = PathBuf::from(in_dir).join(&entry.file);
        let mtime = img_path
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let blocks_json = serde_json::to_string(&entry.blocks).unwrap_or_default();
        let dropped_json = serde_json::to_string(&entry.dropped).unwrap_or_default();
        let key = format!(
            "{}|{mtime}|{:?}|{}|{blocks_json}|{dropped_json}",
            img_path.display(),
            self.preview_mode,
            self.font_path
        );
        if key == self.preview_key && self.preview_texture.is_some() {
            return;
        }
        let img = match load_image(&img_path) {
            Ok(i) => i,
            Err(_) => {
                self.verify_text = format!("Could not read image: {}", entry.file);
                return;
            }
        };
        let rendered = match self.preview_mode {
            PreviewMode::Original => img,
            PreviewMode::Ocr => render_ocr_diagnostic(&img, &entry),
            PreviewMode::Rendered => {
                let font = if self.font_path.trim().is_empty() {
                    None
                } else {
                    Some(self.font_path.trim())
                };
                render_entry(&img, &entry, font)
            }
        };
        let (w, h) = (rendered.width(), rendered.height());
        let color = ColorImage::from_rgb([w as usize, h as usize], rendered.as_raw());
        self.preview_texture = Some(ctx.load_texture("preview", color, egui::TextureOptions::LINEAR));
        self.preview_size = [w as f32, h as f32];
        self.preview_key = key;
    }

    fn file_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        if let Some(data) = &self.extracted {
            names = data.images.iter().map(|e| e.file.clone()).collect();
        } else if let Ok((data, _, _)) = self.resolve_editor() {
            names = data.images.iter().map(|e| e.file.clone()).collect();
        }
        names.sort();
        names
    }

    // ------------------------------------------------------------ messages

    fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                WorkerMsg::Log(m) => self.log(m),
                WorkerMsg::Status(s) => self.status = s,
                WorkerMsg::Progress { done, total, label } => {
                    self.progress = if total == 0 { 0.0 } else { done as f32 / total as f32 };
                    self.status = label;
                }
                WorkerMsg::OcrReady(res) => match res {
                    Ok(engine) => {
                        self.ocr = Some(engine);
                        self.ocr_ready = true;
                        self.ocr_error = None;
                        self.log("OCR engine ready (RapidOCR PP-OCRv4, embedded).".into());
                        if self.pending_extract {
                            self.pending_extract = false;
                            self.start_extract();
                        }
                    }
                    Err(e) => {
                        self.ocr_error = Some(e.clone());
                        self.status = format!("OCR engine failed to start: {e}");
                    }
                },
                WorkerMsg::ExtractDone { data, cancelled } => {
                    self.busy = false;
                    self.progress = 1.0;
                    let n = data.images.len();
                    self.editor_text =
                        serde_json::to_string_pretty(&compact_map_from_data(&data)).unwrap_or_default();
                    self.extracted = Some(data);
                    self.preview_key.clear();
                    self.tab = Tab::Json;
                    let prefix = if cancelled { "Stopped. " } else { "" };
                    self.status = format!(
                        "{prefix}Extracted {n} images. Click 'Inspect OCR Boxes' to verify or 'Copy JSON for AI'."
                    );
                }
                WorkerMsg::RenderDone { rendered, copied, failed, cancelled } => {
                    self.busy = false;
                    self.progress = 1.0;
                    let prefix = if cancelled { "Stopped — " } else { "Complete — " };
                    let summary = format!("{prefix}Rendered {rendered}, copied {copied}, failed {failed}");
                    self.log(summary.clone());
                    self.status = format!("{summary} -> {}", self.output_dir);
                }
            }
        }
    }
}

fn verify_text_for(entry: &crate::model::ImageEntry) -> String {
    let mut lines: Vec<String> = Vec::new();
    for blk in &entry.blocks {
        let new_t = blk.new_text.trim();
        if !new_t.is_empty() && new_t != blk.original_text.trim() {
            lines.push(format!(
                "[#{} · conf {:.2}] OCR: \"{}\"  -->  NEW: \"{}\"",
                blk.id, blk.confidence, blk.original_text, new_t
            ));
        } else {
            lines.push(format!(
                "[#{} · conf {:.2}] OCR: \"{}\"",
                blk.id, blk.confidence, blk.original_text
            ));
        }
    }
    for drop in &entry.dropped {
        lines.push(format!("[FILTERED · {}] \"{}\"", drop.reason, drop.text));
    }
    if lines.is_empty() {
        lines.push("No text detected on this image with current settings.".into());
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Workers
// ---------------------------------------------------------------------------

fn extract_worker(
    input_dir: PathBuf,
    engine: Arc<OcrEngine>,
    min_conf: f32,
    min_chars: usize,
    tx: Sender<WorkerMsg>,
    cancel: Arc<AtomicBool>,
) {
    let send = |m: WorkerMsg| {
        let _ = tx.send(m);
    };
    let mut img_files: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&input_dir) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_file() && is_image_file(&p) {
                img_files.push(p);
            }
        }
    }
    img_files.sort();
    if img_files.is_empty() {
        send(WorkerMsg::Status("No supported images found in folder.".into()));
        send(WorkerMsg::ExtractDone { data: ExtractedData::default(), cancelled: false });
        return;
    }
    let total = img_files.len();
    let mut data = ExtractedData::default();
    let mut cancelled = false;
    for (idx, img_path) in img_files.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        let name = img_path.file_name().unwrap_or_default().to_string_lossy().to_string();
        send(WorkerMsg::Progress {
            done: idx + 1,
            total,
            label: format!("Scanning ({}/{total}): {name}", idx + 1),
        });
        match load_image(img_path) {
            Ok(rgb) => {
                let (w, h) = (rgb.width(), rgb.height());
                match engine.recognize_image(&rgb) {
                    Ok(raw_lines) => {
                        let (blocks, dropped) =
                            group_lines_into_blocks(&raw_lines, min_conf, min_chars);
                        send(WorkerMsg::Log(format!(
                            "OK   {name}: {} kept, {} filtered",
                            blocks.len(),
                            dropped.len()
                        )));
                        data.images.push(crate::model::ImageEntry {
                            file: name,
                            width: w,
                            height: h,
                            raw_lines,
                            blocks,
                            dropped,
                        });
                    }
                    Err(e) => send(WorkerMsg::Log(format!("FAIL {name}: {e}"))),
                }
            }
            Err(e) => send(WorkerMsg::Log(format!("SKIP {name}: unreadable image ({e})"))),
        }
    }
    // Persist captions.json next to the input images (Python behaviour).
    if let Ok(s) = serde_json::to_string_pretty(&data) {
        let _ = std::fs::write(input_dir.join("captions.json"), s);
    }
    send(WorkerMsg::ExtractDone { data, cancelled });
}

fn render_worker(
    input_dir: PathBuf,
    output_dir: PathBuf,
    data: ExtractedData,
    font: Option<String>,
    tx: Sender<WorkerMsg>,
    cancel: Arc<AtomicBool>,
) {
    use rayon::prelude::*;
    if let Err(e) = std::fs::create_dir_all(&output_dir) {
        let _ = tx.send(WorkerMsg::Status(format!("Render error: {e}")));
        let _ = tx.send(WorkerMsg::RenderDone { rendered: 0, copied: 0, failed: 0, cancelled: false });
        return;
    }
    let total = data.images.len();
    let counts = std::sync::Mutex::new((0usize, 0usize, 0usize)); // rendered, copied, failed
    let font_ref = font.as_deref();
    let progress_tx = tx.clone();
    let completed = std::sync::atomic::AtomicUsize::new(0);

    data.images.par_iter().for_each(|entry| {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        let name = &entry.file;
        let src = input_dir.join(name);
        let dst = output_dir.join(name);
        let outcome: Result<&'static str, String> = (|| {
            if !src.exists() {
                return Err("missing source file".to_string());
            }
            if !entry.blocks.iter().any(|b| !b.new_text.trim().is_empty()) {
                std::fs::copy(&src, &dst).map_err(|e| e.to_string())?;
                return Ok("copied");
            }
            let img = load_image(&src).map_err(|e| e.to_string())?;
            let rendered = render_entry(&img, entry, font_ref);
            save_image(&rendered, &dst).map_err(|e| e.to_string())?;
            Ok("rendered")
        })();
        match outcome {
            Ok("rendered") => counts.lock().unwrap().0 += 1,
            Ok("copied") => counts.lock().unwrap().1 += 1,
            Ok(_) => {}
            Err(e) => {
                counts.lock().unwrap().2 += 1;
                let _ = tx.send(WorkerMsg::Log(format!("FAIL {name}: {e}")));
            }
        }
        let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = progress_tx.send(WorkerMsg::Progress {
            done,
            total,
            label: format!("Rendering ({done}/{total}): {name}"),
        });
    });

    let (rendered, copied, failed) = *counts.lock().unwrap();
    let cancelled = cancel.load(Ordering::SeqCst);
    let _ = tx.send(WorkerMsg::RenderDone { rendered, copied, failed, cancelled });
}

// ---------------------------------------------------------------------------
// eframe plumbing
// ---------------------------------------------------------------------------

impl eframe::App for CaptionStudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // eframe 0.37 calls `ui` (not `update`); panels are shown inside
        // the root `Ui`. Clone the context up front so the borrow of `ui`
        // is free for the panels below.
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) {
            self.save_settings();
        }
        self.drain_messages();
        if self.busy {
            ctx.request_repaint();
        }

        // --- Confirm-copy-originals modal (Python askYesNo) ---
        if self.confirm_copy_originals {
            let mut open = true;
            egui::Window::new("No Text Changes")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(&ctx, |ui| {
                    ui.label("No caption differs from the original. Copy originals anyway?");
                    ui.horizontal(|ui| {
                        if ui.button("Yes, copy originals").clicked() {
                            self.confirm_copy_originals = false;
                            if let Some(data) = self.pending_render.take() {
                                self.launch_render(data);
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.confirm_copy_originals = false;
                            self.pending_render = None;
                        }
                    });
                });
            if !open {
                self.confirm_copy_originals = false;
                self.pending_render = None;
            }
        }

        // --- Status bar ---
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(egui::ProgressBar::new(self.progress).show_percentage());
                });
            });
        });

        // --- Left column ---
        egui::Panel::left("left_panel")
            .resizable(true)
            .default_size(370.0)
            .show(ui, |ui| {
                ui.heading("Caption Studio");
                ui.label("Batch Image Translator & OCR Verifier");
                if let Some(e) = &self.ocr_error {
                    ui.colored_label(egui::Color32::RED, format!("OCR error: {e}"));
                } else if !self.ocr_ready {
                    ui.label("Warming up OCR engine…");
                }
                ui.separator();

                egui::CollapsingHeader::new("1. Folders & Font").default_open(true).show(ui, |ui| {
                    ui.label("Input Folder:");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.input_dir);
                        if ui.button("Browse").clicked() {
                            self.browse_input();
                        }
                    });
                    ui.label("Output Folder:");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.output_dir);
                        if ui.button("Browse").clicked() {
                            self.browse_output();
                        }
                    });
                    ui.label("Custom Font (.ttf / .otf, optional):");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.font_path);
                        if ui.button("Browse").clicked() {
                            self.browse_font();
                        }
                    });
                });

                egui::CollapsingHeader::new("OCR Threshold Tuning").default_open(true).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("OCR confidence threshold:");
                        ui.add(
                            egui::DragValue::new(&mut self.conf)
                                .range(0.10..=0.95)
                                .speed(0.05),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Ignore unboxed text < chars:");
                        ui.add(egui::DragValue::new(&mut self.minchars).range(0..=60).speed(1));
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Apply Thresholds (Instant)").clicked() {
                            self.reapply_thresholds();
                        }
                        if ui.button("Inspect OCR Boxes").clicked() {
                            self.preview_mode = PreviewMode::Ocr;
                            self.tab = Tab::Preview;
                            self.preview_key.clear();
                        }
                    });
                });

                egui::CollapsingHeader::new("2. Batch Workflow").default_open(true).show(ui, |ui| {
                    ui.add_enabled_ui(!self.busy, |ui| {
                        if ui.button("Scan & Extract Text").clicked() {
                            self.start_extract();
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Save captions.json").clicked() {
                                self.save_full_json();
                            }
                            if ui.button("Load captions.json").clicked() {
                                self.load_full_json();
                            }
                        });
                        if ui.button("Render Translated Images").clicked() {
                            self.start_render();
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Live Preview").clicked() {
                            self.tab = Tab::Preview;
                        }
                        if ui.button("Open Output").clicked() {
                            self.open_output_folder();
                        }
                        if ui.button("Stop").clicked() {
                            self.cancel.store(true, Ordering::SeqCst);
                        }
                    });
                });

                egui::CollapsingHeader::new("Activity Log").default_open(true).show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for line in &self.log_lines {
                                ui.monospace(line);
                            }
                        });
                });
            });

        // --- Right column: tabbed workspace ---
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Json, "Translation JSON");
                ui.selectable_value(&mut self.tab, Tab::Preview, "Live Preview & OCR Check");
            });
            ui.separator();
            match self.tab {
                Tab::Json => {
                    ui.horizontal(|ui| {
                        if ui.button("Copy JSON for AI").clicked() {
                            self.copy_compact_for_ai();
                        }
                        if ui.button("Paste Translated JSON").clicked() {
                            self.paste_clipboard();
                        }
                        if ui.button("Import .json File...").clicked() {
                            self.load_translated_file();
                        }
                        if ui.button("Verify OCR / Preview ->").clicked() {
                            self.tab = Tab::Preview;
                        }
                    });
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let resp = ui.add(
                            egui::TextEdit::multiline(&mut self.editor_text)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .desired_rows(30),
                        );
                        if resp.changed() {
                            self.preview_key.clear();
                        }
                    });
                }
                Tab::Preview => {
                    self.refresh_preview(&ctx);
                    ui.horizontal(|ui| {
                        let names = self.file_names();
                        if ui.button("◀").clicked() {
                            step_preview(&mut self.preview_file, &names, -1);
                            self.preview_key.clear();
                        }
                        egui::ComboBox::from_id_salt("preview_file")
                            .selected_text(&self.preview_file)
                            .show_ui(ui, |ui| {
                                for n in &names {
                                    ui.selectable_value(&mut self.preview_file, n.clone(), n);
                                }
                            });
                        if ui.button("▶").clicked() {
                            step_preview(&mut self.preview_file, &names, 1);
                            self.preview_key.clear();
                        }
                        ui.radio_value(&mut self.preview_mode, PreviewMode::Rendered, "Rendered");
                        ui.radio_value(&mut self.preview_mode, PreviewMode::Ocr, "OCR Boxes");
                        ui.radio_value(&mut self.preview_mode, PreviewMode::Original, "Original");
                    });
                    // Image area
                    let avail = ui.available_size();
                    if let Some(tex) = &self.preview_texture {
                        let (iw, ih) = (self.preview_size[0], self.preview_size[1]);
                        let scale = (avail.x / iw).min(avail.y / ih).min(1.5).max(0.05);
                        let size = egui::Vec2::new(iw * scale, ih * scale);
                        egui::ScrollArea::both().show(ui, |ui| {
                            ui.image((tex.id(), size));
                        });
                    } else {
                        ui.label("Run 'Scan & Extract Text' or paste JSON,\nthen switch here to verify OCR boxes or rendered captions.");
                    }
                    egui::CollapsingHeader::new("Detected Blocks Check (Green = Kept in OCR Boxes | Orange = Filtered Out)").default_open(true).show(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                            ui.monospace(&self.verify_text);
                        });
                    });
                }
            }
        });
    }

}

fn step_preview(current: &mut String, names: &[String], delta: i32) {
    if names.is_empty() {
        return;
    }
    let idx = names.iter().position(|n| n == current).unwrap_or(0) as i32;
    let next = (idx + delta).rem_euclid(names.len() as i32) as usize;
    *current = names[next].clone();
}

pub fn run_app() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1160.0, 680.0])
            .with_min_inner_size([920.0, 550.0])
            .with_title("Caption Studio — Batch Image Translator & OCR Verifier"),
        ..Default::default()
    };
    eframe::run_native(
        "Caption Studio — Batch Image Translator & OCR Verifier",
        options,
        Box::new(|cc| Ok(Box::new(CaptionStudioApp::new(cc)))),
    )
}
