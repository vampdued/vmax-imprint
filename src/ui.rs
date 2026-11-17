//! Redesign layout (SPEC §3) — header (brand + 4-step stepper + the one
//! primary) / sidebar 316px (folders & font, OCR threshold tuning,
//! Project) / workspace (tabbed: Translation JSON editor and Live Preview
//! & OCR Check — Rendered / OCR Boxes / Original) / inspector 380px
//! (Detected Blocks) / activity log 188px / status bar 32px.
//!
//! OCR is pre-warmed in a background thread at launch. Scanning runs on a
//! worker thread; rendering runs on a worker thread with rayon inside.
//! The UI thread only ever receives progress/log/done messages over a
//! channel, so the interface stays responsive during a batch.

use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::SystemTime;

use anyhow::Result;
use crossbeam_channel::{unbounded, Receiver, Sender};
use egui::{ColorImage, TextureHandle};

use crate::image_io::{is_image_file, load_image, save_image};
use crate::layout::group_lines_into_blocks;
use crate::model::{ExtractedData, Settings};
use crate::ocr::OcrEngine;
use crate::parser::{
    compact_map_from_data, merge_translations, parse_flexible_json_or_dict, resolve_data,
};
use crate::render::{render_entry, render_ocr_diagnostic};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Json,
    Preview,
}

/// The four workflow steps (SPEC §2): Scan → Translate → Review → Render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowStep {
    Scan,
    Translate,
    Review,
    Render,
}

impl WorkflowStep {
    const ALL: [WorkflowStep; 4] = [
        WorkflowStep::Scan,
        WorkflowStep::Translate,
        WorkflowStep::Review,
        WorkflowStep::Render,
    ];

    fn label(self) -> &'static str {
        match self {
            WorkflowStep::Scan => "Scan",
            WorkflowStep::Translate => "Translate",
            WorkflowStep::Review => "Review",
            WorkflowStep::Render => "Render",
        }
    }
}

/// A step's state in the header stepper: done (✓), current (blue), or
/// still to do (numbered).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepState {
    Todo,
    Current,
    Done,
}

/// Everything the header needs to draw the stepper and its primary.
struct WorkflowState {
    current: WorkflowStep,
    states: [StepState; 4],
    subtitles: [String; 4],
    images: usize,
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

/// One Activity Log line (redesign Phase 5, item 15): when it happened
/// (local HH:MM:SS), its severity, the image it is about (when it is about
/// one — a render outcome, a per-file warning), and what happened.
#[derive(Debug, Clone)]
struct LogLine {
    time: String,
    level: LogLevel,
    file: Option<String>,
    result: String,
}

impl LogLine {
    fn new(level: LogLevel, file: Option<String>, result: String) -> Self {
        LogLine {
            time: chrono::Local::now().format("%H:%M:%S").to_string(),
            level,
            file,
            result,
        }
    }

    fn info(result: String) -> Self {
        LogLine::new(LogLevel::Info, None, result)
    }

    fn warn(result: String) -> Self {
        LogLine::new(LogLevel::Warn, None, result)
    }

    fn error(result: String) -> Self {
        LogLine::new(LogLevel::Error, None, result)
    }

    /// A per-file line (a render outcome, a per-file warning).
    fn file(level: LogLevel, file: String, result: String) -> Self {
        LogLine::new(level, Some(file), result)
    }

    /// The line as plain text (for Copy log): `HH:MM:SS LEVEL [file] result`.
    fn to_text(&self) -> String {
        let level = match self.level {
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        };
        match &self.file {
            Some(f) => format!("{} {level} {f} — {}", self.time, self.result),
            None => format!("{} {level} {}", self.time, self.result),
        }
    }
}

/// The log filter (Phase 5, item 15): All lines, or only Warnings
/// (warnings + errors).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogFilter {
    All,
    Warnings,
}

/// The lines the log table shows, newest first, under a filter. Pure so
/// the ordering + filter are unit-tested without building the app.
fn log_visible<'a>(lines: &'a [LogLine], filter: LogFilter) -> Vec<&'a LogLine> {
    lines
        .iter()
        .rev()
        .filter(|l| match filter {
            LogFilter::All => true,
            LogFilter::Warnings => l.level != LogLevel::Info,
        })
        .collect()
}

/// The log panel's next height after a drag-handle move, or `None` when
/// the drag closes the panel. Pure so the resize + drag-to-close rule are
/// unit-tested without building the app. `delta_y` is the pointer's
/// vertical move this frame (up is negative, so dragging up grows the
/// panel). Dragging down past 48px closes it; otherwise the height stays
/// in 64–480px.
fn log_height_after_drag(height: f32, delta_y: f32) -> Option<f32> {
    let new_h = height - delta_y;
    if new_h < 48.0 {
        None
    } else {
        Some(new_h.clamp(64.0, 480.0))
    }
}

/// One file's outcome in a render run (for the file-list status chips).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileOutcome {
    Rendered,
    Copied,
    Failed,
}

enum WorkerMsg {
    Log(String),
    Status(String),
    Progress {
        done: usize,
        total: usize,
        label: String,
    },
    OcrReady(Result<Arc<OcrEngine>, String>),
    ExtractDone {
        data: ExtractedData,
        cancelled: bool,
    },
    FileRendered {
        file: String,
        outcome: FileOutcome,
    },
    RenderDone {
        rendered: usize,
        copied: usize,
        failed: usize,
        cancelled: bool,
    },
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
    /// Fit-indicator cache for the inspector cards: (file, block, sizing,
    /// font, uniform/smallest size, text) -> (fits, lines, size). Cleared
    /// when it grows past 256 entries; the key carries everything the fit
    /// depends on, so a stale entry is just unused.
    fit_cache: std::collections::HashMap<String, (bool, usize, i32)>,
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
    /// Filter text for the file list.
    preview_filter: String,
    /// Preview zoom: Fit (the whole image in the stage) or 100% (native
    /// resolution). Wheel zoom switches out of Fit into a manual zoom.
    preview_zoom_fit: bool,
    /// Manual zoom factor (1.0 = 100%), used when `preview_zoom_fit` is
    /// false; wheel zoom adjusts it around the cursor.
    preview_zoom: f32,
    /// Stage pan offset (image centre relative to the stage centre), in
    /// screen points; drag pans, `0` resets to Fit.
    preview_pan: egui::Vec2,
    /// Per-file render outcomes from the last run (file-list status chips).
    /// Cleared when a new run starts or the dataset changes.
    rendered_files: std::collections::HashSet<String>,
    failed_files: std::collections::HashSet<String>,
    /// 44px file-list thumbnails, decoded lazily as rows appear, capped at
    /// 128 (insertion order in `thumb_order`; `None` = undecodable, so a
    /// broken file isn't retried every frame).
    thumbs: std::collections::HashMap<String, Option<TextureHandle>>,
    thumb_order: std::collections::VecDeque<String>,
    /// Status-bar counts for the dataset the preview shows.
    preview_file_count: usize,
    preview_untranslated: usize,
    /// The editor JSON's parse error, if it failed to resolve.
    preview_json_error: Option<String>,
    log_lines: Vec<LogLine>,
    /// The log filter (All / Warnings) from the log bar.
    log_filter: LogFilter,
    /// Whether the log follows new lines (true while the user hasn't
    /// scrolled away from the top, where the newest lines are).
    log_follow: bool,
    /// How many log lines the last frame saw (new lines = a follow scroll).
    log_len_seen: usize,
    /// Whether the activity log panel is shown (the X in its header hides
    /// it; the status bar brings it back).
    show_log: bool,
    /// The activity log panel's height in logical px. App-managed (the
    /// panel uses `.exact_size(log_height)`) so a drag resize stays put:
    /// egui's resizable-panel state snapped back to the pre-drag size
    /// for the user. Clamped to 64–480 while open; dragging the handle
    /// down past 48 closes the panel instead.
    log_height: f32,
    /// The screenshot test mode (`--shot-test`), if this run is one.
    shot_test: Option<ShotTest>,
    /// The last save destination (the output folder after a render, the
    /// file after Save captions), shown in the status bar.
    last_save_path: Option<String>,
    status: String,
    progress: f32,
    busy: bool,
    cancel: Arc<AtomicBool>,
    confirm_copy_originals: bool,
    pending_render: Option<ExtractedData>,
    /// The Paste Translated JSON modal (JSON tab, Phase 4 item 14): its
    /// textarea content, and whether it is open.
    show_paste_modal: bool,
    paste_text: String,
    /// The Bulk edit modal (the full-document editor, Phase 4 item 14):
    /// whether it is open. The editor binds to `editor_text` directly.
    show_bulk_edit: bool,
    /// The per-file import error list (JSON tab, Phase 4 item 14): shown
    /// in a modal after an import that skipped files; None = no report.
    import_report: Option<Vec<String>>,
    /// When the threshold sliders last moved: dragging schedules a
    /// debounced (50 ms) instant `reapply_thresholds` instead of
    /// re-filtering on every slider tick.
    thresholds_changed_at: Option<std::time::Instant>,
    /// The last completed render run (rendered, copied unchanged, failed),
    /// for the workflow stepper and the status bar. Cleared whenever the
    /// dataset changes (scan, load, paste, threshold change) so a stale
    /// "done" never survives new edits.
    render_summary: Option<(usize, usize, usize)>,

    tx: Sender<WorkerMsg>,
    rx: Receiver<WorkerMsg>,
}

