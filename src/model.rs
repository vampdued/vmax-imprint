//! Shared data model — the JSON shape of `captions.json`, kept wire-compatible
//! with the Python `caption_replacer.py` output so existing files load as-is.

use serde::{Deserialize, Serialize};

/// One raw OCR detection, kept for every line with `score >= 0.10` so that
/// changing the confidence threshold later re-filters instantly without
/// re-running neural inference (Python: `raw_lines`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawLine {
    /// [x1, y1, x2, y2] in native image pixels.
    pub bbox: [i32; 4],
    pub text: String,
    pub score: f32,
    pub bg_color: [u8; 3],
    pub text_color: [u8; 3],
    pub is_boxed: bool,
}

/// A line dropped by threshold filtering, with the reason shown in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroppedItem {
    pub bbox: [i32; 4],
    pub text: String,
    pub reason: String,
}

/// A merged block of one or more OCR lines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub id: usize,
    /// [x1, y1, x2, y2] union of the member lines.
    pub bbox: [i32; 4],
    pub line_bboxes: Vec<[i32; 4]>,
    pub original_text: String,
    #[serde(default)]
    pub new_text: String,
    pub confidence: f32,
    pub text_color: [u8; 3],
    pub bg_color: [u8; 3],
    pub is_boxed: bool,
    pub font_size_est: i32,
    /// Manual Keep/Filter override from the inspector (redesign Phase 4):
    /// `None` = no override (the block is kept — it survived the thresholds
    /// to be here at all), `Some(false)` = the user filtered it out,
    /// `Some(true)` = the user kept a block the thresholds would drop
    /// (a promoted dropped item). Old JSON loads as `None`; `None` is not
    /// written, so the wire shape is unchanged for un-overridden blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept_override: Option<bool>,
}

impl Block {
    /// The block's effective kept state: a manual override wins; otherwise
    /// a block is kept.
    pub fn kept(&self) -> bool {
        self.kept_override.unwrap_or(true)
    }
}

/// One image in the extracted set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageEntry {
    pub file: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub raw_lines: Vec<RawLine>,
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub dropped: Vec<DroppedItem>,
}

/// Top-level `captions.json` document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtractedData {
    #[serde(default)]
    pub images: Vec<ImageEntry>,
}

impl ExtractedData {
    pub fn entry(&self, file: &str) -> Option<&ImageEntry> {
        self.images.iter().find(|e| e.file == file)
    }
    pub fn entry_mut(&mut self, file: &str) -> Option<&mut ImageEntry> {
        self.images.iter_mut().find(|e| e.file == file)
    }
}

/// How the translation font size is chosen when rendering.
///
/// * `UniformGrow` (A, default): one size for every block in the image —
///   the median of the blocks' `font_size_est` (the original text's size)
///   — and each plate grows (wider/taller) to fit its translation at that
///   size. The original meme uses one size for all its captions; the old
///   per-block fit gave each block its own size (45/39/29 on the user's
///   _13 image), which reads as "the font sizes are different".
/// * `PerBlock` (B): the original behaviour — each block is fitted
///   independently to its own footprint, so plates stay on the original
///   footprint but sizes differ.
/// * `UniformSmallest` (C): one size for every block, the smallest of the
///   per-block fitted sizes, so all blocks fit their original footprints
///   at one (smaller) size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizingMode {
    UniformGrow,
    PerBlock,
    UniformSmallest,
}

impl Default for SizingMode {
    fn default() -> Self {
        SizingMode::UniformGrow
    }
}

impl SizingMode {
    pub fn label(self) -> &'static str {
        match self {
            SizingMode::UniformGrow => "Uniform — plates grow",
            SizingMode::PerBlock => "Per-block — plates stay put",
            SizingMode::UniformSmallest => "Uniform — smallest size",
        }
    }
    pub fn all() -> [SizingMode; 3] {
        [
            SizingMode::UniformGrow,
            SizingMode::PerBlock,
            SizingMode::UniformSmallest,
        ]
    }
}

/// Persisted UI settings — same file and keys as the Python app
/// (`~/.caption_replacer.json`) so settings carry over.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub font: String,
    #[serde(default = "default_conf")]
    pub conf: f64,
    #[serde(default = "default_minchars")]
    pub minchars: i32,
    #[serde(default)]
    pub sizing: SizingMode,
}

pub fn default_conf() -> f64 {
    0.6
}
pub fn default_minchars() -> i32 {
    12
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            input: String::new(),
            output: String::new(),
            font: String::new(),
            conf: 0.6,
            minchars: 12,
            sizing: SizingMode::UniformGrow,
        }
    }
}

impl Settings {
    pub fn path() -> std::path::PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        std::path::PathBuf::from(home).join(".caption_replacer.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }
}

/// Clamp a box to image bounds (Python: `clamp_box`).
pub fn clamp_box(b: [i32; 4], w: i32, h: i32) -> [i32; 4] {
    [b[0].max(0), b[1].max(0), b[2].min(w), b[3].min(h)]
}

pub fn color_dist(c1: [u8; 3], c2: [u8; 3]) -> f32 {
    let d = [
        c1[0] as f32 - c2[0] as f32,
        c1[1] as f32 - c2[1] as f32,
        c1[2] as f32 - c2[2] as f32,
    ];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Detect which script family a text belongs to (Python: `detect_script`).
/// "" = Latin/default, "indic" / "japanese" / "korean" / "chinese".
pub fn detect_script(text: &str) -> &'static str {
    for ch in text.chars() {
        let c = ch as u32;
        if (0x0900..=0x0DFF).contains(&c) {
            return "indic";
        }
        if (0x3040..=0x30FF).contains(&c) {
            return "japanese";
        }
        if (0xAC00..=0xD7AF).contains(&c) {
            return "korean";
        }
        if (0x4E00..=0x9FFF).contains(&c) {
            return "chinese";
        }
    }
    ""
}
