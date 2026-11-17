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

/// How loud a log line is. The Activity Log renders each level in its own
/// colour (info = plain text, warning = the warning token, error = danger),
/// so severity is visible at a glance instead of living in a `WARN ` prefix
/// that wraps onto its own line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogLevel {
    Info,
    Warn,
    Error,
}

/// One Activity Log line: its text plus the level that decides its colour.
#[derive(Debug, Clone)]
struct LogLine {
    text: String,
    level: LogLevel,
}

impl LogLine {
    fn info(text: String) -> Self {
        LogLine { text, level: LogLevel::Info }
    }
    fn warn(text: String) -> Self {
        LogLine { text, level: LogLevel::Warn }
    }
    fn error(text: String) -> Self {
        LogLine { text, level: LogLevel::Error }
    }
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
    sizing: crate::model::SizingMode,

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
    /// The block id selected in the Detected Blocks list (or by clicking
    /// its box in the OCR Boxes view). Kept as an id, not an index, so it
    /// survives re-sorts; cleared whenever the preview file changes.
    selected_block: Option<usize>,
    /// Set when a click on the image selects a block, so the list scrolls
    /// the row into view on the next frame.
    blocks_scroll_to_selected: bool,
    /// The entry the preview was built from — the Detected Blocks rows and
    /// the image click-to-select read it, instead of re-resolving JSON.
    preview_entry: Option<crate::model::ImageEntry>,
    /// Where the preview image actually landed on screen + the scale it was
    /// drawn at, so an image click maps back to image pixels.
    preview_image_rect: egui::Rect,
    preview_image_scale: f32,
    /// Filter text for the preview-file dropdown.
    preview_filter: String,
    /// Focus the filter field the first frame the dropdown opens.
    preview_filter_focus: bool,
    /// Status-bar counts for the dataset the preview shows.
    preview_file_count: usize,
    preview_untranslated: usize,
    /// The editor JSON's parse error, if it failed to resolve.
    preview_json_error: Option<String>,
    log_lines: Vec<LogLine>,
    status: String,
    progress: f32,
    busy: bool,
    cancel: Arc<AtomicBool>,
    confirm_copy_originals: bool,
    pending_render: Option<ExtractedData>,
    /// The left panel's width, restored from settings and kept in sync as
    /// the user drags the divider (saved with the rest of the settings).
    panel_width: f32,

    tx: Sender<WorkerMsg>,
    rx: Receiver<WorkerMsg>,
}

