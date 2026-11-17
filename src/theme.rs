//! Design system: two themes (Studio dark, Studio Light), colour tokens, radii and type.
//!
//! Ported from `storytold/craft-anything`'s `crates/ui-egui/src/theme.rs`
//! (MIT OR Apache-2.0), adapted to stock egui 0.37 and to this app: the
//! craft-fonts build input and the confidence palette are dropped (this app
//! has neither), and the theme is applied once at startup rather than
//! switched at runtime. The caption *rendering* fonts (`fonts.rs`, DejaVu /
//! cosmic-text) are untouched — this only styles the egui UI chrome.
//!
//! A calm desktop look: neutral graphite (or near-white) surfaces, lightly
//! rounded cards and controls, sentence-case Figtree throughout, and the
//! indigo app colour for highlights, progress, links and selection.

use egui::epaint::text::VariationCoords;
use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, FontTweak, Stroke, TextStyle, Visuals};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeKind {
    #[default]
    Studio,
    StudioLight,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 2] = [ThemeKind::Studio, ThemeKind::StudioLight];

    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::Studio => "Studio (dark)",
            ThemeKind::StudioLight => "Studio Light",
        }
    }

    /// The value stored in settings, if a theme switch is ever added.
    pub fn id(self) -> &'static str {
        match self {
            ThemeKind::Studio => "studio",
            ThemeKind::StudioLight => "studio-light",
        }
    }

    pub fn from_id(s: &str) -> ThemeKind {
        match s.trim().to_ascii_lowercase().replace(['_', ' '], "-").as_str() {
            "studio-light" | "light" => ThemeKind::StudioLight,
            _ => ThemeKind::Studio,
        }
    }
}

/// Colour and shape tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub kind: ThemeKind,
    /// Title bar, rails, panels.
    pub chrome: Color32,
    /// The main reading surface behind cards.
    pub canvas: Color32,
    pub card: Color32,
    pub card_border: Color32,
    /// A card a step above `card` (hovered cards, chips).
    pub raised: Color32,
    /// What can be typed in or pressed: a shade lighter than the card it sits on, so it reads
    /// as something to use.
    pub field: Color32,
    pub field_border: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub icon: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    /// Indigo that reads as text on the theme's surfaces.
    pub accent_ink: Color32,
    /// Indigo wash for soft highlights.
    pub accent_soft: Color32,
    /// Text drawn on an `accent` fill.
    pub on_accent: Color32,
    pub separator: Color32,
    pub shadow: Color32,
    pub danger: Color32,
    pub warning: Color32,
    pub success: Color32,
    pub code_bg: Color32,
    pub radius_sm: f32,
    pub radius: f32,
    pub radius_lg: f32,
}

/// The app colour: deep indigo (#1659c4). White on it passes WCAG AA; on its
/// bright accent (#2d81ff, used for hover) it doesn't.
pub const BRAND: Color32 = Color32::from_rgb(0x16, 0x59, 0xc4);

