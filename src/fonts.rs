//! Font handling for rendering — replaces PIL `find_font` / `ImageFont`.
//!
//! Shaping is done by `cosmic-text` (rustybuzz), so Indic vowel signs and
//! CJK glyphs shape correctly. Font lookup order mirrors the Python one:
//! a user-picked custom font first, then the script's system fonts, then
//! the Latin defaults. On Windows the system fonts are Segoe UI / Nirmala
//! UI / Yu Gothic / Malgun Gothic / Microsoft YaHei; on other systems the
//! Noto equivalents. A DejaVu Sans Bold copy is embedded as a last-resort
//! fallback so rendering never fails on a machine with no matching font.

use cosmic_text::{Attrs, Family, FontSystem};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Embedded fallback (DejaVu Sans Bold, freely licensed) — only used when
/// no system font matches.
pub static EMBEDDED_FALLBACK: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf");

/// Windows system font families the Python app tried first, per script.
pub const SCRIPT_FONTS: &[(&str, &[&str])] = &[
    (
        "indic",
        &["Nirmala UI", "Nirmala", "Noto Sans Tamil", "Noto Sans Devanagari", "Lohit Tamil"],
    ),
    (
        "japanese",
        &["Yu Gothic", "Meiryo", "MS Gothic", "Noto Sans CJK JP"],
    ),
    ("korean", &["Malgun Gothic", "Noto Sans CJK KR"]),
    (
        "chinese",
        &["Microsoft YaHei", "SimSun", "Noto Sans CJK SC"],
    ),
];

pub const DEFAULT_FONTS: &[&str] = &[
    "Segoe UI",
    "Arial",
    "Calibri",
    "Roboto",
    "Liberation Sans",
    "DejaVu Sans",
];

/// A process-wide font system, created once (font loading is expensive).
static FONT_SYSTEM: OnceLock<Mutex<FontSystem>> = OnceLock::new();

pub fn font_system() -> &'static Mutex<FontSystem> {
    FONT_SYSTEM.get_or_init(|| {
        let mut fs = FontSystem::new();
        // Embedded fallback, always available.
        fs.db_mut().load_font_data(EMBEDDED_FALLBACK.to_vec());
        Mutex::new(fs)
    })
}

/// Cache of leaked `&'static str` family names for custom fonts (leaked
/// once per distinct font file, not per call).
static CUSTOM_FAMILY_CACHE: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();

fn leak_family(name: &str) -> &'static str {
    let cache = CUSTOM_FAMILY_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().unwrap();
    if let Some(v) = map.get(name) {
        return v;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    map.insert(name.to_string(), leaked);
    leaked
}

/// Load a user-picked custom font file into the shared font system and
/// return its family name (so `Attrs` can select it). Returns `None` if the
/// file cannot be loaded.
pub fn load_custom_font(path: &str) -> Option<&'static str> {
    if path.is_empty() {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    let mut fs = font_system().lock().ok()?;
    let before = fs.db().faces().count();
    fs.db_mut().load_font_data(data);
    // The faces added by this load are the ones after `before`.
    let face = fs.db().faces().nth(before)?;
    face.families
        .first()
        .map(|(name, _)| leak_family(name))
}

/// Does the font database contain a face for this family?
fn family_available(fs: &FontSystem, name: &str) -> bool {
    fs.db()
        .faces()
        .any(|f| f.families.iter().any(|(n, _)| n == name))
}

/// First available family for a text, in Python priority order:
/// custom font -> script fonts -> Latin defaults -> SansSerif.
/// The returned name is `&'static str` (script/default names are literals;
/// custom names come from the leak-once cache).
///
/// Takes `&FontSystem` instead of locking: every caller already holds the
/// global font-system lock, and `std::sync::Mutex` is non-reentrant, so
/// locking here would deadlock the thread (this hung `cargo test`).
pub fn family_for(fs: &FontSystem, text: &str, custom_family: Option<&'static str>) -> Family<'static> {
    if let Some(f) = custom_family {
        if family_available(fs, f) {
            return Family::Name(f);
        }
    }
    let script = crate::model::detect_script(text);
    if let Some((_, fonts)) = SCRIPT_FONTS.iter().find(|(k, _)| *k == script) {
        for f in *fonts {
            if family_available(fs, f) {
                return Family::Name(f);
            }
        }
    }
    for f in DEFAULT_FONTS {
        if family_available(fs, f) {
            return Family::Name(f);
        }
    }
    Family::SansSerif
}

/// Build bold `Attrs` for a text, with the right family.
/// `fs` is the caller's already-locked font system (see `family_for`).
pub fn attrs_for(fs: &FontSystem, text: &str, custom_family: Option<&'static str>) -> Attrs<'static> {
    Attrs::new()
        .family(family_for(fs, text, custom_family))
        .weight(cosmic_text::Weight::BOLD)
}