impl CaptionStudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Craft-style UI chrome (ported theme/font/icon patterns): Plex +
        // Chakra Petch, Midnight tokens, and the Lucide SVG loader.
        // The caption *rendering* fonts (fonts.rs) are untouched.
        crate::theme::install_fonts(&cc.egui_ctx);
        let settings = Settings::load();
        // Dark only (redesign handoff, SPEC §4): no theme switch.
        crate::theme::apply(&cc.egui_ctx, crate::theme::ThemeKind::Studio, false);
        egui_extras::install_image_loaders(&cc.egui_ctx);

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
            fit_cache: std::collections::HashMap::new(),
            preview_texture: None,
            preview_size: [0.0, 0.0],
            verify_text: "Scan images to fill this in, or paste translated JSON here.".to_string(),
            selected_block: None,
            blocks_scroll_to_selected: false,
            preview_entry: None,
            preview_image_rect: egui::Rect::NOTHING,
            preview_image_scale: 1.0,
            preview_filter: String::new(),
            preview_zoom_fit: true,
            preview_zoom: 1.0,
            preview_pan: egui::Vec2::ZERO,
            rendered_files: std::collections::HashSet::new(),
            failed_files: std::collections::HashSet::new(),
            thumbs: std::collections::HashMap::new(),
            thumb_order: std::collections::VecDeque::new(),
            preview_file_count: 0,
            preview_untranslated: 0,
            preview_json_error: None,
            log_lines: Vec::new(),
            log_filter: LogFilter::All,
            log_follow: true,
            log_len_seen: 0,
            show_log: settings.show_log,
            log_height: settings.log_height.clamp(64.0, 480.0),
            shot_test: None,
            last_save_path: None,
            status: "Choose an input folder, then click Scan & Extract Text.".to_string(),
            progress: 0.0,
            busy: false,
            cancel: Arc::new(AtomicBool::new(false)),
            confirm_copy_originals: false,
            pending_render: None,
            show_paste_modal: false,
            paste_text: String::new(),
            show_bulk_edit: false,
            import_report: None,
            thresholds_changed_at: None,
            render_summary: None,
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

    /// A per-file log line (a render outcome, a per-file warning): the file
    /// shows in its own column.
    fn log_file(&mut self, level: LogLevel, file: String, result: String) {
        self.log_at(LogLine::file(level, file, result));
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
            log_height: self.log_height.clamp(64.0, 480.0),
            show_log: self.show_log,
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

    /// The workflow stepper's state (SPEC §2): Scan → Translate → Review →
    /// Render. Scan is done once an extraction exists; Translate is done
    /// when every kept block has a real translation (non-empty and different
    /// from the original); Review is done when Render is; Render is done
    /// when a render run completed after the last dataset change. The
    /// current step is the first one not done (Render again once all are
    /// done — re-render after edits). Translations are read from the
    /// resolved data (editor JSON merged over the scan), the same source
    /// the preview renders.
    /// Drop the last run's render state (summary + per-file chips): the
    /// dataset changed, so a stale "rendered" would lie.
    fn clear_render_state(&mut self) {
        self.render_summary = None;
        self.rendered_files.clear();
        self.failed_files.clear();
    }

    /// The dataset the workflow reads: the editor JSON merged over the scan
    /// when it parses, else the scan itself. Shared by the stepper and the
    /// Scan-again edit check so both read the same source the preview does.
    fn resolved_data(&self) -> Option<ExtractedData> {
        if self.extracted.is_some() {
            match self.resolve_editor() {
                Ok((d, _, _)) => Some(d),
                Err(_) => self.extracted.clone(),
            }
        } else {
            None
        }
    }

    fn workflow_state(&self) -> WorkflowState {
        derive_workflow_state(self.resolved_data().as_ref(), self.render_summary)
    }

    /// The header primary for the current step (SPEC §2): Scan images →
    /// Review → Render N images. Translate's primary goes to Review (and
    /// jumps to the first file with an untranslated block). Once every
    /// step is done the workflow is complete, so the primary becomes
    /// "Open output" — the next thing to do — instead of pointing back
    /// at Review or offering another render.
    fn header_primary(&self) -> (&'static str, String) {
        let wf = self.workflow_state();
        if wf.states.iter().all(|s| matches!(s, StepState::Done)) {
            return ("folder-open", "Open output".to_string());
        }
        match wf.current {
            WorkflowStep::Scan => ("scan", "Scan images".to_string()),
            WorkflowStep::Translate => ("eye", "Review".to_string()),
            WorkflowStep::Review | WorkflowStep::Render => {
                ("play", format!("Render {} images", wf.images))
            }
        }
    }

    /// Run the header primary's action for the current step.
    fn run_header_primary(&mut self) {
        if self
            .workflow_state()
            .states
            .iter()
            .all(|s| matches!(s, StepState::Done))
        {
            self.open_output_folder();
            return;
        }
        match self.workflow_state().current {
            WorkflowStep::Scan => self.start_extract(),
            WorkflowStep::Translate => {
                self.tab = Tab::Preview;
                // Jump to the first file with an untranslated block.
                if let Ok((data, _, _)) = self.resolve_editor() {
                    if let Some(entry) = data.images.iter().find(|e| {
                        e.blocks.iter().any(|b| {
                            let t = b.new_text.trim();
                            t.is_empty() || t == b.original_text.trim()
                        })
                    }) {
                        self.set_preview_file(entry.file.clone());
                    }
                } else if let Some(data) = &self.extracted {
                    if let Some(entry) = data.images.iter().find(|e| {
                        e.blocks.iter().any(|b| {
                            let t = b.new_text.trim();
                            t.is_empty() || t == b.original_text.trim()
                        })
                    }) {
                        self.set_preview_file(entry.file.clone());
                    }
                }
            }
            WorkflowStep::Review | WorkflowStep::Render => self.start_render(),
        }
    }

    /// Copy the activity log to the clipboard.
    fn copy_log(&mut self) {
        // In display order (newest first, matching the table).
        let text = self
            .log_lines
            .iter()
            .rev()
            .map(|l| l.to_text())
            .collect::<Vec<_>>()
            .join("\n");
        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text)) {
            Ok(()) => self.status = "Activity log copied to clipboard.".into(),
            Err(_) => self.status = "Couldn't copy the log — try again.".into(),
        }
    }

    // ------------------------------------------------------------ actions

    fn browse_input(&mut self) {
        if let Some(folder) = rfd::FileDialog::new()
            .set_title("Select Input Images Folder")
            .pick_folder()
        {
            self.input_dir = folder.to_string_lossy().to_string();
            self.output_dir = folder
                .join("translated_output")
                .to_string_lossy()
                .to_string();
            self.extracted = None;
            self.clear_render_state();
            self.preview_texture = None;
            self.preview_key.clear();
            self.save_settings();
        }
    }

    fn browse_output(&mut self) {
        if let Some(folder) = rfd::FileDialog::new()
            .set_title("Select Output Folder")
            .pick_folder()
        {
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
        self.clear_render_state();
        if !data.images.iter().any(|img| !img.raw_lines.is_empty()) {
            self.status = "This captions.json predates raw OCR storage. Click 'Scan & Extract Text' once to enable instant tuning.".into();
            return;
        }
        // Preserve translations from the editor, keyed by original text.
        let mut existing: std::collections::HashMap<
            String,
            std::collections::HashMap<String, String>,
        > = std::collections::HashMap::new();
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
            // Manual Keep/Filter overrides survive a re-derivation: an
            // overridden block is kept as-is (its id, translation and
            // override stay), and a block the thresholds would drop is
            // re-added rather than lost. Only non-overridden blocks are
            // re-derived.
            let overridden: Vec<crate::model::Block> = img
                .blocks
                .iter()
                .filter(|b| b.kept_override.is_some())
                .cloned()
                .collect();
            let (blocks, dropped) = group_lines_into_blocks(&img.raw_lines, min_conf, min_chars);
            let mut blocks = blocks;
            let mut dropped = dropped;
            if let Some(saved) = existing.get(&img.file) {
                for b in blocks.iter_mut() {
                    if let Some(v) = saved.get(&b.original_text) {
                        b.new_text = v.clone();
                    }
                }
            }
            // Swap in the overridden blocks for any new block with the same
            // original text (the id and override stay stable), then re-add
            // the overridden blocks the thresholds dropped entirely.
            for b in blocks.iter_mut() {
                if let Some(old) = overridden
                    .iter()
                    .find(|o| o.original_text == b.original_text)
                {
                    *b = old.clone();
                }
            }
            let kept_texts: std::collections::HashSet<String> =
                blocks.iter().map(|b| b.original_text.clone()).collect();
            for old in &overridden {
                if !kept_texts.contains(&old.original_text) {
                    blocks.push(old.clone());
                }
            }
            // A re-added overridden block must not also appear as a dropped
            // item: the thresholds dropped its lines, but the block itself
            // is the card. (Matched by bbox containment, not text — two
            // different lines can share the same text.)
            dropped.retain(|d| {
                !overridden.iter().any(|o| {
                    d.bbox[0] >= o.bbox[0]
                        && d.bbox[1] >= o.bbox[1]
                        && d.bbox[2] <= o.bbox[2]
                        && d.bbox[3] <= o.bbox[3]
                })
            });
            // Restore the visual (top-to-bottom, left-to-right) order the
            // grouping produces; the re-added blocks appended at the end.
            blocks.sort_by_key(|b| (b.bbox[1], b.bbox[0]));
            total_kept += blocks.iter().filter(|b| b.kept()).count();
            total_dropped += dropped.len() + blocks.iter().filter(|b| !b.kept()).count();
            img.blocks = blocks;
            img.dropped = dropped;
        }
        // Warn when a translated block becomes filtered: a block that had a
        // translation in the editor and no longer survives the thresholds
        // loses that translation when the editor is regenerated below.
        let mut lost_total = 0usize;
        for img in data.images.iter() {
            if let Some(saved) = existing.get(&img.file) {
                let kept: std::collections::HashSet<&str> = img
                    .blocks
                    .iter()
                    .map(|b| b.original_text.as_str())
                    .collect();
                let n = saved.keys().filter(|o| !kept.contains(o.as_str())).count();
                if n > 0 {
                    lost_total += n;
                    self.log_warn(format!(
                        "{}: {n} translated block(s) became filtered — the translation is lost.",
                        img.file
                    ));
                }
            }
        }
        self.extracted = Some(data);
        self.preview_key.clear();
        let compact = compact_map_from_data(self.extracted.as_ref().unwrap());
        self.editor_text = serde_json::to_string_pretty(&compact).unwrap_or_default();
        let mut status = format!(
            "Thresholds applied (conf >= {min_conf:.2}, unboxed >= {min_chars} chars): {total_kept} kept, {total_dropped} filtered out."
        );
        if lost_total > 0 {
            status.push_str(&format!(
                " {lost_total} translated block(s) became filtered — see the log."
            ));
        }
        self.status = status;
        self.save_settings();
    }

    fn copy_compact_for_ai(&mut self) {
        if self.editor_text.trim().is_empty() {
            self.status = "Nothing to copy — Scan & Extract Text first or load a JSON file.".into();
            return;
        }
        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(self.editor_text.clone())) {
            Ok(()) => {
                self.status = "Copied JSON to clipboard — paste it into your AI translator.".into()
            }
            Err(e) => self.status = format!("Couldn't copy to the clipboard ({e}) — try again."),
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
                    self.last_save_path = Some(path.to_string_lossy().into_owned());
                    self.status = format!(
                        "Saved metadata to {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    );
                }
                Err(e) => {
                    self.status = format!("Couldn't save ({e}) — check the folder and try again.")
                }
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
                .and_then(|s| {
                    serde_json::from_str::<ExtractedData>(&s).map_err(anyhow::Error::from)
                }) {
                Ok(data) => {
                    let n = data.images.len();
                    self.editor_text = serde_json::to_string_pretty(&compact_map_from_data(&data))
                        .unwrap_or_default();
                    self.extracted = Some(data);
                    self.clear_render_state();
                    self.preview_key.clear();
                    self.status = format!(
                        "Loaded {n} images from {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    );
                }
                Err(e) => {
                    self.status =
                        format!("Couldn't load that JSON ({e}) — check the file and try again.")
                }
            }
        }
    }

    /// Load the compact translation map from a file. Returns true when a
    /// file was picked (so a caller showing a dialog knows to close it);
    /// false when the user cancelled the file picker.
    fn load_translated_file(&mut self) -> bool {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Select Translated JSON File")
            .add_filter("JSON files", &["json"])
            .pick_file()
        {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    // Trial-merge against the extracted scan to list the
                    // files that won't import (unknown file, stale block
                    // ids); the valid files import via the editor text.
                    let mut report: Vec<String> = Vec::new();
                    match parse_flexible_json_or_dict(text.trim()) {
                        Ok(parsed) => {
                            if let Some(data) = self.extracted.clone() {
                                let (_, warnings, _) = merge_translations(&parsed, &data);
                                report = warnings;
                            }
                        }
                        Err(_) => {
                            report.push("The file isn't valid JSON — nothing was imported.".into());
                        }
                    }
                    self.editor_text = text;
                    self.clear_render_state();
                    self.preview_key.clear();
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    if report.is_empty() {
                        self.status = format!("Imported translations from {name}");
                    } else {
                        self.import_report = Some(report);
                        self.status = format!(
                            "Imported translations from {name} — some files were skipped, see the report."
                        );
                    }
                }
                Err(e) => {
                    self.status =
                        format!("Couldn't read that file ({e}) — check the file and try again.")
                }
            }
            true
        } else {
            false
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
        self.clear_render_state();
        self.cancel.store(false, Ordering::SeqCst);
        self.progress = 0.0;
        self.status = "Starting scan…".into();
        let tx = self.tx.clone();
        let cancel = self.cancel.clone();
        let min_conf = self.safe_conf();
        let min_chars = self.safe_minchars();
        let engine = self.ocr.clone().expect("OCR engine ready");
        std::thread::spawn(move || {
            extract_worker(
                PathBuf::from(in_dir),
                engine,
                min_conf,
                min_chars,
                tx,
                cancel,
            );
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
                self.status =
                    format!("Couldn't parse the JSON — {e}. Fix the marked spot and try again.");
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
        self.clear_render_state();
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

    /// The 44px thumbnail for a file-list row: decoded lazily (at most
    /// `budget` decodes per frame, so a long scroll doesn't stall the UI)
    /// and capped at 128 cached textures (FIFO eviction). `None` in the
    /// cache means the file couldn't be decoded — it isn't retried. A
    /// budget-exhausted miss is NOT cached, so it retries next frame.
    fn thumb_texture(
        &mut self,
        ctx: &egui::Context,
        name: &str,
        budget: &mut usize,
    ) -> Option<TextureHandle> {
        if let Some(t) = self.thumbs.get(name) {
            return t.clone();
        }
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        let path = PathBuf::from(self.input_dir.trim()).join(name);
        let tex = crate::image_io::load_image(&path).ok().and_then(|img| {
            let thumb = image::imageops::thumbnail(&img, 44, 44);
            let (w, h) = (thumb.width() as usize, thumb.height() as usize);
            let color = ColorImage::from_rgb([w, h], thumb.as_raw());
            Some(ctx.load_texture(format!("thumb:{name}"), color, egui::TextureOptions::LINEAR))
        });
        if self.thumbs.len() >= 128 {
            if let Some(old) = self.thumb_order.pop_front() {
                self.thumbs.remove(&old);
            }
        }
        self.thumb_order.push_back(name.to_string());
        self.thumbs.insert(name.to_string(), tex.clone());
        tex
    }

    /// One file-list row: 44px thumbnail, name, "N blocks", status chip.
    /// Returns true when the row was clicked (the caller selects the file).
    fn file_list_row(
        &mut self,
        ui: &mut egui::Ui,
        name: &str,
        data: Option<&crate::model::ExtractedData>,
        budget: &mut usize,
    ) -> bool {
        let t = crate::theme::Tokens::get(ui.ctx());
        let entry = data.and_then(|d| d.entry(name));
        let blocks = entry.map(|e| e.blocks.len()).unwrap_or(0);
        let status = entry.map(|e| {
            file_status(
                e,
                self.rendered_files.contains(name),
                self.failed_files.contains(name),
            )
        });
        let selected = name == self.preview_file;
        // 64px: the thumbnail (44px) + two text lines + padding. At 52px
        // the status chip sat on the row's bottom edge and the selection
        // stroke cut straight through it; at 60px the thumbnail and the
        // chip still filled the row edge-to-edge (shot 03: "the image
        // and content are overflowing"), so the row grows again to give
        // both clear breathing room.
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 64.0), egui::Sense::click());
        let resp = resp.accessible_name(format!(
            "{} · {} blocks{}",
            name,
            blocks,
            status
                .map(|s| format!(" · {}", file_status_label(s)))
                .unwrap_or_default()
        ));
        let clicked = resp.clicked();
        if selected || resp.hovered() {
            ui.painter().rect_filled(rect, t.radius_sm, t.card);
        }
        let stroke = if selected {
            egui::Stroke::new(2.0, t.accent)
        } else {
            egui::Stroke::new(1.0, t.card_border)
        };
        ui.painter()
            .rect_stroke(rect, t.radius_sm, stroke, egui::StrokeKind::Inside);
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink(4.0))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.horizontal(|ui| {
            // Thumbnail: the 44px cell, the image centred inside it
            // (thumbnails keep their aspect, so one side is < 44).
            let (cell, _) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
            if let Some(tex) = self.thumb_texture(ui.ctx(), name, budget) {
                let (tw, th) = (tex.size()[0] as f32, tex.size()[1] as f32);
                let img_rect = egui::Rect::from_center_size(cell.center(), egui::vec2(tw, th));
                ui.painter().image(
                    tex.id(),
                    img_rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                ui.painter().rect_filled(cell, 4.0, t.raised);
            }
            ui.vertical(|ui| {
                // Hover-only sense: egui's default `selectable_labels`
                // gives every label click-and-drag sense (and `.sense()`
                // alone doesn't stop it — the selectable block ORs the
                // click sense back in), so the filename and count labels
                // stole the click from the row and only the thumbnail /
                // gaps selected it. These are metadata, not text to copy,
                // so they are not selectable and the whole row selects.
                let shown = tail_ellipsis(ui, name, ui.available_width());
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(shown)
                            .size(crate::theme::LABEL)
                            .color(t.text),
                    )
                    .truncate()
                    .selectable(false)
                    .sense(egui::Sense::hover()),
                );
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("{blocks} blocks"))
                                .size(crate::theme::CAPTION)
                                .color(t.text_dim),
                        )
                        .selectable(false)
                        .sense(egui::Sense::hover()),
                    );
                    if let Some(s) = status {
                        let color = match s {
                            FileStatus::Rendered => t.ok,
                            FileStatus::NoText => t.warning,
                            FileStatus::Failed => t.danger,
                            FileStatus::Untranslated => t.text_dim,
                            FileStatus::Ready => t.accent_ink,
                        };
                        status_chip(ui, file_status_label(s), color);
                    }
                });
            });
        });
        clicked
    }

    fn refresh_preview(&mut self, ctx: &egui::Context, data: Option<&crate::model::ExtractedData>) {
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
        let Some(data) = data else {
            self.preview_entry = None;
            self.verify_text = match &self.preview_json_error {
                Some(e) => {
                    format!("Couldn't preview the JSON — {e}. Fix the marked spot and try again.")
                }
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
                self.verify_text = format!(
                    "Couldn't read {} — it may have moved or be corrupt.",
                    entry.file
                );
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
        self.preview_texture =
            Some(ctx.load_texture("preview", color, egui::TextureOptions::LINEAR));
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
        names.sort_by(|a, b| natural_cmp(a, b));
        names
    }

    /// Inspector block cards (redesign Phase 4, item 13): one card per
    /// block (kept + manually filtered) and one per dropped item. A block
    /// card has a title row (id, conf, kept/filtered chip, Filter out /
    /// Keep toggle), the read-only OCR text, an editable translation
    /// textarea (edits flow into the compact JSON + live preview), and a
    /// fit indicator. Clicking a card's title selects the block (the stage
    /// highlights it); focusing the textarea selects it too.
    fn show_block_cards(&mut self, ui: &mut egui::Ui, entry: &crate::model::ImageEntry) {
        use egui::text::{LayoutJob, TextFormat};
        let t = crate::theme::Tokens::get(ui.ctx());
        let mono = egui::FontId::monospace(crate::theme::CODE);
        let file = entry.file.clone();
        // Snapshot the entry so the cards can mutate `extracted` (a toggle,
        // a promotion, a translation edit) without borrowing the preview
        // entry while drawing it. The change shows on the next frame.
        let blocks: Vec<crate::model::Block> = entry.blocks.clone();
        let dropped: Vec<crate::model::DroppedItem> = entry.dropped.clone();
        for blk in &blocks {
            let selected = self.selected_block == Some(blk.id);
            let frame = egui::Frame::new()
                .fill(t.card)
                .stroke(egui::Stroke::new(
                    1.0,
                    if selected { t.accent } else { t.card_border },
                ))
                .corner_radius(t.r_sm())
                .inner_margin(egui::Margin::same(12));
            frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // Title: "Block #n" + conf. Green at/above the conf
                    // threshold (it passes); amber below it.
                    let mut job = LayoutJob::default();
                    job.append(
                        &format!("Block #{} ", blk.id),
                        0.0,
                        TextFormat {
                            font_id: egui::FontId::proportional(crate::theme::CARD),
                            color: t.text,
                            ..Default::default()
                        },
                    );
                    let conf_col = if blk.confidence >= self.safe_conf() {
                        t.ok
                    } else {
                        t.warning
                    };
                    job.append(
                        &format!("{:.2}", blk.confidence),
                        0.0,
                        TextFormat { font_id: mono.clone(), color: conf_col, ..Default::default() },
                    );
                    let title_resp = ui.selectable_label(selected, job);
                    focus_ring(ui, &title_resp);
                    if selected && self.blocks_scroll_to_selected {
                        title_resp.scroll_to_me(Some(egui::Align::Center));
                        self.blocks_scroll_to_selected = false;
                    }
                    if title_resp.clicked() {
                        self.selected_block = Some(blk.id);
                        self.preview_key.clear();
                    }
                    let (chip, col) = if blk.kept() {
                        ("kept", t.ok)
                    } else {
                        ("filtered", t.warning)
                    };
                    status_chip(ui, chip, col);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if blk.kept() {
                            let resp = ui
                                .small_button("Filter out")
                                .on_hover_text("Exclude this block from the render — the original text stays in the image. Reversible.");
                            if resp.clicked() {
                                self.set_block_override(&file, blk.id, false);
                            }
                        } else {
                            let resp = ui
                                .small_button("Keep")
                                .on_hover_text("Render this block again with its translation.");
                            if resp.clicked() {
                                self.set_block_override(&file, blk.id, true);
                            }
                        }
                    });
                });
                ui.add_space(4.0);
                // The OCR text, read-only.
                ui.label(
                    egui::RichText::new(format!("\"{}\"", blk.original_text))
                        .font(mono.clone())
                        .color(t.text_dim),
                );
                ui.add_space(4.0);
                // The translation, editable — disabled while the block is
                // filtered out (it won't render, so there is nothing to
                // translate); the Keep button re-enables it.
                let mut text = blk.new_text.clone();
                ui.add_enabled_ui(blk.kept(), |ui| {
                    let resp = egui::TextEdit::multiline(&mut text)
                        .hint_text("Translation…")
                        .font(mono.clone())
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .show(ui);
                    focus_ring(ui, &resp.response);
                    if resp.response.changed() {
                        self.set_block_text(&file, blk.id, text);
                    }
                    if resp.response.has_focus() && self.selected_block != Some(blk.id) {
                        self.selected_block = Some(blk.id);
                        self.preview_key.clear();
                    }
                });
                ui.add_space(4.0);
                // Fit indicator: what the render will actually do with the
                // effective text (the translation, or the original while
                // untranslated). A filtered block won't render, so it gets
                // a dim note instead of a fit line.
                if blk.kept() {
                    let (fits, lines, size) = self.fit_for(&file, entry, blk);
                    let eff = if blk.new_text.trim().is_empty() {
                        blk.original_text.trim()
                    } else {
                        blk.new_text.trim()
                    };
                    let chars = eff.chars().count();
                    let label = fit_label(fits, lines, chars, size);
                    let col = if fits { t.ok } else { t.warning };
                    ui.label(
                        egui::RichText::new(label)
                            .size(crate::theme::CAPTION)
                            .color(col),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("Filtered out — won't render")
                            .size(crate::theme::CAPTION)
                            .color(t.text_faint),
                    );
                }
            });
            ui.add_space(8.0);
        }
        for drop in &dropped {
            let frame = egui::Frame::new()
                .fill(t.card)
                .stroke(egui::Stroke::new(1.0, t.card_border))
                .corner_radius(t.r_sm())
                .inner_margin(egui::Margin::same(12));
            frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Filtered")
                            .font(egui::FontId::proportional(crate::theme::CARD))
                            .color(t.text_faint),
                    );
                    status_chip(ui, "filtered", t.warning);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Keep needs the raw lines to rebuild the block; an
                        // old captions.json without them disables it.
                        let promotable = !entry.raw_lines.is_empty();
                        let resp = ui
                            .add_enabled(
                                promotable,
                                egui::Button::new("Keep").small(),
                            )
                            .on_hover_text(if promotable {
                                "Keep this text — it becomes a block you can translate."
                            } else {
                                "Re-scan to keep filtered text (this captions.json has no raw OCR lines)."
                            });
                        if resp.clicked() {
                            self.promote_dropped_item(&file, drop);
                        }
                    });
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("\"{}\"", drop.text))
                        .font(mono.clone())
                        .color(t.text_dim),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(&drop.reason)
                        .size(crate::theme::CAPTION)
                        .color(t.text_faint),
                );
            });
            ui.add_space(8.0);
        }
    }

    /// Rebuild the editor's compact JSON from the extracted scan, clear the
    /// render state and the preview so the next frame re-renders. Call after
    /// an inspector edit changes `extracted` (a translation, a Keep/Filter
    /// toggle, a promotion).
    fn sync_editor_from_extracted(&mut self) {
        if let Some(data) = self.extracted.clone() {
            let compact = compact_map_from_data(&data);
            self.editor_text = serde_json::to_string_pretty(&compact).unwrap_or_default();
        }
        self.clear_render_state();
        self.preview_key.clear();
    }

    /// Set a block's Keep/Filter override in `extracted`, then sync the
    /// editor + preview. (The inspector draws the resolved preview entry,
    /// so the change shows on the next frame.)
    fn set_block_override(&mut self, file: &str, id: usize, kept: bool) {
        if let Some(data) = self.extracted.as_mut() {
            if let Some(entry) = data.entry_mut(file) {
                if let Some(blk) = entry.blocks.iter_mut().find(|b| b.id == id) {
                    blk.kept_override = Some(kept);
                }
            }
        }
        self.sync_editor_from_extracted();
    }

    /// Promote a dropped item to a block in `extracted` (the inspector's
    /// "Keep" toggle), then sync the editor + preview. Returns false when
    /// the raw lines are gone (old captions.json) — the caller disables
    /// Keep in that case.
    fn promote_dropped_item(&mut self, file: &str, drop: &crate::model::DroppedItem) -> bool {
        let promoted = self.extracted.as_ref().and_then(|data| {
            data.entry(file).and_then(|entry| {
                let next_id = entry.blocks.iter().map(|b| b.id).max().unwrap_or(0) + 1;
                crate::layout::promote_dropped(drop, &entry.raw_lines, next_id)
            })
        });
        let Some(block) = promoted else {
            return false;
        };
        if let Some(data) = self.extracted.as_mut() {
            if let Some(entry) = data.entry_mut(file) {
                entry
                    .dropped
                    .retain(|d| d.text != drop.text || d.bbox != drop.bbox);
                entry.blocks.push(block);
            }
        }
        self.sync_editor_from_extracted();
        true
    }

    /// Write a block's translation into `extracted` (the inspector's
    /// textarea), then sync the editor + preview.
    fn set_block_text(&mut self, file: &str, id: usize, text: String) {
        if let Some(data) = self.extracted.as_mut() {
            if let Some(entry) = data.entry_mut(file) {
                if let Some(blk) = entry.blocks.iter_mut().find(|b| b.id == id) {
                    blk.new_text = text;
                }
            }
        }
        self.sync_editor_from_extracted();
    }

    /// (fits, lines, size) for a block's effective text under the current
    /// sizing mode and font, using the same bounds the renderer uses.
    /// Cached per (file, block, sizing, font, uniform/smallest size, text)
    /// so the inspector doesn't re-shape every card every frame.
    fn fit_for(
        &mut self,
        file: &str,
        entry: &crate::model::ImageEntry,
        blk: &crate::model::Block,
    ) -> (bool, usize, i32) {
        let text = if blk.new_text.trim().is_empty() {
            blk.original_text.trim()
        } else {
            blk.new_text.trim()
        };
        let uniform = crate::render::uniform_size_for(entry);
        let smallest = crate::render::smallest_size_for(
            entry,
            entry.width as i32,
            if self.font_path.trim().is_empty() {
                None
            } else {
                crate::fonts::load_custom_font(self.font_path.trim())
            },
        );
        let key = format!(
            "{}|{}|{:?}|{}|{}|{}|{}",
            file,
            blk.id,
            self.sizing,
            self.font_path.trim(),
            uniform,
            smallest,
            text
        );
        if let Some(v) = self.fit_cache.get(&key) {
            return *v;
        }
        let (start_size, max_w, max_h) = crate::render::fit_bounds_for(
            blk,
            self.sizing,
            uniform,
            smallest,
            entry.width as i32,
            entry.height as i32,
        );
        let custom_family = if self.font_path.trim().is_empty() {
            None
        } else {
            crate::fonts::load_custom_font(self.font_path.trim())
        };
        let fitted = crate::render::fit_text_to_box(text, max_w, max_h, start_size, custom_family);
        let v = (fitted.size >= start_size, fitted.lines.len(), fitted.size);
        if self.fit_cache.len() > 256 {
            self.fit_cache.clear();
        }
        self.fit_cache.insert(key, v);
        v
    }

    // ------------------------------------------------------------ messages

    fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                WorkerMsg::Log(m) => self.log(m),
                WorkerMsg::Status(s) => self.status = s,
                WorkerMsg::Progress { done, total, label } => {
                    self.progress = if total == 0 {
                        0.0
                    } else {
                        done as f32 / total as f32
                    };
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
                        self.status =
                            format!("OCR engine failed to start — restart the app to retry. ({e})");
                        self.log_error(format!("OCR engine failed to start: {e}"));
                    }
                },
                WorkerMsg::ExtractDone { data, cancelled } => {
                    self.busy = false;
                    self.progress = 1.0;
                    let n = data.images.len();
                    self.editor_text = serde_json::to_string_pretty(&compact_map_from_data(&data))
                        .unwrap_or_default();
                    // Point the preview at the first file: without this
                    // the JSON tab and the inspector show the "Scan
                    // images…" hint even though the scan just finished,
                    // because the per-image JSON and the block list both
                    // look up the preview file, and it was still empty.
                    let first_file = data.images.iter().map(|e| e.file.clone()).min();
                    let first_verify = first_file
                        .as_ref()
                        .and_then(|f| data.entry(f))
                        .map(verify_text_for);
                    self.extracted = Some(data);
                    self.preview_key.clear();
                    if let Some(f) = first_file {
                        self.set_preview_file(f);
                    }
                    if let Some(v) = first_verify {
                        self.verify_text = v;
                    }
                    self.tab = Tab::Json;
                    let prefix = if cancelled { "Stopped. " } else { "" };
                    self.status = format!(
                        "{prefix}Extracted {n} images. Click 'Inspect OCR Boxes' to verify or 'Copy JSON for AI'."
                    );
                }
                WorkerMsg::FileRendered { file, outcome } => match outcome {
                    FileOutcome::Rendered => {
                        self.failed_files.remove(&file);
                        self.rendered_files.insert(file.clone());
                        self.log_file(LogLevel::Info, file, "rendered".into());
                    }
                    FileOutcome::Failed => {
                        self.rendered_files.remove(&file);
                        self.failed_files.insert(file.clone());
                        self.log_file(LogLevel::Error, file, "failed".into());
                    }
                    FileOutcome::Copied => {
                        // Copied unchanged (no translated blocks): the chip
                        // derives from the block counts, so nothing to store.
                        self.log_file(LogLevel::Info, file, "copied unchanged".into());
                    }
                },
                WorkerMsg::RenderDone {
                    rendered,
                    copied,
                    failed,
                    cancelled,
                } => {
                    self.busy = false;
                    self.progress = 1.0;
                    self.render_summary = Some((rendered, copied, failed));
                    self.last_save_path = Some(self.output_dir.clone());
                    let prefix = if cancelled {
                        "Stopped — "
                    } else {
                        "Complete — "
                    };
                    let summary = format!(
                        "{prefix}rendered {rendered}, copied {copied} unchanged, failed {failed}"
                    );
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
        lines.push("No text found. This image will be copied unchanged.".into());
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
        send(WorkerMsg::Status(
            "No supported images found — add .jpg, .png, or .webp files.".into(),
        ));
        send(WorkerMsg::ExtractDone {
            data: ExtractedData::default(),
            cancelled: false,
        });
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
        let name = img_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
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
                            "{name}: {} kept, {} filtered",
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
            Err(e) => send(WorkerMsg::Log(format!(
                "SKIP {name}: unreadable image ({e})"
            ))),
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
        let _ = tx.send(WorkerMsg::Status(format!(
            "Couldn't start the render ({e}) — check the output folder and try again."
        )));
        let _ = tx.send(WorkerMsg::RenderDone {
            rendered: 0,
            copied: 0,
            failed: 0,
            cancelled: false,
        });
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
        let file_outcome = match &outcome {
            Ok("rendered") => {
                counts.lock().unwrap().0 += 1;
                FileOutcome::Rendered
            }
            Ok("copied") => {
                counts.lock().unwrap().1 += 1;
                FileOutcome::Copied
            }
            Ok(_) => FileOutcome::Copied,
            Err(e) => {
                counts.lock().unwrap().2 += 1;
                let _ = tx.send(WorkerMsg::Log(format!("FAIL {name}: {e}")));
                FileOutcome::Failed
            }
        };
        let _ = tx.send(WorkerMsg::FileRendered {
            file: name.clone(),
            outcome: file_outcome,
        });
        let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = progress_tx.send(WorkerMsg::Progress {
            done,
            total,
            label: format!("Rendering ({done}/{total}): {name}"),
        });
    });

    let (rendered, copied, failed) = *counts.lock().unwrap();
    let cancelled = cancel.load(Ordering::SeqCst);
    let _ = tx.send(WorkerMsg::RenderDone {
        rendered,
        copied,
        failed,
        cancelled,
    });
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
        // Threshold debounce: a slider drag schedules a re-filter 50 ms
        // after the last movement, so dragging doesn't re-run the grouping
        // on every tick (the slider also requests a repaint at ~60 ms so
        // this check fires even after the drag ends).
        if let Some(t) = self.thresholds_changed_at {
            if t.elapsed() >= std::time::Duration::from_millis(50) {
                self.thresholds_changed_at = None;
                self.reapply_thresholds();
            }
        }

        // Shot-test mode: drive the state machine before the modals and
        // panels are drawn, so this frame renders the new state.
        self.shot_test_step(&ctx);

        // --- Confirm-copy-originals modal (Python askYesNo) ---
        // While it is open the panels behind it are disabled (a focus trap:
        // egui windows are non-modal, so without this Tab and clicks escape
        // to the panels) and Escape dismisses it.
        if self.confirm_copy_originals {
            let mut open = true;
            egui::Window::new("Nothing translated")
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
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

        // --- Paste Translated JSON modal (JSON tab, Phase 4 item 14) ---
        // The pasted text goes into the editor on Apply; "Paste from
        // clipboard" fills the textarea from the clipboard first.
        if self.show_paste_modal {
            let mut open = true;
            egui::Window::new("Paste Translated JSON")
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .resizable(false)
                .default_size([700.0, 450.0])
                .open(&mut open)
                .show(&ctx, |ui| {
                    ui.label("Paste the translated JSON (the compact map from the AI) below, then Apply.");
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.paste_text)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .desired_rows(20)
                                .hint_text("{\"_1.jpg\": {\"1\": \"translation\"}}"),
                        );
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Paste from clipboard").clicked() {
                            match arboard::Clipboard::new()
                                .and_then(|mut cb| cb.get_text())
                            {
                                Ok(clip) => self.paste_text = clip,
                                Err(_) => {
                                    self.status =
                                        "Clipboard empty — copy the translated JSON first.".into()
                                }
                            }
                        }
                        // The file import lives here, not as a second
                        // top-level button: it loads the same compact map
                        // as Paste, only from a file. A picked file closes
                        // the dialog (the import already landed in the
                        // editor); cancelling the picker leaves it open so
                        // the pasted text isn't lost.
                        if ui.button("Import from file…").clicked()
                            && self.load_translated_file()
                        {
                            self.show_paste_modal = false;
                        }
                        let apply = ui.button("Apply");
                        focus_ring(ui, &apply);
                        if apply.clicked() {
                            self.editor_text = self.paste_text.clone();
                            self.clear_render_state();
                            self.preview_key.clear();
                            self.show_paste_modal = false;
                            self.status =
                                "Pasted translated JSON. Switch to Live Preview or Render.".into();
                        }
                        let cancel = ui.button("Cancel");
                        focus_ring(ui, &cancel);
                        if cancel.clicked() {
                            self.show_paste_modal = false;
                        }
                    });
                });
            if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.show_paste_modal = false;
            }
        }

        // --- Bulk edit modal (JSON tab, Phase 4 item 14) ---
        // The full-document editor, kept from the old JSON tab: it binds to
        // `editor_text` directly, so edits are live the moment they are
        // typed; Done just closes the modal.
        if self.show_bulk_edit {
            let mut open = true;
            egui::Window::new("Bulk edit — Translation JSON")
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .resizable(true)
                .default_size([900.0, 600.0])
                // Capped, so the window can't span the full height and put
                // the Done button on top of the status bar.
                .max_size([900.0, 720.0])
                .open(&mut open)
                .show(&ctx, |ui| {
                    ui.label("The full compact map for every image. Edits apply immediately; the per-image view and the preview update from here.");
                    ui.add_space(8.0);
                    // Capped height, and the editor sits on an opaque
                    // field fill — without it the JSON tab behind the
                    // modal showed through the text edit as double text.
                    let t = crate::theme::Tokens::get(ui.ctx());
                    egui::Frame::new()
                        .fill(t.field)
                        .corner_radius(egui::CornerRadius::same(t.radius_sm as u8))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(480.0)
                                .show(ui, |ui| {
                                    let mut layouter = |ui: &egui::Ui,
                                                        buf: &dyn egui::TextBuffer,
                                                        wrap_width: f32| {
                                        let mut job = json_highlight_job(buf.as_str(), &t);
                                        job.wrap.max_width = wrap_width;
                                        ui.fonts_mut(|f| f.layout_job(job))
                                    };
                                    let resp = ui.add(
                                        egui::TextEdit::multiline(&mut self.editor_text)
                                            .font(egui::TextStyle::Monospace)
                                            .desired_width(f32::INFINITY)
                                            .desired_rows(24)
                                            .layouter(&mut layouter)
                                            .hint_text("Scan images to fill this in, or paste translated JSON here."),
                                    )
                                    .accessible_name("Translation JSON");
                                    focus_ring(ui, &resp);
                                    if resp.changed() {
                                        self.preview_key.clear();
                                        self.resolve_preview_data();
                                    }
                                });
                        });
                    ui.add_space(8.0);
                    let done = ui.button("Done");
                    focus_ring(ui, &done);
                    if done.clicked() {
                        self.show_bulk_edit = false;
                    }
                });
            if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.show_bulk_edit = false;
            }
        }

        // --- Import report modal (JSON tab, Phase 4 item 14) ---
        // After an import that skipped files, the per-file errors are
        // listed here; the valid files were already imported.
        if let Some(report) = self.import_report.clone() {
            let mut open = true;
            egui::Window::new("Import — some files skipped")
                .collapsible(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .resizable(false)
                .default_size([600.0, 300.0])
                .open(&mut open)
                .show(&ctx, |ui| {
                    ui.label("The valid files were imported. These were skipped:");
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for line in &report {
                            ui.label(line);
                        }
                    });
                    ui.add_space(8.0);
                    let ok = ui.button("OK");
                    focus_ring(ui, &ok);
                    if ok.clicked() {
                        self.import_report = None;
                    }
                });
            if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.import_report = None;
            }
        }

        let modal_open = self.confirm_copy_originals
            || self.show_paste_modal
            || self.show_bulk_edit
            || self.import_report.is_some();

        // --- Status bar (SPEC §3: exact 32px) ---
        // Counts + last save path + the status message (the accesskit live
        // region). The progress bar shows ONLY while a job runs (defect 6:
        // at idle it read as a second status line).
        egui::Panel::bottom("status_bar")
            .exact_size(32.0)
            .show(ui, |ui| {
                ui.add_enabled_ui(!modal_open, |ui| {
                    ui.horizontal(|ui| {
                        let t = crate::theme::Tokens::get(ui.ctx());
                        // Right-aligned items FIRST: they claim their space
                        // from the right, so the left-hand labels (counts,
                        // save path, status) truncate to what is left
                        // instead of running underneath the button.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // When the log panel is hidden, the bar offers it
                            // back (the X in the log header hides it).
                            if !self.show_log && ui.small_button("Show log").clicked() {
                                self.show_log = true;
                                self.save_settings();
                            }
                            // A solid accent fill, not the 17%-alpha wash (1.23:1
                            // against its track — the bar read as empty at 100%),
                            // and capped at ~220px so it reads as a bar instead of a
                            // second status line. Only while a job runs.
                            if self.busy {
                                ui.add(
                                    egui::ProgressBar::new(self.progress)
                                        .fill(t.accent)
                                        .desired_width(220.0)
                                        .show_percentage(),
                                );
                            }
                        });
                        // A JSON error replaces the counts (danger colour, full
                        // error on hover) so a broken paste is visible without
                        // switching tabs.
                        if let Some(e) = &self.preview_json_error {
                            ui.add(
                                egui::Label::new(egui::RichText::new("JSON error").color(t.danger))
                                    .truncate(),
                            )
                            .on_hover_text(e.clone());
                            ui.separator();
                        } else if self.preview_file_count > 0 {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(render_counts(
                                        self.preview_file_count,
                                        self.render_summary,
                                    ))
                                    .color(t.text_dim),
                                )
                                .truncate(),
                            );
                            ui.separator();
                        }
                        // The last save destination (the output folder after a
                        // render, the file after Save captions). Only the
                        // folder/file name shows in the 32px bar — the full
                        // path is the tooltip — so the useful part isn't the
                        // part cut off at the window edge.
                        if let Some(path) = &self.last_save_path {
                            let shown = std::path::Path::new(path)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.clone());
                            ui.add(
                                egui::Label::new(egui::RichText::new(shown).color(t.text_dim))
                                    .truncate(),
                            )
                            .on_hover_text(path.clone());
                            ui.separator();
                        }
                        // A polite live region: a screen reader announces the status
                        // when it changes (batch finished, error) instead of the user
                        // having to go looking for it. The label truncates with an
                        // ellipsis so a long path can't silently clip against the bar.
                        // While OCR warms up, the warm-up state shows here (it used
                        // to sit in the sidebar and push the PROJECT card off).
                        let status_text = if !self.ocr_ready {
                            "Warming up OCR engine…".to_string()
                        } else {
                            self.status.clone()
                        };
                        let status_resp = ui.add(egui::Label::new(&status_text).truncate());
                        status_resp.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Status, true, status_text.clone())
                        });
                        ui.ctx().accesskit_node_builder(status_resp.id, |node| {
                            node.set_live(egui::accesskit::Live::Polite);
                        });
                    });
                });
            });

        // --- Activity log (bottom row, SPEC §3: 188px) ---
        // Structured table (Phase 5, item 15): newest first, zebra rows,
        // filters All / Warnings with counts, Copy log, Stop (disabled at
        // idle). Auto-scroll follows the newest line until the user scrolls
        // away from the top. The panel is resizable (drag its top edge) and
        // closable (the X in its header); the status bar brings it back.
        if self.show_log {
            // App-managed height: egui's resizable-panel state snapped the
            // panel back to its pre-drag size on release, so the height
            // lives in the App (and in Settings) and the resize handle is
            // drawn by hand. Drag the grip to resize (64–480px), drag it
            // down past the floor to close, double-click to reset to 188px.
            egui::Panel::bottom("activity_log")
                .exact_size(self.log_height)
                .show(ui, |ui| {
                    ui.add_enabled_ui(!modal_open, |ui| {
                        let t = crate::theme::Tokens::get(ui.ctx());
                        let handle = ui.allocate_response(
                            egui::vec2(ui.available_width(), 8.0),
                            egui::Sense::click_and_drag(),
                        );
                        if handle.hovered() || handle.dragged() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                        }
                        let grip = egui::Rect::from_center_size(
                            handle.rect.center(),
                            egui::vec2(48.0, 3.0),
                        );
                        ui.painter().rect_filled(
                            grip,
                            1.5,
                            if handle.hovered() || handle.dragged() {
                                t.accent
                            } else {
                                t.separator
                            },
                        );
                        if handle.double_clicked() {
                            self.log_height = 188.0;
                            self.save_settings();
                        }
                        if handle.dragged() {
                            match log_height_after_drag(self.log_height, handle.drag_delta().y) {
                                None => {
                                    // Dragged down past the floor: close.
                                    self.show_log = false;
                                    self.log_height = 188.0;
                                    self.save_settings();
                                }
                                Some(h) => self.log_height = h,
                            }
                        }
                        if handle.drag_stopped() {
                            self.save_settings();
                        }
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("ACTIVITY")
                                    .font(crate::theme::display(13.0))
                                    .color(t.accent_ink),
                            );
                            // Filters with counts (warnings = warn + error lines).
                            let total = self.log_lines.len();
                            let warnings = self
                                .log_lines
                                .iter()
                                .filter(|l| l.level != LogLevel::Info)
                                .count();
                            let all_label = format!("All ({total})");
                            // The count covers warnings AND errors, so the
                            // label says so instead of "Warnings".
                            let warn_label = format!("Warnings & errors ({warnings})");
                            if let Some(f) = segmented(
                                ui,
                                self.log_filter,
                                &[
                                    (LogFilter::All, all_label.as_str()),
                                    (LogFilter::Warnings, warn_label.as_str()),
                                ],
                            ) {
                                self.log_filter = f;
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    // Close hides the panel (the status bar brings
                                    // it back); first in a right-to-left row, so it
                                    // sits at the far right.
                                    if icon_text_button(ui, "x", "Close").clicked() {
                                        self.show_log = false;
                                        self.save_settings();
                                    }
                                    // Stop is only meaningful while a job runs — it stays
                                    // disabled at idle so its state tells the truth.
                                    ui.add_enabled_ui(self.busy, |ui| {
                                        if icon_text_button(ui, "octagon-x", "Stop").clicked() {
                                            self.cancel.store(true, Ordering::SeqCst);
                                        }
                                    });
                                    if icon_text_button(ui, "copy", "Copy log").clicked() {
                                        self.copy_log();
                                    }
                                },
                            );
                        });
                        ui.separator();
                        let visible = log_visible(&self.log_lines, self.log_filter);
                        let new_lines = self.log_lines.len() != self.log_len_seen;
                        let out = egui::ScrollArea::vertical().show(ui, |ui| {
                            for (i, line) in visible.iter().enumerate() {
                                // Zebra rows: alternate the card fill against the
                                // panel background so adjacent lines stay distinct.
                                let frame = egui::Frame::new()
                                    .fill(if i % 2 == 0 {
                                        t.card
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    })
                                    .inner_margin(egui::Margin::symmetric(6, 2));
                                let row = frame.show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.monospace(
                                            egui::RichText::new(&line.time).color(t.text_faint),
                                        );
                                        // The level is a short coloured tag (colour
                                        // is not the only cue — the tag text names
                                        // it); the result itself stays plain so the
                                        // log reads quietly instead of shouting.
                                        match line.level {
                                            LogLevel::Warn => {
                                                ui.colored_label(
                                                    t.warning,
                                                    egui::RichText::new("WARN").monospace().small(),
                                                );
                                            }
                                            LogLevel::Error => {
                                                ui.colored_label(
                                                    t.danger,
                                                    egui::RichText::new("ERROR")
                                                        .monospace()
                                                        .small(),
                                                );
                                            }
                                            LogLevel::Info => {
                                                ui.colored_label(
                                                    t.ok,
                                                    egui::RichText::new("INFO").monospace().small(),
                                                );
                                            }
                                        }
                                        if let Some(f) = &line.file {
                                            ui.monospace(egui::RichText::new(f).color(t.text_dim));
                                        }
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&line.result).monospace(),
                                            )
                                            .wrap(),
                                        );
                                    });
                                });
                                // Follow the newest line (the first row) while the
                                // user hasn't scrolled away from the top.
                                if i == 0 && new_lines && self.log_follow {
                                    row.response.scroll_to_me(Some(egui::Align::TOP));
                                }
                            }
                        });
                        // The user scrolled away from the top (the newest lines):
                        // stop following until they come back to the top.
                        self.log_follow = out.state.clamped_offset().y <= 1.0;
                        self.log_len_seen = self.log_lines.len();
                    });
                });
        }

        // --- Header (SPEC §3: 60px) ---
        // Brand (316px, aligned with the sidebar) | the 4-step stepper |
        // the one primary for the current step. The old title bar lived in
        // the sidebar; the old Scan/Render primaries lived in Batch Workflow
        // — both are here now, and there is exactly one primary.
        egui::Panel::top("header").exact_size(60.0).show(ui, |ui| {
            ui.add_enabled_ui(!modal_open, |ui| {
                ui.horizontal_centered(|ui| {
                    let t = crate::theme::Tokens::get(ui.ctx());
                    ui.allocate_ui_with_layout(
                        egui::vec2(316.0, 60.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(crate::icons::image_sized(
                                "vmax-mark",
                                egui::vec2(32.0, 24.0),
                                t.text,
                            ));
                            ui.vertical(|ui| {
                                ui.label(crate::theme::heading(
                                    "Caption Studio",
                                    crate::theme::SECTION,
                                ));
                                ui.label(crate::theme::label_text(
                                    "Batch image translator",
                                    crate::theme::CAPTION,
                                ));
                            });
                        },
                    );
                    ui.separator();
                    let wf = self.workflow_state();
                    // The primary reserves ~200px on the right; the four
                    // steps share the rest. The stepper is a status
                    // display only — it used to run the step's action on
                    // click, which duplicated the header primary (Scan /
                    // Render) and the tabs (Translate / Review), so the
                    // click handler was removed (round-2 duplicate-button
                    // unification). Navigation lives in the tabs; the
                    // action lives in the header primary.
                    let steps_w = (ui.available_width() - 200.0).max(0.0);
                    for (i, step) in WorkflowStep::ALL.iter().enumerate() {
                        step_button(
                            ui,
                            (steps_w / 4.0 - 8.0).max(0.0),
                            i + 1,
                            step.label(),
                            &wf.subtitles[i],
                            wf.states[i],
                        );
                        ui.add_space(8.0);
                    }
                    ui.add_enabled_ui(!self.busy, |ui| {
                        let (icon, label) = self.header_primary();
                        if primary_icon_text_button(ui, icon, &label, false).clicked() {
                            self.run_header_primary();
                        }
                    });
                });
            });
        });

        // --- Left column (sidebar, SPEC §3: fixed 316px) ---
        // The title bar moved to the header; the old panel-width persistence
        // retired with the fixed width (Q: no resizable sidebar in the
        // redesign — the mockup's columns are fixed).
        egui::Panel::left("sidebar")
            .exact_size(316.0)
            .resizable(false)
            .show(ui, |ui| {
                ui.add_enabled_ui(!modal_open, |ui| {
                    // The three cards scroll: at 1080p (and at 125% scaling,
                    // where the panel is only ~584 logical px tall) they
                    // overflow the panel, and without this the Project card —
                    // Save / Load / Open output — was unreachable.
                    // The OCR error stays here (it needs the sidebar's room);
                    // the warm-up line moved to the status bar — it pushed
                    // the scroll area down ~28px and the PROJECT card off the
                    // panel while warming up.
                    if let Some(e) = &self.ocr_error {
                        ui.colored_label(
                            crate::theme::Tokens::get(ui.ctx()).danger,
                            format!("OCR failed to start — restart the app to retry. ({e})"),
                        );
                    }
                    ui.separator();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        // One unified container for the whole sidebar: the
                        // three sections share a single frame and one inner
                        // width, separated by hairlines — the separate boxes
                        // made the layout feel fragmented.
                        let t = crate::theme::Tokens::get(ui.ctx());
                        egui::Frame::new()
                            .fill(t.card)
                            .stroke(egui::Stroke::new(1.0, t.card_border))
                            .corner_radius(egui::CornerRadius::same(t.radius_sm as u8))
                            .inner_margin(egui::Margin::same(12))
                            .show(ui, |ui| {
                                // Source & Output section (SPEC §2): path rows with the tail
                                // shown + full path on hover, 36px browse buttons; the font
                                // field stays editable (short values, placeholder for the
                                // accepted extensions); the sizing select's popup opens
                                // UPWARD so it never covers the OCR section below (defect 1).
                                sidebar_section(ui, "SOURCE & OUTPUT", |ui| {
                                    if path_row(ui, "Input folder", &self.input_dir).clicked() {
                                        self.browse_input();
                                    }
                                    ui.add_space(8.0);
                                    if path_row(ui, "Output folder", &self.output_dir).clicked() {
                                        self.browse_output();
                                    }
                                    ui.add_space(8.0);
                                    let font_label = ui
                                        .label(crate::theme::label_text(
                                            "Custom font (optional)",
                                            crate::theme::LABEL,
                                        ))
                                        .id;
                                    ui.horizontal(|ui| {
                                        // Forced to 36px like the path rows
                                        // above: a bare singleline TextEdit
                                        // sizes to its text and came out a
                                        // different height (shot 01: "no
                                        // consistent size").
                                        let field_w = ui.available_width()
                                            - 36.0
                                            - ui.spacing().item_spacing.x;
                                        let resp = ui
                                            .add_sized(
                                                egui::vec2(field_w, 36.0),
                                                egui::TextEdit::singleline(&mut self.font_path)
                                                    .hint_text(".ttf  .otf  .ttc"),
                                            )
                                            .labelled_by(font_label);
                                        focus_ring(ui, &resp);
                                        let path = self.font_path.clone();
                                        if !path.is_empty() {
                                            resp.on_hover_text(path);
                                        }
                                        let browse = crate::icons::button_box(
                                            ui,
                                            "folder",
                                            36.0,
                                            false,
                                            "Browse for font file",
                                        );
                                        focus_ring(ui, &browse);
                                        if browse.clicked() {
                                            self.browse_font();
                                        }
                                    });
                                    ui.add_space(8.0);
                                    ui.label(crate::theme::label_text(
                                        "Font sizing",
                                        crate::theme::LABEL,
                                    ));
                                    let sizing_btn = ui.add_sized(
                                        egui::vec2(ui.available_width(), 36.0),
                                        egui::Button::new(format!("{} ▾", self.sizing.label())),
                                    );
                                    focus_ring(ui, &sizing_btn);
                                    let mut picked: Option<crate::model::SizingMode> = None;
                                    // Shot-test: force the popup open for the sizing
                                    // shot (it needs the button's response id, which
                                    // only exists while the sidebar is drawn).
                                    if self
                                        .shot_test
                                        .as_ref()
                                        .map(|st| st.is_current(ShotKind::SizingPopup))
                                        .unwrap_or(false)
                                    {
                                        egui::Popup::open_id(
                                            ui.ctx(),
                                            egui::Popup::default_response_id(&sizing_btn),
                                        );
                                    }
                                    egui::Popup::menu(&sizing_btn)
                                        .close_behavior(
                                            egui::PopupCloseBehavior::CloseOnClickOutside,
                                        )
                                        // The popup is exactly the button's
                                        // width, so it stays inside the 316px
                                        // sidebar instead of spilling into the
                                        // workspace. `Popup::width` only sets
                                        // the Area's *default* width — the
                                        // Area still shrinks to its content
                                        // (the longest item, ~215px) — so the
                                        // width is also set on the content.
                                        // The explicit opaque frame keeps the
                                        // sidebar text from showing through
                                        // on the first open frame.
                                        .width(sizing_btn.rect.width())
                                        .frame(egui::Frame::popup(ui.style()).fill(t.card))
                                        // Defect 1: the old ComboBox opened downward and
                                        // covered the OCR Thresholds card. This popup opens
                                        // upward (its bottom edge at the button's top edge)
                                        // on the top layer, falling back to downward only
                                        // when it doesn't fit above.
                                        .align(egui::RectAlign::TOP_START)
                                        .align_alternatives(&[egui::RectAlign::BOTTOM_START])
                                        .show(|ui| {
                                            // Match the button's width (less the
                                            // popup frame's inner margin).
                                            ui.set_width(
                                                (sizing_btn.rect.width()
                                                    - ui.style().spacing.menu_margin.sum().x)
                                                    .max(0.0),
                                            );
                                            for mode in crate::model::SizingMode::all() {
                                                if ui
                                                    .selectable_value(
                                                        &mut self.sizing,
                                                        mode,
                                                        mode.label(),
                                                    )
                                                    .clicked()
                                                {
                                                    picked = Some(mode);
                                                    egui::Popup::close_id(
                                                        ui.ctx(),
                                                        egui::Popup::default_response_id(
                                                            &sizing_btn,
                                                        ),
                                                    );
                                                }
                                            }
                                        });
                                    if let Some(mode) = picked {
                                        self.sizing = mode;
                                        self.save_settings();
                                        self.preview_key.clear();
                                    }
                                });
                                ui.add_space(12.0);
                                ui.separator();
                                ui.add_space(12.0);

                                // OCR Thresholds section (SPEC §2): always visible; two
                                // sliders with mono value outputs; dragging schedules a
                                // debounced (50 ms) instant re-filter — no Apply button
                                // anymore. Reset restores .60/12.
                                sidebar_section(ui, "OCR THRESHOLDS", |ui| {
                                    ui.label(crate::theme::label_text(
                                        "Minimum confidence",
                                        crate::theme::LABEL,
                                    ));
                                    ui.horizontal(|ui| {
                                        let slider_w = (ui.available_width() - 52.0).max(60.0);
                                        let conf_resp = ui.add_sized(
                                            egui::vec2(slider_w, 28.0),
                                            egui::Slider::new(&mut self.conf, 0.0..=1.0)
                                                .step_by(0.01)
                                                .show_value(false),
                                        );
                                        focus_ring(ui, &conf_resp);
                                        ui.monospace(format!("{:.2}", self.conf));
                                        if conf_resp.changed() && self.extracted.is_some() {
                                            self.thresholds_changed_at =
                                                Some(std::time::Instant::now());
                                            ui.ctx().request_repaint_after(
                                                std::time::Duration::from_millis(60),
                                            );
                                        }
                                    });
                                    ui.add_space(4.0);
                                    ui.label(crate::theme::label_text(
                                        "Minimum text length",
                                        crate::theme::LABEL,
                                    ));
                                    ui.horizontal(|ui| {
                                        let slider_w = (ui.available_width() - 52.0).max(60.0);
                                        let min_resp = ui.add_sized(
                                            egui::vec2(slider_w, 28.0),
                                            egui::Slider::new(&mut self.minchars, 1..=40)
                                                .show_value(false),
                                        );
                                        focus_ring(ui, &min_resp);
                                        ui.monospace(format!("{}", self.minchars));
                                        if min_resp.changed() && self.extracted.is_some() {
                                            self.thresholds_changed_at =
                                                Some(std::time::Instant::now());
                                            ui.ctx().request_repaint_after(
                                                std::time::Duration::from_millis(60),
                                            );
                                        }
                                    });
                                    ui.add_space(4.0);
                                    ui.label(crate::theme::label_text(
                                        "Changes apply instantly to the preview.",
                                        crate::theme::CAPTION,
                                    ));
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        if icon_text_button(ui, "rotate-ccw", "Reset").clicked() {
                                            self.conf = crate::model::default_conf();
                                            self.minchars = crate::model::default_minchars();
                                            self.save_settings();
                                            if self.extracted.is_some() {
                                                self.thresholds_changed_at =
                                                    Some(std::time::Instant::now());
                                                ui.ctx().request_repaint_after(
                                                    std::time::Duration::from_millis(60),
                                                );
                                            }
                                        }
                                        // No "Inspect boxes" here: it only
                                        // switched the stage to OCR mode and
                                        // jumped to the Preview tab — exactly
                                        // what the toolbar's "OCR Boxes" mode
                                        // already does, so the thresholds
                                        // section keeps just Reset.
                                    });
                                });
                                ui.add_space(12.0);
                                ui.separator();
                                ui.add_space(12.0);

                                // Project section (SPEC §2): project file
                                // operations only — Save / Load / Open output.
                                // "Scan again" used to sit here, but it
                                // duplicated the header primary "Scan images"
                                // (both visible at once), so it was removed
                                // outright; re-scanning is the header
                                // primary's job.
                                sidebar_section(ui, "PROJECT", |ui| {
                                    // Save + Load share the first row, each
                                    // exactly half the inner width — the grid
                                    // sized buttons to their labels, so the
                                    // columns drifted apart. Open output takes
                                    // the second row full-width.
                                    let gap = ui.spacing().item_spacing.x;
                                    let half = ((ui.available_width() - gap) / 2.0).max(40.0);
                                    ui.horizontal(|ui| {
                                        if icon_text_button_sized(ui, "save", "Save captions", half)
                                            .clicked()
                                        {
                                            self.save_full_json();
                                        }
                                        if icon_text_button_sized(
                                            ui,
                                            "folder-open",
                                            "Load captions",
                                            half,
                                        )
                                        .clicked()
                                        {
                                            self.load_full_json();
                                        }
                                    });
                                    ui.add_space(8.0);
                                    if icon_text_button_sized(
                                        ui,
                                        "folder-open",
                                        "Open output",
                                        ui.available_width(),
                                    )
                                    .clicked()
                                    {
                                        self.open_output_folder();
                                    }
                                });
                            });
                    });
                });
            });

        // --- Inspector (right column, SPEC §3: fixed 380px) ---
        // The Detected Blocks list moved out of the Preview tab: the stage
        // keeps the image full-width and the inspector owns the blocks
        // (cards, Phase 4 item 13). The JSON tab (item 14) lives in the
        // central panel.
        egui::Panel::right("inspector")
            .exact_size(380.0)
            .resizable(false)
            .show(ui, |ui| {
                ui.add_enabled_ui(!modal_open, |ui| {
                    ui.label(crate::theme::heading("Detected Blocks", crate::theme::CARD));
                    let copy_resp = ui.small_button("Copy block list");
                    focus_ring(ui, &copy_resp);
                    if copy_resp.clicked() {
                        match arboard::Clipboard::new()
                            .and_then(|mut cb| cb.set_text(self.verify_text.clone()))
                        {
                            Ok(()) => self.status = "Block list copied to clipboard.".into(),
                            Err(_) => {
                                self.status = "Couldn't copy the block list — try again.".into()
                            }
                        }
                    }
                    // Block cards (editable translations, Keep/Filter toggles,
                    // fit indicator), so a card can be clicked to highlight its
                    // box. When there is no entry yet (no scan, no JSON), the
                    // guidance text stays.
                    let entry = self.preview_entry.clone();
                    egui::ScrollArea::vertical().show(ui, |ui| match &entry {
                        Some(entry) if !entry.blocks.is_empty() || !entry.dropped.is_empty() => {
                            self.show_block_cards(ui, entry);
                        }
                        _ => {
                            // A plain label, not monospace: the empty-state
                            // message is a sentence ("No text found…"), and
                            // the mono font read as an editable code field
                            // (a caret showed after "copied").
                            let t = crate::theme::Tokens::get(ui.ctx());
                            ui.label(
                                egui::RichText::new(&self.verify_text)
                                    .size(crate::theme::BODY)
                                    .color(t.text_dim),
                            );
                        }
                    });
                });
            });

        // --- Right column: tabbed workspace ---
        // The central workspace sits on `canvas` (the main reading surface),
        // a step above the chrome panels — this is what the `canvas` token is for.
        let canvas = crate::theme::Tokens::get(ui.ctx()).canvas;
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).fill(canvas))
            .show(ui, |ui| {
            ui.add_enabled_ui(!modal_open, |ui| {
            ui.horizontal(|ui| {
                let json_tab = ui.selectable_value(&mut self.tab, Tab::Json, "Translation JSON");
                focus_ring(ui, &json_tab);
                let preview_tab = ui.selectable_value(&mut self.tab, Tab::Preview, "Live Preview & OCR Check");
                focus_ring(ui, &preview_tab);
            });
            ui.separator();
            match self.tab {
                Tab::Json => {
                    // Per-image read-only JSON (Phase 4 item 14): the preview
                    // file's blocks only. Copy still copies the full compact
                    // map for the AI; Paste opens a modal; Bulk edit opens
                    // the full-document editor as a modal.
                    ui.horizontal(|ui| {
                        if icon_text_button(ui, "copy", "Copy JSON for AI").clicked() {
                            self.copy_compact_for_ai();
                        }
                        if icon_text_button(ui, "clipboard-paste", "Paste Translated JSON…").clicked() {
                            self.show_paste_modal = true;
                            self.paste_text.clear();
                        }
                        // No "Import Translated JSON…" button here: it
                        // loads the same compact map as Paste, only from a
                        // file instead of the clipboard, so the file import
                        // lives inside the paste dialog as "Import from
                        // file…" — one door for loading translations.
                        if icon_text_button(ui, "file-json", "Bulk edit…").clicked() {
                            // Regenerate the bulk map from the resolved
                            // data so it matches the current scan — the raw
                            // imported text can carry stale block ids that
                            // "aren't in the scan", and the bulk map would
                            // then disagree with the per-image JSON.
                            if let Some(data) = self.resolve_preview_data() {
                                self.editor_text =
                                    serde_json::to_string_pretty(&compact_map_from_data(&data))
                                        .unwrap_or_default();
                            }
                            self.show_bulk_edit = true;
                        }
                        // No "Verify in Preview" button here: the header
                        // primary "Review" already jumps to the preview
                        // (and to the first untranslated block), and the
                        // "Live Preview & OCR Check" tab is the same
                        // navigation — three ways to one place.
                    });
                    let data = self.resolve_preview_data();
                    let text = per_image_json(data.as_ref(), &self.preview_file);
                    // The editor fills the rest of the central panel: a
                    // fixed 30-row height left a dead gap below it when
                    // the log was closed (shot 01: "the border isn't
                    // responsive"). The multiline TextEdit scrolls
                    // internally, so no outer ScrollArea is needed.
                    let t = crate::theme::Tokens::get(ui.ctx());
                    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                        let mut job = json_highlight_job(buf.as_str(), &t);
                        job.wrap.max_width = wrap_width;
                        ui.fonts_mut(|f| f.layout_job(job))
                    };
                    let mut text = text;
                    let resp = ui.add_sized(
                        ui.available_size(),
                        egui::TextEdit::multiline(&mut text)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .layouter(&mut layouter)
                            .interactive(false)
                            .hint_text("Scan images to fill this in, or paste translated JSON here."),
                    )
                    .accessible_name("Translation JSON for this image");
                    focus_ring(ui, &resp);
                }
                Tab::Preview => {
                    // The dataset is resolved once per frame: the file list
                    // and the stage both read it, and refresh_preview no
                    // longer resolves it itself.
                    let t = crate::theme::Tokens::get(ui.ctx());
                    let data = self.resolve_preview_data();
                    self.refresh_preview(&ctx, data.as_ref());
                    let names = self.file_names();
                    let cur_idx = names.iter().position(|n| n == &self.preview_file).unwrap_or(0);
                    // --- Docked stage header bar (SPEC §3.1): a dedicated
                    // 40px bar above the stage, t.card with a 1px bottom
                    // stroke, so the view/zoom controls no longer float
                    // over the canvas. Left: prev / "N of M" / next + the
                    // active file name. Right: mode + zoom groups.
                    let bar = egui::Frame::new()
                        .fill(t.card)
                        .inner_margin(egui::Margin::symmetric(8, 4));
                    let bar_resp = bar.show(ui, |ui| {
                        ui.set_min_height(32.0);
                        ui.horizontal(|ui| {
                            if crate::icons::button(ui, "chevron-left", crate::theme::ControlSize::Medium, false, "Previous file").clicked() {
                                step_preview(&mut self.preview_file, &names, -1);
                                self.selected_block = None;
                                self.preview_key.clear();
                            }
                            ui.label(crate::theme::label_text(
                                &format!("{} of {}", if names.is_empty() { 0 } else { cur_idx + 1 }, names.len()),
                                crate::theme::LABEL,
                            ));
                            if crate::icons::button(ui, "chevron-right", crate::theme::ControlSize::Medium, false, "Next file").clicked() {
                                step_preview(&mut self.preview_file, &names, 1);
                                self.selected_block = None;
                                self.preview_key.clear();
                            }
                            // The active file name, truncated, with the full
                            // name on hover. Its width is bounded by the
                            // right-hand groups' reserved width, so a long
                            // name truncates instead of pushing the mode /
                            // zoom groups off the bar.
                            if !self.preview_file.is_empty() {
                                let zoom_pct = format!(
                                    "{}%",
                                    (self.preview_zoom * 100.0).round() as i32
                                );
                                let mode_w = segmented_button_width(
                                    ui,
                                    &["Original", "Detected Boxes", "Rendered"],
                                ) * 3.0;
                                let zoom_w = segmented_button_width(
                                    ui,
                                    &["Fit View", zoom_pct.as_str()],
                                ) * 2.0;
                                let right_reserve = mode_w
                                    + zoom_w
                                    + 1.0
                                    + 12.0
                                    + ui.spacing().item_spacing.x * 4.0;
                                let name_w = (ui.available_width() - right_reserve).max(0.0);
                                ui.add_sized(
                                    egui::vec2(name_w, 32.0),
                                    egui::Label::new(
                                        egui::RichText::new(&self.preview_file)
                                            .size(crate::theme::LABEL)
                                            .color(t.text),
                                    )
                                    .truncate()
                                    .selectable(false),
                                )
                                .on_hover_text(&self.preview_file);
                            }
                            // The mode + zoom controls ride at the right end.
                            // Disabled until a preview exists — with zero images
                            // the segmented groups lit up ("Rendered", "Fit")
                            // over an empty stage.
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                // A right inset, so the groups sit inside the
                                // bar instead of flush against the panel's
                                // right edge (shot 03: "not properly within
                                // the view"). In a right-to-left layout the
                                // first item lands at the right edge.
                                ui.add_space(12.0);
                                ui.add_enabled_ui(!names.is_empty(), |ui| {
                                // The manual-zoom button shows the LIVE zoom
                                // (e.g. "200%"), not a fixed "100%" — the old
                                // label lit up at any manual zoom and lied
                                // about the level. Clicking it resets to 100%.
                                let zoom_pct =
                                    format!("{}%", (self.preview_zoom * 100.0).round() as i32);
                                if let Some(fit) = segmented(
                                    ui,
                                    self.preview_zoom_fit,
                                    &[(true, "Fit View"), (false, zoom_pct.as_str())],
                                ) {
                                    self.preview_zoom_fit = fit;
                                    self.preview_zoom = 1.0;
                                    self.preview_pan = egui::Vec2::ZERO;
                                }
                                // 16px vertical divider between the mode
                                // and zoom groups.
                                let (div, _) = ui.allocate_exact_size(
                                    egui::vec2(1.0, 16.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(div, 0.0, t.card_border);
                                if let Some(mode) = segmented(ui, self.preview_mode, &[
                                    (PreviewMode::Rendered, "Rendered"),
                                    (PreviewMode::Ocr, "Detected Boxes"),
                                    (PreviewMode::Original, "Original"),
                                ]) {
                                    self.preview_mode = mode;
                                    self.preview_key.clear();
                                }
                                });
                            });
                        });
                    });
                    // The bar's bottom stroke: 1px card_border along its
                    // bottom edge (a Frame stroke would box all four sides).
                    let bar_rect = bar_resp.response.rect;
                    ui.painter().hline(
                        bar_rect.x_range(),
                        bar_rect.bottom(),
                        egui::Stroke::new(1.0, t.card_border),
                    );
                    // --- Workspace: file list (230px) | stage. ---
                    let rest = ui.available_size();
                    let (filter_id, stage_resp) = ui
                        .horizontal_top(|ui| {
                            // File list.
                            let filter_resp = ui
                                .allocate_ui_with_layout(
                                    egui::vec2(250.0, rest.y),
                                    egui::Layout::top_down(egui::Align::LEFT),
                                    |ui| {
                                        // Filter + a clear (×) button when
                                        // there is text to clear.
                                        let filter_resp = ui
                                            .horizontal(|ui| {
                                                let filter_resp = ui.add(
                                                    egui::TextEdit::singleline(
                                                        &mut self.preview_filter,
                                                    )
                                                    .hint_text(format!(
                                                        "Filter {} images…",
                                                        names.len()
                                                    ))
                                                    .desired_width(250.0 - 32.0),
                                                );
                                                focus_ring(ui, &filter_resp);
                                                if !self.preview_filter.is_empty()
                                                    && ui
                                                        .small_button("×")
                                                        .on_hover_text("Clear filter")
                                                        .clicked()
                                                {
                                                    self.preview_filter.clear();
                                                }
                                                filter_resp
                                            })
                                            .inner;
                                        let filtered = filter_file_names(&names, &self.preview_filter);
                                        let mut budget = 4;
                                        let mut picked: Option<String> = None;
                                        // The list scrolls with the wheel
                                        // and the Up/Down keys, and the
                                        // scrollbar shows when the content
                                        // overflows — the hidden scrollbar
                                        // left the overflow undiscoverable
                                        // (the bottom row sat half-clipped
                                        // with no sign the list scrolled and
                                        // no way to drag to the bottom rows).
                                        // The ScrollArea fills the column
                                        // height instead of shrinking to its
                                        // content, so as many whole rows as
                                        // possible are visible.
                                        egui::ScrollArea::vertical()
                                            .auto_shrink([false, false])
                                            .scroll_bar_visibility(
                                                egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                                            )
                                            .show(ui, |ui| {
                                            if filtered.is_empty() {
                                                // While a scan runs the list
                                                // is empty because nothing has
                                                // been found yet, not because
                                                // the filter excluded it.
                                                let msg = if self.busy {
                                                    "Scanning…"
                                                } else {
                                                    "No files match."
                                                };
                                                ui.label(crate::theme::label_text(msg, crate::theme::CAPTION));
                                            }
                                            for n in filtered {
                                                if self.file_list_row(ui, n, data.as_ref(), &mut budget) {
                                                    picked = Some(n.clone());
                                                }
                                            }
                                        });
                                        if let Some(name) = picked {
                                            self.set_preview_file(name);
                                        }
                                        filter_resp
                                    },
                                )
                                .inner;
                            ui.separator();
                            // Stage.
                            let stage_w = ui.available_width();
                            let stage_resp = ui
                                .allocate_ui_with_layout(
                                    egui::vec2(stage_w, rest.y),
                                    egui::Layout::top_down(egui::Align::LEFT),
                                    |ui| {
                                        if matches!(self.preview_mode, PreviewMode::Ocr) {
                                            ui.label(crate::theme::label_text("Green boxes = kept · Orange boxes = filtered out", crate::theme::CAPTION));
                                        }
                                        let stage_size = ui.available_size();
                                        let (stage_rect, stage_resp) = ui.allocate_exact_size(stage_size, egui::Sense::click_and_drag());
                                        // Checkerboard stage background (SPEC §5).
                                        let painter = ui.painter();
                                        painter.rect_filled(stage_rect, 0.0, t.canvas);
                                        let cell = 16.0;
                                        let mut y = stage_rect.min.y;
                                        let mut row = 0usize;
                                        while y < stage_rect.max.y {
                                            let mut x = stage_rect.min.x;
                                            let mut col = 0usize;
                                            while x < stage_rect.max.x {
                                                if (row + col) % 2 == 1 {
                                                    let cell_rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell, cell)).intersect(stage_rect);
                                                    painter.rect_filled(cell_rect, 0.0, t.card);
                                                }
                                                x += cell;
                                                col += 1;
                                            }
                                            y += cell;
                                            row += 1;
                                        }
                                        if let Some(tex) = &self.preview_texture {
                                            let (iw, ih) = (self.preview_size[0], self.preview_size[1]);
                                            let fit_scale = (stage_rect.width() / iw).min(stage_rect.height() / ih).min(1.5).max(0.05);
                                            // Wheel zoom (around the cursor)
                                            // and drag pan both exit Fit into
                                            // a manual zoom — Fit is a preset,
                                            // not a mode. `0` resets to Fit.
                                            // Zoom/pan only change the display,
                                            // never the texture, so the
                                            // preview key is left alone.
                                            if stage_resp.hovered() {
                                                let scroll = ui.ctx().input(|i| i.smooth_scroll_delta);
                                                if scroll.y != 0.0 {
                                                    if self.preview_zoom_fit {
                                                        self.preview_zoom = fit_scale;
                                                        self.preview_zoom_fit = false;
                                                    }
                                                    let old_scale = self.preview_zoom;
                                                    let new_scale = (old_scale * (-scroll.y * 0.002).exp()).clamp(0.05, 4.0);
                                                    if let (Some(cursor), true) = (stage_resp.hover_pos(), self.preview_image_rect.is_positive()) {
                                                        let img_pt = (cursor - self.preview_image_rect.min) / old_scale;
                                                        let new_size = egui::vec2(iw * new_scale, ih * new_scale);
                                                        let new_min = cursor - img_pt * new_scale;
                                                        self.preview_pan = new_min + new_size / 2.0 - stage_rect.center();
                                                        self.preview_pan = clamp_pan(self.preview_pan, new_size, stage_rect.size());
                                                    }
                                                    self.preview_zoom = new_scale;
                                                }
                                                if ui.ctx().input(|i| i.key_pressed(egui::Key::Num0)) {
                                                    self.preview_zoom_fit = true;
                                                    self.preview_pan = egui::Vec2::ZERO;
                                                }
                                            }
                                            if stage_resp.dragged() {
                                                if self.preview_zoom_fit {
                                                    self.preview_zoom = fit_scale;
                                                    self.preview_zoom_fit = false;
                                                }
                                                self.preview_pan += stage_resp.drag_delta();
                                                let size = egui::vec2(iw * self.preview_zoom, ih * self.preview_zoom);
                                                self.preview_pan = clamp_pan(self.preview_pan, size, stage_rect.size());
                                            }
                                            let scale = if self.preview_zoom_fit { fit_scale } else { self.preview_zoom };
                                            let size = egui::vec2(iw * scale, ih * scale);
                                            // Keep the pan in bounds every
                                            // frame, not only while dragging
                                            // or zooming, so a stale or
                                            // programmatically set pan can
                                            // never leave the image off-stage.
                                            self.preview_pan =
                                                clamp_pan(self.preview_pan, size, stage_rect.size());
                                            let image_rect = egui::Rect::from_center_size(stage_rect.center() + self.preview_pan, size);
                                            self.preview_image_rect = image_rect;
                                            self.preview_image_scale = scale;
                                            // The image is clipped to the
                                            // stage rect, so even at max
                                            // zoom it can never draw over the
                                            // file list or the toolbar.
                                            ui.painter().with_clip_rect(stage_rect).image(
                                                tex.id(),
                                                image_rect,
                                                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
                                                egui::Color32::WHITE,
                                            );
                                            // Position cue: at a manual zoom
                                            // the image is cropped with no
                                            // scrollbar, so the user can't
                                            // tell which part they're seeing.
                                            // A small "Drag to pan" hint at
                                            // the stage's bottom-right while
                                            // the image overflows the stage.
                                            if !self.preview_zoom_fit
                                                && (size.x > stage_rect.width()
                                                    || size.y > stage_rect.height())
                                            {
                                                let hint = "Drag to pan";
                                                let font =
                                                    egui::FontId::proportional(crate::theme::CAPTION);
                                                let text_w = ui
                                                    .ctx()
                                                    .fonts_mut(|f| {
                                                        f.layout_no_wrap(hint.to_string(), font.clone(), t.text)
                                                            .size()
                                                            .x
                                                    });
                                                let pad = egui::vec2(8.0, 4.0);
                                                let bg_rect = egui::Rect::from_min_size(
                                                    egui::pos2(
                                                        stage_rect.max.x - text_w - pad.x * 2.0 - 8.0,
                                                        stage_rect.max.y
                                                            - crate::theme::CAPTION
                                                            - pad.y * 2.0
                                                            - 8.0,
                                                    ),
                                                    egui::vec2(text_w + pad.x * 2.0, crate::theme::CAPTION + pad.y * 2.0),
                                                );
                                                let painter =
                                                    ui.painter().with_clip_rect(stage_rect);
                                                painter.rect_filled(
                                                    bg_rect,
                                                    egui::CornerRadius::same(4),
                                                    egui::Color32::from_black_alpha(160),
                                                );
                                                painter.text(
                                                    bg_rect.center(),
                                                    egui::Align2::CENTER_CENTER,
                                                    hint,
                                                    font,
                                                    t.text,
                                                );
                                            }
                                            // Click (not drag) in OCR Boxes
                                            // mode selects the block under the
                                            // pointer — the list scrolls to
                                            // its row and the box turns Max
                                            // Blue (and vice versa).
                                            if stage_resp.clicked() && matches!(self.preview_mode, PreviewMode::Ocr) {
                                                if let (Some(pos), Some(entry)) = (stage_resp.interact_pointer_pos(), &self.preview_entry) {
                                                    let x = (pos.x - image_rect.min.x) / scale;
                                                    let y = (pos.y - image_rect.min.y) / scale;
                                                    self.selected_block = block_at(entry, x, y);
                                                    self.blocks_scroll_to_selected = true;
                                                    self.preview_key.clear();
                                                }
                                            }
                                        } else {
                                            ui.painter().text(
                                                stage_rect.center(),
                                                egui::Align2::CENTER_CENTER,
                                                "Scan & Extract Text or paste translated JSON to preview.",
                                                egui::FontId::proportional(crate::theme::BODY),
                                                t.text_dim,
                                            );
                                        }
                                        stage_resp
                                    },
                                )
                                .inner;
                            (filter_resp.id, stage_resp)
                        })
                        .inner;
                    // Keyboard nav (Up/Down move, Enter focuses the stage):
                    // active while nothing has focus, or while the filter
                    // has it — the filter is the only text field in this tab.
                    let focused = ui.ctx().memory(|m| m.focused());
                    let list_nav = focused.is_none() || focused == Some(filter_id);
                    if list_nav {
                        let filtered = filter_file_names(&names, &self.preview_filter);
                        if ui.ctx().input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                            step_in_names(&mut self.preview_file, &filtered, 1);
                            self.selected_block = None;
                            self.preview_key.clear();
                        }
                        if ui.ctx().input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                            step_in_names(&mut self.preview_file, &filtered, -1);
                            self.selected_block = None;
                            self.preview_key.clear();
                        }
                        if ui.ctx().input(|i| i.key_pressed(egui::Key::Enter)) {
                            stage_resp.request_focus();
                        }
                    }
                }
            }
            });
        });
    }
}