impl Tokens {
    pub fn for_kind(kind: ThemeKind) -> Self {
        match kind {
            // Warm near-black (#121316), #101014 for the sidebar, cards and
            // controls warm ink (#f2f1ee) laid over the background at 4-15%.
            ThemeKind::Studio => Tokens {
                kind,
                chrome: Color32::from_rgb(0x10, 0x10, 0x14),
                canvas: Color32::from_rgb(0x12, 0x13, 0x16),
                card: Color32::from_rgb(0x1b, 0x1c, 0x1f),
                card_border: Color32::from_rgb(0x34, 0x34, 0x36),
                raised: Color32::from_rgb(0x22, 0x23, 0x25),
                field: Color32::from_rgb(0x22, 0x23, 0x26),
                field_border: Color32::from_rgb(0x6c, 0x6c, 0x6c),
                hover: Color32::from_rgb(0x2a, 0x2b, 0x2e),
                pressed: Color32::from_rgb(0x33, 0x34, 0x37),
                text: Color32::from_rgb(0xf2, 0xf1, 0xee),
                text_dim: Color32::from_rgb(0xb3, 0xb3, 0xb2),
                text_faint: Color32::from_rgb(0x8d, 0x8d, 0x8d),
                icon: Color32::from_rgb(0xa8, 0xa8, 0xa6),
                accent: BRAND,
                accent_hover: Color32::from_rgb(0x2d, 0x81, 0xff),
                accent_ink: Color32::from_rgb(0x74, 0xaa, 0xff),
                accent_soft: Color32::from_rgba_unmultiplied(0x2d, 0x81, 0xff, 44),
                on_accent: Color32::WHITE,
                separator: Color32::from_rgb(0x24, 0x25, 0x27),
                shadow: Color32::from_black_alpha(120),
                danger: Color32::from_rgb(0xff, 0x62, 0x59),
                warning: Color32::from_rgb(0xe0, 0xa2, 0x3a),
                success: Color32::from_rgb(0x5b, 0xbf, 0x8f),
                code_bg: Color32::from_rgb(0x06, 0x06, 0x07),
                radius_sm: RADIUS_SM,
                radius: RADIUS,
                radius_lg: RADIUS_LG,
            },

            // Warm paper (#f2f1ee), sunken #e9e8e4 for the sidebar, raised
            // #faf9f7 for cards, ink #101014.
            ThemeKind::StudioLight => Tokens {
                kind,
                chrome: Color32::from_rgb(0xe9, 0xe8, 0xe4),
                canvas: Color32::from_rgb(0xf2, 0xf1, 0xee),
                card: Color32::from_rgb(0xfa, 0xf9, 0xf7),
                card_border: Color32::from_rgb(0xce, 0xcd, 0xcb),
                raised: Color32::from_rgb(0xe9, 0xe8, 0xe4),
                field: Color32::WHITE,
                field_border: Color32::from_rgb(0x86, 0x85, 0x85),
                hover: Color32::from_rgb(0xe4, 0xe4, 0xe1),
                pressed: Color32::from_rgb(0xdb, 0xda, 0xd8),
                text: Color32::from_rgb(0x10, 0x10, 0x14),
                text_dim: Color32::from_rgb(0x3d, 0x3d, 0x40),
                text_faint: Color32::from_rgb(0x5d, 0x5c, 0x5e),
                icon: Color32::from_rgb(0x4f, 0x4f, 0x52),
                accent: BRAND,
                accent_hover: Color32::from_rgb(0x2d, 0x81, 0xff),
                accent_ink: BRAND,
                accent_soft: Color32::from_rgba_unmultiplied(0x2d, 0x81, 0xff, 28),
                on_accent: Color32::WHITE,
                separator: Color32::from_rgb(0xdb, 0xda, 0xd8),
                shadow: Color32::from_black_alpha(20),
                danger: Color32::from_rgb(0xb3, 0x26, 0x1e),
                warning: Color32::from_rgb(0x9a, 0x67, 0x00),
                success: Color32::from_rgb(0x12, 0x75, 0x36),
                code_bg: Color32::from_rgb(0xe9, 0xe8, 0xe4),
                radius_sm: RADIUS_SM,
                radius: RADIUS,
                radius_lg: RADIUS_LG,
            },
        }
    }

    /// The tokens of the active theme (defaults to Studio dark before `apply` runs).
    pub fn get(ctx: &egui::Context) -> Tokens {
        ctx.data(|d| d.get_temp::<Tokens>(tokens_id())).unwrap_or_else(|| Tokens::for_kind(ThemeKind::Studio))
    }

    pub fn dark(&self) -> bool {
        self.kind == ThemeKind::Studio
    }

    pub fn r_sm(&self) -> CornerRadius {
        CornerRadius::same(self.radius_sm as u8)
    }

    pub fn r(&self) -> CornerRadius {
        CornerRadius::same(self.radius as u8)
    }

    pub fn r_lg(&self) -> CornerRadius {
        CornerRadius::same(self.radius_lg as u8)
    }
}

/// Corners: tags and segments, controls and fields, cards and dialogs. Rounded, but only just.
const RADIUS_SM: f32 = 4.0;
const RADIUS: f32 = 6.0;
const RADIUS_LG: f32 = 8.0;

fn tokens_id() -> egui::Id {
    egui::Id::unique("caption-studio-theme")
}

// ---- fonts -------------------------------------------------------------------------------------