impl CaptionStudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Craft-style UI chrome (ported theme/font/icon patterns): Plex +
        // Chakra Petch, Midnight tokens, and the Lucide SVG loader.
        // The caption *rendering* fonts (fonts.rs) are untouched.
        crate::theme::install_fonts(&cc.egui_ctx);
        crate::theme::apply(&cc.egui_ctx, crate::theme::ThemeKind::Studio, false);
        egui_extras::install_image_loaders(&cc.egui_ctx);

        let settings = Settings::load();
        let (tx, rx) = unbounded::<WorkerMsg>();
        let app = CaptionStudioApp {
            input_dir: settings.input,
            output_dir: settings.output,
            font_path: settings.font,
            conf: settings.conf,
            minchars: settings.minchars,
            sizing: settings.sizing,
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
            selected_block: None,
            blocks_scroll_to_selected: false,
            preview_entry: None,
            preview_image_rect: egui::Rect::NOTHING,
            preview_image_scale: 1.0,
            preview_filter: String::new(),
            preview_filter_focus: false,
            preview_file_count: 0,
            preview_untranslated: 0,
            preview_json_error: None,
            log_lines: Vec::new(),
            status: "Choose an input folder, then click Scan & Extract Text.".to_string(),
            progress: 0.0,
            busy: false,
            cancel: Arc::new(AtomicBool::new(false)),
            confirm_copy_originals: false,
            pending_render: None,
            panel_width: settings.panel_width,
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
        self.log_at(LogLine::info(msg));
    }

    fn log_warn(&mut self, msg: String) {
        self.log_at(LogLine::warn(msg));
    }

    fn log_error(&mut self, msg: String) {
        self.log_at(LogLine::error(msg));
    }

    fn log_at(&mut self, line: LogLine) {
        self.log_lines.push(line);
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
            sizing: self.sizing,
            panel_width: self.panel_width,
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
            self.status = "Nothing to copy — Scan & Extract Text first or load a JSON file.".into();
            return;
        }
        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(self.editor_text.clone())) {
            Ok(()) => self.status = "Copied JSON to clipboard — paste it into your AI translator.".into(),
            Err(e) => self.status = format!("Couldn't copy to the clipboard ({e}) — try again."),
        }
    }

    fn paste_clipboard(&mut self) {
        match arboard::Clipboard::new().and_then(|mut cb| cb.get_text()) {
            Ok(clip) => {
                self.editor_text = clip;
                self.preview_key.clear();
                self.status = "Pasted translated JSON from clipboard. Switch to Live Preview or Render.".into();
            }
            Err(_) => self.status = "Clipboard empty — copy the translated JSON first.".into(),
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
                Err(e) => self.status = format!("Couldn't save ({e}) — check the folder and try again."),
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
                Err(e) => self.status = format!("Couldn't load that JSON ({e}) — check the file and try again."),
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
                Err(e) => self.status = format!("Couldn't read that file ({e}) — check the file and try again."),
            }
        }
    }

    fn open_output_folder(&mut self) {
        let out = self.output_dir.trim();
        if out.is_empty() || !Path::new(out).exists() {
            self.status = "Output folder doesn't exist yet — render an image first.".into();
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
            self.status = format!("Couldn't open the folder ({e}) — check the path and try again.");
        }
    }

    // ------------------------------------------------------------ workers

    fn start_extract(&mut self) {
        let in_dir = self.input_dir.trim().to_string();
        if in_dir.is_empty() || !Path::new(&in_dir).is_dir() {
            self.status = "Select an input folder first.".into();
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
        self.status = "Starting scan…".into();
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
            self.status = "Select an input folder first.".into();
            return;
        }
        if out_dir.is_empty() {
            self.status = "Select an output folder first.".into();
            return;
        }
        if Path::new(&out_dir) == Path::new(&in_dir) {
            self.status = "Output folder must differ from Input folder, or renders would overwrite your originals.".into();
            return;
        }
        let (data, warnings, count) = match self.resolve_editor() {
            Ok(v) => v,
            Err(e) => {
                self.status = format!("Couldn't parse the JSON — {e}. Fix the marked spot and try again.");
                return;
            }
        };
        for w in &warnings {
            // The warning text already says what happened ("Skipped …") and
            // the level paints it amber — no `WARN ` prefix to orphan onto
            // its own line when the filename wraps.
            self.log_warn(w.clone());
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
        let sizing = self.sizing;
        std::thread::spawn(move || {
            render_worker(in_dir, out_dir, data, font, sizing, tx, cancel);
        });
    }

    // ------------------------------------------------------------ preview

    /// Resolve the current editor + extracted data, pick the selected file's
    /// entry, and rebuild the verify text + preview texture when the cache
    /// key changed (Python `_refresh_inline_preview`).
    /// Switch the preview file, clearing anything tied to the old file
    /// (selection + the rendered texture, so the next frame re-renders).
    fn set_preview_file(&mut self, name: String) {
        self.preview_file = name;
        self.selected_block = None;
        self.preview_key.clear();
    }

    /// Resolve the dataset the preview shows — the editor JSON when pasted,
    /// else the extracted scan — and refresh the status-bar counts + JSON
    /// validity. Returns None when there is nothing to preview.
    fn resolve_preview_data(&mut self) -> Option<crate::model::ExtractedData> {
        if self.editor_text.trim().is_empty() {
            self.preview_json_error = None;
            return match &self.extracted {
                Some(d) => {
                    let (f, u) = counts_for(d);
                    self.preview_file_count = f;
                    self.preview_untranslated = u;
                    Some(d.clone())
                }
                None => {
                    self.preview_file_count = 0;
                    self.preview_untranslated = 0;
                    None
                }
            };
        }
        match self.resolve_editor() {
            Ok((d, _, _)) => {
                let (f, u) = counts_for(&d);
                self.preview_file_count = f;
                self.preview_untranslated = u;
                self.preview_json_error = None;
                Some(d)
            }
            Err(e) => {
                self.preview_json_error = Some(e.to_string());
                self.preview_file_count = 0;
                self.preview_untranslated = 0;
                None
            }
        }
    }

    fn refresh_preview(&mut self, ctx: &egui::Context) {
        // Owned: the resolution below borrows `self` mutably, so the input
        // dir can't stay borrowed from `self` across it.
        let in_dir = self.input_dir.trim().to_string();
        if in_dir.is_empty() || !Path::new(&in_dir).is_dir() {
            self.verify_text = "Select a valid Input Folder on the left to preview images.".into();
            self.preview_entry = None;
            self.preview_file_count = 0;
            self.preview_untranslated = 0;
            self.preview_json_error = None;
            return;
        }
        let Some(data) = self.resolve_preview_data() else {
            self.preview_entry = None;
            self.verify_text = match &self.preview_json_error {
                Some(e) => format!(
                    "Couldn't preview the JSON — {e}. Fix the marked spot and try again."
                ),
                None => "Scan & Extract Text or paste translated JSON to preview.".into(),
            };
            return;
        };
        if data.images.is_empty() {
            self.verify_text = "No images found in extracted metadata.".into();
            self.preview_entry = None;
            return;
        }
        if !data.images.iter().any(|e| e.file == self.preview_file) {
            let mut names: Vec<&str> = data.images.iter().map(|e| e.file.as_str()).collect();
            names.sort();
            if let Some(first) = names.first() {
                self.set_preview_file(first.to_string());
            }
        }
        let Some(entry) = data.entry(&self.preview_file).cloned() else {
            self.preview_entry = None;
            return;
        };
        self.preview_entry = Some(entry.clone());
        self.verify_text = verify_text_for(&entry);

        let img_path = PathBuf::from(&in_dir).join(&entry.file);
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
            "{}|{mtime}|{:?}|{}|{blocks_json}|{dropped_json}|{:?}",
            img_path.display(),
            self.preview_mode,
            self.font_path,
            self.selected_block
        );
        if key == self.preview_key && self.preview_texture.is_some() {
            return;
        }
        let img = match load_image(&img_path) {
            Ok(i) => i,
            Err(_) => {
                self.verify_text = format!("Couldn't read {} — it may have moved or be corrupt.", entry.file);
                return;
            }
        };
        let rendered = match self.preview_mode {
            PreviewMode::Original => img,
            PreviewMode::Ocr => render_ocr_diagnostic(&img, &entry, self.selected_block),
            PreviewMode::Rendered => {
                let font = if self.font_path.trim().is_empty() {
                    None
                } else {
                    Some(self.font_path.trim())
                };
                render_entry(&img, &entry, font, self.sizing)
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

    /// Structured Detected Blocks rows: one row per kept block (conf chip +
    /// OCR text, dim, → NEW text when translated), dropped rows faint and
    /// not selectable. Clicking a row selects the block and re-renders the
    /// preview with its box in Max Blue.
    fn show_block_rows(&mut self, ui: &mut egui::Ui, entry: &crate::model::ImageEntry) {
        use egui::text::{LayoutJob, TextFormat};
        let t = crate::theme::Tokens::get(ui.ctx());
        let font = egui::FontId::monospace(crate::theme::CODE);
        // Confidence at/above 0.90 reads as kept-and-sure (diagnostic green);
        // below it the row carries the theme's amber warning instead.
        let conf_green = egui::Color32::from_rgb(34, 197, 94);
        for blk in &entry.blocks {
            let mut job = LayoutJob::default();
            let conf_col = if blk.confidence >= 0.90 { conf_green } else { t.warning };
            job.append(
                &format!("#{} ", blk.id),
                0.0,
                TextFormat { font_id: font.clone(), color: t.accent_ink, ..Default::default() },
            );
            job.append(
                &format!("{:.2} ", blk.confidence),
                0.0,
                TextFormat { font_id: font.clone(), color: conf_col, ..Default::default() },
            );
            job.append(
                &format!("OCR: \"{}\"", blk.original_text),
                0.0,
                TextFormat { font_id: font.clone(), color: t.text_dim, ..Default::default() },
            );
            let new_t = blk.new_text.trim();
            if !new_t.is_empty() && new_t != blk.original_text.trim() {
                job.append(
                    &format!("  →  NEW: \"{}\"", new_t),
                    0.0,
                    TextFormat { font_id: font.clone(), color: t.text, ..Default::default() },
                );
            } else if new_t.is_empty() {
                job.append(
                    "  · untranslated",
                    0.0,
                    TextFormat { font_id: font.clone(), color: t.text_faint, ..Default::default() },
                );
            }
            let selected = self.selected_block == Some(blk.id);
            let resp = ui.selectable_label(selected, job);
            if selected && self.blocks_scroll_to_selected {
                resp.scroll_to_me(Some(egui::Align::Center));
                self.blocks_scroll_to_selected = false;
            }
            if resp.clicked() {
                self.selected_block = Some(blk.id);
                self.preview_key.clear();
            }
        }
        for drop in &entry.dropped {
            let mut job = LayoutJob::default();
            job.append(
                &format!("[FILTERED · {}] \"{}\"", drop.reason, drop.text),
                0.0,
                TextFormat { font_id: font.clone(), color: t.text_faint, ..Default::default() },
            );
            ui.label(job);
        }
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
                        self.log("OCR engine ready.".into());
                        if self.pending_extract {
                            self.pending_extract = false;
                            self.start_extract();
                        }
                    }
                    Err(e) => {
                        self.ocr_error = Some(e.clone());
                        self.status = format!("OCR engine failed to start — restart the app to retry. ({e})");
                        self.log_error(format!("OCR engine failed to start: {e}"));
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
                    let summary = format!("{prefix}rendered {rendered}, copied {copied} unchanged, failed {failed}");
                    self.log(summary.clone());
                    // The leaf folder name only — the full output path
                    // crowded the status bar against the progress bar, and
                    // the folder is one click away via "Open Output Folder".
                    let leaf = Path::new(&self.output_dir)
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| self.output_dir.clone());
                    self.status = format!("{summary}. Saved to {leaf}.");
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
        send(WorkerMsg::Status("No supported images found — add .jpg, .png, or .webp files.".into()));
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
    sizing: crate::model::SizingMode,
    tx: Sender<WorkerMsg>,
    cancel: Arc<AtomicBool>,
) {
    use rayon::prelude::*;
    if let Err(e) = std::fs::create_dir_all(&output_dir) {
        let _ = tx.send(WorkerMsg::Status(format!("Couldn't start the render ({e}) — check the output folder and try again.")));
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
            let rendered = render_entry(&img, entry, font_ref, sizing);
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
        // While it is open the panels behind it are disabled (a focus trap:
        // egui windows are non-modal, so without this Tab and clicks escape
        // to the panels) and Escape dismisses it.
        if self.confirm_copy_originals {
            let mut open = true;
            egui::Window::new("Nothing translated")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(&ctx, |ui| {
                    ui.label("Every caption matches the original, so rendering would just put the original text back on the images. Render them anyway?");
                    ui.horizontal(|ui| {
                        let yes = ui.button("Render anyway");
                        focus_ring(ui, &yes);
                        if yes.clicked() {
                            self.confirm_copy_originals = false;
                            if let Some(data) = self.pending_render.take() {
                                self.launch_render(data);
                            }
                        }
                        let cancel = ui.button("Go back");
                        focus_ring(ui, &cancel);
                        if cancel.clicked() {
                            self.confirm_copy_originals = false;
                            self.pending_render = None;
                        }
                    });
                });
            if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.confirm_copy_originals = false;
                self.pending_render = None;
            }
        }
        let modal_open = self.confirm_copy_originals;

        // --- Status bar ---
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.add_enabled_ui(!modal_open, |ui| {
            ui.horizontal(|ui| {
                // A polite live region: a screen reader announces the status
                // when it changes (batch finished, error) instead of the user
                // having to go looking for it. The label truncates with an
                // ellipsis so a long path can't silently clip against the bar.
                let status_resp = ui.add(egui::Label::new(&self.status).truncate());
                status_resp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Status, true, self.status.clone())
                });
                ui.ctx().accesskit_node_builder(status_resp.id, |node| {
                    node.set_live(egui::accesskit::Live::Polite);
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // A solid accent fill, not the 17%-alpha wash (1.23:1
                    // against its track — the bar read as empty at 100%),
                    // and capped at ~220px so it reads as a bar instead of a
                    // second status line.
                    let t = crate::theme::Tokens::get(ui.ctx());
                    ui.add(
                        egui::ProgressBar::new(self.progress)
                            .fill(t.accent)
                            .desired_width(220.0)
                            .show_percentage(),
                    );
                    // Counts for the dataset the preview shows, left of the
                    // bar: file count + how many blocks still have no
                    // translation. A JSON error replaces the counts (danger
                    // colour, full error on hover) so a broken paste is
                    // visible without switching tabs.
                    if self.preview_file_count > 0 || self.preview_json_error.is_some() {
                        match &self.preview_json_error {
                            Some(e) => {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new("JSON error").color(t.danger),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(e.clone());
                            }
                            None => {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(format!(
                                            "{} files · {} untranslated",
                                            self.preview_file_count, self.preview_untranslated
                                        ))
                                        .color(t.text_dim),
                                    )
                                    .truncate(),
                                );
                            }
                        }
                    }
                });
            });
            });
        });

        // --- Left column ---
        let panel_resp = egui::Panel::left("left_panel")
            .resizable(true)
            .default_size(self.panel_width)
            .show(ui, |ui| {
            ui.add_enabled_ui(!modal_open, |ui| {
                // No outer ScrollArea: the fixed sections fit (tuning starts
                // collapsed, ~470px against ~990px at the default size), so
                // the Activity Log below can take the remaining panel height
                // instead of living in a fixed 220px box with dead space
                // under it.
                // Title bar: the VMAX interlock mark (32px wide, the kit's
                // minimum on-screen size) beside the app title in Chakra
                // Petch, with the subtitle below. The mark is tinted with
                // the text colour, so it reads white on Midnight and black
                // on Daylight.
                ui.horizontal(|ui| {
                    let t = crate::theme::Tokens::get(ui.ctx());
                    ui.add(crate::icons::image_sized("vmax-mark", egui::vec2(32.0, 24.0), t.text));
                    ui.label(crate::theme::heading("Caption Studio", crate::theme::SECTION));
                });
                ui.label(crate::theme::label_text("Batch Image Translator & OCR Verifier", crate::theme::SMALL));
                if let Some(e) = &self.ocr_error {
                    ui.colored_label(crate::theme::Tokens::get(ui.ctx()).danger, format!("OCR failed to start — restart the app to retry. ({e})"));
                } else if !self.ocr_ready {
                    ui.label("Warming up OCR engine…");
                }
                ui.separator();

                egui::CollapsingHeader::new("Folders & Font").default_open(true).show(ui, |ui| {
                    let input_label = ui.label("Input Folder:").id;
                    ui.horizontal(|ui| {
                        let resp = ui.text_edit_singleline(&mut self.input_dir).labelled_by(input_label);
                        let path = self.input_dir.clone();
                        if !path.is_empty() {
                            resp.on_hover_text(path);
                        }
                        if crate::icons::button(ui, "folder", crate::theme::ControlSize::Small, false, "Browse for input folder").clicked() {
                            self.browse_input();
                        }
                    });
                    let output_label = ui.label("Output Folder:").id;
                    ui.horizontal(|ui| {
                        let resp = ui.text_edit_singleline(&mut self.output_dir).labelled_by(output_label);
                        let path = self.output_dir.clone();
                        if !path.is_empty() {
                            resp.on_hover_text(path);
                        }
                        if crate::icons::button(ui, "folder", crate::theme::ControlSize::Small, false, "Browse for output folder").clicked() {
                            self.browse_output();
                        }
                    });
                    let font_label = ui.label("Custom Font (.ttf / .otf / .ttc, optional):").id;
                    ui.horizontal(|ui| {
                        let resp = ui.text_edit_singleline(&mut self.font_path).labelled_by(font_label);
                        let path = self.font_path.clone();
                        if !path.is_empty() {
                            resp.on_hover_text(path);
                        }
                        if crate::icons::button(ui, "folder", crate::theme::ControlSize::Small, false, "Browse for font file").clicked() {
                            self.browse_font();
                        }
                    });
                    ui.horizontal(|ui| {
                        let sizing_label = ui.label("Font sizing:").id;
                        let mut changed = false;
                        let combo = egui::ComboBox::from_id_salt("sizing_mode")
                            .selected_text(self.sizing.label())
                            .show_ui(ui, |ui| {
                                for mode in crate::model::SizingMode::all() {
                                    if ui
                                        .selectable_value(&mut self.sizing, mode, mode.label())
                                        .changed()
                                    {
                                        changed = true;
                                    }
                                }
                            });
                        combo.response.labelled_by(sizing_label);
                        if changed {
                            self.save_settings();
                            self.preview_key.clear();
                        }
                    });
                });

                egui::CollapsingHeader::new("OCR Threshold Tuning").default_open(false).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let conf_label = ui.label("OCR confidence threshold:").id;
                        ui.add(
                            egui::DragValue::new(&mut self.conf)
                                .range(0.10..=0.95)
                                .speed(0.05),
                        )
                        .labelled_by(conf_label);
                    });
                    ui.horizontal(|ui| {
                        let min_label = ui.label("Minimum length for unboxed text (chars):").id;
                        ui.add(egui::DragValue::new(&mut self.minchars).range(0..=60).speed(1))
                            .labelled_by(min_label);
                    });
                    ui.horizontal(|ui| {
                        if icon_text_button(ui, "check", "Apply Thresholds (Instant)").clicked() {
                            self.reapply_thresholds();
                        }
                        if icon_text_button(ui, "search", "Inspect OCR Boxes").clicked() {
                            self.preview_mode = PreviewMode::Ocr;
                            self.tab = Tab::Preview;
                            self.preview_key.clear();
                        }
                    });
                });

                egui::CollapsingHeader::new("Batch Workflow").default_open(true).show(ui, |ui| {
                    ui.add_enabled_ui(!self.busy, |ui| {
                        if primary_icon_text_button(ui, "scan", "Scan & Extract Text", true).clicked() {
                            self.start_extract();
                        }
                        ui.horizontal(|ui| {
                            if icon_text_button(ui, "save", "Save captions.json").clicked() {
                                self.save_full_json();
                            }
                            if icon_text_button(ui, "folder-open", "Load captions.json").clicked() {
                                self.load_full_json();
                            }
                        });
                        if primary_icon_text_button(ui, "play", "Render Translated Images", true).clicked() {
                            self.start_render();
                        }
                    });
                    ui.horizontal(|ui| {
                        if icon_text_button(ui, "eye", "Live Preview & OCR Check").clicked() {
                            self.tab = Tab::Preview;
                        }
                        if icon_text_button(ui, "folder-open", "Open Output Folder").clicked() {
                            self.open_output_folder();
                        }
                        // Stop is only meaningful while a job runs — it stays
                        // disabled at idle so its state tells the truth.
                        ui.add_enabled_ui(self.busy, |ui| {
                            if icon_text_button(ui, "octagon-x", "Stop").clicked() {
                                self.cancel.store(true, Ordering::SeqCst);
                            }
                        });
                    });
                });

                egui::CollapsingHeader::new("Activity Log").default_open(true).show(ui, |ui| {
                    // The log fills the rest of the panel: it sizes to its
                    // content up to the remaining height, then scrolls, always
                    // showing the newest line. Severity is a short coloured
                    // tag that can't wrap away from its message; the message
                    // itself stays in the plain text colour so the log reads
                    // quietly instead of shouting in amber.
                    let t = crate::theme::Tokens::get(ui.ctx());
                    egui::ScrollArea::vertical()
                        .max_height(ui.available_height())
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for line in &self.log_lines {
                                ui.horizontal(|ui| {
                                    match line.level {
                                        LogLevel::Warn => {
                                            ui.colored_label(t.warning, egui::RichText::new("WARN").monospace().small());
                                        }
                                        LogLevel::Error => {
                                            ui.colored_label(t.danger, egui::RichText::new("ERROR").monospace().small());
                                        }
                                        LogLevel::Info => {}
                                    }
                                    ui.monospace(&line.text);
                                });
                            }
                        });
                });
            });
            });
        // Keep the persisted width in sync with the divider the user drags —
        // the saved value is what the next launch restores.
        let w = panel_resp.response.rect.width();
        if (w - self.panel_width).abs() > 0.5 {
            self.panel_width = w;
        }

        // --- Right column: tabbed workspace ---
        // The central workspace sits on `canvas` (the main reading surface),
        // a step above the chrome panels — this is what the `canvas` token is for.
        let canvas = crate::theme::Tokens::get(ui.ctx()).canvas;
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).fill(canvas))
            .show(ui, |ui| {
            ui.add_enabled_ui(!modal_open, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Json, "Translation JSON");
                ui.selectable_value(&mut self.tab, Tab::Preview, "Live Preview & OCR Check");
            });
            ui.separator();
            match self.tab {
                Tab::Json => {
                    ui.horizontal(|ui| {
                        if icon_text_button(ui, "copy", "Copy JSON for AI").clicked() {
                            self.copy_compact_for_ai();
                        }
                        if icon_text_button(ui, "clipboard-paste", "Paste Translated JSON").clicked() {
                            self.paste_clipboard();
                        }
                        if icon_text_button(ui, "file-json", "Import Translated JSON...").clicked() {
                            self.load_translated_file();
                        }
                        if primary_icon_text_button(ui, "arrow-right", "Verify in Preview", false).clicked() {
                            self.tab = Tab::Preview;
                        }
                    });
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        // Syntax-highlighted JSON (keys, strings, numbers,
                        // punctuation, empty strings) with a hint when it is
                        // empty, so the editor reads as a document instead of
                        // a wall of one-colour text.
                        let t = crate::theme::Tokens::get(ui.ctx());
                        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                            let mut job = json_highlight_job(buf.as_str(), &t);
                            job.wrap.max_width = wrap_width;
                            ui.fonts_mut(|f| f.layout_job(job))
                        };
                        let resp = ui.add(
                            egui::TextEdit::multiline(&mut self.editor_text)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .desired_rows(30)
                                .layouter(&mut layouter)
                                .hint_text("Scan images to fill this in, or paste translated JSON here."),
                        )
                        .accessible_name("Translation JSON");
                        if resp.changed() {
                            self.preview_key.clear();
                            // Keep the status-bar counts + JSON validity live
                            // while editing, not just on the Preview tab.
                            self.resolve_preview_data();
                        }
                    });
                }
                Tab::Preview => {
                    self.refresh_preview(&ctx);
                    // File navigation: bigger steppers (Medium, 32px) for a
                    // real touch target, and the file picker is a filterable
                    // dropdown — full names share a long prefix, so the
                    // button shows the short name and the full path on hover.
                    ui.horizontal(|ui| {
                        let names = self.file_names();
                        let shorts = short_file_names(&names);
                        let cur_idx = names.iter().position(|n| n == &self.preview_file).unwrap_or(0);
                        let short_current = shorts
                            .get(cur_idx)
                            .cloned()
                            .unwrap_or_else(|| self.preview_file.clone());
                        if crate::icons::button(ui, "chevron-left", crate::theme::ControlSize::Medium, false, "Previous file").clicked() {
                            step_preview(&mut self.preview_file, &names, -1);
                            self.selected_block = None;
                            self.preview_key.clear();
                        }
                        let full_path = PathBuf::from(self.input_dir.trim())
                            .join(&self.preview_file)
                            .display()
                            .to_string();
                        let btn = ui
                            .button(format!("{short_current} ▾"))
                            .on_hover_text(full_path)
                            .accessible_name(format!("Preview file {}", self.preview_file));
                        if btn.clicked() {
                            self.preview_filter_focus = true;
                        }
                        let mut picked: Option<String> = None;
                        egui::Popup::menu(&btn)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                            .show(|ui| {
                                let filter_resp = ui.add(
                                    egui::TextEdit::singleline(&mut self.preview_filter)
                                        .hint_text("Filter files…")
                                        .desired_width(260.0),
                                );
                                if self.preview_filter_focus {
                                    filter_resp.request_focus();
                                    self.preview_filter_focus = false;
                                }
                                let filtered = filter_file_names(&names, &self.preview_filter);
                                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                                    if filtered.is_empty() {
                                        ui.label(crate::theme::label_text(
                                            "No files match.",
                                            crate::theme::CAPTION,
                                        ));
                                    }
                                    for n in filtered {
                                        let idx = names.iter().position(|x| x == n).unwrap_or(0);
                                        let short = shorts
                                            .get(idx)
                                            .cloned()
                                            .unwrap_or_else(|| n.clone());
                                        let path = PathBuf::from(self.input_dir.trim())
                                            .join(n)
                                            .display()
                                            .to_string();
                                        let resp = ui
                                            .selectable_label(n == &self.preview_file, short)
                                            .on_hover_text(path);
                                        if resp.clicked() {
                                            picked = Some(n.clone());
                                            self.preview_filter.clear();
                                            egui::Popup::close_id(
                                                ui.ctx(),
                                                egui::Popup::default_response_id(&btn),
                                            );
                                        }
                                    }
                                });
                            });
                        if crate::icons::button(ui, "chevron-right", crate::theme::ControlSize::Medium, false, "Next file").clicked() {
                            step_preview(&mut self.preview_file, &names, 1);
                            self.selected_block = None;
                            self.preview_key.clear();
                        }
                        if let Some(name) = picked {
                            self.set_preview_file(name);
                        }
                    });
                    // The mode choice gets its own row as a segmented control,
                    // so the file row isn't cramped and the three modes read
                    // as one choice.
                    ui.horizontal(|ui| {
                        ui.label(crate::theme::label_text("View:", crate::theme::LABEL));
                        ui.selectable_value(&mut self.preview_mode, PreviewMode::Rendered, "Rendered");
                        ui.selectable_value(&mut self.preview_mode, PreviewMode::Ocr, "OCR Boxes");
                        ui.selectable_value(&mut self.preview_mode, PreviewMode::Original, "Original");
                    });
                    // Image left, Detected Blocks right. The old layout
                    // stacked the image above the blocks and the image's
                    // ScrollArea ate the whole height, so the blocks — the
                    // text alternative to the image, and the main
                    // OCR-verification affordance — sat below the fold with
                    // no way to reach them, and a portrait image left half
                    // the panel as dead black space. Side by side, both are
                    // always on screen.
                    let blocks_w = 340.0;
                    ui.horizontal_top(|ui| {
                        let total = ui.available_size();
                        let gap = ui.spacing().item_spacing.x;
                        let image_w = (total.x - blocks_w - gap).max(240.0);
                        // --- Image pane ---
                        ui.allocate_ui_with_layout(
                            egui::vec2(image_w, total.y),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                if let Some(tex) = &self.preview_texture {
                                    let (iw, ih) = (self.preview_size[0], self.preview_size[1]);
                                    let scale = (image_w / iw).min(total.y / ih).min(1.5).max(0.05);
                                    let size = egui::Vec2::new(iw * scale, ih * scale);
                                    // Centre the image in its pane when it is
                                    // smaller than the pane; when it is larger
                                    // the offsets are zero and the ScrollArea
                                    // lets the user pan it.
                                    let x_off = ((image_w - size.x) / 2.0).max(0.0);
                                    let y_off = ((total.y - size.y) / 2.0).max(0.0);
                                    egui::ScrollArea::both().show(ui, |ui| {
                                        ui.add_space(y_off);
                                        ui.horizontal(|ui| {
                                            ui.add_space(x_off);
                                            // An accessible name for the image
                                            // (filename + "rendered preview"):
                                            // a screen reader otherwise gets
                                            // an unnamed texture, and the
                                            // Detected Blocks list beside it
                                            // is the text alternative.
                                            // Click sense (the Image widget
                                            // defaults to hover-only): in OCR
                                            // Boxes mode a click selects the
                                            // block under the pointer — the
                                            // list scrolls to its row and the
                                            // box turns Max Blue.
                                            let img_resp = ui
                                                .add(
                                                    egui::Image::new((tex.id(), size))
                                                        .sense(egui::Sense::click()),
                                                )
                                                .accessible_name(format!(
                                                    "{} rendered preview",
                                                    self.preview_file
                                                ));
                                            self.preview_image_rect = img_resp.rect;
                                            self.preview_image_scale = scale;
                                            if img_resp.clicked()
                                                && matches!(self.preview_mode, PreviewMode::Ocr)
                                            {
                                                if let (Some(pos), Some(entry)) = (
                                                    img_resp.interact_pointer_pos(),
                                                    &self.preview_entry,
                                                ) {
                                                    let x = (pos.x - self.preview_image_rect.min.x)
                                                        / self.preview_image_scale;
                                                    let y = (pos.y - self.preview_image_rect.min.y)
                                                        / self.preview_image_scale;
                                                    self.selected_block = block_at(entry, x, y);
                                                    self.blocks_scroll_to_selected = true;
                                                    self.preview_key.clear();
                                                }
                                            }
                                        });
                                    });
                                } else {
                                    ui.label("Run 'Scan & Extract Text' or paste JSON,\nthen switch here to verify OCR boxes or rendered captions.");
                                }
                            },
                        );
                        // --- Detected Blocks pane ---
                        ui.allocate_ui_with_layout(
                            egui::vec2(blocks_w, total.y),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                // The green/orange legend belongs with the
                                // image it describes (OCR Boxes mode only —
                                // the other modes draw no coloured boxes).
                                if matches!(self.preview_mode, PreviewMode::Ocr) {
                                    ui.label(crate::theme::label_text("Green boxes = kept · Orange boxes = filtered out", crate::theme::CAPTION));
                                }
                                ui.label(crate::theme::heading("Detected Blocks", crate::theme::CARD));
                                if ui.small_button("Copy block list").clicked() {
                                    match arboard::Clipboard::new()
                                        .and_then(|mut cb| cb.set_text(self.verify_text.clone()))
                                    {
                                        Ok(()) => {
                                            self.status = "Block list copied to clipboard.".into()
                                        }
                                        Err(_) => {
                                            self.status =
                                                "Couldn't copy the block list — try again.".into()
                                        }
                                    }
                                }
                                // Structured rows (conf chip + OCR → NEW)
                                // instead of the old monospace blob, so a row
                                // can be clicked to highlight its box. When
                                // there is no entry yet (no scan, no JSON),
                                // the guidance text stays.
                                let entry = self.preview_entry.clone();
                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    match &entry {
                                        Some(entry)
                                            if !entry.blocks.is_empty()
                                                || !entry.dropped.is_empty() =>
                                        {
                                            self.show_block_rows(ui, entry);
                                        }
                                        _ => {
                                            ui.monospace(&self.verify_text);
                                        }
                                    }
                                });
                            },
                        );
                    });
                }
            }
            });
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