/// Step the selection within a (possibly filtered) name list, wrapping.
fn step_in_names(current: &mut String, names: &[&String], delta: i32) {
    if names.is_empty() {
        return;
    }
    let idx = names.iter().position(|n| *n == current).unwrap_or(0) as i32;
    let next = (idx + delta).rem_euclid(names.len() as i32) as usize;
    *current = names[next].clone();
}

/// Clamp a stage pan so at least 40px of the image stays visible (and
/// grabbable) in the stage.
/// Clamp the stage pan so the image stays inside the stage: when the
/// image is smaller than the stage it is centred (no pan); when it is
/// larger, the pan is limited so the image's edge can't cross the stage's
/// opposite edge — the image always covers the stage, never slides off it
/// (and never spills over the file list / toolbar). The pan is the offset
/// of the image's centre from the stage's centre.
fn clamp_pan(pan: egui::Vec2, size: egui::Vec2, stage: egui::Vec2) -> egui::Vec2 {
    let clamp_axis = |p: f32, img: f32, stg: f32| -> f32 {
        if img <= stg {
            0.0
        } else {
            p.clamp(-(img - stg) / 2.0, (img - stg) / 2.0)
        }
    };
    egui::vec2(
        clamp_axis(pan.x, size.x, stage.x),
        clamp_axis(pan.y, size.y, stage.y),
    )
}