/// Figtree, a variable font: one file set to each weight used here.
/// OFL, see `assets/fonts/Figtree-OFL.txt`.
pub static FIGTREE: &[u8] = include_bytes!("../assets/fonts/Figtree-Variable.ttf");

/// The faces cut from it: text, labels, and headings and emphasis.
const FIGTREE_FACES: [(&str, f32); 3] = [("Figtree", 400.0), ("Figtree-Medium", 500.0), ("Figtree-SemiBold", 600.0)];

/// JetBrains Mono (code). OFL, see `assets/fonts/JetBrainsMono-OFL.txt`.
pub static JETBRAINS_MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");

/// Install Figtree (three weights: text, labels, and semibold for headings)
/// and JetBrains Mono (code). The default egui fonts stay behind them as
/// fallbacks (emoji, CJK, and anything Figtree does not cover).
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    for (name, weight) in FIGTREE_FACES {
        let tweak = FontTweak { coords: VariationCoords::new([(*b"wght", weight)]), ..Default::default() };
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(FIGTREE).tweak(tweak)));
    }

    fonts.font_data.insert("JetBrainsMono".to_owned(), Arc::new(FontData::from_static(JETBRAINS_MONO)));

    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "Figtree".to_owned());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "JetBrainsMono".to_owned());

    let base: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for (fam, stack) in weighted_families(&base, &[]) {
        fonts.families.insert(fam, stack);
    }

    ctx.set_fonts(fonts);
}

/// The medium and semibold families: Figtree's own weight first, then the
/// whole proportional stack.
fn weighted_families(base: &[String], bold: &[String]) -> Vec<(FontFamily, Vec<String>)> {
    [("medium", "Figtree-Medium"), ("semibold", "Figtree-SemiBold")]
        .into_iter()
        .map(|(fam, primary)| {
            let mut stack = vec![primary.to_owned()];
            stack.extend(bold.iter().cloned());
            stack.extend(base.iter().cloned());
            (FontFamily::Name(fam.into()), stack)
        })
        .collect()
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

/// The type scale: every text size in the interface is one of these.
/// Headings (`heading`, Figtree semibold), largest first: a screen's title, a step or a dialog's
/// title, a card's or a list item's title.
pub const TITLE: f32 = 26.0;
pub const SECTION: f32 = 18.0;
pub const CARD: f32 = 16.0;

/// Text: the line under a title (and Simple mode's main field), reading text, secondary text
/// and list rows, the labels of small controls and section captions, fine print (paths, counts,
/// notes), and tags painted in tight spots.
pub const LEAD: f32 = 17.0;
pub const BODY: f32 = 15.0;
pub const SMALL: f32 = 14.0;
pub const LABEL: f32 = 13.0;
pub const CAPTION: f32 = 12.0;
pub const TINY: f32 = 11.0;

/// Code, paths and numbers in JetBrains Mono.
pub const CODE: f32 = 12.5;

/// The spacing steps: every gap between things is one of these.
pub mod space {
    /// A nudge between two lines that belong together.
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
}

/// Icon sizes.
pub mod icon {
    pub const XS: f32 = 12.0;
    pub const SM: f32 = 14.0;
    pub const MD: f32 = 16.0;
    pub const LG: f32 = 20.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
}

/// The padding inside frames, built from the spacing steps.
pub mod pad {
    use egui::Margin;

    /// Around the rows of a menu or a dropdown's list.
    pub const MENU: Margin = Margin::same(4);
    /// Code blocks and dense workbench boxes.
    pub const TIGHT: Margin = Margin::same(8);
    pub const PANEL: Margin = Margin::same(12);
    pub const CARD: Margin = Margin::same(16);
    /// Dialogs, side panes and a screen's main card.
    pub const ROOMY: Margin = Margin::same(24);
    /// A card that holds one row.
    pub const ROW: Margin = Margin::symmetric(16, 12);
    /// A banner across the window.
    pub const BAR: Margin = Margin::symmetric(16, 8);
    /// A tag around a word or two.
    pub const CHIP: Margin = Margin::symmetric(8, 4);
    /// A strip that only keeps its content off the edges.
    pub const GUTTER: Margin = Margin::symmetric(8, 0);
}

/// The sizes a control (button, icon button, switch, field) comes in. Height, label size, side
/// padding and icon size go together, so two controls of one size always line up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ControlSize {
    /// Inline and dense: toolbars, rows of a list.
    Small,
    /// The default.
    #[default]
    Medium,
    /// A screen's main switch or field.
    Large,
    /// The one action a screen is for.
    Hero,
}