/// (file count, untranslated-block count) for a dataset. A block counts as
/// untranslated while its new text is empty — nothing has been entered yet.
fn counts_for(data: &crate::model::ExtractedData) -> (usize, usize) {
    let files = data.images.len();
    let untranslated = data
        .images
        .iter()
        .flat_map(|e| e.blocks.iter())
        .filter(|b| b.new_text.trim().is_empty())
        .count();
    (files, untranslated)
}

/// Short display names for the file dropdown: the longest prefix every name
/// shares is stripped (it carries no information), and anything still longer
/// than 30 chars is middle-truncated so the differing `_N.jpg` suffix stays
/// visible. One file, or an empty remainder, falls back to the full name.
fn short_file_names(names: &[String]) -> Vec<String> {
    if names.len() < 2 {
        return names.to_vec();
    }
    let mut prefix: Vec<char> = names[0].chars().collect();
    for n in &names[1..] {
        let keep = prefix.iter().zip(n.chars()).take_while(|(a, b)| *a == b).count();
        prefix.truncate(keep);
        if prefix.is_empty() {
            break;
        }
    }
    let prefix: String = prefix.into_iter().collect();
    names
        .iter()
        .map(|n| {
            let short = n.strip_prefix(prefix.as_str()).unwrap_or(n.as_str());
            let short = if short.is_empty() { n.as_str() } else { short };
            let chars: Vec<char> = short.chars().collect();
            if chars.len() > 30 {
                let head: String = chars[..13].iter().collect();
                let tail: String = chars[chars.len() - 14..].iter().collect();
                format!("{head}…{tail}")
            } else {
                short.to_string()
            }
        })
        .collect()
}