fn step_preview(current: &mut String, names: &[String], delta: i32) {
    if names.is_empty() {
        return;
    }
    let idx = names.iter().position(|n| n == current).unwrap_or(0) as i32;
    let next = (idx + delta).rem_euclid(names.len() as i32) as usize;
    *current = names[next].clone();
}

/// The per-button width of a segmented group: the widest label plus the
/// button's horizontal padding on both sides. The docked stage header bar
/// uses it to reserve the right-hand groups' width before laying out the
/// (flexible, truncating) file name, so a long name can never push the
/// mode/zoom groups off the bar.
fn segmented_button_width(ui: &egui::Ui, labels: &[&str]) -> f32 {
    let t = crate::theme::Tokens::get(ui.ctx());
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    let max_text_w = labels
        .iter()
        .map(|label| {
            ui.ctx().fonts_mut(|f| {
                f.layout_no_wrap((*label).to_string(), font_id.clone(), t.text)
                    .size()
                    .x
            })
        })
        .fold(0.0, f32::max);
    max_text_w + ui.spacing().button_padding.x * 2.0
}

/// A segmented control (mockup `.seg`): adjacent buttons, the selected one
/// solid accent with white text, the rest on the unified `t.raised` track
/// with a border. Returns the picked value, if the user clicked one.
fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    current: T,
    options: &[(T, &str)],
) -> Option<T> {
    let t = crate::theme::Tokens::get(ui.ctx());
    let mut picked = None;
    // Equal widths within the group: measure the widest label and give
    // every button that width, so the group reads as one consistent
    // control instead of label-sized buttons (shot 03: "not consistent
    // shape"). The button's own horizontal padding is added back so the
    // widest label doesn't touch the button's edges.
    let labels: Vec<&str> = options.iter().map(|(_, label)| *label).collect();
    let btn_w = segmented_button_width(ui, &labels);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let n = options.len();
        for (i, (value, label)) in options.iter().enumerate() {
            let selected = *value == current;
            let r = t.radius_sm as u8;
            let radius = if n == 1 {
                egui::CornerRadius::same(r)
            } else if i == 0 {
                egui::CornerRadius {
                    nw: r,
                    ne: 0,
                    sw: r,
                    se: 0,
                }
            } else if i == n - 1 {
                egui::CornerRadius {
                    nw: 0,
                    ne: r,
                    sw: 0,
                    se: r,
                }
            } else {
                egui::CornerRadius::ZERO
            };
            let text = egui::RichText::new(*label).color(if selected {
                egui::Color32::WHITE
            } else {
                t.text
            });
            let btn = egui::Button::new(text)
                // 32px, the Medium control height: the segmented groups
                // came out shorter than the chevron buttons beside them
                // (shot 03: "not consistent shape"), so the two groups
                // and the toolbar read as different controls.
                .min_size(egui::vec2(btn_w, 32.0))
                .fill(if selected { t.accent } else { t.raised })
                .stroke(if selected {
                    egui::Stroke::NONE
                } else {
                    egui::Stroke::new(1.0, t.card_border)
                })
                .corner_radius(radius);
            let resp = ui.add(btn);
            focus_ring(ui, &resp);
            if resp.clicked() {
                picked = Some(*value);
            }
        }
    });
    picked
}