impl ControlSize {
    pub const ALL: [ControlSize; 4] = [ControlSize::Small, ControlSize::Medium, ControlSize::Large, ControlSize::Hero];

    pub const fn height(self) -> f32 {
        match self {
            ControlSize::Small => 24.0,
            ControlSize::Medium => 32.0,
            ControlSize::Large => 40.0,
            ControlSize::Hero => 48.0,
        }
    }

    /// The label's size (set in the medium weight).
    pub const fn text(self) -> f32 {
        match self {
            ControlSize::Small => LABEL,
            // The size of the text around it, and of what is typed into a field beside it.
            ControlSize::Medium | ControlSize::Large => BODY,
            ControlSize::Hero => CARD,
        }
    }

    /// The room left and right of the label.
    pub const fn pad_x(self) -> f32 {
        match self {
            ControlSize::Small => space::MD,
            ControlSize::Medium => space::LG,
            ControlSize::Large | ControlSize::Hero => space::XL,
        }
    }

    pub const fn icon(self) -> f32 {
        match self {
            ControlSize::Small => icon::SM,
            ControlSize::Medium | ControlSize::Large => icon::MD,
            ControlSize::Hero => icon::LG,
        }
    }
}

/// The line height for a paragraph set at `size`: a little more air than the font's own, which
/// packs wrapped lines tightly.
pub fn leading(size: f32) -> f32 {
    (size * 1.45).round()
}

/// A heading: Figtree semibold, at one of the heading sizes above.
pub fn heading(text: impl Into<String>, size: f32) -> egui::RichText {
    egui::RichText::new(text).font(semibold(size))
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// `a` blended toward `b` by `t` (0..=1), channel by channel (alpha included).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a()))
}

/// A short label (captions, buttons, the title bar): Figtree medium at one of the text sizes.
pub fn label_text(text: &str, size: f32) -> egui::RichText {
    egui::RichText::new(text).font(medium(size))
}

/// Scroll bars that can always be seen and grabbed. egui's thin preset paints the thumb in the
/// button fill and its track in the field colour, which here are the same, and narrows it to 2 px
/// at rest, so it showed only on hover. The thumb is the text colour, see-through enough to stay
/// quiet but 3:1 against the surfaces it sits on (WCAG 1.4.11), 6 px at rest and 10 px on hover.
pub fn scroll_style() -> egui::style::ScrollStyle {
    egui::style::ScrollStyle {
        floating: true,
        bar_width: 8.0,
        floating_width: 6.0,
        floating_allocated_width: 8.0,
        handle_min_length: space::XXL,
        foreground_color: true,
        // No track: only the thumb shows, a pill that thickens a little under the pointer.
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.0,
        dormant_handle_opacity: SCROLL_THUMB_AT_REST,
        active_handle_opacity: 0.6,
        interact_handle_opacity: 0.8,
        ..egui::style::ScrollStyle::solid()
    }
}

/// How opaque a scroll bar's thumb is at rest (over the text colour).
const SCROLL_THUMB_AT_REST: f32 = 0.5;

/// How far one notch of the mouse wheel scrolls, in points. egui's own step is 40, which takes
/// two or three turns for what a browser scrolls in one; it still eases to the new place.
pub const WHEEL_STEP: f32 = 100.0;

