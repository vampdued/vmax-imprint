//! Icons. Lucide SVGs are embedded (see `assets/icons/`, ISC license —
//! `assets/icons/LICENSE-LUCIDE.txt`), recoloured to white, rasterised by
//! the egui_extras SVG loader at the exact on-screen pixel size, and tinted
//! per use.
//!
//! Ported from `storytold/craft-anything`'s `crates/ui-egui/src/icons.rs`
//! (MIT OR Apache-2.0), adapted to this app: no i18n, no animated hover
//! (a plain hover fill), and a minimal focus ring. `file-json` is the
//! Lucide `braces` glyph — current Lucide has no `file-json`/`file-json-2`
//! file, and braces read as JSON.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use egui::{Color32, Rect, Response, Sense, Stroke, Vec2};

use crate::theme::{ControlSize, Tokens};

/// The embedded Lucide SVGs (name, bytes). `file-json` holds the `braces` glyph.
pub static ICONS: &[(&str, &[u8])] = &[
    ("folder", include_bytes!("../assets/icons/folder.svg")),
    ("folder-open", include_bytes!("../assets/icons/folder-open.svg")),
    ("scan", include_bytes!("../assets/icons/scan.svg")),
    ("search", include_bytes!("../assets/icons/search.svg")),
    ("save", include_bytes!("../assets/icons/save.svg")),
    ("play", include_bytes!("../assets/icons/play.svg")),
    ("eye", include_bytes!("../assets/icons/eye.svg")),
    ("octagon-x", include_bytes!("../assets/icons/octagon-x.svg")),
    ("copy", include_bytes!("../assets/icons/copy.svg")),
    ("clipboard-paste", include_bytes!("../assets/icons/clipboard-paste.svg")),
    ("file-json", include_bytes!("../assets/icons/file-json.svg")),
    ("arrow-right", include_bytes!("../assets/icons/arrow-right.svg")),
    ("chevron-left", include_bytes!("../assets/icons/chevron-left.svg")),
    ("chevron-right", include_bytes!("../assets/icons/chevron-right.svg")),
    ("check", include_bytes!("../assets/icons/check.svg")),
    ("x", include_bytes!("../assets/icons/x.svg")),
    ("square", include_bytes!("../assets/icons/square.svg")),
];

/// Every icon the UI uses (plus `square`, the unknown-name fallback); the
/// test below asserts each one is embedded. `x` is embedded in `ICONS` but
/// no control uses it, so it is not listed here.
/// (Test-only: kept out of the non-test build.)
#[cfg(test)]
pub const USED_ICONS: &[&str] = &[
    "folder",
    "folder-open",
    "scan",
    "search",
    "save",
    "play",
    "eye",
    "octagon-x",
    "copy",
    "clipboard-paste",
    "file-json",
    "arrow-right",
    "chevron-left",
    "chevron-right",
    "check",
    "square",
];

fn white_icons() -> &'static HashMap<&'static str, Arc<[u8]>> {
    static MAP: OnceLock<HashMap<&'static str, Arc<[u8]>>> = OnceLock::new();

    MAP.get_or_init(|| {
        ICONS
            .iter()
            .map(|(name, bytes)| {
                let svg = String::from_utf8_lossy(bytes).replace("currentColor", "#ffffff").replace("stroke-width=\"2\"", "stroke-width=\"1.75\"");
                (*name, Arc::from(svg.into_bytes().into_boxed_slice()))
            })
            .collect()
    })
}

/// Whether an icon name is embedded. (Public API surface for future
/// screens; unused by the current UI.)
#[allow(dead_code)]
pub fn exists(name: &str) -> bool {
    white_icons().contains_key(name)
}

/// An egui image for an icon, tinted. Unknown names fall back to a square.
pub fn image(name: &str, size: f32, tint: Color32) -> egui::Image<'static> {
    let (name, bytes) = match white_icons().get(name) {
        Some(b) => (name, b.clone()),
        None => ("square", white_icons().get("square").cloned().unwrap_or_else(|| Arc::from(Vec::new().into_boxed_slice()))),
    };

    egui::Image::from_bytes(format!("bytes://icons/{name}.svg"), egui::load::Bytes::Shared(bytes)).fit_to_exact_size(Vec2::splat(size)).tint(tint)
}

/// Paint an icon centred in `rect`.
pub fn paint(ui: &egui::Ui, rect: Rect, name: &str, size: f32, tint: Color32) {
    let r = Rect::from_center_size(rect.center(), Vec2::splat(size));
    image(name, size, tint).paint_at(ui, r);
}

/// Square icon button at a control size: transparent until hovered; `selected` gets the accent
/// treatment.
pub fn button(ui: &mut egui::Ui, name: &str, size: ControlSize, selected: bool, tooltip: &str) -> Response {
    button_box(ui, name, size.height(), selected, tooltip)
}

/// [`button`] in a box of any size.
pub fn button_box(ui: &mut egui::Ui, name: &str, box_size: f32, selected: bool, tooltip: &str) -> Response {
    let t = Tokens::get(ui.ctx());

    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(box_size), Sense::click());

    if selected {
        ui.painter().rect_filled(rect, t.radius_sm, t.accent_soft);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }

    let tint = if selected {
        t.accent_ink
    } else if resp.hovered() {
        t.text
    } else {
        t.icon_rest
    };

    paint(ui, rect, name, (box_size * 0.55).round(), tint);

    if resp.has_focus() {
        // `accent_ink` (#8FA8FF), not `accent`: the Max Blue ring was 2.44:1
        // against the field it sits on (below the 3:1 a focus indicator
        // needs); accent_ink is 5.47:1 there and 9.21:1 on chrome.
        ui.painter().rect_stroke(rect.expand(1.0), t.radius_sm, Stroke::new(1.5, t.accent_ink), egui::StrokeKind::Outside);
    }

    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);

    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, if tooltip.is_empty() { name } else { tooltip }));

    // A sighted keyboard user tabbing to an icon-only button sees the glyph
    // and the focus ring but no text saying what it does (`on_hover_text`
    // fires on hover only) — so show the tooltip on keyboard focus too.
    // Not when hovered as well, or the two tooltips would stack.
    if resp.has_focus() && !resp.hovered() && !tooltip.is_empty() {
        resp.show_tooltip_text(tooltip);
    }

    if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_recoloured_and_present() {
        let m = white_icons();
        assert_eq!(m.len(), ICONS.len());
        for (name, b) in m.iter() {
            let s = std::str::from_utf8(b).unwrap();
            assert!(!s.contains("currentColor"), "{name}");
        }
        for name in USED_ICONS {
            assert!(exists(name), "{name} is used but not embedded");
        }
    }
}