/// A small status chip (file-list rows): coloured text on a tint of the
/// same colour.
fn status_chip(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    let frame = egui::Frame::new()
        .fill(color.linear_multiply(0.15))
        .corner_radius(egui::CornerRadius::same(4))
        .inner_margin(egui::Margin::symmetric(6, 2));
    frame.show(ui, |ui| {
        // Not selectable: a selectable label takes click-and-drag sense
        // and would steal the file-list row's click (same reason as the
        // row's filename/count labels).
        ui.add(
            egui::Label::new(
                egui::RichText::new(text)
                    .size(crate::theme::CAPTION)
                    .color(color),
            )
            .selectable(false)
            .sense(egui::Sense::hover()),
        );
    });
}

/// The file-list status chip for one image. Failed wins (the last render
/// run), then rendered, then no text, then untranslated, then ready — all
/// translated but not rendered yet. The plan's chip set has no "ready": it
/// covers the state the other four miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileStatus {
    Rendered,
    NoText,
    Failed,
    Untranslated,
    Ready,
}

fn file_status(entry: &crate::model::ImageEntry, rendered: bool, failed: bool) -> FileStatus {
    if failed {
        return FileStatus::Failed;
    }
    if rendered {
        return FileStatus::Rendered;
    }
    // Only kept blocks count: a file whose blocks are all filtered out
    // renders nothing, so it reads as "no text", not "untranslated".
    if entry.blocks.iter().all(|b| !b.kept()) {
        return FileStatus::NoText;
    }
    if entry
        .blocks
        .iter()
        .any(|b| b.kept() && b.new_text.trim().is_empty())
    {
        return FileStatus::Untranslated;
    }
    FileStatus::Ready
}