/// Case-insensitive substring filter for the file dropdown. An empty query
/// matches everything.
fn filter_file_names<'a>(names: &'a [String], query: &str) -> Vec<&'a String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return names.iter().collect();
    }
    names.iter().filter(|n| n.to_lowercase().contains(&q)).collect()
}

/// The id of the first kept block whose box contains the point (image
/// pixels), for click-to-select in the OCR Boxes view. Dropped boxes are
/// not selectable.
fn block_at(entry: &crate::model::ImageEntry, x: f32, y: f32) -> Option<usize> {
    entry
        .blocks
        .iter()
        .find(|b| {
            let [x1, y1, x2, y2] = b.bbox;
            x >= x1 as f32 && x < x2 as f32 && y >= y1 as f32 && y < y2 as f32
        })
        .map(|b| b.id)
}

/// A text button with a Lucide icon to its left, tinted like the label
/// (dimmed when the button is disabled).
fn icon_text_button(ui: &mut egui::Ui, icon: &str, label: &str) -> egui::Response {
    let t = crate::theme::Tokens::get(ui.ctx());
    let tint = if ui.is_enabled() { t.text } else { t.text_faint };
    let image = crate::icons::image(icon, crate::theme::ControlSize::Small.icon(), tint);
    let resp = ui.add(egui::Button::image_and_text(image, label));
    focus_ring(ui, &resp);
    resp
}