/// Apply a theme. `roomy` = bigger type, more air (not used here; the app runs compact).
pub fn apply(ctx: &egui::Context, kind: ThemeKind, roomy: bool) {
    let t = Tokens::for_kind(kind);

    ctx.data_mut(|d| d.insert_temp(tokens_id(), t));

    ctx.set_theme(if t.dark() { egui::Theme::Dark } else { egui::Theme::Light });

    let mut v = if t.dark() { Visuals::dark() } else { Visuals::light() };

    // Each glyph on a whole pixel. egui's default draws a glyph at one of four offsets inside a
    // pixel for more even spacing, which softens small text on a display at 100 %.
    v.text_options.subpixel_binning = false;

    v.panel_fill = t.chrome;
    v.window_fill = t.card;
    v.window_stroke = Stroke::new(1.0, t.card_border);
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.raised;
    v.code_bg_color = t.code_bg;
    v.override_text_color = Some(t.text);
    v.hyperlink_color = t.accent_ink;
    v.warn_fg_color = t.warning;
    v.error_fg_color = t.danger;
    v.window_corner_radius = t.r_lg();
    v.menu_corner_radius = t.r();
    v.window_shadow = egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: t.shadow };
    v.popup_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: t.shadow };
    v.text_edit_bg_color = Some(t.field);
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.handle_shape = egui::style::HandleShape::Circle;
    v.striped = false;
    v.indent_has_left_vline = false;

    // egui's own buttons, combo boxes and fields share the corner of ours.
    let r = t.r();
    let w = &mut v.widgets;

    w.noninteractive.bg_fill = t.card;
    w.noninteractive.weak_bg_fill = t.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.separator);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text_dim);
    w.noninteractive.corner_radius = t.r_sm();

    // Buttons and combo boxes are field-coloured boxes with a hairline edge.
    // One edge for every control, ours and egui's: soft at rest, firmer under the pointer, the
    // app colour while it's pressed or open.
    for (wv, bg, weak, stroke) in [
        (&mut w.inactive, t.field, t.field, t.card_border),
        (&mut w.hovered, t.hover, t.hover, t.field_border),
        (&mut w.active, t.pressed, t.pressed, t.accent),
        // An open dropdown is edged in the app colour, like a field being typed in.
        (&mut w.open, t.hover, t.hover, t.accent),
    ] {
        wv.bg_fill = bg;
        wv.weak_bg_fill = weak;
        wv.bg_stroke = Stroke::new(1.0, stroke);
        wv.fg_stroke = Stroke::new(1.0, t.text);
        wv.corner_radius = r;
        wv.expansion = 0.0;
    }

    ctx.set_visuals(v);

    let control = if roomy { ControlSize::Medium } else { ControlSize::Small };

    ctx.global_style_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(if roomy { LABEL } else { TINY })),
            (TextStyle::Body, FontId::proportional(if roomy { BODY } else { LABEL })),
            (TextStyle::Button, FontId::proportional(control.text())),
            (TextStyle::Heading, semibold(if roomy { SECTION } else { CARD })),
            (TextStyle::Monospace, FontId::monospace(if roomy { LABEL } else { CODE })),
        ]
        .into();

        s.spacing.item_spacing = egui::vec2(space::SM, if roomy { space::SM } else { space::XS });

        // Padding that brings a line of the label up to the control's height; a row of plain text
        // stays as tall as a small control.
        let pad_y = ((control.height() - control.text() * 1.2) / 2.0).round();
        s.spacing.button_padding = egui::vec2(if roomy { space::MD } else { space::SM }, pad_y);
        s.spacing.interact_size = egui::Vec2::splat(ControlSize::Small.height());
        s.spacing.menu_margin = pad::MENU;

        // A dropdown's list shows nine rows and a half before it scrolls: the half row says there's more.
        s.spacing.combo_height = 9.5 * (ControlSize::Medium.height() + space::XXS);
        s.spacing.window_margin = pad::ROOMY;
        s.spacing.icon_width = icon::SM;
        s.spacing.scroll = scroll_style();
        s.spacing.tooltip_width = 320.0;
        s.interaction.tooltip_delay = 0.3;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_applies_and_fonts_install() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        apply(&ctx, ThemeKind::Studio, false);
        let t = Tokens::get(&ctx);
        assert_eq!(t.kind, ThemeKind::Studio);
        assert!(t.dark());
        assert_eq!(t.accent, BRAND);
        // The heading style uses the semibold family.
        let heading = ctx.global_style().text_styles.get(&TextStyle::Heading).cloned().unwrap();
        assert_eq!(heading.family, FontFamily::Name("semibold".into()));
    }

    #[test]
    fn control_sizes_step_up() {
        for pair in ControlSize::ALL.windows(2) {
            assert_eq!(pair[1].height() - pair[0].height(), space::SM, "{pair:?}");
        }
        assert_eq!(ControlSize::default(), ControlSize::Medium);
    }
}