fn file_status_label(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Rendered => "rendered",
        FileStatus::NoText => "no text",
        FileStatus::Failed => "failed",
        FileStatus::Untranslated => "untranslated",
        FileStatus::Ready => "ready",
    }
}

/// The per-image compact JSON for one file (JSON tab, Phase 4 item 14):
/// {"<file>": {<id>: "<translation>", ...}} — the same values the full
/// compact map carries, read-only. Empty when there is no entry for the file.
fn per_image_json(data: Option<&crate::model::ExtractedData>, file: &str) -> String {
    let Some(entry) = data.and_then(|d| d.entry(file)) else {
        return String::new();
    };
    let mut map = serde_json::Map::new();
    for blk in &entry.blocks {
        let text = if blk.new_text.trim().is_empty() {
            blk.original_text.clone()
        } else {
            blk.new_text.clone()
        };
        map.insert(blk.id.to_string(), serde_json::Value::String(text));
    }
    let mut doc = serde_json::Map::new();
    doc.insert(file.to_string(), serde_json::Value::Object(map));
    serde_json::to_string_pretty(&serde_json::Value::Object(doc)).unwrap_or_default()
}

/// The status-bar counts (Phase 5, item 16): `N images · N rendered ·
/// N copied unchanged · N failed` for the last render run (0s when no run
/// has finished yet).
fn render_counts(images: usize, summary: Option<(usize, usize, usize)>) -> String {
    let (rendered, copied, failed) = summary.unwrap_or((0, 0, 0));
    format!("{images} images · {rendered} rendered · {copied} copied unchanged · {failed} failed")
}

/// (file count, untranslated-block count) for a dataset. A block counts as
/// untranslated while its new text is empty — nothing has been entered yet.
/// Filtered-out blocks need no translation, so they count in neither.
fn counts_for(data: &crate::model::ExtractedData) -> (usize, usize) {
    let files = data.images.len();
    let untranslated = data
        .images
        .iter()
        .flat_map(|e| e.blocks.iter())
        .filter(|b| b.kept() && b.new_text.trim().is_empty())
        .count();
    (files, untranslated)
}

/// The inspector's fit line, with singular/plural agreement ("1 line",
/// "1 char" — the old format always said "lines"/"chars", so a one-line
/// block read "1 lines").
fn fit_label(fits: bool, lines: usize, chars: usize, size: i32) -> String {
    let line_word = if lines == 1 { "line" } else { "lines" };
    let char_word = if chars == 1 { "char" } else { "chars" };
    if fits {
        format!("Fits plate · {lines} {line_word} · {chars} {char_word}")
    } else {
        format!("Shrinks to {size}px · {lines} {line_word} · {chars} {char_word}")
    }
}

/// Pure derivation of the header stepper state from a dataset + the last
/// render summary (see `workflow_state` for the rules). Kept pure so the
/// step logic is unit-tested without building the app.
fn derive_workflow_state(
    data: Option<&crate::model::ExtractedData>,
    render_summary: Option<(usize, usize, usize)>,
) -> WorkflowState {
    let (images, total_blocks, translated, unchanged) = match data {
        Some(d) => {
            // Only kept blocks count towards Translate: a filtered-out
            // block needs no translation, and an image with no kept blocks
            // is copied unchanged.
            let total = d
                .images
                .iter()
                .flat_map(|e| e.blocks.iter())
                .filter(|b| b.kept())
                .count();
            let translated = d
                .images
                .iter()
                .flat_map(|e| e.blocks.iter())
                .filter(|b| {
                    let t = b.new_text.trim();
                    b.kept() && !t.is_empty() && t != b.original_text.trim()
                })
                .count();
            let unchanged = d
                .images
                .iter()
                .filter(|e| e.blocks.iter().all(|b| !b.kept()))
                .count();
            (d.images.len(), total, translated, unchanged)
        }
        None => (0, 0, 0, 0),
    };
    let scan_done = data.is_some();
    // Zero kept blocks after a scan is vacuously done (nothing to
    // translate); with nothing scanned at all, Translate is still to do.
    let translate_done = scan_done && (total_blocks == 0 || translated == total_blocks);
    // Review is done once a render was attempted (the user reviewed before
    // rendering); Render is done only when a run finished with no failures.
    // A failed run leaves Render current, with the failed count in its
    // subtitle, so the user re-renders instead of moving on.
    let review_done = render_summary.is_some();
    let render_done = render_summary.map(|(_, _, f)| f == 0).unwrap_or(false);
    let done = [scan_done, translate_done, review_done, render_done];
    let current_idx = done.iter().position(|d| !d).unwrap_or(3);
    let mut states = [StepState::Todo; 4];
    for (i, s) in states.iter_mut().enumerate() {
        *s = if done[i] {
            StepState::Done
        } else if i == current_idx {
            StepState::Current
        } else {
            StepState::Todo
        };
    }
    let subtitles = [
        if scan_done {
            format!("{images} images · {total_blocks} blocks")
        } else {
            "No images yet".to_string()
        },
        if scan_done {
            format!("{translated} of {total_blocks} translated")
        } else {
            "—".to_string()
        },
        if scan_done {
            format!("{unchanged} will copy unchanged")
        } else {
            "—".to_string()
        },
        match render_summary {
            Some((r, _, 0)) => format!("{r} rendered"),
            Some((r, _, f)) => format!("{r} rendered · {f} failed"),
            None => "Not rendered yet".to_string(),
        },
    ];
    WorkflowState {
        current: WorkflowStep::ALL[current_idx],
        states,
        subtitles,
        images,
    }
}

/// Case-insensitive substring filter for the file list. An empty query
/// matches everything.
fn filter_file_names<'a>(names: &'a [String], query: &str) -> Vec<&'a String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return names.iter().collect();
    }
    names
        .iter()
        .filter(|n| n.to_lowercase().contains(&q))
        .collect()
}

/// Natural ordering for file names: digit runs compare by value, so
/// `2 of 75.png` sorts before `10 of 75.png` (plain `sort()` puts 10
/// first — redesign defect 2, SPEC §11). Non-digit runs compare
/// lexicographically; equal numbers tie-break on fewer leading zeros.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek(), bi.peek()) {
            (None, None) => return Equal,
            (None, Some(_)) => return Less,
            (Some(_), None) => return Greater,
            (Some(&ac), Some(&bc)) => {
                if ac.is_ascii_digit() && bc.is_ascii_digit() {
                    let mut an = String::new();
                    while let Some(&c) = ai.peek() {
                        if c.is_ascii_digit() {
                            an.push(c);
                            ai.next();
                        } else {
                            break;
                        }
                    }
                    let mut bn = String::new();
                    while let Some(&c) = bi.peek() {
                        if c.is_ascii_digit() {
                            bn.push(c);
                            bi.next();
                        } else {
                            break;
                        }
                    }
                    let at = an.trim_start_matches('0');
                    let bt = bn.trim_start_matches('0');
                    let ord = at.len().cmp(&bt.len()).then_with(|| at.cmp(bt));
                    if ord != Equal {
                        return ord;
                    }
                    // Same number: fewer leading zeros first.
                    let ord = an.len().cmp(&bn.len());
                    if ord != Equal {
                        return ord;
                    }
                } else {
                    let ord = ac.cmp(&bc);
                    if ord != Equal {
                        return ord;
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
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
/// One item of the header stepper (SPEC §2): a 22px circle (number, or a
/// green ✓ once done) beside the step label and its live subtitle. Done
/// steps get a green circle, the current step a blue circle + blue border,
/// future steps a dim circle + hairline border. Custom-painted because a
/// plain egui button can't carry the circle + two text sizes. Status only:
/// the stepper used to run the step's action on click, which duplicated
/// the header primary (Scan / Render) and the tabs (Translate / Review),
/// so it is no longer clickable or focusable — navigation lives in the
/// tabs, the action in the header primary.
fn step_button(
    ui: &mut egui::Ui,
    width: f32,
    number: usize,
    label: &str,
    subtitle: &str,
    state: StepState,
) -> egui::Response {
    let t = crate::theme::Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 40.0), egui::Sense::hover());
    let resp = resp.accessible_name(format!("Step {number}: {label}, {subtitle}"));
    let painter = ui.painter_at(rect);
    let bg = t.card;
    let border = match state {
        StepState::Current => t.accent,
        _ => t.card_border,
    };
    painter.rect(
        rect,
        t.radius_sm,
        bg,
        egui::Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    let circle_center = egui::pos2(rect.min.x + 21.0, rect.center().y);
    let (circle_fill, circle_text) = match state {
        StepState::Done => (t.ok, egui::Color32::BLACK),
        StepState::Current => (t.accent, egui::Color32::WHITE),
        StepState::Todo => (egui::Color32::TRANSPARENT, t.text_faint),
    };
    painter.circle_filled(circle_center, 11.0, circle_fill);
    if matches!(state, StepState::Todo) {
        painter.circle_stroke(circle_center, 11.0, egui::Stroke::new(1.0, t.text_faint));
    }
    let mark = if matches!(state, StepState::Done) {
        "✓".to_string()
    } else {
        number.to_string()
    };
    painter.text(
        circle_center,
        egui::Align2::CENTER_CENTER,
        mark,
        crate::theme::display(12.0),
        circle_text,
    );
    let text_x = rect.min.x + 42.0;
    painter.text(
        egui::pos2(text_x, rect.center().y - 8.0),
        egui::Align2::LEFT_CENTER,
        label,
        crate::theme::medium(13.0),
        t.text,
    );
    painter.text(
        egui::pos2(text_x, rect.center().y + 8.0),
        egui::Align2::LEFT_CENTER,
        subtitle,
        egui::FontId::proportional(12.0),
        t.text_dim,
    );
    resp
}

/// A sidebar card (SPEC §3): p2 fill, hairline border, 8px radius, the
/// section title in Chakra 600 13px accent_ink, 12px inner padding and a
/// 12px gap to the next card (the caller adds the gap).
/// One section of the unified sidebar: a heading + contents, no frame of
/// its own — the sidebar is a single container, so sections are separated
/// by hairlines, not boxes (the separate cards made the layout feel
/// fragmented, and their inner widths drifted apart).
fn sidebar_section(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.label(
        egui::RichText::new(title)
            .font(crate::theme::display(13.0))
            .color(t.accent_ink),
    );
    ui.add_space(8.0);
    add_contents(ui);
}

/// The width of `text` in the body font, from per-glyph advances (memoized
/// by egui, so repeated calls are cheap; kerning is ignored, which only
/// costs a few pixels of slack in the ellipsis).
fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    let font_id = egui::FontId::proportional(crate::theme::BODY);
    ui.ctx()
        .fonts_mut(|fonts| text.chars().map(|c| fonts.glyph_width(&font_id, c)).sum())
}

/// "…" + the last `keep` chars of `text` (the whole text when it already
/// fits, "…" when nothing fits). Pure, so it can be unit-tested.
fn tail_ellipsis_str(text: &str, keep: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if keep == 0 {
        return "…".to_string();
    }
    if keep >= chars.len() {
        return text.to_string();
    }
    std::iter::once('…')
        .chain(chars[chars.len() - keep..].iter().copied())
        .collect()
}

/// The tail of `text` that fits in `max_width`, with a leading "…" when
/// anything was cut — folder paths read tail-first ("…\\translated_output").
fn tail_ellipsis(ui: &egui::Ui, text: &str, max_width: f32) -> String {
    if text_width(ui, text) <= max_width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let candidate = tail_ellipsis_str(text, mid);
        if text_width(ui, &candidate) <= max_width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    tail_ellipsis_str(text, lo)
}

/// A folder path row: the label above, then a 36px field showing the tail
/// of the path (leading "…" when it overflows) with the full path on hover,
/// and a 36px browse button. The path is set via Browse, not typed — a
/// singleline TextEdit shows the start of a long path and can't show a
/// leading ellipsis. Returns the browse button's response.
fn path_row(ui: &mut egui::Ui, label: &str, path: &str) -> egui::Response {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.label(crate::theme::label_text(label, crate::theme::LABEL));
    ui.horizontal(|ui| {
        let gap = ui.spacing().item_spacing.x;
        let field_w = (ui.available_width() - 36.0 - gap).max(40.0);
        let shown = if path.is_empty() {
            "(none)".to_string()
        } else {
            tail_ellipsis(ui, path, field_w - 16.0)
        };
        let field_resp = egui::Frame::new()
            .fill(t.field)
            .stroke(egui::Stroke::new(1.0, t.field_border))
            .corner_radius(egui::CornerRadius::same(t.radius_sm as u8))
            .inner_margin(egui::Margin::symmetric(8, 0))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(field_w - 16.0, 36.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        // No wrap: the tail-ellipsis already fits the text
                        // to this width, and a wrapped second line would
                        // make the field taller than the 36px the other
                        // rows use (shot 01: "no consistent size").
                        // `.truncate()` is the safety net — it cuts the
                        // text (with …) instead of wrapping it.
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(shown)
                                    .font(egui::FontId::proportional(crate::theme::BODY))
                                    .color(if path.is_empty() {
                                        t.text_faint
                                    } else {
                                        t.text
                                    }),
                            )
                            .truncate(),
                        );
                    },
                );
            })
            .response;
        if !path.is_empty() {
            field_resp.on_hover_text(path);
        }
        let browse = crate::icons::button_box(ui, "folder", 36.0, false, "Browse");
        focus_ring(ui, &browse);
        browse
    })
    .inner
}

fn icon_text_button(ui: &mut egui::Ui, icon: &str, label: &str) -> egui::Response {
    let t = crate::theme::Tokens::get(ui.ctx());
    let tint = if ui.is_enabled() {
        t.text
    } else {
        t.text_faint
    };
    let image = crate::icons::image(icon, crate::theme::ControlSize::Small.icon(), tint);
    let resp = ui.add(egui::Button::image_and_text(image, label));
    focus_ring(ui, &resp);
    resp
}

/// `icon_text_button` at a fixed width and the standard 36px control
/// height — the Project section's two columns are exactly half the inner
/// width each, so the four buttons line up instead of sizing to their
/// labels.
fn icon_text_button_sized(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    width: f32,
) -> egui::Response {
    let t = crate::theme::Tokens::get(ui.ctx());
    let tint = if ui.is_enabled() {
        t.text
    } else {
        t.text_faint
    };
    let image = crate::icons::image(icon, crate::theme::ControlSize::Small.icon(), tint);
    let resp = ui.add_sized(
        egui::vec2(width, 36.0),
        egui::Button::image_and_text(image, label),
    );
    focus_ring(ui, &resp);
    resp
}

/// A primary action: accent fill, white icon and label. The screen's main
/// steps (Scan, Review, Render) get this so they read as the thing to do,
/// not as one more secondary button. `full_width` makes it the panel's full
/// width at the default control height (Scan and Render).
fn primary_icon_text_button(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    full_width: bool,
) -> egui::Response {
    let t = crate::theme::Tokens::get(ui.ctx());
    let size = if full_width {
        crate::theme::ControlSize::Medium
    } else {
        crate::theme::ControlSize::Small
    };
    let tint = if ui.is_enabled() {
        egui::Color32::WHITE
    } else {
        t.text_faint
    };
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
            // `accent_ink`, not `accent`: accent_ink is 9.21:1 on the field
            // and on chrome (both black now — the contrast test pins this),
            // while the Max Blue ring would be 4.06:1: it passes, but the
            // lighter blue reads as the focus colour everywhere else too.
            egui::Stroke::new(1.5, t.accent_ink),
            egui::StrokeKind::Outside,
        );
    }
}

/// Syntax-highlight the Translation JSON editor: keys in accent_ink, string
/// values in the text colour, numbers/booleans/null in amber, punctuation
/// dimmed, and empty strings (`""`) visibly dimmed so a missing translation
/// reads as empty instead of blending in. "Dimmed" here is `text_dim`, not
/// `text_faint`: faint would pass on the field now (5.5:1 on black), but dim
/// (8.08:1) is kept because an empty string must read as a deliberate
/// signal, not ambient chrome text. Tolerant of invalid JSON (the user
/// is mid-edit) — it tokenizes by shape and never parses. Byte-safe: the
/// structural characters are all ASCII, so scanning bytes for them can never
/// mistake a multi-byte character for one, and spans only ever start and end
/// on character boundaries.
fn json_highlight_job(text: &str, t: &crate::theme::Tokens) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let font = egui::FontId::monospace(crate::theme::CODE);
    let mut job = LayoutJob::default();
    let push = |job: &mut LayoutJob, s: &str, color: egui::Color32| {
        job.append(
            s,
            0.0,
            TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
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
                    push(&mut job, s, t.text_dim);
                } else if is_key {
                    push(&mut job, s, t.accent_ink);
                } else {
                    push(&mut job, s, t.text);
                }
            }
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                push(&mut job, &text[i..i + 1], t.text_dim);
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
                    let Some(ch) = text[i..].chars().next() else {
                        break;
                    };
                    if matches!(
                        ch,
                        '"' | '{' | '}' | '[' | ']' | ':' | ',' | '-' | 't' | 'f' | 'n'
                    ) || ch.is_ascii_digit()
                    {
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
/// transparent; the title bar is dark). If the embedded PNG
/// ever fails to decode, the app still starts, just without an icon.
static VMAX_ICON_PNG: &[u8] = include_bytes!("../assets/icons/vmax-icon-1024.png");

// --- Shot-test mode ---
// A CLI test mode (`caption-studio --shot-test <input> <output>
// [--json <file>]`) that drives the app through every reviewable state
// on the user's PC and saves a PNG per state into the output folder, so
// the screenshots can be uploaded for review. It runs the REAL scan (real
// OCR) on the given folder and imports the REAL translated JSON (if
// given); the render summaries and the import report are staged (the mode
// captures UI states, it does not run a render). It exits with
// `std::process::exit(0)` after writing a summary — a graceful close would
// call `save_settings()` and overwrite the user's saved input/output
// folders with the test folders.

/// One screenshot the mode captures, in capture order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShotKind {
    Idle,
    Scanning,
    PreviewRenderedFit,
    PreviewOcrBoxes,
    PreviewOriginal,
    Preview100Pct,
    PreviewZoomedPan,
    JsonTab,
    InspectorBlocks,
    InspectorFiltered,
    ModalPaste,
    ModalBulkEdit,
    ModalImportReport,
    ModalNothingTranslated,
    SizingPopup,
    RenderDone,
    RenderFailed,
    LogClosed,
    EmptyImage,
}