/// A primary action: accent fill, white icon and label. The screen's main
/// steps (Scan, Render, "Verify in Preview") get this so they read as the
/// thing to do, not as one more secondary button. `full_width` makes it the
/// panel's full width at the default control height (Scan and Render).
fn primary_icon_text_button(ui: &mut egui::Ui, icon: &str, label: &str, full_width: bool) -> egui::Response {
    let t = crate::theme::Tokens::get(ui.ctx());
    let size = if full_width { crate::theme::ControlSize::Medium } else { crate::theme::ControlSize::Small };
    let tint = if ui.is_enabled() { egui::Color32::WHITE } else { t.text_faint };
    let image = crate::icons::image(icon, size.icon(), tint);
    let button = egui::Button::image_and_text(image, label).fill(t.accent);
    let resp = if full_width {
        ui.add_sized(egui::vec2(ui.available_width(), size.height()), button)
    } else {
        ui.add(button)
    };
    focus_ring(ui, &resp);
    resp
}

/// Draw the keyboard-focus ring around a button, matching the one
/// `icons::button` draws — egui's own buttons have no focus style, so
/// without this keyboard focus is invisible everywhere except icon buttons.
fn focus_ring(ui: &egui::Ui, resp: &egui::Response) {
    if resp.has_focus() {
        let t = crate::theme::Tokens::get(ui.ctx());
        ui.painter().rect_stroke(
            resp.rect.expand(1.0),
            t.radius_sm,
            // `accent_ink`, not `accent`: the indigo ring was 2.44:1 against
            // the field it sits on (below the 3:1 a focus indicator needs);
            // accent_ink is 6.69:1 there and 8.08:1 on chrome.
            egui::Stroke::new(1.5, t.accent_ink),
            egui::StrokeKind::Outside,
        );
    }
}

/// Syntax-highlight the Translation JSON editor: keys in accent_ink, string
/// values in the text colour, numbers/booleans/null in amber, punctuation
/// dimmed, and empty strings (`""`) visibly dimmed so a missing translation
/// reads as empty instead of blending in. Tolerant of invalid JSON (the user
/// is mid-edit) — it tokenizes by shape and never parses. Byte-safe: the
/// structural characters are all ASCII, so scanning bytes for them can never
/// mistake a multi-byte character for one, and spans only ever start and end
/// on character boundaries.
fn json_highlight_job(text: &str, t: &crate::theme::Tokens) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let font = egui::FontId::monospace(crate::theme::CODE);
    let mut job = LayoutJob::default();
    let push = |job: &mut LayoutJob, s: &str, color: egui::Color32| {
        job.append(s, 0.0, TextFormat { font_id: font.clone(), color, ..Default::default() });
    };
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        match bytes[i] {
            b'"' => {
                // A string: find its end, honouring escapes. Multi-byte
                // characters inside can never contain a `"` or `\` byte.
                let start = i;
                i += 1;
                let mut closed = false;
                while i < n {
                    if bytes[i] == b'\\' {
                        i = (i + 2).min(n);
                        continue;
                    }
                    if bytes[i] == b'"' {
                        i += 1;
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                let s = &text[start..i];
                // A key is a string followed (past whitespace) by `:`.
                let mut j = i;
                while j < n && (bytes[j] as char).is_whitespace() {
                    j += 1;
                }
                let is_key = closed && j < n && bytes[j] == b':';
                if s == "\"\"" {
                    push(&mut job, s, t.text_faint);
                } else if is_key {
                    push(&mut job, s, t.accent_ink);
                } else {
                    push(&mut job, s, t.text);
                }
            }
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                push(&mut job, &text[i..i + 1], t.text_faint);
                i += 1;
            }
            b'0'..=b'9' | b'-' => {
                // A number: digits, `.`, `e`/`E`, `+`/`-` after `e`. Only
                // ASCII bytes advance the cursor, so a multi-byte character
                // ends the number without ever splitting it.
                let start = i;
                i += 1;
                while i < n {
                    match bytes[i] {
                        b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-' => i += 1,
                        _ => break,
                    }
                }
                push(&mut job, &text[start..i], t.warning);
            }
            b't' | b'f' | b'n' => {
                let rest = &text[i..];
                let word = if rest.starts_with("true") {
                    "true"
                } else if rest.starts_with("false") {
                    "false"
                } else if rest.starts_with("null") {
                    "null"
                } else {
                    ""
                };
                if word.is_empty() {
                    push(&mut job, &text[i..i + 1], t.text);
                    i += 1;
                } else {
                    push(&mut job, word, t.warning);
                    i += word.len();
                }
            }
            _ => {
                // Whitespace and anything else: plain text, coalesced into
                // one span. Advance by whole characters so the span never
                // ends mid-character.
                let start = i;
                loop {
                    let Some(ch) = text[i..].chars().next() else { break };
                    if matches!(ch, '"' | '{' | '}' | '[' | ']' | ':' | ',' | '-' | 't' | 'f' | 'n') || ch.is_ascii_digit() {
                        break;
                    }
                    i += ch.len_utf8();
                }
                push(&mut job, &text[start..i], t.text);
            }
        }
    }
    job
}