impl ShotKind {
    /// The shots in capture order. 19, not 20: the re-scan modal shot
    /// went with "Scan again" (its modal was removed as a duplicate of
    /// the header primary), so the shots after it renumbered down by one.
    const ALL: [ShotKind; 19] = [
        ShotKind::Idle,
        ShotKind::Scanning,
        ShotKind::PreviewRenderedFit,
        ShotKind::PreviewOcrBoxes,
        ShotKind::PreviewOriginal,
        ShotKind::Preview100Pct,
        ShotKind::PreviewZoomedPan,
        ShotKind::JsonTab,
        ShotKind::InspectorBlocks,
        ShotKind::InspectorFiltered,
        ShotKind::ModalPaste,
        ShotKind::ModalBulkEdit,
        ShotKind::ModalImportReport,
        ShotKind::ModalNothingTranslated,
        ShotKind::SizingPopup,
        ShotKind::RenderDone,
        ShotKind::RenderFailed,
        ShotKind::LogClosed,
        ShotKind::EmptyImage,
    ];

    fn file_name(self) -> &'static str {
        match self {
            ShotKind::Idle => "01-idle.png",
            ShotKind::Scanning => "02-scanning.png",
            ShotKind::PreviewRenderedFit => "03-preview-rendered-fit.png",
            ShotKind::PreviewOcrBoxes => "04-preview-ocr-boxes.png",
            ShotKind::PreviewOriginal => "05-preview-original.png",
            ShotKind::Preview100Pct => "06-preview-100pct.png",
            ShotKind::PreviewZoomedPan => "07-preview-zoomed-pan.png",
            ShotKind::JsonTab => "08-json-tab.png",
            ShotKind::InspectorBlocks => "09-inspector-blocks.png",
            ShotKind::InspectorFiltered => "10-inspector-filtered.png",
            ShotKind::ModalPaste => "11-modal-paste.png",
            ShotKind::ModalBulkEdit => "12-modal-bulk-edit.png",
            ShotKind::ModalImportReport => "13-modal-import-report.png",
            ShotKind::ModalNothingTranslated => "14-modal-nothing-translated.png",
            ShotKind::SizingPopup => "15-sizing-popup.png",
            ShotKind::RenderDone => "16-render-done.png",
            ShotKind::RenderFailed => "17-render-failed.png",
            ShotKind::LogClosed => "18-log-closed.png",
            ShotKind::EmptyImage => "19-empty-image.png",
        }
    }

    /// Whether the shot needs at least one file with at least one block.
    fn needs_blocks(self) -> bool {
        matches!(
            self,
            ShotKind::InspectorBlocks | ShotKind::InspectorFiltered
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShotPhase {
    /// The shot's state is applied; waiting for textures/thumbnails to
    /// settle before requesting the screenshot.
    Settling,
    /// The screenshot was requested; waiting for the callback.
    Waiting,
    /// Waiting for the OCR engine to warm up before starting the scan.
    WaitingForOcr,
    /// The scan is running; waiting for it to finish.
    WaitingForScan,
}

struct ShotTest {
    input: std::path::PathBuf,
    out_dir: std::path::PathBuf,
    json: Option<std::path::PathBuf>,
    current: usize,
    phase: ShotPhase,
    settle_frames: u32,
    frames_waited: u32,
    /// Set by the screenshot callback when the PNG is written.
    shot_done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The capture record, written to `shots.txt` at the end.
    log: Vec<String>,
    /// Warnings from the real JSON import (for the import-report shot).
    import_warnings: Vec<String>,
    /// The user's input/output folders, restored before exit so the test
    /// folders never land in the saved settings.
    orig_input: String,
    orig_output: String,
    /// The (file, block id) whose `kept_override` the filtered shot set.
    filtered_override: Option<(String, usize)>,
    /// Whether the synthetic empty-image entry is in the data.
    synthetic_empty: bool,
}

impl ShotTest {
    fn new(
        input: std::path::PathBuf,
        out_dir: std::path::PathBuf,
        json: Option<std::path::PathBuf>,
        orig_input: String,
        orig_output: String,
    ) -> Self {
        Self {
            input,
            out_dir,
            json,
            current: 0,
            phase: ShotPhase::Settling,
            settle_frames: 10,
            frames_waited: 0,
            shot_done: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            log: Vec::new(),
            import_warnings: Vec::new(),
            orig_input,
            orig_output,
            filtered_override: None,
            synthetic_empty: false,
        }
    }

    fn current_shot(&self) -> ShotKind {
        ShotKind::ALL[self.current]
    }

    fn is_current(&self, kind: ShotKind) -> bool {
        self.current_shot() == kind
    }
}

impl CaptionStudioApp {
    /// One step of the shot-test state machine. Called from `ui()` before
    /// the panels are drawn, so the frame renders the new state.
    fn shot_test_step(&mut self, ctx: &egui::Context) {
        let Some(mut st) = self.shot_test.take() else {
            return;
        };
        match st.phase {
            ShotPhase::Settling => {
                if st.settle_frames > 0 {
                    st.settle_frames -= 1;
                }
                if st.settle_frames == 0 {
                    self.shot_request_screenshot(ctx, &st);
                    st.phase = ShotPhase::Waiting;
                    st.frames_waited = 0;
                }
                ctx.request_repaint();
            }
            ShotPhase::Waiting => {
                if st.shot_done.load(std::sync::atomic::Ordering::SeqCst) {
                    if self.shot_advance(&mut st, true) {
                        self.shot_finish(st);
                    }
                } else {
                    st.frames_waited += 1;
                    if st.frames_waited > 180 {
                        // The screenshot never arrived (the backend may not
                        // support it) — don't hang; record the miss and move on.
                        if self.shot_advance(&mut st, false) {
                            self.shot_finish(st);
                        }
                    } else {
                        ctx.request_repaint();
                    }
                }
            }
            ShotPhase::WaitingForOcr => {
                if self.ocr_ready {
                    self.shot_apply(&mut st, ShotKind::Scanning);
                    st.phase = ShotPhase::Settling;
                    st.settle_frames = 10;
                } else if self.ocr_error.is_some() {
                    st.log.push(
                        "FAILED: the OCR engine did not start — no shots after the idle one."
                            .into(),
                    );
                    self.shot_finish(st);
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            ShotPhase::WaitingForScan => {
                if self.extracted.is_some() && !self.busy {
                    if self.file_names().is_empty() {
                        st.log
                            .push("FAILED: the scan found no images — no data shots.".into());
                        self.shot_finish(st);
                    }
                    self.shot_import_json(&mut st);
                    let kind = st.current_shot();
                    self.shot_apply(&mut st, kind);
                    st.phase = ShotPhase::Settling;
                    st.settle_frames = 10;
                } else if self.ocr_error.is_some() {
                    st.log
                        .push("FAILED: the scan errored — no data shots.".into());
                    self.shot_finish(st);
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        // `shot_finish` exits the process, so reaching here means the mode
        // is still running.
        if self.shot_test.is_none() {
            self.shot_test = Some(st);
        }
    }

    /// Record the just-captured shot and move to the next one (skipping
    /// shots that can't be staged). Returns true when every shot is done.
    fn shot_advance(&mut self, st: &mut ShotTest, captured: bool) -> bool {
        let kind = st.current_shot();
        st.log.push(format!(
            "{} {}",
            kind.file_name(),
            if captured {
                "captured"
            } else {
                "MISSED (screenshot timed out)"
            }
        ));
        // The next shot, skipping ones that can't be staged (no blocks in
        // the scan for the inspector shots).
        let mut guard = 0;
        loop {
            st.current += 1;
            guard += 1;
            if st.current >= ShotKind::ALL.len() || guard > ShotKind::ALL.len() {
                return true;
            }
            let next = st.current_shot();
            if next == ShotKind::Scanning {
                st.phase = ShotPhase::WaitingForOcr;
                return false;
            }
            if kind == ShotKind::Scanning {
                // The scan is still running; wait for it before the data shots.
                st.phase = ShotPhase::WaitingForScan;
                return false;
            }
            if next.needs_blocks() && !self.has_blocks() {
                st.log.push(format!(
                    "{} skipped (no blocks in the scan)",
                    next.file_name()
                ));
                continue;
            }
            self.shot_apply(st, next);
            st.phase = ShotPhase::Settling;
            st.settle_frames = 10;
            return false;
        }
    }

    fn has_blocks(&self) -> bool {
        self.extracted
            .as_ref()
            .map(|d| d.images.iter().any(|e| !e.blocks.is_empty()))
            .unwrap_or(false)
    }

    /// Set the app's state for one shot. Returns nothing; shots that can't
    /// be staged are skipped by `shot_advance` before this is called.
    fn shot_apply(&mut self, st: &mut ShotTest, kind: ShotKind) {
        self.shot_cleanup_synthetic(st);
        // Every shot starts from a clean slate: no modal open, the log
        // shown, the preview at Fit.
        self.confirm_copy_originals = false;
        self.show_paste_modal = false;
        self.show_bulk_edit = false;
        self.import_report = None;
        self.show_log = true;
        self.preview_zoom_fit = true;
        // Reset the manual zoom too: the toolbar's zoom button shows
        // `preview_zoom` as a %, so leaving a stale 2.0 from the
        // zoomed-pan shot would read "200%" on every later Fit shot.
        self.preview_zoom = 1.0;
        self.preview_pan = egui::Vec2::ZERO;
        self.selected_block = None;
        st.shot_done
            .store(false, std::sync::atomic::Ordering::SeqCst);
        match kind {
            ShotKind::Idle => {
                self.tab = Tab::Json;
            }
            ShotKind::Scanning => {
                self.input_dir = st.input.to_string_lossy().to_string();
                self.output_dir = st.out_dir.to_string_lossy().to_string();
                self.tab = Tab::Preview;
                self.start_extract();
            }
            ShotKind::PreviewRenderedFit => {
                self.tab = Tab::Preview;
                self.preview_mode = PreviewMode::Rendered;
                self.preview_file = self.file_names().first().cloned().unwrap_or_default();
                self.preview_key.clear();
            }
            ShotKind::PreviewOcrBoxes => {
                self.tab = Tab::Preview;
                self.preview_mode = PreviewMode::Ocr;
                self.preview_file = self.file_names().first().cloned().unwrap_or_default();
                self.preview_key.clear();
            }
            ShotKind::PreviewOriginal => {
                self.tab = Tab::Preview;
                self.preview_mode = PreviewMode::Original;
                self.preview_file = self.file_names().first().cloned().unwrap_or_default();
                self.preview_key.clear();
            }
            ShotKind::Preview100Pct => {
                self.tab = Tab::Preview;
                self.preview_mode = PreviewMode::Rendered;
                self.preview_zoom_fit = false;
                self.preview_zoom = 1.0;
                self.preview_file = self.file_names().first().cloned().unwrap_or_default();
                self.preview_key.clear();
            }
            ShotKind::PreviewZoomedPan => {
                self.tab = Tab::Preview;
                self.preview_mode = PreviewMode::Rendered;
                self.preview_zoom_fit = false;
                self.preview_zoom = 2.0;
                // Deliberately far off-stage; the per-frame clamp pulls it
                // back to the stage edge, which is what this shot verifies.
                self.preview_pan = egui::vec2(5000.0, 5000.0);
                self.preview_file = self.file_names().first().cloned().unwrap_or_default();
                self.preview_key.clear();
            }
            ShotKind::JsonTab => {
                self.tab = Tab::Json;
            }
            ShotKind::InspectorBlocks => {
                self.tab = Tab::Preview;
                self.preview_mode = PreviewMode::Rendered;
                if let Some((file, id)) = self.first_block() {
                    self.preview_file = file;
                    self.selected_block = Some(id);
                    self.preview_key.clear();
                }
            }
            ShotKind::InspectorFiltered => {
                self.tab = Tab::Preview;
                self.preview_mode = PreviewMode::Rendered;
                if let Some((file, id)) = self.first_block() {
                    // Filter out the first block so its card (and the dashed
                    // orange diagnostic) show; restored in cleanup.
                    if let Some(data) = self.extracted.as_mut() {
                        if let Some(entry) = data.images.iter_mut().find(|e| e.file == file) {
                            if let Some(blk) = entry.blocks.iter_mut().find(|b| b.id == id) {
                                blk.kept_override = Some(false);
                                st.filtered_override = Some((file.clone(), id));
                            }
                        }
                    }
                    self.preview_file = file;
                    self.selected_block = Some(id);
                    self.preview_key.clear();
                }
            }
            ShotKind::ModalPaste => {
                self.tab = Tab::Json;
                self.paste_text = "{\n  \"example.jpg\": {\n    \"1\": \"Translated caption here\",\n    \"2\": \"\"\n  }\n}"
                    .to_string();
                self.show_paste_modal = true;
            }
            ShotKind::ModalBulkEdit => {
                self.tab = Tab::Json;
                self.show_bulk_edit = true;
            }
            ShotKind::ModalImportReport => {
                self.tab = Tab::Json;
                let report = if st.import_warnings.is_empty() {
                    vec![
                        "Skipped \"block 4\" in meme_1.jpg — isn't in the scan.".to_string(),
                        "Skipped \"block 3\" in meme_10.jpg — isn't in the scan.".to_string(),
                    ]
                } else {
                    st.import_warnings.clone()
                };
                self.import_report = Some(report);
            }
            ShotKind::ModalNothingTranslated => {
                self.tab = Tab::Preview;
                self.confirm_copy_originals = true;
            }
            ShotKind::SizingPopup => {
                // The popup is opened by the sidebar hook (it needs the
                // sizing button's response id, which only exists while the
                // sidebar is drawn).
                self.tab = Tab::Preview;
            }
            ShotKind::RenderDone => {
                self.tab = Tab::Preview;
                self.preview_mode = PreviewMode::Rendered;
                self.stage_render_summary(false);
            }
            ShotKind::RenderFailed => {
                self.tab = Tab::Preview;
                self.preview_mode = PreviewMode::Rendered;
                self.stage_render_summary(true);
            }
            ShotKind::LogClosed => {
                self.tab = Tab::Preview;
                self.show_log = false;
            }
            ShotKind::EmptyImage => {
                self.tab = Tab::Preview;
                self.preview_mode = PreviewMode::Rendered;
                self.stage_empty_image(st);
            }
        }
    }

    /// The first (file, block id) in the scan, for the inspector shots.
    fn first_block(&self) -> Option<(String, usize)> {
        let data = self.extracted.as_ref()?;
        for entry in &data.images {
            if let Some(blk) = entry.blocks.first() {
                return Some((entry.file.clone(), blk.id));
            }
        }
        None
    }

    /// Stage a render summary (the mode captures UI states; it does not run
    /// a render). `failed` stages one failed file instead of a clean run.
    fn stage_render_summary(&mut self, failed: bool) {
        let names = self.file_names();
        let empty = self
            .extracted
            .as_ref()
            .map(|d| d.images.iter().filter(|e| e.blocks.is_empty()).count())
            .unwrap_or(0);
        let rendered = names.len().saturating_sub(empty);
        let failed_count = if failed && !names.is_empty() { 1 } else { 0 };
        let rendered = rendered.saturating_sub(failed_count);
        self.render_summary = Some((rendered, empty, failed_count));
        self.rendered_files = names.iter().take(rendered).cloned().collect();
        self.failed_files = if failed_count > 0 {
            names.iter().skip(rendered).take(1).cloned().collect()
        } else {
            std::collections::HashSet::new()
        };
        self.last_save_path = Some(self.output_dir.clone());
        let summary = format!(
            "Complete — rendered {rendered}, copied {empty} unchanged, failed {failed_count}"
        );
        self.log(summary.clone());
        self.status = format!("{summary}. Saved to {}.", self.output_dir);
        if failed_count > 0 {
            if let Some(f) = self.failed_files.iter().next().cloned() {
                self.log_file(LogLevel::Error, f, "failed: render error (staged)".into());
            }
        }
    }

    /// Stage the empty-image state: a real blank PNG in the output folder,
    /// a synthetic entry for it, and the input folder briefly pointed at
    /// the output folder so the preview can load it. Undone by cleanup.
    fn stage_empty_image(&mut self, st: &mut ShotTest) {
        let blank = st.out_dir.join("blank-test.png");
        if !blank.exists() {
            let img = image::RgbaImage::from_pixel(800, 600, image::Rgba([255, 255, 255, 255]));
            let _ = img.save(&blank);
        }
        if let Some(data) = self.extracted.as_mut() {
            if data.entry("blank-test.png").is_none() {
                data.images.push(crate::model::ImageEntry {
                    file: "blank-test.png".into(),
                    width: 800,
                    height: 600,
                    raw_lines: Vec::new(),
                    blocks: Vec::new(),
                    dropped: Vec::new(),
                });
                st.synthetic_empty = true;
            }
        }
        self.input_dir = st.out_dir.to_string_lossy().to_string();
        self.preview_file = "blank-test.png".into();
        self.preview_key.clear();
    }

    /// Undo the synthetic state a shot staged (the empty-image entry, the
    /// filtered override, the temporary input-folder switch).
    fn shot_cleanup_synthetic(&mut self, st: &mut ShotTest) {
        if st.synthetic_empty {
            if let Some(data) = self.extracted.as_mut() {
                data.images.retain(|e| e.file != "blank-test.png");
            }
            self.input_dir = st.input.to_string_lossy().to_string();
            st.synthetic_empty = false;
        }
        if let Some((file, id)) = st.filtered_override.take() {
            if let Some(data) = self.extracted.as_mut() {
                if let Some(entry) = data.images.iter_mut().find(|e| e.file == file) {
                    if let Some(blk) = entry.blocks.iter_mut().find(|b| b.id == id) {
                        blk.kept_override = None;
                    }
                }
            }
        }
    }

    /// Import the translated JSON given on the command line (same
    /// trial-merge as the file-picker import, but from a path). The
    /// warnings are kept for the import-report shot.
    fn shot_import_json(&mut self, st: &mut ShotTest) {
        let Some(path) = st.json.clone() else {
            return;
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let mut report: Vec<String> = Vec::new();
                match parse_flexible_json_or_dict(text.trim()) {
                    Ok(parsed) => {
                        if let Some(data) = self.extracted.clone() {
                            let (_, warnings, _) = merge_translations(&parsed, &data);
                            report = warnings;
                        }
                    }
                    Err(_) => {
                        report.push("The file isn't valid JSON — nothing was imported.".into());
                    }
                }
                st.import_warnings = report.clone();
                self.editor_text = text;
                self.clear_render_state();
                self.preview_key.clear();
                if report.is_empty() {
                    self.status = "Imported translations from the shot-test JSON".into();
                } else {
                    self.import_report = Some(report);
                    self.status =
                        "Imported translations — some files were skipped, see the report.".into();
                }
            }
            Err(e) => {
                st.log.push(format!(
                    "NOTE: couldn't read the --json file ({e}) — skipped."
                ));
            }
        }
    }

    /// Ask egui for a screenshot of this frame and save it as the shot's
    /// PNG. The callback sets `shot_done` so the state machine advances.
    fn shot_request_screenshot(&self, ctx: &egui::Context, st: &ShotTest) {
        let name = st.current_shot().file_name().to_string();
        let path = st.out_dir.join(&name);
        let done = st.shot_done.clone();
        ctx.request_screenshot(move |image| {
            let rgba = image::RgbaImage::from_raw(
                image.width() as u32,
                image.height() as u32,
                image.as_raw().to_vec(),
            );
            match rgba {
                Some(rgba) => {
                    let _ = rgba.save(&path);
                }
                None => {
                    eprintln!("shot-test: couldn't convert the screenshot for {name}");
                }
            }
            done.store(true, std::sync::atomic::Ordering::SeqCst);
        });
    }

    /// Write the capture record, restore the user's folders, and exit.
    /// `std::process::exit` (not a graceful close) so `save_settings()`
    /// never runs with the test folders.
    fn shot_finish(&mut self, mut st: ShotTest) -> ! {
        self.shot_cleanup_synthetic(&mut st);
        self.input_dir = st.orig_input.clone();
        self.output_dir = st.orig_output.clone();
        let mut record = String::from("Caption Studio shot-test\n");
        record.push_str(&format!("input: {}\n", st.input.display()));
        record.push_str(&format!("output: {}\n", st.out_dir.display()));
        record.push('\n');
        for line in &st.log {
            record.push_str(line);
            record.push('\n');
        }
        let _ = std::fs::write(st.out_dir.join("shots.txt"), record);
        println!(
            "shot-test: done — {} shots in {}",
            st.log.len(),
            st.out_dir.display()
        );
        std::process::exit(0);
    }
}

pub fn run_app() -> eframe::Result<()> {
    let icon = eframe::icon_data::from_png_bytes(VMAX_ICON_PNG).unwrap_or_default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            // Redesign handoff (SPEC §3): minimum 1280×720; the old
            // 1160×680 initial size is below the new minimum.
            .with_inner_size([1600.0, 900.0])
            .with_min_inner_size([1280.0, 720.0])
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

/// The shot-test mode: the same app at a fixed 1920×1080 window, driving
/// itself through every reviewable state and saving a PNG per state into
/// `output`. Validates the folders first so a typo fails fast on the
/// command line instead of opening a window that does nothing.
pub fn run_shot_test(
    input: std::path::PathBuf,
    output: std::path::PathBuf,
    json: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    if !input.is_dir() {
        anyhow::bail!(
            "--shot-test: input folder doesn't exist: {}",
            input.display()
        );
    }
    if let Some(j) = &json {
        if !j.is_file() {
            anyhow::bail!("--shot-test: --json file doesn't exist: {}", j.display());
        }
    }
    std::fs::create_dir_all(&output)?;
    let icon = eframe::icon_data::from_png_bytes(VMAX_ICON_PNG).unwrap_or_default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            // Fixed at the eyeball size so every shot is the same size.
            .with_inner_size([1920.0, 1080.0])
            .with_min_inner_size([1280.0, 720.0])
            .with_icon(icon)
            .with_title("Caption Studio — Shot Test"),
        ..Default::default()
    };
    let input_clone = input.clone();
    let output_clone = output.clone();
    eframe::run_native(
        "Caption Studio — Shot Test",
        options,
        Box::new(move |cc| {
            let mut app = CaptionStudioApp::new(cc);
            let st = ShotTest::new(
                input_clone,
                output_clone,
                json,
                app.input_dir.clone(),
                app.output_dir.clone(),
            );
            // The first shot (idle) settles, then the machine runs.
            app.shot_test = Some(st);
            Ok(Box::new(app))
        }),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_highlight_colours_keys_values_numbers_and_empty_strings() {
        let t = crate::theme::Tokens::for_kind(crate::theme::ThemeKind::Studio);
        let text =
            "{\n  \"meme.png\": {\n    \"0\": \"Hello\",\n    \"1\": \"\",\n    \"2\": 42\n  }\n}";
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
        assert_eq!(colour_of("\"\""), Some(t.text_dim));
        assert_eq!(colour_of("42"), Some(t.warning));
        assert_eq!(colour_of("{"), Some(t.text_dim));
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
    fn filter_file_names_is_case_insensitive_substring() {
        let names = vec!["Alpha_01.jpg".to_string(), "beta_02.jpg".to_string()];
        assert_eq!(filter_file_names(&names, "").len(), 2);
        assert_eq!(filter_file_names(&names, "ALPHA"), vec![&names[0]]);
        assert_eq!(filter_file_names(&names, "02"), vec![&names[1]]);
        assert!(filter_file_names(&names, "zzz").is_empty());
    }

    #[test]
    fn file_status_covers_all_five_states() {
        fn entry(blocks: Vec<(&str, &str)>) -> crate::model::ImageEntry {
            crate::model::ImageEntry {
                file: "a.png".into(),
                width: 100,
                height: 100,
                raw_lines: vec![],
                blocks: blocks
                    .into_iter()
                    .enumerate()
                    .map(|(i, (orig, new))| crate::model::Block {
                        id: i,
                        bbox: [0, 0, 10, 10],
                        line_bboxes: vec![],
                        original_text: orig.into(),
                        new_text: new.into(),
                        confidence: 0.9,
                        text_color: [0, 0, 0],
                        bg_color: [255, 255, 255],
                        is_boxed: false,
                        font_size_est: 20,
                        kept_override: None,
                    })
                    .collect(),
                dropped: vec![],
            }
        }
        // Failed wins over rendered.
        assert_eq!(
            file_status(&entry(vec![("one", "uno")]), true, true),
            FileStatus::Failed
        );
        assert_eq!(
            file_status(&entry(vec![("one", "uno")]), true, false),
            FileStatus::Rendered
        );
        // No blocks at all: no text (even unrendered).
        assert_eq!(
            file_status(&entry(vec![]), false, false),
            FileStatus::NoText
        );
        // A block with no translation yet.
        assert_eq!(
            file_status(&entry(vec![("one", "")]), false, false),
            FileStatus::Untranslated
        );
        // All translated, not rendered: ready.
        assert_eq!(
            file_status(&entry(vec![("one", "uno")]), false, false),
            FileStatus::Ready
        );
        // A translation equal to the original still counts as translated
        // for the chip (it is not empty); the render copies it unchanged.
        assert_eq!(
            file_status(&entry(vec![("one", "one")]), false, false),
            FileStatus::Ready
        );
        // A block the user filtered out needs no translation: the file
        // reads as "no text", not "untranslated".
        let mut e = entry(vec![("one", "")]);
        e.blocks[0].kept_override = Some(false);
        assert_eq!(file_status(&e, false, false), FileStatus::NoText);
    }

    #[test]
    fn step_in_names_wraps_and_ignores_an_empty_list() {
        let names = vec![
            "b.jpg".to_string(),
            "a.jpg".to_string(),
            "c.jpg".to_string(),
        ];
        let refs: Vec<&String> = names.iter().collect();
        let mut cur = "c.jpg".to_string();
        step_in_names(&mut cur, &refs, 1);
        assert_eq!(cur, "b.jpg");
        step_in_names(&mut cur, &refs, -1);
        assert_eq!(cur, "c.jpg");
        let mut cur = "b.jpg".to_string();
        step_in_names(&mut cur, &[], 1);
        assert_eq!(cur, "b.jpg");
    }

    #[test]
    fn clamp_pan_keeps_the_image_inside_the_stage() {
        let stage = egui::vec2(800.0, 600.0);
        // A small image is centred — no pan, however far it is dragged.
        let pan = clamp_pan(egui::vec2(1000.0, 0.0), egui::vec2(100.0, 100.0), stage);
        assert_eq!(pan.x, 0.0);
        assert_eq!(pan.y, 0.0);
        // A huge image may pan only until its edge meets the stage's edge.
        let pan = clamp_pan(
            egui::vec2(1000.0, 1000.0),
            egui::vec2(2000.0, 1000.0),
            stage,
        );
        assert_eq!(pan.x, 600.0);
        assert_eq!(pan.y, 200.0);
    }

    #[test]
    fn shot_test_covers_nineteen_named_shots_in_order() {
        assert_eq!(ShotKind::ALL.len(), 19);
        assert_eq!(ShotKind::ALL[0].file_name(), "01-idle.png");
        assert_eq!(ShotKind::ALL[1].file_name(), "02-scanning.png");
        assert_eq!(ShotKind::ALL[18].file_name(), "19-empty-image.png");
        // The file names are unique (no shot overwrites another).
        let mut names: Vec<&str> = ShotKind::ALL.iter().map(|s| s.file_name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 19);
        // Only the inspector shots need blocks; the rest stage without them.
        assert!(ShotKind::InspectorBlocks.needs_blocks());
        assert!(ShotKind::InspectorFiltered.needs_blocks());
        assert!(!ShotKind::ModalPaste.needs_blocks());
        assert!(!ShotKind::EmptyImage.needs_blocks());
    }

    #[test]
    fn tail_ellipsis_keeps_the_suffix() {
        assert_eq!(tail_ellipsis_str("abc", 0), "…");
        assert_eq!(tail_ellipsis_str("abc", 1), "…c");
        assert_eq!(tail_ellipsis_str("abc", 2), "…bc");
        assert_eq!(tail_ellipsis_str("abc", 3), "abc");
        assert_eq!(tail_ellipsis_str("abc", 9), "abc");
        assert_eq!(
            tail_ellipsis_str("C:\\Users\\VampDued\\Downloads\\translated_output", 17),
            "…translated_output"
        );
    }

    #[test]
    fn natural_sort_orders_numbers_by_value() {
        let mut names = vec![
            "10 of 75.png".to_string(),
            "2 of 75.png".to_string(),
            "1 of 75.png".to_string(),
            "13 of 75.png".to_string(),
            "9 of 75.png".to_string(),
        ];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            vec![
                "1 of 75.png",
                "2 of 75.png",
                "9 of 75.png",
                "10 of 75.png",
                "13 of 75.png"
            ]
        );
        // Leading zeros tie-break, non-digit runs stay lexicographic.
        assert_eq!(natural_cmp("01.jpg", "1.jpg"), std::cmp::Ordering::Greater);
        assert_eq!(natural_cmp("a2", "a10"), std::cmp::Ordering::Less);
        assert_eq!(natural_cmp("a", "b"), std::cmp::Ordering::Less);
    }

    #[test]
    fn fit_label_agrees_in_number() {
        assert_eq!(fit_label(true, 1, 59, 20), "Fits plate · 1 line · 59 chars");
        assert_eq!(
            fit_label(true, 3, 157, 20),
            "Fits plate · 3 lines · 157 chars"
        );
        assert_eq!(fit_label(true, 1, 1, 20), "Fits plate · 1 line · 1 char");
        assert_eq!(
            fit_label(false, 2, 40, 14),
            "Shrinks to 14px · 2 lines · 40 chars"
        );
    }

    #[test]
    fn workflow_steps_advance_with_data_and_render() {
        fn entry(file: &str, blocks: Vec<(&str, &str)>) -> crate::model::ImageEntry {
            crate::model::ImageEntry {
                file: file.to_string(),
                width: 100,
                height: 100,
                raw_lines: vec![],
                blocks: blocks
                    .into_iter()
                    .enumerate()
                    .map(|(i, (orig, new))| crate::model::Block {
                        id: i,
                        bbox: [0, 0, 10, 10],
                        line_bboxes: vec![],
                        original_text: orig.to_string(),
                        new_text: new.to_string(),
                        confidence: 0.9,
                        text_color: [0, 0, 0],
                        bg_color: [255, 255, 255],
                        is_boxed: false,
                        font_size_est: 20,
                        kept_override: None,
                    })
                    .collect(),
                dropped: vec![],
            }
        }
        // Nothing scanned: Scan is current.
        let wf = derive_workflow_state(None, None);
        assert_eq!(wf.current, WorkflowStep::Scan);
        assert_eq!(
            wf.states,
            [
                StepState::Current,
                StepState::Todo,
                StepState::Todo,
                StepState::Todo
            ]
        );
        assert_eq!(wf.subtitles[0], "No images yet");
        // Scanned, one of three translated (a translation equal to the
        // original does NOT count): Translate is current.
        let data = crate::model::ExtractedData {
            images: vec![
                entry(
                    "a.png",
                    vec![("one", "uno"), ("two", ""), ("three", "three")],
                ),
                entry("b.png", vec![]),
            ],
        };
        let wf = derive_workflow_state(Some(&data), None);
        assert_eq!(wf.current, WorkflowStep::Translate);
        assert_eq!(
            wf.states,
            [
                StepState::Done,
                StepState::Current,
                StepState::Todo,
                StepState::Todo
            ]
        );
        assert_eq!(wf.subtitles[1], "1 of 3 translated");
        assert_eq!(wf.subtitles[2], "1 will copy unchanged");
        // All translated: Review is current.
        let data = crate::model::ExtractedData {
            images: vec![entry("a.png", vec![("one", "uno"), ("two", "dos")])],
        };
        let wf = derive_workflow_state(Some(&data), None);
        assert_eq!(wf.current, WorkflowStep::Review);
        // Rendered: everything done, current stays Render (re-render).
        let wf = derive_workflow_state(Some(&data), Some((2, 0, 0)));
        assert_eq!(wf.current, WorkflowStep::Render);
        assert_eq!(wf.states, [StepState::Done; 4]);
        assert_eq!(wf.subtitles[3], "2 rendered");
        // A run with failures: Review is done (it was reviewed), but Render
        // stays current with the failed count in its subtitle.
        let wf = derive_workflow_state(Some(&data), Some((1, 0, 1)));
        assert_eq!(wf.current, WorkflowStep::Render);
        assert_eq!(
            wf.states,
            [
                StepState::Done,
                StepState::Done,
                StepState::Done,
                StepState::Current
            ]
        );
        assert_eq!(wf.subtitles[3], "1 rendered · 1 failed");
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
            kept_override: None,
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
    fn filtered_blocks_skip_the_workflow_counts() {
        // One filtered-out untranslated block + one kept translated block:
        // Translate counts only the kept block, so it is done.
        let mut filtered = test_block(0, "");
        filtered.kept_override = Some(false);
        let data = crate::model::ExtractedData {
            images: vec![crate::model::ImageEntry {
                file: "a.png".into(),
                width: 100,
                height: 100,
                raw_lines: vec![],
                blocks: vec![filtered, test_block(1, "z")],
                dropped: vec![],
            }],
        };
        assert_eq!(counts_for(&data), (1, 0));
        let wf = derive_workflow_state(Some(&data), None);
        assert_eq!(wf.subtitles[1], "1 of 1 translated");
        assert_eq!(wf.current, WorkflowStep::Review);
        // An image with no kept blocks at all is copied unchanged.
        let mut filtered = test_block(0, "");
        filtered.kept_override = Some(false);
        let data = crate::model::ExtractedData {
            images: vec![crate::model::ImageEntry {
                file: "a.png".into(),
                width: 100,
                height: 100,
                raw_lines: vec![],
                blocks: vec![filtered],
                dropped: vec![],
            }],
        };
        let wf = derive_workflow_state(Some(&data), None);
        assert_eq!(wf.subtitles[2], "1 will copy unchanged");
        assert_eq!(wf.current, WorkflowStep::Review);
    }

    #[test]
    fn per_image_json_shows_one_file_only() {
        let data = crate::model::ExtractedData {
            images: vec![
                crate::model::ImageEntry {
                    file: "a.png".into(),
                    width: 100,
                    height: 100,
                    raw_lines: vec![],
                    blocks: vec![test_block(1, "uno"), test_block(2, "")],
                    dropped: vec![],
                },
                crate::model::ImageEntry {
                    file: "b.png".into(),
                    width: 100,
                    height: 100,
                    raw_lines: vec![],
                    blocks: vec![test_block(1, "dos")],
                    dropped: vec![],
                },
            ],
        };
        let text = per_image_json(Some(&data), "a.png");
        assert!(text.contains("\"a.png\""));
        assert!(!text.contains("\"b.png\""));
        // Block 1 has a translation, block 2 falls back to the original.
        assert!(text.contains("\"1\": \"uno\""));
        assert!(text.contains("\"2\": \"x\""));
        assert_eq!(per_image_json(Some(&data), "missing.png"), "");
        assert_eq!(per_image_json(None, "a.png"), "");
    }

    #[test]
    fn log_height_after_drag_resizes_and_closes() {
        // Dragging up (negative delta) grows the panel.
        assert_eq!(log_height_after_drag(188.0, -20.0), Some(208.0));
        // Dragging down shrinks it.
        assert_eq!(log_height_after_drag(188.0, 20.0), Some(168.0));
        // It never goes below 64 or above 480 while open.
        assert_eq!(log_height_after_drag(70.0, 10.0), Some(64.0));
        assert_eq!(log_height_after_drag(470.0, -20.0), Some(480.0));
        // Dragging down past 48 closes the panel instead.
        assert_eq!(log_height_after_drag(64.0, 20.0), None);
        assert_eq!(log_height_after_drag(188.0, 200.0), None);
    }

    #[test]
    fn log_visible_is_newest_first_and_warnings_filter_info() {
        let lines = vec![
            LogLine::info("first".into()),
            LogLine::warn("second".into()),
            LogLine::file(LogLevel::Error, "a.png".into(), "failed".into()),
        ];
        let all = log_visible(&lines, LogFilter::All);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].result, "failed");
        assert_eq!(all[2].result, "first");
        let warnings = log_visible(&lines, LogFilter::Warnings);
        assert_eq!(warnings.len(), 2);
        assert!(warnings.iter().all(|l| l.level != LogLevel::Info));
        // The per-file line carries its file; the plain text form names
        // the time, level, file and result.
        let text = lines[2].to_text();
        assert!(text.contains("ERROR"));
        assert!(text.contains("a.png"));
        assert!(text.contains("failed"));
        assert_eq!(text.split(' ').count(), 5); // HH:MM:SS + LEVEL + file + — + result
    }

    #[test]
    fn render_counts_reads_the_last_run() {
        assert_eq!(
            render_counts(12, None),
            "12 images · 0 rendered · 0 copied unchanged · 0 failed"
        );
        assert_eq!(
            render_counts(12, Some((10, 2, 0))),
            "12 images · 10 rendered · 2 copied unchanged · 0 failed"
        );
    }

    #[test]
    fn empty_image_verify_text_names_the_copy() {
        let entry = crate::model::ImageEntry {
            file: "empty.png".into(),
            width: 100,
            height: 100,
            raw_lines: vec![],
            blocks: vec![],
            dropped: vec![],
        };
        assert_eq!(
            verify_text_for(&entry),
            "No text found. This image will be copied unchanged."
        );
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