/// The VMAX interlock mark as the window/taskbar icon (white on
/// transparent; the title bar is dark in both themes). If the embedded PNG
/// ever fails to decode, the app still starts, just without an icon.
static VMAX_ICON_PNG: &[u8] = include_bytes!("../assets/icons/vmax-icon-1024.png");

pub fn run_app() -> eframe::Result<()> {
    let icon = eframe::icon_data::from_png_bytes(VMAX_ICON_PNG).unwrap_or_default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1160.0, 680.0])
            .with_min_inner_size([920.0, 550.0])
            .with_icon(icon)
            .with_title("Caption Studio — Batch Image Translator & OCR Verifier"),
        ..Default::default()
    };
    eframe::run_native(
        "Caption Studio — Batch Image Translator & OCR Verifier",
        options,
        Box::new(|cc| Ok(Box::new(CaptionStudioApp::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_highlight_colours_keys_values_numbers_and_empty_strings() {
        let t = crate::theme::Tokens::for_kind(crate::theme::ThemeKind::Studio);
        let text = "{\n  \"meme.png\": {\n    \"0\": \"Hello\",\n    \"1\": \"\",\n    \"2\": 42\n  }\n}";
        let job = json_highlight_job(text, &t);
        // The job covers the whole input, section by section, with no gaps.
        assert_eq!(job.text, text);
        job.debug_sanity_check();
        // `LayoutJob::append` merges adjacent spans that share a format, so
        // a section may be wider than the token (e.g. ` "Hello"`); match by
        // containment, not exact equality.
        let colour_of = |needle: &str| {
            job.sections
                .iter()
                .find(|s| job.text[s.byte_range.start.0..s.byte_range.end.0].contains(needle))
                .map(|s| s.format.color)
        };
        assert_eq!(colour_of("\"meme.png\""), Some(t.accent_ink));
        assert_eq!(colour_of("\"0\""), Some(t.accent_ink));
        assert_eq!(colour_of("\"Hello\""), Some(t.text));
        assert_eq!(colour_of("\"\""), Some(t.text_faint));
        assert_eq!(colour_of("42"), Some(t.warning));
        assert_eq!(colour_of("{"), Some(t.text_faint));
    }

    #[test]
    fn json_highlight_survives_multibyte_text_and_broken_json() {
        let t = crate::theme::Tokens::for_kind(crate::theme::ThemeKind::Studio);
        // Tamil text, an emoji, and an unterminated string (mid-edit).
        let text = "{\"a\": \"வணக்கம் 🎉\", \"b\": \"unterminated";
        let job = json_highlight_job(text, &t);
        assert_eq!(job.text, text);
        job.debug_sanity_check();
    }

    #[test]
    fn short_file_names_strip_the_shared_prefix_and_keep_the_suffix() {
        let names = vec![
            "scene1_frame_01.jpg".to_string(),
            "scene1_frame_02.jpg".to_string(),
            "scene1_frame_10.jpg".to_string(),
        ];
        assert_eq!(short_file_names(&names), vec!["01.jpg", "02.jpg", "10.jpg"]);
        // One file: nothing shared to strip.
        assert_eq!(short_file_names(&names[..1]), vec!["scene1_frame_01.jpg"]);
        // A long remainder is middle-truncated, suffix intact.
        let long = vec![
            "a_very_long_scene_name_alpha_version_extended_cut_01.jpg".to_string(),
            "a_very_long_scene_name_beta_version_extended_cut_01.jpg".to_string(),
        ];
        let shorts = short_file_names(&long);
        assert!(shorts[0].ends_with("01.jpg"));
        assert!(shorts[0].chars().count() <= 30);
    }

    #[test]
    fn filter_file_names_is_case_insensitive_substring() {
        let names = vec!["Alpha_01.jpg".to_string(), "beta_02.jpg".to_string()];
        assert_eq!(filter_file_names(&names, "").len(), 2);
        assert_eq!(filter_file_names(&names, "ALPHA"), vec![&names[0]]);
        assert_eq!(filter_file_names(&names, "02"), vec![&names[1]]);
        assert!(filter_file_names(&names, "zzz").is_empty());
    }

    fn test_block(id: usize, new_text: &str) -> crate::model::Block {
        crate::model::Block {
            id,
            bbox: [10, 20, 50, 60],
            line_bboxes: vec![[10, 20, 50, 60]],
            original_text: "x".into(),
            new_text: new_text.into(),
            confidence: 0.9,
            text_color: [0, 0, 0],
            bg_color: [0, 0, 0],
            is_boxed: true,
            font_size_est: 10,
        }
    }

    #[test]
    fn counts_for_counts_empty_new_text_as_untranslated() {
        let data = crate::model::ExtractedData {
            images: vec![
                crate::model::ImageEntry {
                    file: "a.png".into(),
                    width: 100,
                    height: 100,
                    raw_lines: vec![],
                    blocks: vec![test_block(0, ""), test_block(1, "z")],
                    dropped: vec![],
                },
                crate::model::ImageEntry {
                    file: "b.png".into(),
                    width: 100,
                    height: 100,
                    raw_lines: vec![],
                    blocks: vec![],
                    dropped: vec![],
                },
            ],
        };
        assert_eq!(counts_for(&data), (2, 1));
    }

    #[test]
    fn block_at_finds_the_box_and_ignores_outside_points() {
        let entry = crate::model::ImageEntry {
            file: "a.png".into(),
            width: 100,
            height: 100,
            raw_lines: vec![],
            blocks: vec![test_block(7, "z")],
            dropped: vec![crate::model::DroppedItem {
                bbox: [70, 70, 90, 90],
                text: "d".into(),
                reason: "low-conf".into(),
            }],
        };
        assert_eq!(block_at(&entry, 30.0, 40.0), Some(7));
        assert_eq!(block_at(&entry, 5.0, 5.0), None);
        // The right/bottom edges are exclusive, and dropped boxes are not
        // selectable.
        assert_eq!(block_at(&entry, 50.0, 40.0), None);
        assert_eq!(block_at(&entry, 80.0, 80.0), None);
    }
}
